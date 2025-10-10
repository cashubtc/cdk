//! MultiMint Wallet
//!
//! Wrapper around core [`Wallet`] that enables the use of multiple mint unit
//! pairs

use std::collections::BTreeMap;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use cdk_common::database;
use cdk_common::database::WalletDatabase;
use cdk_common::wallet::{Transaction, TransactionDirection};
use tokio::sync::RwLock;
use tracing::instrument;
use zeroize::Zeroize;

use super::builder::WalletBuilder;
use super::receive::ReceiveOptions;
use super::send::{PreparedSend, SendOptions};
use super::Error;
use crate::amount::SplitTarget;
use crate::mint_url::MintUrl;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::nut23::QuoteState;
use crate::nuts::{CurrencyUnit, MeltOptions, Proof, Proofs, SpendingConditions, Token};
use crate::types::Melted;
#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
use crate::wallet::mint_connector::transport::tor_transport::TorAsync;
use crate::wallet::types::MintQuote;
use crate::{Amount, Wallet};

// Transfer timeout constants
/// Total timeout for waiting for Lightning payment confirmation during transfers
/// This needs to be long enough to handle slow networks and Lightning routing
const TRANSFER_PAYMENT_TIMEOUT_SECS: u64 = 120; // 2 minutes

/// Transfer mode for mint-to-mint transfers
#[derive(Debug, Clone)]
pub enum TransferMode {
    /// Transfer exact amount to target (target receives specified amount)
    ExactReceive(Amount),
    /// Transfer all available balance (source will be emptied)
    FullBalance,
}

/// Result of a transfer operation with detailed breakdown
#[derive(Debug, Clone)]
pub struct TransferResult {
    /// Amount deducted from source mint
    pub amount_sent: Amount,
    /// Amount received at target mint
    pub amount_received: Amount,
    /// Total fees paid for the transfer
    pub fees_paid: Amount,
    /// Remaining balance in source mint after transfer
    pub source_balance_after: Amount,
    /// New balance in target mint after transfer
    pub target_balance_after: Amount,
}

/// Configuration for individual wallets within MultiMintWallet
#[derive(Clone, Default, Debug)]
pub struct WalletConfig {
    /// Custom mint connector implementation
    pub mint_connector: Option<Arc<dyn super::MintConnector + Send + Sync>>,
    /// Custom auth connector implementation
    #[cfg(feature = "auth")]
    pub auth_connector: Option<Arc<dyn super::auth::AuthMintConnector + Send + Sync>>,
    /// Target number of proofs to maintain at each denomination
    pub target_proof_count: Option<usize>,
}

impl WalletConfig {
    /// Create a new empty WalletConfig
    pub fn new() -> Self {
        Self::default()
    }

    /// Set custom mint connector
    pub fn with_mint_connector(
        mut self,
        connector: Arc<dyn super::MintConnector + Send + Sync>,
    ) -> Self {
        self.mint_connector = Some(connector);
        self
    }

    /// Set custom auth connector
    #[cfg(feature = "auth")]
    pub fn with_auth_connector(
        mut self,
        connector: Arc<dyn super::auth::AuthMintConnector + Send + Sync>,
    ) -> Self {
        self.auth_connector = Some(connector);
        self
    }

    /// Set target proof count
    pub fn with_target_proof_count(mut self, count: usize) -> Self {
        self.target_proof_count = Some(count);
        self
    }
}

/// Multi Mint Wallet
///
/// A wallet that manages multiple mints but supports only one currency unit.
/// This simplifies the interface by removing the need to specify both mint and unit.
///
/// # Examples
///
/// ## Creating and using a multi-mint wallet
/// ```ignore
/// # use cdk::wallet::MultiMintWallet;
/// # use cdk::mint_url::MintUrl;
/// # use cdk::Amount;
/// # use cdk::nuts::CurrencyUnit;
/// # use std::sync::Arc;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// // Create a multi-mint wallet with a database
/// // For real usage, you would use cdk_sqlite::wallet::memory::empty().await? or similar
/// let seed = [0u8; 64];  // Use a secure random seed in production
/// let database = cdk_sqlite::wallet::memory::empty().await?;
///
/// let wallet = MultiMintWallet::new(
///     Arc::new(database),
///     seed,
///     CurrencyUnit::Sat,
/// ).await?;
///
/// // Add mints to the wallet
/// let mint_url1: MintUrl = "https://mint1.example.com".parse()?;
/// let mint_url2: MintUrl = "https://mint2.example.com".parse()?;
/// wallet.add_mint(mint_url1.clone()).await?;
/// wallet.add_mint(mint_url2).await?;
///
/// // Check total balance across all mints
/// let balance = wallet.total_balance().await?;
/// println!("Total balance: {} sats", balance);
///
/// // Send tokens from a specific mint
/// let prepared = wallet.prepare_send(
///     mint_url1,
///     Amount::from(100),
///     Default::default()
/// ).await?;
/// let token = prepared.confirm(None).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct MultiMintWallet {
    /// Storage backend
    localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
    seed: [u8; 64],
    /// The currency unit this wallet supports
    unit: CurrencyUnit,
    /// Wallets indexed by mint URL
    wallets: Arc<RwLock<BTreeMap<MintUrl, Wallet>>>,
    /// Proxy configuration for HTTP clients (optional)
    proxy_config: Option<url::Url>,
    /// Shared Tor transport to be cloned into each TorHttpClient (if enabled)
    #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
    shared_tor_transport: Option<TorAsync>,
}

impl MultiMintWallet {
    /// Create a new [MultiMintWallet] for a specific currency unit
    pub async fn new(
        localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        seed: [u8; 64],
        unit: CurrencyUnit,
    ) -> Result<Self, Error> {
        let wallet = Self {
            localstore,
            seed,
            unit,
            wallets: Arc::new(RwLock::new(BTreeMap::new())),
            proxy_config: None,
            #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
            shared_tor_transport: None,
        };

        // Automatically load wallets from database for this currency unit
        wallet.load_wallets().await?;

        Ok(wallet)
    }

    /// Create a new [MultiMintWallet] with proxy configuration
    ///
    /// All wallets in this MultiMintWallet will use the specified proxy.
    /// This allows you to route all mint connections through a proxy server.
    pub async fn new_with_proxy(
        localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        seed: [u8; 64],
        unit: CurrencyUnit,
        proxy_url: url::Url,
    ) -> Result<Self, Error> {
        let wallet = Self {
            localstore,
            seed,
            unit,
            wallets: Arc::new(RwLock::new(BTreeMap::new())),
            proxy_config: Some(proxy_url),
            #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
            shared_tor_transport: None,
        };

        // Automatically load wallets from database for this currency unit
        wallet.load_wallets().await?;

        Ok(wallet)
    }

    /// Create a new [MultiMintWallet] with Tor transport for all wallets
    ///
    /// When the `tor` feature is enabled (and not on wasm32), this constructor
    /// creates a single Tor transport (TorAsync) that is cloned into each
    /// TorHttpClient used by per-mint Wallets. This ensures only one Tor instance
    /// is bootstrapped and shared across wallets.
    #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
    pub async fn new_with_tor(
        localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        seed: [u8; 64],
        unit: CurrencyUnit,
    ) -> Result<Self, Error> {
        let wallet = Self {
            localstore,
            seed,
            unit,
            wallets: Arc::new(RwLock::new(BTreeMap::new())),
            proxy_config: None,
            shared_tor_transport: Some(TorAsync::new()),
        };

        // Automatically load wallets from database for this currency unit
        wallet.load_wallets().await?;

        Ok(wallet)
    }

    /// Adds a mint to this [MultiMintWallet]
    ///
    /// Creates a wallet for the specified mint using default or global settings.
    /// For custom configuration, use `add_mint_with_config()`.
    #[instrument(skip(self))]
    pub async fn add_mint(&self, mint_url: MintUrl) -> Result<(), Error> {
        // Create wallet with default settings
        let wallet = self
            .create_wallet_with_config(mint_url.clone(), None)
            .await?;

        // Insert into wallets map
        let mut wallets = self.wallets.write().await;
        wallets.insert(mint_url, wallet);

        Ok(())
    }

    /// Adds a mint to this [MultiMintWallet] with custom configuration
    ///
    /// The provided configuration is used to create the wallet with custom connectors
    /// and settings. Configuration is stored within the Wallet instance itself.
    #[instrument(skip(self))]
    pub async fn add_mint_with_config(
        &self,
        mint_url: MintUrl,
        config: WalletConfig,
    ) -> Result<(), Error> {
        // Create wallet with the provided config
        let wallet = self
            .create_wallet_with_config(mint_url.clone(), Some(&config))
            .await?;

        // Insert into wallets map
        let mut wallets = self.wallets.write().await;
        wallets.insert(mint_url, wallet);

        Ok(())
    }

    /// Set or update configuration for a mint
    ///
    /// If the wallet already exists, it will be updated with the new config.
    /// If the wallet doesn't exist, it will be created with the specified config.
    #[instrument(skip(self))]
    pub async fn set_mint_config(
        &self,
        mint_url: MintUrl,
        config: WalletConfig,
    ) -> Result<(), Error> {
        // Check if wallet already exists
        if self.has_mint(&mint_url).await {
            // Update existing wallet in place
            let mut wallets = self.wallets.write().await;
            if let Some(wallet) = wallets.get_mut(&mint_url) {
                // Update target_proof_count if provided
                if let Some(count) = config.target_proof_count {
                    wallet.set_target_proof_count(count);
                }

                // Update connector if provided
                if let Some(connector) = config.mint_connector {
                    wallet.set_client(connector);
                }

                // TODO: Handle auth_connector if provided
                #[cfg(feature = "auth")]
                if let Some(_auth_connector) = config.auth_connector {
                    // For now, we can't easily inject auth_connector into the wallet
                    // This would require additional work on the Wallet API
                    // We'll note this as a future enhancement
                }
            }
            Ok(())
        } else {
            // Wallet doesn't exist, create it with the provided config
            self.add_mint_with_config(mint_url, config).await
        }
    }

    /// Set the auth client (AuthWallet) for a specific mint
    ///
    /// This allows updating the auth wallet for an existing mint wallet without recreating it.
    #[cfg(feature = "auth")]
    #[instrument(skip_all)]
    pub async fn set_auth_client(
        &self,
        mint_url: &MintUrl,
        auth_wallet: Option<super::auth::AuthWallet>,
    ) -> Result<(), Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.set_auth_client(auth_wallet).await;
        Ok(())
    }

    /// Remove mint from MultiMintWallet
    #[instrument(skip(self))]
    pub async fn remove_mint(&self, mint_url: &MintUrl) {
        let mut wallets = self.wallets.write().await;
        wallets.remove(mint_url);
    }

    /// Internal: Create wallet with optional custom configuration
    ///
    /// Priority order for configuration:
    /// 1. Custom connector from config (if provided)
    /// 2. Global settings (proxy/Tor)
    /// 3. Default HttpClient
    async fn create_wallet_with_config(
        &self,
        mint_url: MintUrl,
        config: Option<&WalletConfig>,
    ) -> Result<Wallet, Error> {
        // Check if custom connector is provided in config
        if let Some(cfg) = config {
            if let Some(custom_connector) = &cfg.mint_connector {
                // Use custom connector with WalletBuilder
                let builder = WalletBuilder::new()
                    .mint_url(mint_url.clone())
                    .unit(self.unit.clone())
                    .localstore(self.localstore.clone())
                    .seed(self.seed)
                    .target_proof_count(cfg.target_proof_count.unwrap_or(3))
                    .shared_client(custom_connector.clone());

                // TODO: Handle auth_connector if provided
                #[cfg(feature = "auth")]
                if let Some(_auth_connector) = &cfg.auth_connector {
                    // For now, we can't easily inject auth_connector into the wallet
                    // This would require additional work on the Wallet/WalletBuilder API
                    // We'll note this as a future enhancement
                }

                return builder.build();
            }
        }

        // Fall back to existing logic: proxy/Tor/default
        let target_proof_count = config.and_then(|c| c.target_proof_count).unwrap_or(3);

        let wallet = if let Some(proxy_url) = &self.proxy_config {
            // Create wallet with proxy-configured client
            let client = crate::wallet::HttpClient::with_proxy(
                mint_url.clone(),
                proxy_url.clone(),
                None,
                true,
            )
            .unwrap_or_else(|_| {
                #[cfg(feature = "auth")]
                {
                    crate::wallet::HttpClient::new(mint_url.clone(), None)
                }
                #[cfg(not(feature = "auth"))]
                {
                    crate::wallet::HttpClient::new(mint_url.clone())
                }
            });
            WalletBuilder::new()
                .mint_url(mint_url.clone())
                .unit(self.unit.clone())
                .localstore(self.localstore.clone())
                .seed(self.seed)
                .target_proof_count(target_proof_count)
                .client(client)
                .build()?
        } else {
            #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
            if let Some(tor) = &self.shared_tor_transport {
                // Create wallet with Tor transport client, cloning the shared transport
                let client = {
                    let transport = tor.clone();
                    #[cfg(feature = "auth")]
                    {
                        crate::wallet::TorHttpClient::with_transport(
                            mint_url.clone(),
                            transport,
                            None,
                        )
                    }
                    #[cfg(not(feature = "auth"))]
                    {
                        crate::wallet::TorHttpClient::with_transport(mint_url.clone(), transport)
                    }
                };

                WalletBuilder::new()
                    .mint_url(mint_url.clone())
                    .unit(self.unit.clone())
                    .localstore(self.localstore.clone())
                    .seed(self.seed)
                    .target_proof_count(target_proof_count)
                    .client(client)
                    .build()?
            } else {
                // Create wallet with default client
                Wallet::new(
                    &mint_url.to_string(),
                    self.unit.clone(),
                    self.localstore.clone(),
                    self.seed,
                    Some(target_proof_count),
                )?
            }

            #[cfg(not(all(feature = "tor", not(target_arch = "wasm32"))))]
            {
                // Create wallet with default client
                Wallet::new(
                    &mint_url.to_string(),
                    self.unit.clone(),
                    self.localstore.clone(),
                    self.seed,
                    Some(target_proof_count),
                )?
            }
        };

        Ok(wallet)
    }

    /// Load all wallets from database that have proofs for this currency unit
    #[instrument(skip(self))]
    async fn load_wallets(&self) -> Result<(), Error> {
        let mints = self.localstore.get_mints().await.map_err(Error::Database)?;

        // Get all proofs for this currency unit to determine which mints are relevant
        let all_proofs = self
            .localstore
            .get_proofs(None, Some(self.unit.clone()), None, None)
            .await
            .map_err(Error::Database)?;

        for (mint_url, _mint_info) in mints {
            // Check if this mint has any proofs for the specified currency unit
            // or if we have no proofs at all (initial setup)
            let mint_has_proofs_for_unit =
                all_proofs.is_empty() || all_proofs.iter().any(|proof| proof.mint_url == mint_url);

            if mint_has_proofs_for_unit {
                // Add mint to the MultiMintWallet if not already present
                if !self.has_mint(&mint_url).await {
                    self.add_mint(mint_url.clone()).await?
                }
            }
        }

        Ok(())
    }

    /// Get Wallets from MultiMintWallet
    #[instrument(skip(self))]
    pub async fn get_wallets(&self) -> Vec<Wallet> {
        self.wallets.read().await.values().cloned().collect()
    }

    /// Get Wallet from MultiMintWallet
    #[instrument(skip(self))]
    pub async fn get_wallet(&self, mint_url: &MintUrl) -> Option<Wallet> {
        self.wallets.read().await.get(mint_url).cloned()
    }

    /// Check if mint is in wallet
    #[instrument(skip(self))]
    pub async fn has_mint(&self, mint_url: &MintUrl) -> bool {
        self.wallets.read().await.contains_key(mint_url)
    }

    /// Get the currency unit for this wallet
    pub fn unit(&self) -> &CurrencyUnit {
        &self.unit
    }

    /// Get wallet balances for all mints
    #[instrument(skip(self))]
    pub async fn get_balances(&self) -> Result<BTreeMap<MintUrl, Amount>, Error> {
        let mut balances = BTreeMap::new();

        for (mint_url, wallet) in self.wallets.read().await.iter() {
            let wallet_balance = wallet.total_balance().await?;
            balances.insert(mint_url.clone(), wallet_balance);
        }

        Ok(balances)
    }

    /// List proofs.
    #[instrument(skip(self))]
    pub async fn list_proofs(&self) -> Result<BTreeMap<MintUrl, Vec<Proof>>, Error> {
        let mut mint_proofs = BTreeMap::new();

        for (mint_url, wallet) in self.wallets.read().await.iter() {
            let wallet_proofs = wallet.get_unspent_proofs().await?;
            mint_proofs.insert(mint_url.clone(), wallet_proofs);
        }
        Ok(mint_proofs)
    }

    /// List transactions
    #[instrument(skip(self))]
    pub async fn list_transactions(
        &self,
        direction: Option<TransactionDirection>,
    ) -> Result<Vec<Transaction>, Error> {
        let mut transactions = Vec::new();

        for (_, wallet) in self.wallets.read().await.iter() {
            let wallet_transactions = wallet.list_transactions(direction).await?;
            transactions.extend(wallet_transactions);
        }

        transactions.sort();

        Ok(transactions)
    }

    /// Get total balance across all wallets (since all wallets use the same currency unit)
    #[instrument(skip(self))]
    pub async fn total_balance(&self) -> Result<Amount, Error> {
        let mut total = Amount::ZERO;
        for (_, wallet) in self.wallets.read().await.iter() {
            total += wallet.total_balance().await?;
        }
        Ok(total)
    }

    /// Prepare to send tokens from a specific mint with optional transfer from other mints
    ///
    /// This method ensures that sends always happen from only one mint. If the specified
    /// mint doesn't have sufficient balance and `allow_transfer` is enabled in options,
    /// it will first transfer funds from other mints to the target mint.
    #[instrument(skip(self))]
    pub async fn prepare_send(
        &self,
        mint_url: MintUrl,
        amount: Amount,
        opts: MultiMintSendOptions,
    ) -> Result<PreparedSend, Error> {
        // Ensure the mint exists
        let wallets = self.wallets.read().await;
        let target_wallet = wallets.get(&mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        // Check current balance of target mint
        let target_balance = target_wallet.total_balance().await?;

        // If target mint has sufficient balance, prepare send directly
        if target_balance >= amount {
            return target_wallet.prepare_send(amount, opts.send_options).await;
        }

        // If transfer is not allowed, return insufficient funds error
        if !opts.allow_transfer {
            return Err(Error::InsufficientFunds);
        }

        // Calculate how much we need to transfer
        let transfer_needed = amount - target_balance;

        // Check if transfer amount exceeds max_transfer_amount
        if let Some(max_transfer) = opts.max_transfer_amount {
            if transfer_needed > max_transfer {
                return Err(Error::InsufficientFunds);
            }
        }

        // Find source wallets with available funds for transfer
        let mut available_for_transfer = Amount::ZERO;
        let mut source_mints = Vec::new();

        for (source_mint_url, wallet) in wallets.iter() {
            if source_mint_url == &mint_url {
                continue; // Skip the target mint
            }

            // Check if this mint is excluded from transfers
            if opts.excluded_mints.contains(source_mint_url) {
                continue;
            }

            // Check if we have a restricted allowed list and this mint isn't in it
            if !opts.allowed_mints.is_empty() && !opts.allowed_mints.contains(source_mint_url) {
                continue;
            }

            let balance = wallet.total_balance().await?;
            if balance > Amount::ZERO {
                source_mints.push((source_mint_url.clone(), balance));
                available_for_transfer += balance;
            }
        }

        // Check if we have enough funds across all mints
        if available_for_transfer < transfer_needed {
            return Err(Error::InsufficientFunds);
        }

        // Drop the read lock before performing transfers
        drop(wallets);

        // Perform transfers from source wallets to target wallet
        self.transfer_parallel(&mint_url, transfer_needed, source_mints)
            .await?;

        // Now prepare the send from the target mint
        let wallets = self.wallets.read().await;
        let target_wallet = wallets.get(&mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        target_wallet.prepare_send(amount, opts.send_options).await
    }

    /// Transfer funds from a single source wallet to target mint using Lightning Network (melt/mint)
    ///
    /// This function properly accounts for fees by handling different transfer modes:
    /// - ExactReceive: Target receives exactly the specified amount, source pays amount + fees
    /// - FullBalance: All source balance is transferred, target receives balance - fees
    pub async fn transfer(
        &self,
        source_mint_url: &MintUrl,
        target_mint_url: &MintUrl,
        mode: TransferMode,
    ) -> Result<TransferResult, Error> {
        // Get wallets for the specified mints and clone them to release the lock
        let (source_wallet, target_wallet) = {
            let wallets = self.wallets.read().await;
            let source = wallets
                .get(source_mint_url)
                .ok_or(Error::UnknownMint {
                    mint_url: source_mint_url.to_string(),
                })?
                .clone();
            let target = wallets
                .get(target_mint_url)
                .ok_or(Error::UnknownMint {
                    mint_url: target_mint_url.to_string(),
                })?
                .clone();
            (source, target)
        };

        // Get initial balance
        let source_balance_initial = source_wallet.total_balance().await?;

        // Handle different transfer modes
        let (final_mint_quote, final_melt_quote) = match mode {
            TransferMode::ExactReceive(amount) => {
                self.handle_exact_receive_transfer(
                    &source_wallet,
                    &target_wallet,
                    amount,
                    source_balance_initial,
                )
                .await?
            }
            TransferMode::FullBalance => {
                self.handle_full_balance_transfer(
                    &source_wallet,
                    &target_wallet,
                    source_balance_initial,
                )
                .await?
            }
        };

        // Execute the transfer
        let (melted, actual_receive_amount) = self
            .execute_transfer(
                &source_wallet,
                &target_wallet,
                &final_mint_quote,
                &final_melt_quote,
            )
            .await?;

        // Get final balances
        let source_balance_final = source_wallet.total_balance().await?;
        let target_balance_final = target_wallet.total_balance().await?;

        let amount_sent = source_balance_initial - source_balance_final;
        let fees_paid = melted.fee_paid;

        tracing::info!(
            "Transferred {} from {} to {} via Lightning (sent: {} sats, received: {} sats, fee: {} sats)",
            amount_sent,
            source_wallet.mint_url,
            target_wallet.mint_url,
            amount_sent,
            actual_receive_amount,
            fees_paid
        );

        Ok(TransferResult {
            amount_sent,
            amount_received: actual_receive_amount,
            fees_paid,
            source_balance_after: source_balance_final,
            target_balance_after: target_balance_final,
        })
    }

    /// Handle exact receive transfer mode - target gets exactly the specified amount
    async fn handle_exact_receive_transfer(
        &self,
        source_wallet: &Wallet,
        target_wallet: &Wallet,
        amount: Amount,
        source_balance: Amount,
    ) -> Result<(MintQuote, crate::wallet::types::MeltQuote), Error> {
        // Step 1: Create mint quote at target mint for the exact amount we want to receive
        let mint_quote = target_wallet.mint_quote(amount, None).await?;

        // Step 2: Create melt quote at source mint for the invoice
        let melt_quote = source_wallet
            .melt_quote(mint_quote.request.clone(), None)
            .await?;

        // Step 3: Check if source has enough balance for the total amount needed (amount + melt fees)
        let total_needed = melt_quote.amount + melt_quote.fee_reserve;
        if source_balance < total_needed {
            return Err(Error::InsufficientFunds);
        }

        Ok((mint_quote, melt_quote))
    }

    /// Handle full balance transfer mode - all source balance is transferred
    async fn handle_full_balance_transfer(
        &self,
        source_wallet: &Wallet,
        target_wallet: &Wallet,
        source_balance: Amount,
    ) -> Result<(MintQuote, crate::wallet::types::MeltQuote), Error> {
        if source_balance == Amount::ZERO {
            return Err(Error::InsufficientFunds);
        }

        // Step 1: Create melt quote for full balance to discover fees
        // We need to create a dummy mint quote first to get an invoice
        let dummy_mint_quote = target_wallet.mint_quote(source_balance, None).await?;
        let probe_melt_quote = source_wallet
            .melt_quote(dummy_mint_quote.request.clone(), None)
            .await?;

        // Step 2: Calculate actual receive amount (balance - fees)
        let receive_amount = source_balance
            .checked_sub(probe_melt_quote.fee_reserve)
            .ok_or(Error::InsufficientFunds)?;

        if receive_amount == Amount::ZERO {
            return Err(Error::InsufficientFunds);
        }

        // Step 3: Create final mint quote for the net amount
        let final_mint_quote = target_wallet.mint_quote(receive_amount, None).await?;

        // Step 4: Create final melt quote with the new invoice
        let final_melt_quote = source_wallet
            .melt_quote(final_mint_quote.request.clone(), None)
            .await?;

        Ok((final_mint_quote, final_melt_quote))
    }

    /// Execute the actual transfer using the prepared quotes
    async fn execute_transfer(
        &self,
        source_wallet: &Wallet,
        target_wallet: &Wallet,
        final_mint_quote: &MintQuote,
        final_melt_quote: &crate::wallet::types::MeltQuote,
    ) -> Result<(Melted, Amount), Error> {
        // Step 1: Subscribe to mint quote updates before melting
        let mut subscription = target_wallet
            .subscribe(super::WalletSubscription::Bolt11MintQuoteState(vec![
                final_mint_quote.id.clone(),
            ]))
            .await;

        // Step 2: Melt from source wallet using the final melt quote
        let melted = source_wallet.melt(&final_melt_quote.id).await?;

        // Step 3: Wait for payment confirmation via subscription
        tracing::debug!(
            "Waiting for Lightning payment confirmation (max {} seconds) for transfer from {} to {}",
            TRANSFER_PAYMENT_TIMEOUT_SECS,
            source_wallet.mint_url,
            target_wallet.mint_url
        );

        // Wait for payment notification with overall timeout
        let timeout_duration = tokio::time::Duration::from_secs(TRANSFER_PAYMENT_TIMEOUT_SECS);

        loop {
            match tokio::time::timeout(timeout_duration, subscription.recv()).await {
                Ok(Some(notification)) => {
                    // Check if this is a mint quote response with paid state
                    if let crate::nuts::nut17::NotificationPayload::MintQuoteBolt11Response(
                        quote_response,
                    ) = notification.deref()
                    {
                        if quote_response.state == QuoteState::Paid {
                            // Quote is paid, now mint the tokens
                            target_wallet
                                .mint(
                                    &final_mint_quote.id,
                                    crate::amount::SplitTarget::default(),
                                    None,
                                )
                                .await?;
                            break;
                        }
                    }
                }
                Ok(None) => {
                    // Subscription closed
                    tracing::warn!("Subscription closed while waiting for mint quote payment");
                    return Err(Error::TransferTimeout {
                        source_mint: source_wallet.mint_url.to_string(),
                        target_mint: target_wallet.mint_url.to_string(),
                        amount: final_mint_quote.amount.unwrap_or(Amount::ZERO),
                    });
                }
                Err(_) => {
                    // Overall timeout reached
                    tracing::warn!(
                        "Transfer timed out after {} seconds waiting for Lightning payment confirmation",
                        TRANSFER_PAYMENT_TIMEOUT_SECS
                    );
                    return Err(Error::TransferTimeout {
                        source_mint: source_wallet.mint_url.to_string(),
                        target_mint: target_wallet.mint_url.to_string(),
                        amount: final_mint_quote.amount.unwrap_or(Amount::ZERO),
                    });
                }
            }
        }

        let actual_receive_amount = final_mint_quote.amount.unwrap_or(Amount::ZERO);
        Ok((melted, actual_receive_amount))
    }

    /// Transfer funds from multiple source wallets to target mint in parallel
    async fn transfer_parallel(
        &self,
        target_mint_url: &MintUrl,
        total_amount: Amount,
        source_mints: Vec<(MintUrl, Amount)>,
    ) -> Result<(), Error> {
        let mut remaining_amount = total_amount;
        let mut transfer_tasks = Vec::new();

        // Create transfer tasks for each source wallet
        for (source_mint_url, available_balance) in source_mints {
            if remaining_amount == Amount::ZERO {
                break;
            }

            let transfer_amount = std::cmp::min(remaining_amount, available_balance);
            remaining_amount -= transfer_amount;

            let self_clone = self.clone();
            let source_mint_url = source_mint_url.clone();
            let target_mint_url = target_mint_url.clone();

            // Spawn parallel transfer task
            #[cfg(not(target_arch = "wasm32"))]
            let task = tokio::spawn(async move {
                self_clone
                    .transfer(
                        &source_mint_url,
                        &target_mint_url,
                        TransferMode::ExactReceive(transfer_amount),
                    )
                    .await
                    .map(|result| result.amount_received)
            });

            #[cfg(target_arch = "wasm32")]
            let task = tokio::task::spawn_local(async move {
                self_clone
                    .transfer(
                        &source_mint_url,
                        &target_mint_url,
                        TransferMode::ExactReceive(transfer_amount),
                    )
                    .await
                    .map(|result| result.amount_received)
            });

            transfer_tasks.push(task);
        }

        // Wait for all transfers to complete
        let mut total_transferred = Amount::ZERO;
        for task in transfer_tasks {
            match task.await {
                Ok(Ok(amount)) => {
                    total_transferred += amount;
                }
                Ok(Err(e)) => {
                    tracing::error!("Transfer failed: {}", e);
                    return Err(e);
                }
                Err(e) => {
                    tracing::error!("Transfer task panicked: {}", e);
                    return Err(Error::Internal);
                }
            }
        }

        // Check if we transferred less than expected (accounting for fees)
        // We don't return an error here as fees are expected
        if total_transferred < total_amount {
            let fee_paid = total_amount - total_transferred;
            tracing::info!(
                "Transfer completed with fees: requested {}, received {}, total fees {}",
                total_amount,
                total_transferred,
                fee_paid
            );
        }

        Ok(())
    }

    /// Mint quote for wallet
    #[instrument(skip(self))]
    pub async fn mint_quote(
        &self,
        mint_url: &MintUrl,
        amount: Amount,
        description: Option<String>,
    ) -> Result<MintQuote, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.mint_quote(amount, description).await
    }

    /// Check a specific mint quote status
    #[instrument(skip(self))]
    pub async fn check_mint_quote(
        &self,
        mint_url: &MintUrl,
        quote_id: &str,
    ) -> Result<MintQuote, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        // Check the quote state from the mint
        wallet.mint_quote_state(quote_id).await?;

        // Get the updated quote from local storage
        let quote = wallet
            .localstore
            .get_mint_quote(quote_id)
            .await
            .map_err(Error::Database)?
            .ok_or(Error::UnknownQuote)?;

        Ok(quote)
    }

    /// Check all mint quotes
    /// If quote is paid, wallet will mint
    #[instrument(skip(self))]
    pub async fn check_all_mint_quotes(&self, mint_url: Option<MintUrl>) -> Result<Amount, Error> {
        let mut total_amount = Amount::ZERO;
        match mint_url {
            Some(mint_url) => {
                let wallets = self.wallets.read().await;
                let wallet = wallets.get(&mint_url).ok_or(Error::UnknownMint {
                    mint_url: mint_url.to_string(),
                })?;

                total_amount = wallet.check_all_mint_quotes().await?;
            }
            None => {
                for (_, wallet) in self.wallets.read().await.iter() {
                    let amount = wallet.check_all_mint_quotes().await?;
                    total_amount += amount;
                }
            }
        }

        Ok(total_amount)
    }

    /// Mint a specific quote
    #[instrument(skip(self))]
    pub async fn mint(
        &self,
        mint_url: &MintUrl,
        quote_id: &str,
        conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet
            .mint(quote_id, SplitTarget::default(), conditions)
            .await
    }

    /// Wait for a mint quote to be paid and automatically mint the proofs
    #[cfg(not(target_arch = "wasm32"))]
    #[instrument(skip(self))]
    pub async fn wait_for_mint_quote(
        &self,
        mint_url: &MintUrl,
        quote_id: &str,
        split_target: SplitTarget,
        conditions: Option<SpendingConditions>,
        timeout_secs: u64,
    ) -> Result<Proofs, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        // Get the mint quote from local storage
        let quote = wallet
            .localstore
            .get_mint_quote(quote_id)
            .await
            .map_err(Error::Database)?
            .ok_or(Error::UnknownQuote)?;

        // Wait for the quote to be paid and mint the proofs
        let timeout_duration = tokio::time::Duration::from_secs(timeout_secs);
        wallet
            .wait_and_mint_quote(quote, split_target, conditions, timeout_duration)
            .await
    }

    /// Receive token with multi-mint options
    ///
    /// This method can:
    /// - Receive tokens from trusted mints (already added to the wallet)
    /// - Optionally receive from untrusted mints by adding them to the wallet
    /// - Optionally transfer tokens from untrusted mints to a trusted mint (and remove the untrusted mint)
    ///
    /// # Examples
    /// ```no_run
    /// # use cdk::wallet::{MultiMintWallet, MultiMintReceiveOptions};
    /// # use cdk::mint_url::MintUrl;
    /// # async fn example(wallet: MultiMintWallet) -> Result<(), Box<dyn std::error::Error>> {
    /// // Receive from a trusted mint
    /// let token = "cashuAey...";
    /// let amount = wallet
    ///     .receive(token, MultiMintReceiveOptions::default())
    ///     .await?;
    ///
    /// // Receive from untrusted mint and add it to the wallet
    /// let options = MultiMintReceiveOptions::default().allow_untrusted(true);
    /// let amount = wallet.receive(token, options).await?;
    ///
    /// // Receive from untrusted mint, transfer to trusted mint, then remove untrusted mint
    /// let trusted_mint: MintUrl = "https://trusted.mint".parse()?;
    /// let options = MultiMintReceiveOptions::default().transfer_to_mint(Some(trusted_mint));
    /// let amount = wallet.receive(token, options).await?;
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip_all)]
    pub async fn receive(
        &self,
        encoded_token: &str,
        opts: MultiMintReceiveOptions,
    ) -> Result<Amount, Error> {
        let token_data = Token::from_str(encoded_token)?;
        let unit = token_data.unit().unwrap_or_default();

        // Ensure the token uses the same currency unit as this wallet
        if unit != self.unit {
            return Err(Error::MultiMintCurrencyUnitMismatch {
                expected: self.unit.clone(),
                found: unit,
            });
        }

        let mint_url = token_data.mint_url()?;
        let is_trusted = self.has_mint(&mint_url).await;

        // If mint is not trusted and we don't allow untrusted mints, error
        if !is_trusted && !opts.allow_untrusted {
            return Err(Error::UnknownMint {
                mint_url: mint_url.to_string(),
            });
        }

        // If mint is untrusted and we need to transfer, ensure we have a target mint
        let should_transfer = !is_trusted && opts.transfer_to_mint.is_some();

        // Add the untrusted mint temporarily if needed
        if !is_trusted {
            self.add_mint(mint_url.clone()).await?;
        }

        let wallets = self.wallets.read().await;
        let wallet = wallets.get(&mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        // We need the keysets information to properly convert from token proof to proof
        let keysets_info = match self
            .localstore
            .get_mint_keysets(token_data.mint_url()?)
            .await?
        {
            Some(keysets_info) => keysets_info,
            // Hit the keysets endpoint if we don't have the keysets for this Mint
            None => wallet.load_mint_keysets().await?,
        };
        let proofs = token_data.proofs(&keysets_info)?;

        let mut amount_received = Amount::ZERO;

        match wallet
            .receive_proofs(proofs, opts.receive_options, token_data.memo().clone())
            .await
        {
            Ok(amount) => {
                amount_received += amount;
            }
            Err(err) => {
                // If we added the mint temporarily for transfer only, remove it before returning error
                if !is_trusted && opts.transfer_to_mint.is_some() {
                    drop(wallets);
                    self.remove_mint(&mint_url).await;
                }
                return Err(err);
            }
        }

        drop(wallets);

        // If we should transfer to a trusted mint, do so now
        if should_transfer {
            if let Some(target_mint) = opts.transfer_to_mint {
                // Ensure target mint exists and is trusted
                if !self.has_mint(&target_mint).await {
                    // Clean up untrusted mint if we're only using it for transfer
                    self.remove_mint(&mint_url).await;
                    return Err(Error::UnknownMint {
                        mint_url: target_mint.to_string(),
                    });
                }

                // Transfer the entire balance from the untrusted mint to the target mint
                // Use FullBalance mode for efficient transfer of all funds
                let transfer_result = self
                    .transfer(&mint_url, &target_mint, TransferMode::FullBalance)
                    .await;

                // Handle transfer result - log details but don't fail if balance was zero
                match transfer_result {
                    Ok(result) => {
                        if result.amount_sent > Amount::ZERO {
                            tracing::info!(
                                "Transferred {} sats from untrusted mint {} to trusted mint {} (received: {}, fees: {})",
                                result.amount_sent,
                                mint_url,
                                target_mint,
                                result.amount_received,
                                result.fees_paid
                            );
                        }
                    }
                    Err(Error::InsufficientFunds) => {
                        // No balance to transfer, which is fine
                        tracing::debug!("No balance to transfer from untrusted mint {}", mint_url);
                    }
                    Err(e) => return Err(e),
                }

                // Remove the untrusted mint after transfer
                self.remove_mint(&mint_url).await;
            }
        }
        // Note: If allow_untrusted is true but no transfer is requested,
        // the untrusted mint is kept in the wallet (as intended)

        Ok(amount_received)
    }

    /// Restore
    #[instrument(skip(self))]
    pub async fn restore(&self, mint_url: &MintUrl) -> Result<Amount, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.restore().await
    }

    /// Verify token matches p2pk conditions
    #[instrument(skip(self, token))]
    pub async fn verify_token_p2pk(
        &self,
        token: &Token,
        conditions: SpendingConditions,
    ) -> Result<(), Error> {
        let mint_url = token.mint_url()?;
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(&mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.verify_token_p2pk(token, conditions).await
    }

    /// Verifys all proofs in token have valid dleq proof
    #[instrument(skip(self, token))]
    pub async fn verify_token_dleq(&self, token: &Token) -> Result<(), Error> {
        let mint_url = token.mint_url()?;
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(&mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.verify_token_dleq(token).await
    }

    /// Create a melt quote for a specific mint
    #[instrument(skip(self, bolt11))]
    pub async fn melt_quote(
        &self,
        mint_url: &MintUrl,
        bolt11: String,
        options: Option<MeltOptions>,
    ) -> Result<crate::wallet::types::MeltQuote, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.melt_quote(bolt11, options).await
    }

    /// Melt (pay invoice) from a specific mint using a quote ID
    #[instrument(skip(self))]
    pub async fn melt_with_mint(
        &self,
        mint_url: &MintUrl,
        quote_id: &str,
    ) -> Result<Melted, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.melt(quote_id).await
    }

    /// Create MPP (Multi-Path Payment) melt quotes from multiple mints
    ///
    /// This function allows manual specification of which mints and amounts to use for MPP.
    /// Returns a vector of (MintUrl, MeltQuote) pairs.
    #[instrument(skip(self, bolt11))]
    pub async fn mpp_melt_quote(
        &self,
        bolt11: String,
        mint_amounts: Vec<(MintUrl, Amount)>,
    ) -> Result<Vec<(MintUrl, crate::wallet::types::MeltQuote)>, Error> {
        let mut quotes = Vec::new();
        let mut tasks = Vec::new();

        // Spawn parallel tasks to get quotes from each mint
        for (mint_url, amount) in mint_amounts {
            let wallets = self.wallets.read().await;
            let wallet = wallets
                .get(&mint_url)
                .ok_or(Error::UnknownMint {
                    mint_url: mint_url.to_string(),
                })?
                .clone();
            drop(wallets);

            let bolt11_clone = bolt11.clone();
            let mint_url_clone = mint_url.clone();

            // Convert amount to millisats for MeltOptions
            let amount_msat = u64::from(amount) * 1000;
            let options = Some(MeltOptions::new_mpp(amount_msat));

            #[cfg(not(target_arch = "wasm32"))]
            let task = tokio::spawn(async move {
                let quote = wallet.melt_quote(bolt11_clone, options).await;
                (mint_url_clone, quote)
            });

            #[cfg(target_arch = "wasm32")]
            let task = tokio::task::spawn_local(async move {
                let quote = wallet.melt_quote(bolt11_clone, options).await;
                (mint_url_clone, quote)
            });

            tasks.push(task);
        }

        // Collect all quote results
        for task in tasks {
            match task.await {
                Ok((mint_url, Ok(quote))) => {
                    quotes.push((mint_url, quote));
                }
                Ok((mint_url, Err(e))) => {
                    tracing::error!("Failed to get melt quote from {}: {}", mint_url, e);
                    return Err(e);
                }
                Err(e) => {
                    tracing::error!("Task failed: {}", e);
                    return Err(Error::Internal);
                }
            }
        }

        Ok(quotes)
    }

    /// Execute MPP melts using previously obtained quotes
    #[instrument(skip(self))]
    pub async fn mpp_melt(
        &self,
        quotes: Vec<(MintUrl, String)>, // (mint_url, quote_id)
    ) -> Result<Vec<(MintUrl, Melted)>, Error> {
        let mut results = Vec::new();
        let mut tasks = Vec::new();

        for (mint_url, quote_id) in quotes {
            let wallets = self.wallets.read().await;
            let wallet = wallets
                .get(&mint_url)
                .ok_or(Error::UnknownMint {
                    mint_url: mint_url.to_string(),
                })?
                .clone();
            drop(wallets);

            let mint_url_clone = mint_url.clone();

            #[cfg(not(target_arch = "wasm32"))]
            let task = tokio::spawn(async move {
                let melted = wallet.melt(&quote_id).await;
                (mint_url_clone, melted)
            });

            #[cfg(target_arch = "wasm32")]
            let task = tokio::task::spawn_local(async move {
                let melted = wallet.melt(&quote_id).await;
                (mint_url_clone, melted)
            });

            tasks.push(task);
        }

        // Collect all melt results
        for task in tasks {
            match task.await {
                Ok((mint_url, Ok(melted))) => {
                    results.push((mint_url, melted));
                }
                Ok((mint_url, Err(e))) => {
                    tracing::error!("Failed to melt from {}: {}", mint_url, e);
                    return Err(e);
                }
                Err(e) => {
                    tracing::error!("Task failed: {}", e);
                    return Err(Error::Internal);
                }
            }
        }

        Ok(results)
    }

    /// Melt (pay invoice) with automatic wallet selection (deprecated, use specific mint functions for better control)
    ///
    /// Automatically selects the best wallet to pay from based on:
    /// - Available balance
    /// - Fees
    ///
    /// # Examples
    /// ```no_run
    /// # use cdk::wallet::MultiMintWallet;
    /// # use cdk::Amount;
    /// # use std::sync::Arc;
    /// # async fn example(wallet: Arc<MultiMintWallet>) -> Result<(), Box<dyn std::error::Error>> {
    /// // Pay a lightning invoice from any mint with sufficient balance
    /// let invoice = "lnbc100n1p...";
    ///
    /// let result = wallet.melt(invoice, None, None).await?;
    /// println!("Paid {} sats, fee was {} sats", result.amount, result.fee_paid);
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self, bolt11))]
    pub async fn melt(
        &self,
        bolt11: &str,
        options: Option<MeltOptions>,
        max_fee: Option<Amount>,
    ) -> Result<Melted, Error> {
        // Parse the invoice to get the amount
        let invoice = bolt11
            .parse::<crate::Bolt11Invoice>()
            .map_err(Error::Invoice)?;

        let amount = invoice
            .amount_milli_satoshis()
            .map(|msats| Amount::from(msats / 1000))
            .ok_or(Error::InvoiceAmountUndefined)?;

        let wallets = self.wallets.read().await;
        let mut eligible_wallets = Vec::new();

        for (mint_url, wallet) in wallets.iter() {
            let balance = wallet.total_balance().await?;
            if balance >= amount {
                eligible_wallets.push((mint_url.clone(), wallet.clone()));
            }
        }

        if eligible_wallets.is_empty() {
            return Err(Error::InsufficientFunds);
        }

        // Try to get quotes from eligible wallets and select the best one
        let mut best_quote = None;
        let mut best_wallet = None;

        for (_, wallet) in eligible_wallets.iter() {
            match wallet.melt_quote(bolt11.to_string(), options).await {
                Ok(quote) => {
                    if let Some(max_fee) = max_fee {
                        if quote.fee_reserve > max_fee {
                            continue;
                        }
                    }

                    if best_quote.is_none() {
                        best_quote = Some(quote);
                        best_wallet = Some(wallet.clone());
                    } else if let Some(ref existing_quote) = best_quote {
                        if quote.fee_reserve < existing_quote.fee_reserve {
                            best_quote = Some(quote);
                            best_wallet = Some(wallet.clone());
                        }
                    }
                }
                Err(_) => continue,
            }
        }

        if let (Some(quote), Some(wallet)) = (best_quote, best_wallet) {
            return wallet.melt(&quote.id).await;
        }

        Err(Error::InsufficientFunds)
    }

    /// Swap proofs with automatic wallet selection
    #[instrument(skip(self))]
    pub async fn swap(
        &self,
        amount: Option<Amount>,
        conditions: Option<SpendingConditions>,
    ) -> Result<Option<Proofs>, Error> {
        // Find a wallet that has proofs
        let wallets = self.wallets.read().await;

        for (_, wallet) in wallets.iter() {
            let balance = wallet.total_balance().await?;
            if balance > Amount::ZERO {
                // Try to swap with this wallet
                let proofs = wallet.get_unspent_proofs().await?;
                if !proofs.is_empty() {
                    return wallet
                        .swap(amount, SplitTarget::default(), proofs, conditions, false)
                        .await;
                }
            }
        }

        Err(Error::InsufficientFunds)
    }

    /// Consolidate proofs from multiple wallets into fewer, larger proofs
    /// This can help reduce the number of proofs and optimize wallet performance
    #[instrument(skip(self))]
    pub async fn consolidate(&self) -> Result<Amount, Error> {
        let mut total_consolidated = Amount::ZERO;
        let wallets = self.wallets.read().await;

        for (mint_url, wallet) in wallets.iter() {
            // Get all unspent proofs for this wallet
            let proofs = wallet.get_unspent_proofs().await?;
            if proofs.len() > 1 {
                // Consolidate by swapping all proofs for a single set
                let proofs_amount = proofs.total_amount()?;

                // Swap for optimized proof set
                match wallet
                    .swap(
                        Some(proofs_amount),
                        SplitTarget::default(),
                        proofs,
                        None,
                        false,
                    )
                    .await
                {
                    Ok(_) => {
                        total_consolidated += proofs_amount;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to consolidate proofs for mint {:?}: {}",
                            mint_url,
                            e
                        );
                    }
                }
            }
        }

        Ok(total_consolidated)
    }

    /// Mint blind auth tokens for a specific mint
    ///
    /// This is a convenience method that calls the underlying wallet's mint_blind_auth.
    #[cfg(feature = "auth")]
    #[instrument(skip_all)]
    pub async fn mint_blind_auth(
        &self,
        mint_url: &MintUrl,
        amount: Amount,
    ) -> Result<Proofs, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.mint_blind_auth(amount).await
    }

    /// Get unspent auth proofs for a specific mint
    ///
    /// This is a convenience method that calls the underlying wallet's get_unspent_auth_proofs.
    #[cfg(feature = "auth")]
    #[instrument(skip_all)]
    pub async fn get_unspent_auth_proofs(
        &self,
        mint_url: &MintUrl,
    ) -> Result<Vec<cdk_common::AuthProof>, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.get_unspent_auth_proofs().await
    }

    /// Set Clear Auth Token (CAT) for authentication at a specific mint
    ///
    /// This is a convenience method that calls the underlying wallet's set_cat.
    #[cfg(feature = "auth")]
    #[instrument(skip_all)]
    pub async fn set_cat(&self, mint_url: &MintUrl, cat: String) -> Result<(), Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.set_cat(cat).await
    }

    /// Set refresh token for authentication at a specific mint
    ///
    /// This is a convenience method that calls the underlying wallet's set_refresh_token.
    #[cfg(feature = "auth")]
    #[instrument(skip_all)]
    pub async fn set_refresh_token(
        &self,
        mint_url: &MintUrl,
        refresh_token: String,
    ) -> Result<(), Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.set_refresh_token(refresh_token).await
    }

    /// Refresh CAT token for a specific mint
    ///
    /// This is a convenience method that calls the underlying wallet's refresh_access_token.
    #[cfg(feature = "auth")]
    #[instrument(skip(self))]
    pub async fn refresh_access_token(&self, mint_url: &MintUrl) -> Result<(), Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.refresh_access_token().await
    }

    /// Query mint for current mint information
    ///
    /// This is a convenience method that calls the underlying wallet's fetch_mint_info.
    #[instrument(skip(self))]
    pub async fn fetch_mint_info(
        &self,
        mint_url: &MintUrl,
    ) -> Result<Option<crate::nuts::MintInfo>, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.fetch_mint_info().await
    }
}

impl Drop for MultiMintWallet {
    fn drop(&mut self) {
        self.seed.zeroize();
    }
}

/// Multi-Mint Receive Options
///
/// Controls how tokens are received, especially from untrusted mints
#[derive(Debug, Clone, Default)]
pub struct MultiMintReceiveOptions {
    /// Whether to allow receiving from untrusted (not yet added) mints
    pub allow_untrusted: bool,
    /// Mint to transfer tokens to from untrusted mints (None means keep in original mint)
    pub transfer_to_mint: Option<MintUrl>,
    /// Base receive options to apply to the wallet receive
    pub receive_options: ReceiveOptions,
}

impl MultiMintReceiveOptions {
    /// Create new default options
    pub fn new() -> Self {
        Default::default()
    }

    /// Allow receiving from untrusted mints
    pub fn allow_untrusted(mut self, allow: bool) -> Self {
        self.allow_untrusted = allow;
        self
    }

    /// Set mint to transfer tokens to from untrusted mints
    pub fn transfer_to_mint(mut self, mint_url: Option<MintUrl>) -> Self {
        self.transfer_to_mint = mint_url;
        self
    }

    /// Set the base receive options for the wallet operation
    pub fn receive_options(mut self, options: ReceiveOptions) -> Self {
        self.receive_options = options;
        self
    }
}

/// Multi-Mint Send Options
///
/// Controls transfer behavior when the target mint doesn't have sufficient balance
#[derive(Debug, Clone, Default)]
pub struct MultiMintSendOptions {
    /// Whether to allow transferring funds from other mints to the sending mint
    /// if the sending mint doesn't have sufficient balance
    pub allow_transfer: bool,
    /// Maximum amount to transfer from other mints (optional limit)
    pub max_transfer_amount: Option<Amount>,
    /// Specific mints allowed for transfers (empty means all mints allowed)
    pub allowed_mints: Vec<MintUrl>,
    /// Specific mints to exclude from transfers
    pub excluded_mints: Vec<MintUrl>,
    /// Base send options to apply to the wallet send
    pub send_options: SendOptions,
}

impl MultiMintSendOptions {
    /// Create new default options
    pub fn new() -> Self {
        Default::default()
    }

    /// Enable transferring funds from other mints if needed
    pub fn allow_transfer(mut self, allow: bool) -> Self {
        self.allow_transfer = allow;
        self
    }

    /// Set maximum amount to transfer from other mints
    pub fn max_transfer_amount(mut self, amount: Amount) -> Self {
        self.max_transfer_amount = Some(amount);
        self
    }

    /// Add a mint to the allowed list for transfers
    pub fn allow_mint(mut self, mint_url: MintUrl) -> Self {
        self.allowed_mints.push(mint_url);
        self
    }

    /// Set all allowed mints for transfers
    pub fn allowed_mints(mut self, mints: Vec<MintUrl>) -> Self {
        self.allowed_mints = mints;
        self
    }

    /// Add a mint to exclude from transfers
    pub fn exclude_mint(mut self, mint_url: MintUrl) -> Self {
        self.excluded_mints.push(mint_url);
        self
    }

    /// Set all excluded mints for transfers
    pub fn excluded_mints(mut self, mints: Vec<MintUrl>) -> Self {
        self.excluded_mints = mints;
        self
    }

    /// Set the base send options for the wallet operation
    pub fn send_options(mut self, options: SendOptions) -> Self {
        self.send_options = options;
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::database::WalletDatabase;

    use super::*;

    async fn create_test_multi_wallet() -> MultiMintWallet {
        let localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("Failed to create in-memory database"),
        );
        let seed = [0u8; 64];
        MultiMintWallet::new(localstore, seed, CurrencyUnit::Sat)
            .await
            .expect("Failed to create MultiMintWallet")
    }

    #[tokio::test]
    async fn test_total_balance_empty() {
        let multi_wallet = create_test_multi_wallet().await;
        let balance = multi_wallet.total_balance().await.unwrap();
        assert_eq!(balance, Amount::ZERO);
    }

    #[tokio::test]
    async fn test_prepare_send_insufficient_funds() {
        use std::str::FromStr;

        let multi_wallet = create_test_multi_wallet().await;
        let mint_url = MintUrl::from_str("https://mint1.example.com").unwrap();
        let options = MultiMintSendOptions::new();

        let result = multi_wallet
            .prepare_send(mint_url, Amount::from(1000), options)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_consolidate_empty() {
        let multi_wallet = create_test_multi_wallet().await;
        let result = multi_wallet.consolidate().await.unwrap();
        assert_eq!(result, Amount::ZERO);
    }

    #[tokio::test]
    async fn test_multi_mint_wallet_creation() {
        let multi_wallet = create_test_multi_wallet().await;
        assert!(multi_wallet.wallets.try_read().is_ok());
    }

    #[tokio::test]
    async fn test_multi_mint_send_options() {
        use std::str::FromStr;

        let mint1 = MintUrl::from_str("https://mint1.example.com").unwrap();
        let mint2 = MintUrl::from_str("https://mint2.example.com").unwrap();
        let mint3 = MintUrl::from_str("https://mint3.example.com").unwrap();

        let options = MultiMintSendOptions::new()
            .allow_transfer(true)
            .max_transfer_amount(Amount::from(500))
            .allow_mint(mint1.clone())
            .allow_mint(mint2.clone())
            .exclude_mint(mint3.clone())
            .send_options(SendOptions::default());

        assert!(options.allow_transfer);
        assert_eq!(options.max_transfer_amount, Some(Amount::from(500)));
        assert_eq!(options.allowed_mints, vec![mint1, mint2]);
        assert_eq!(options.excluded_mints, vec![mint3]);
    }
}

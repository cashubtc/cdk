//! MultiMint Wallet
//!
//! Wrapper around core [`Wallet`] that enables the use of multiple mint unit
//! pairs

use std::collections::BTreeMap;
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
use crate::wallet::types::MintQuote;
use crate::{ensure_cdk, Amount, Wallet};

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
/// wallet.add_mint(mint_url1.clone(), None).await?;
/// wallet.add_mint(mint_url2, None).await?;
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
        Self {
            allow_transfer: false,
            max_transfer_amount: None,
            allowed_mints: Vec::new(),
            excluded_mints: Vec::new(),
            send_options: SendOptions::default(),
        }
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
        };

        // Automatically load wallets from database for this currency unit
        wallet.load_wallets().await?;

        Ok(wallet)
    }

    /// Adds a mint to this [MultiMintWallet]
    #[instrument(skip(self))]
    pub async fn add_mint(
        &self,
        mint_url: MintUrl,
        target_proof_count: Option<usize>,
    ) -> Result<(), Error> {
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
                .target_proof_count(target_proof_count.unwrap_or(3))
                .client(client)
                .build()?
        } else {
            // Create wallet with default client
            Wallet::new(
                &mint_url.to_string(),
                self.unit.clone(),
                self.localstore.clone(),
                self.seed,
                target_proof_count,
            )?
        };

        wallet.fetch_mint_info().await?;
        wallet.refresh_keysets().await?;

        let mut wallets = self.wallets.write().await;
        wallets.insert(mint_url, wallet);

        Ok(())
    }

    /// Remove mint from MultiMintWallet
    #[instrument(skip(self))]
    pub async fn remove_mint(&self, mint_url: &MintUrl) {
        let mut wallets = self.wallets.write().await;
        wallets.remove(mint_url);
    }

    /// Load all wallets from database that have proofs for this currency unit
    #[instrument(skip(self))]
    pub async fn load_wallets(&self) -> Result<(), Error> {
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
                    self.add_mint(mint_url, None).await?;
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
        let mut source_wallets = Vec::new();

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
                source_wallets.push((source_mint_url.clone(), wallet.clone(), balance));
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
        self.transfer_to_mint(&mint_url, transfer_needed, source_wallets)
            .await?;

        // Now prepare the send from the target mint
        let wallets = self.wallets.read().await;
        let target_wallet = wallets.get(&mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        target_wallet.prepare_send(amount, opts.send_options).await
    }

    /// Transfer funds from source wallets to target mint using Lightning Network (melt/mint)
    async fn transfer_to_mint(
        &self,
        target_mint_url: &MintUrl,
        total_amount: Amount,
        source_wallets: Vec<(MintUrl, Wallet, Amount)>,
    ) -> Result<(), Error> {
        let mut remaining_amount = total_amount;

        // Get target wallet for minting
        let wallets = self.wallets.read().await;
        let target_wallet = wallets
            .get(target_mint_url)
            .ok_or(Error::UnknownMint {
                mint_url: target_mint_url.to_string(),
            })?
            .clone();
        drop(wallets);

        for (source_mint_url, source_wallet, available_balance) in source_wallets {
            if remaining_amount == Amount::ZERO {
                break;
            }

            let transfer_amount = std::cmp::min(remaining_amount, available_balance);

            // Step 1: Create a mint quote in the target mint
            let mint_quote = target_wallet.mint_quote(transfer_amount, None).await?;

            // Step 2: Create melt quote in source wallet for the target mint's invoice
            let melt_quote = source_wallet
                .melt_quote(mint_quote.request.clone(), None)
                .await?;

            // Step 3: Melt from source wallet using the melt quote
            let melted = source_wallet.melt(&melt_quote.id).await?;

            // Step 4: Check if the mint quote is paid and mint in target wallet
            // We need to poll for payment confirmation
            let mut attempts = 0;
            const MAX_ATTEMPTS: u32 = 30; // 30 seconds max wait
            const POLL_INTERVAL_MS: u64 = 1000; // 1 second

            loop {
                attempts += 1;

                // Check if the quote is paid
                match target_wallet.mint_quote_state(&mint_quote.id).await {
                    Ok(quote_state) => {
                        if quote_state.state == QuoteState::Paid {
                            // Quote is paid, now mint the tokens
                            target_wallet
                                .mint(&mint_quote.id, crate::amount::SplitTarget::default(), None)
                                .await?;
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Error checking mint quote status: {}", e);
                    }
                }

                if attempts >= MAX_ATTEMPTS {
                    return Err(Error::TransferTimeout {
                        source_mint: source_mint_url.to_string(),
                        target_mint: target_mint_url.to_string(),
                        amount: transfer_amount,
                    });
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
            }

            remaining_amount -= transfer_amount;

            tracing::info!(
                "Transferred {} from {} to {} via Lightning (melted: {} sats, fee: {} sats)",
                transfer_amount,
                source_mint_url,
                target_mint_url,
                melted.amount,
                melted.fee_paid
            );
        }

        if remaining_amount > Amount::ZERO {
            return Err(Error::InsufficientFunds);
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

    /// Receive token
    /// Wallet must be already added to multimintwallet
    #[instrument(skip_all)]
    pub async fn receive(
        &self,
        encoded_token: &str,
        opts: ReceiveOptions,
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

        // Check that the mint is in this wallet
        if !self.has_mint(&mint_url).await {
            return Err(Error::UnknownMint {
                mint_url: mint_url.to_string(),
            });
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

        let mut mint_errors = None;

        match wallet
            .receive_proofs(proofs, opts, token_data.memo().clone())
            .await
        {
            Ok(amount) => {
                amount_received += amount;
            }
            Err(err) => {
                tracing::error!("Could no receive proofs for mint: {}", err);
                mint_errors = Some(err);
            }
        }

        match mint_errors {
            None => Ok(amount_received),
            Some(err) => Err(err),
        }
    }

    /// Pay an bolt11 invoice from specific wallet
    #[instrument(skip(self, bolt11))]
    pub async fn pay_invoice_for_wallet(
        &self,
        mint_url: &MintUrl,
        bolt11: &str,
        options: Option<MeltOptions>,
        max_fee: Option<Amount>,
    ) -> Result<Melted, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        let quote = wallet.melt_quote(bolt11.to_string(), options).await?;
        if let Some(max_fee) = max_fee {
            ensure_cdk!(quote.fee_reserve <= max_fee, Error::MaxFeeExceeded);
        }

        wallet.melt(&quote.id).await
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
        mint_url: &MintUrl,
        token: &Token,
        conditions: SpendingConditions,
    ) -> Result<(), Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.verify_token_p2pk(token, conditions).await
    }

    /// Verifys all proofs in token have valid dleq proof
    #[instrument(skip(self, token))]
    pub async fn verify_token_dleq(&self, mint_url: &MintUrl, token: &Token) -> Result<(), Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        wallet.verify_token_dleq(token).await
    }

    /// Melt (pay invoice) with automatic wallet selection
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
}

impl Drop for MultiMintWallet {
    fn drop(&mut self) {
        self.seed.zeroize();
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

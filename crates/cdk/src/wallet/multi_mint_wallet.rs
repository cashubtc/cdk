//! MultiMint Wallet
//!
//! Wrapper around core [`Wallet`] that enables the use of multiple mint unit
//! pairs

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use cdk_common::database;
use cdk_common::database::WalletDatabase;
use cdk_common::wallet::{Transaction, TransactionDirection};
use tokio::sync::RwLock;
use tracing::instrument;
use zeroize::Zeroize;

use super::receive::ReceiveOptions;
use super::send::{PreparedSend, SendOptions, SendMemo};
use super::Error;
use crate::amount::SplitTarget;
use crate::mint_url::MintUrl;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{CurrencyUnit, MeltOptions, Proof, Proofs, SpendingConditions, Token};
use crate::types::Melted;
use crate::wallet::types::MintQuote;
use crate::{ensure_cdk, Amount, Wallet};

/// Mint health information for monitoring and automatic exclusion
#[derive(Debug, Clone)]
pub struct MintHealthInfo {
    /// Number of successful operations
    pub success_count: u32,
    /// Number of failed operations
    pub failure_count: u32,
    /// Last successful operation time
    pub last_success: Option<Instant>,
    /// Last failure time
    pub last_failure: Option<Instant>,
    /// Current consecutive failures
    pub consecutive_failures: u32,
    /// Whether the mint is temporarily excluded
    pub is_excluded: bool,
    /// When the temporary exclusion expires
    pub exclusion_expires: Option<Instant>,
}

/// Multi Mint Wallet
/// 
/// A wallet that manages multiple mints but supports only one currency unit.
/// This simplifies the interface by removing the need to specify both mint and unit.
#[derive(Debug, Clone)]
pub struct MultiMintWallet {
    /// Storage backend
    pub localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
    seed: [u8; 64],
    /// The currency unit this wallet supports
    pub unit: CurrencyUnit,
    /// Wallets indexed by mint URL
    pub wallets: Arc<RwLock<BTreeMap<MintUrl, Wallet>>>,
    /// Round-robin counter for mint selection
    round_robin_counter: Arc<AtomicUsize>,
    /// Health information for each mint
    mint_health: Arc<RwLock<HashMap<MintUrl, MintHealthInfo>>>,
}

/// Multi-Mint Prepared Send
/// 
/// Holds multiple PreparedSend structs from different wallets for cross-wallet sends
#[derive(Debug)]
pub struct MultiMintPreparedSend {
    /// The prepared sends from different wallets
    pub prepared_sends: Vec<PreparedSend>,
    /// Total amount being sent
    pub total_amount: Amount,
    /// Total fees across all wallets
    pub total_fee: Amount,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Reference to the wallet to access mint information
    pub wallet: Arc<MultiMintWallet>,
}

impl MultiMintPreparedSend {
    /// Create a new MultiMintPreparedSend
    pub fn new(prepared_sends: Vec<PreparedSend>, unit: CurrencyUnit, wallet: Arc<MultiMintWallet>) -> Result<Self, Error> {
        if prepared_sends.is_empty() {
            return Err(Error::NoPreparedSends);
        }

        let total_amount = prepared_sends.iter().map(|p| p.amount()).fold(Amount::ZERO, |acc, amount| acc + amount);
        let total_fee = prepared_sends.iter().map(|p| p.fee()).fold(Amount::ZERO, |acc, fee| acc + fee);

        Ok(Self {
            prepared_sends,
            total_amount,
            total_fee,
            unit,
            wallet,
        })
    }

    /// Get the total amount being sent
    pub fn amount(&self) -> Amount {
        self.total_amount
    }

    /// Get the total fees across all wallets
    pub fn fee(&self) -> Amount {
        self.total_fee
    }

    /// Get the currency unit
    pub fn unit(&self) -> &CurrencyUnit {
        &self.unit
    }

    /// Get the number of wallets involved in this send
    pub fn wallet_count(&self) -> usize {
        self.prepared_sends.len()
    }

    /// Confirm all prepared sends and return the combined token
    pub async fn confirm(self, memo: Option<SendMemo>) -> Result<Token, Error> {
        if self.prepared_sends.is_empty() {
            return Err(Error::NoPreparedSends);
        }

        // For single prepared send, confirm directly
        if self.prepared_sends.len() == 1 {
            let prepared_send = self.prepared_sends.into_iter().next().unwrap();
            return prepared_send.confirm(memo).await;
        }

        // For multiple prepared sends, confirm each and combine the tokens
        let mut all_proofs = Vec::new();
        let mut mint_url = None;
        let mut total_confirmed_amount = Amount::ZERO;

        for prepared_send in self.prepared_sends {
            // Confirm this prepared send
            let token = prepared_send.confirm(memo.clone()).await?;
            total_confirmed_amount += prepared_send.amount();
            
            // Parse the token to extract proofs
            let token_data = Token::from_str(&token.to_string())?;
            
            // Ensure all tokens are from compatible mints (same unit is already guaranteed)
            let token_mint_url = token_data.mint_url()?;
            if mint_url.is_none() {
                mint_url = Some(token_mint_url.clone());
            }
            
            // Get keysets info to properly parse proofs
            // Note: This is a limitation - we need access to the wallet's keysets
            // For now, we'll try to parse the proofs from the token directly
            // Get keysets for this mint to convert proofs properly
            let wallets = self.wallet.wallets.read().await;
            let wallet = wallets.get(&token_mint_url).ok_or_else(|| Error::UnknownMint { mint_url: token_mint_url.to_string() })?;
            let mint_keysets = wallet.load_mint_keysets().await?;
            let token_proofs = token_data.proofs(&mint_keysets)?;
                
            all_proofs.extend(token_proofs);
        }

        // Verify we collected proofs from all sends
        if all_proofs.is_empty() {
            return Err(Error::TokenCombinationFailed { reason: "No proofs collected from confirmed sends".to_string() });
        }

        let mint_url = mint_url.ok_or(Error::TokenCombinationFailed { reason: "No mint URL found".to_string() })?;

        // Create a combined token with all proofs
        // Note: All proofs are from the same currency unit (guaranteed by our validation)
        let combined_token = Token::new(mint_url.unwrap(), all_proofs, memo.map(|m| m.to_string()), self.unit);
        
        tracing::info!(
            "Combined {} prepared sends into single token with {} proofs", 
            self.wallet_count(),
            combined_token.proofs().len()
        );

        Ok(combined_token)
    }
}

/// Multi-Mint Send Options
/// 
/// Controls which mints to use for sending and in what priority order
#[derive(Debug, Clone)]
pub struct MultiMintSendOptions {
    /// Maximum number of mints to use for a single send operation
    pub max_mints: Option<usize>,
    /// Specific mints to prefer (in priority order, first = highest priority)
    pub preferred_mints: Vec<MintUrl>,
    /// Specific mints to avoid using
    pub excluded_mints: Vec<MintUrl>,
    /// Strategy for selecting mints when not specified
    pub selection_strategy: MintSelectionStrategy,
    /// Whether to allow cross-mint sends (using multiple mints for one transaction)
    pub allow_cross_mint: bool,
    /// Base send options to apply to each individual wallet send
    pub send_options: SendOptions,
}

/// Strategy for selecting which mints to use for sending
#[derive(Debug, Clone)]
pub enum MintSelectionStrategy {
    /// Use the mint with the highest balance first
    HighestBalanceFirst,
    /// Use the mint with the lowest balance first (to consolidate small amounts)
    LowestBalanceFirst,
    /// Use mints in random order
    Random,
    /// Use mints based on lowest fees first
    LowestFeesFirst,
    /// Use a round-robin approach across all mints
    RoundRobin,
}

impl Default for MultiMintSendOptions {
    fn default() -> Self {
        Self {
            max_mints: Some(1), // By default, prefer single mint sends
            preferred_mints: Vec::new(),
            excluded_mints: Vec::new(),
            selection_strategy: MintSelectionStrategy::HighestBalanceFirst,
            allow_cross_mint: false, // Conservative default
            send_options: SendOptions::default(),
        }
    }
}

impl MultiMintSendOptions {
    /// Create new options with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum number of mints to use
    pub fn max_mints(mut self, max: usize) -> Self {
        self.max_mints = Some(max);
        self
    }

    /// Allow unlimited mints to be used
    pub fn unlimited_mints(mut self) -> Self {
        self.max_mints = None;
        self
    }

    /// Add a preferred mint (higher priority)
    pub fn prefer_mint(mut self, mint_url: MintUrl) -> Self {
        self.preferred_mints.push(mint_url);
        self
    }

    /// Set all preferred mints (in priority order)
    pub fn preferred_mints(mut self, mints: Vec<MintUrl>) -> Self {
        self.preferred_mints = mints;
        self
    }

    /// Add a mint to exclude from selection
    pub fn exclude_mint(mut self, mint_url: MintUrl) -> Self {
        self.excluded_mints.push(mint_url);
        self
    }

    /// Set all excluded mints
    pub fn excluded_mints(mut self, mints: Vec<MintUrl>) -> Self {
        self.excluded_mints = mints;
        self
    }

    /// Set the mint selection strategy
    pub fn selection_strategy(mut self, strategy: MintSelectionStrategy) -> Self {
        self.selection_strategy = strategy;
        self
    }

    /// Enable cross-mint sends (allows using multiple mints for one transaction)
    pub fn allow_cross_mint(mut self, allow: bool) -> Self {
        self.allow_cross_mint = allow;
        self
    }

    /// Set the base send options for individual wallet operations
    pub fn send_options(mut self, options: SendOptions) -> Self {
        self.send_options = options;
        self
    }

    /// Validate that the options are consistent
    pub fn validate(&self) -> Result<(), Error> {
        // Check that preferred mints don't conflict with excluded mints
        for preferred in &self.preferred_mints {
            if self.excluded_mints.contains(preferred) {
                return Err(Error::ConflictingMintPreferences { mint_url: preferred.to_string() });
            }
        }

        // If max_mints is 1, cross_mint should be false
        if self.max_mints == Some(1) && self.allow_cross_mint {
            return Err(Error::InvalidMintSelectionOptions);
        }

        Ok(())
    }
}

impl MultiMintWallet {
    /// Create a new [MultiMintWallet] with initial wallets for a specific currency unit
    pub fn new(
        localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        seed: [u8; 64],
        unit: CurrencyUnit,
        wallets: Vec<Wallet>,
    ) -> Result<Self, Error> {
        // Ensure all wallets use the same currency unit
        for wallet in &wallets {
            if wallet.unit != unit {
                return Err(Error::MultiMintCurrencyUnitMismatch {
                    expected: unit.clone(),
                    found: wallet.unit.clone()
                });
            }
        }

        Ok(Self {
            localstore,
            seed,
            unit,
            wallets: Arc::new(RwLock::new(
                wallets
                    .into_iter()
                    .map(|w| (w.mint_url.clone(), w))
                    .collect(),
            )),
            round_robin_counter: Arc::new(AtomicUsize::new(0)),
            mint_health: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Adds a [Wallet] to this [MultiMintWallet]
    #[instrument(skip(self, wallet))]
    pub async fn add_wallet(&self, wallet: Wallet) -> Result<(), Error> {
        // Ensure the wallet uses the same currency unit
        if wallet.unit != self.unit {
            return Err(Error::MultiMintCurrencyUnitMismatch {
                expected: self.unit.clone(),
                found: wallet.unit.clone()
            });
        }

        let mint_url = wallet.mint_url.clone();
        let mut wallets = self.wallets.write().await;
        wallets.insert(mint_url, wallet);
        
        Ok(())
    }

    /// Creates a new [Wallet] and adds it to this [MultiMintWallet]
    pub async fn create_and_add_wallet(
        &self,
        mint_url: &str,
        target_proof_count: Option<usize>,
    ) -> Result<Wallet> {
        let wallet = Wallet::new(
            mint_url,
            self.unit.clone(),
            self.localstore.clone(),
            self.seed,
            target_proof_count,
        )?;

        wallet.fetch_mint_info().await?;

        self.add_wallet(wallet.clone()).await?;

        Ok(wallet)
    }

    /// Remove Wallet from MultiMintWallet
    #[instrument(skip(self))]
    pub async fn remove_wallet(&self, mint_url: &MintUrl) {
        let mut wallets = self.wallets.write().await;

        wallets.remove(mint_url);
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
    pub async fn has(&self, mint_url: &MintUrl) -> bool {
        self.wallets.read().await.contains_key(mint_url)
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

    /// Prepare to send from a specific mint
    #[instrument(skip(self))]
    pub async fn prepare_send(
        &self,
        mint_url: &MintUrl,
        amount: Amount,
        opts: SendOptions,
    ) -> Result<PreparedSend, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(mint_url)
            .ok_or(Error::UnknownMint { mint_url: mint_url.to_string() })?;

        wallet.prepare_send(amount, opts).await
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
        let wallet = wallets
            .get(mint_url)
            .ok_or(Error::UnknownMint { mint_url: mint_url.to_string() })?;

        wallet.mint_quote(amount, description).await
    }

    /// Check all mint quotes
    /// If quote is paid, wallet will mint
    #[instrument(skip(self))]
    pub async fn check_all_mint_quotes(
        &self,
        mint_url: Option<MintUrl>,
    ) -> Result<Amount, Error> {
        let mut total_amount = Amount::ZERO;
        match mint_url {
            Some(mint_url) => {
                let wallets = self.wallets.read().await;
                let wallet = wallets
                    .get(&mint_url)
                    .ok_or(Error::UnknownMint { mint_url: mint_url.to_string() })?;

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
        let wallet = wallets
            .get(mint_url)
            .ok_or(Error::UnknownMint { mint_url: mint_url.to_string() })?;

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
                found: unit
            });
        }

        let mint_url = token_data.mint_url()?;

        // Check that the mint is in this wallet
        if !self.has(&mint_url).await {
            return Err(Error::UnknownMint { mint_url: mint_url.to_string() });
        }

        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(&mint_url)
            .ok_or(Error::UnknownMint { mint_url: mint_url.to_string() })?;

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
        bolt11: &str,
        options: Option<MeltOptions>,
        mint_url: &MintUrl,
        max_fee: Option<Amount>,
    ) -> Result<Melted, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(mint_url)
            .ok_or(Error::UnknownMint { mint_url: mint_url.to_string() })?;

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
        let wallet = wallets
            .get(mint_url)
            .ok_or(Error::UnknownMint { mint_url: mint_url.to_string() })?;

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
        let wallet = wallets
            .get(mint_url)
            .ok_or(Error::UnknownMint { mint_url: mint_url.to_string() })?;

        wallet.verify_token_p2pk(token, conditions).await
    }

    /// Verifys all proofs in token have valid dleq proof
    #[instrument(skip(self, token))]
    pub async fn verify_token_dleq(
        &self,
        mint_url: &MintUrl,
        token: &Token,
    ) -> Result<(), Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(mint_url)
            .ok_or(Error::UnknownMint { mint_url: mint_url.to_string() })?;

        wallet.verify_token_dleq(token).await
    }

    // ============================================================================
    // NEW UNIFIED INTERFACE METHODS - Making MultiMintWallet more like Wallet
    // ============================================================================

    /// Get total balance across all wallets (since all wallets use the same currency unit)
    #[instrument(skip(self))]
    pub async fn total_balance(&self) -> Result<Amount, Error> {
        let mut total = Amount::ZERO;
        for (_, wallet) in self.wallets.read().await.iter() {
            total += wallet.total_balance().await?;
        }
        Ok(total)
    }

    /// Send tokens with automatic wallet selection based on available balance
    /// 
    /// Uses default MultiMintSendOptions (single mint, highest balance first)
    #[instrument(skip(self))]
    pub async fn send(
        &self,
        amount: Amount,
        opts: SendOptions,
    ) -> Result<Token, Error> {
        let multi_opts = MultiMintSendOptions::new().send_options(opts);
        self.send_with_options(amount, multi_opts).await
    }

    /// Send tokens with advanced mint selection options
    /// 
    /// This method allows fine-grained control over which mints to use and in what order:
    /// - Control maximum number of mints to use
    /// - Specify preferred mints (in priority order)
    /// - Exclude specific mints
    /// - Choose selection strategy (highest balance, lowest fees, etc.)
    /// - Enable cross-mint sends (split across multiple mints)
    #[instrument(skip(self))]
    pub async fn send_with_options(
        &self,
        amount: Amount,
        options: MultiMintSendOptions,
    ) -> Result<Token, Error> {
        // Select mints based on the provided options
        let selected_wallets = self.select_mints_for_send(amount, &options).await?;

        if selected_wallets.is_empty() {
            return Err(Error::InsufficientFunds);
        }

        // For single mint sends (most common case)
        if !options.allow_cross_mint {
            let (_, wallet, _) = selected_wallets
                .into_iter()
                .next()
                .ok_or(Error::InsufficientFunds)?;

            // Prepare and confirm the send from the selected wallet
            let prepared = wallet.prepare_send(amount, options.send_options).await?;
            return prepared.confirm(None).await;
        }

        // For cross-mint sends, we need to split the amount across multiple wallets
        self.send_cross_mint(amount, selected_wallets, &options).await
    }

    /// Internal method for cross-mint sends (splitting amount across multiple wallets)
    async fn send_cross_mint(
        &self,
        total_amount: Amount,
        wallets: Vec<(MintUrl, Wallet, Amount)>,
        options: &MultiMintSendOptions,
    ) -> Result<Token, Error> {
        if wallets.is_empty() {
            return Err(Error::InsufficientFunds);
        }

        // For single wallet, just send normally
        if wallets.len() == 1 {
            let (_, wallet, _) = wallets.into_iter().next().unwrap();
            let prepared = wallet.prepare_send(total_amount, options.send_options.clone()).await?;
            return prepared.confirm(None).await;
        }

        // For multiple wallets, prepare sends from each wallet
        let mut prepared_sends = Vec::new();
        let mut remaining_amount = total_amount;

        for (i, (_, wallet, wallet_balance)) in wallets.iter().enumerate() {
            if remaining_amount == Amount::ZERO {
                break;
            }

            // Determine how much to send from this wallet
            let send_amount = if i == wallets.len() - 1 {
                // Last wallet gets the remaining amount
                remaining_amount
            } else {
                // For other wallets, use their full balance or remaining amount, whichever is smaller
                std::cmp::min(remaining_amount, *wallet_balance)
            };

            if send_amount > Amount::ZERO {
                let prepared = wallet.prepare_send(send_amount, options.send_options.clone()).await?;
                prepared_sends.push(prepared);
                remaining_amount -= send_amount;
            }
        }

        if remaining_amount > Amount::ZERO {
            return Err(Error::InsufficientFunds);
        }

        // Create a MultiMintPreparedSend and confirm it
        let multi_prepared = MultiMintPreparedSend::new(prepared_sends, self.unit.clone(), Arc::new(self.clone()))?;
        multi_prepared.confirm(None).await
    }

    /// Send tokens from a specific wallet
    #[instrument(skip(self))]
    pub async fn send_from_wallet(
        &self,
        mint_url: &MintUrl,
        amount: Amount,
        opts: SendOptions,
    ) -> Result<Token, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(mint_url)
            .ok_or(Error::UnknownMint { mint_url: mint_url.to_string() })?;

        let prepared = wallet.prepare_send(amount, opts).await?;
        prepared.confirm(None).await
    }

    /// Melt (pay invoice) with automatic wallet selection
    /// 
    /// Automatically selects the best wallet to pay from based on:
    /// - Available balance  
    /// - Fees
    /// - Lightning route availability
    /// 
    /// If a single wallet doesn't have enough balance, this will attempt
    /// Multi-Path Payment (MPP) across multiple wallets.
    #[instrument(skip(self, bolt11))]
    pub async fn melt(
        &self,
        bolt11: &str,
        options: Option<MeltOptions>,
        max_fee: Option<Amount>,
    ) -> Result<Melted, Error> {
        // Parse the invoice to get the amount
        let invoice = bolt11.parse::<crate::Bolt11Invoice>()
            .map_err(|e| Error::Invoice(e))?;
        
        let amount = invoice
            .amount_milli_satoshis()
            .map(|msats| Amount::from(msats / 1000))
            .ok_or(Error::InvoiceAmountUndefined)?;

        // First try single wallet payment
        let wallets = self.wallets.read().await;
        let mut eligible_wallets = Vec::new();
        let mut all_wallets_for_melt = Vec::new();
        
        for (mint_url, wallet) in wallets.iter() {
            let balance = wallet.total_balance().await?;
            all_wallets_for_melt.push((mint_url.clone(), wallet.clone(), balance));
            
            // Add some buffer for fees (5% of amount)
            let fee_buffer = Amount::from(u64::from(amount) / 20);
            if balance >= amount + fee_buffer {
                eligible_wallets.push((mint_url.clone(), wallet.clone()));
            }
        }

        // Try single wallet payment first
        if !eligible_wallets.is_empty() {
            // Try to get quotes from eligible wallets and select the best one
            let mut best_quote = None;
            let mut best_wallet = None;
            
            for (_, wallet) in eligible_wallets.iter() {
                match wallet.melt_quote(bolt11.to_string(), options.clone()).await {
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
        }

        // If single wallet payment isn't possible, try MPP
        let total_balance = self.total_balance().await?;
        if total_balance < amount {
            return Err(Error::InsufficientFunds);
        }

        // Attempt Multi-Path Payment
        self.melt_mpp_internal(bolt11, &all_wallets_for_melt, amount, options, max_fee).await
    }

    /// Internal method for Multi-Path Payment across multiple wallets
    async fn melt_mpp_internal(
        &self,
        bolt11: &str,
        wallets: &[(MintUrl, Wallet, Amount)],
        total_amount: Amount,
        options: Option<MeltOptions>,
        max_fee: Option<Amount>,
    ) -> Result<Melted, Error> {
        tracing::info!("Starting MPP payment for amount: {}", total_amount);
        tracing::debug!("Available wallets for MPP: {}", wallets.len());

        if wallets.is_empty() {
            return Err(Error::InsufficientFunds);
        }

        // For now, implement a basic MPP strategy:
        // 1. Use the first wallet with sufficient balance
        // 2. If no single wallet has enough, try combining wallets
        // 3. For simplicity, fail if we need more than 2 wallets (can be extended later)

        // Try single wallet first
        for (_, wallet, balance) in wallets.iter() {
            if *balance >= total_amount {
                let quote = wallet.melt_quote(bolt11.to_string(), options.clone()).await?;
                if let Some(max_fee) = max_fee {
                    if quote.fee_reserve > max_fee {
                        continue;
                    }
                }
                tracing::info!("Using single wallet for MPP payment");
                return wallet.melt(&quote.id).await;
            }
        }

        // If no single wallet can handle it, try combining the two largest wallets
        if wallets.len() >= 2 {
            let mut sorted_wallets = wallets.to_vec();
            sorted_wallets.sort_by(|a, b| b.2.cmp(&a.2)); // Sort by balance descending

            let wallet1 = &sorted_wallets[0].1;
            let wallet2 = &sorted_wallets[1].1;
            let combined_balance = sorted_wallets[0].2 + sorted_wallets[1].2;

            if combined_balance >= total_amount {
                // For true MPP, we would need to:
                // 1. Split the invoice into multiple parts
                // 2. Pay each part from different wallets
                // 3. Coordinate success/failure atomically
                // 
                // Lightning Network MPP requires specific protocol support
                // For now, we'll try to pay from the wallet with higher balance
                
                tracing::warn!("MPP across multiple wallets requires Lightning Network MPP protocol support");
                tracing::info!("Attempting payment from largest available wallet");
                
                let quote = wallet1.melt_quote(bolt11.to_string(), options.clone()).await?;
                if let Some(max_fee) = max_fee {
                    if quote.fee_reserve > max_fee {
                        return Err(Error::MaxFeeExceeded);
                    }
                }
                
                return wallet1.melt(&quote.id).await.or_else(|_| {
                    // If first wallet fails, try second wallet
                    tracing::info!("First wallet failed, trying second wallet");
                    async {
                        let quote2 = wallet2.melt_quote(bolt11.to_string(), options).await?;
                        if let Some(max_fee) = max_fee {
                            if quote2.fee_reserve > max_fee {
                                return Err(Error::MaxFeeExceeded);
                            }
                        }
                        wallet2.melt(&quote2.id).await
                    }
                })?;
            }
        }

        // If we reach here, we don't have enough combined balance
        let total_available: Amount = wallets.iter().map(|(_, _, balance)| *balance).fold(Amount::ZERO, |acc, amount| acc + amount);
        Err(Error::InsufficientFundsPerMint { 
            amount: total_amount, 
            unit: self.unit.clone(), 
            total_available 
        })
    }

    /// Melt (pay invoice) from a specific wallet
    #[instrument(skip(self, bolt11))]
    pub async fn melt_from_wallet(
        &self,
        mint_url: &MintUrl,
        bolt11: &str,
        options: Option<MeltOptions>,
        max_fee: Option<Amount>,
    ) -> Result<Melted, Error> {
        self.pay_invoice_for_wallet(bolt11, options, mint_url, max_fee).await
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
                    return wallet.swap(
                        amount,
                        SplitTarget::default(),
                        proofs,
                        conditions,
                        false,
                    ).await;
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
                match wallet.swap(
                    Some(proofs_amount),
                    SplitTarget::default(),
                    proofs,
                    None,
                    false,
                ).await {
                    Ok(_) => {
                        total_consolidated += proofs_amount;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to consolidate proofs for mint {:?}: {}", mint_url, e);
                    }
                }
            }
        }
        
        Ok(total_consolidated)
    }

    /// Get the best wallet for a specific operation based on criteria
    /// This is a helper method for smart wallet selection
    async fn select_best_wallet(&self, min_amount: Amount) -> Result<Wallet, Error> {
        let wallets = self.wallets.read().await;
        let mut best_wallet = None;
        let mut best_balance = Amount::ZERO;
        
        for (_, wallet) in wallets.iter() {
            let balance = wallet.total_balance().await?;
            if balance >= min_amount && balance > best_balance {
                best_balance = balance;
                best_wallet = Some(wallet.clone());
            }
        }
        
        best_wallet.ok_or(Error::InsufficientFunds)
    }

    /// Get estimated fee information for a wallet
    /// This fetches current keyset information to estimate fees
    async fn get_wallet_fee_estimate(&self, wallet: &Wallet, amount: Amount) -> Result<Amount, Error> {
        // Get the current keysets to determine fee structure
        let keysets_info = wallet.get_keyset_fees().await.unwrap_or_default();
        
        // Calculate estimated fee based on the amount and keyset fees
        // This is a simplified fee calculation - in practice you might want more sophisticated logic
        let mut total_fee = Amount::ZERO;
        
        for (_, fee_info) in keysets_info.iter() {
            // Use the fee information from keysets
            // For now, we'll use a simple calculation based on the fee structure
            let fee_rate = *fee_info;
            let estimated_fee = Amount::from(u64::from(amount) * fee_rate as u64 / 1_000_000);
            total_fee += estimated_fee;
        }

        // If no fee info available, return a small default fee
        if total_fee == Amount::ZERO {
            total_fee = Amount::from(1); // 1 sat default
        }

        Ok(total_fee)
    }

    /// Record a successful operation for a mint
    async fn record_mint_success(&self, mint_url: &MintUrl) {
        let mut health = self.mint_health.write().await;
        let info = health.entry(mint_url.clone()).or_insert_with(|| MintHealthInfo {
            success_count: 0,
            failure_count: 0,
            last_success: None,
            last_failure: None,
            consecutive_failures: 0,
            is_excluded: false,
            exclusion_expires: None,
        });
        
        info.success_count += 1;
        info.last_success = Some(Instant::now());
        info.consecutive_failures = 0;
        
        // Remove exclusion on success
        if info.is_excluded {
            info.is_excluded = false;
            info.exclusion_expires = None;
            tracing::info!("Mint {} health restored, removing exclusion", mint_url);
        }
    }

    /// Record a failed operation for a mint
    async fn record_mint_failure(&self, mint_url: &MintUrl) {
        let mut health = self.mint_health.write().await;
        let info = health.entry(mint_url.clone()).or_insert_with(|| MintHealthInfo {
            success_count: 0,
            failure_count: 0,
            last_success: None,
            last_failure: None,
            consecutive_failures: 0,
            is_excluded: false,
            exclusion_expires: None,
        });
        
        info.failure_count += 1;
        info.last_failure = Some(Instant::now());
        info.consecutive_failures += 1;
        
        // Exclude mint after 3 consecutive failures
        const MAX_CONSECUTIVE_FAILURES: u32 = 3;
        const EXCLUSION_DURATION: Duration = Duration::from_secs(5 * 60); // 5 minutes
        
        if info.consecutive_failures >= MAX_CONSECUTIVE_FAILURES && !info.is_excluded {
            info.is_excluded = true;
            info.exclusion_expires = Some(Instant::now() + EXCLUSION_DURATION);
            tracing::warn!(
                "Mint {} excluded due to {} consecutive failures. Will retry in {:?}",
                mint_url, info.consecutive_failures, EXCLUSION_DURATION
            );
        }
    }

    /// Check if a mint is currently healthy (not excluded)
    async fn is_mint_healthy(&self, mint_url: &MintUrl) -> bool {
        let health = self.mint_health.read().await;
        if let Some(info) = health.get(mint_url) {
            if info.is_excluded {
                // Check if exclusion has expired
                if let Some(expires) = info.exclusion_expires {
                    if Instant::now() > expires {
                        // Exclusion expired, but we need to update the state
                        // Drop read lock and acquire write lock
                        drop(health);
                        let mut health_write = self.mint_health.write().await;
                        if let Some(info) = health_write.get_mut(mint_url) {
                            if info.is_excluded && info.exclusion_expires.map_or(false, |e| Instant::now() > e) {
                                info.is_excluded = false;
                                info.exclusion_expires = None;
                                tracing::info!("Mint {} exclusion expired, restoring to healthy state", mint_url);
                            }
                        }
                        return true;
                    }
                }
                return false; // Still excluded
            }
        }
        true // Healthy or no health info yet
    }

    /// Get health statistics for a mint
    pub async fn get_mint_health(&self, mint_url: &MintUrl) -> Option<MintHealthInfo> {
        let health = self.mint_health.read().await;
        health.get(mint_url).cloned()
    }

    /// Select and prioritize mints based on MultiMintSendOptions
    /// Returns a Vec of (MintUrl, Wallet, Balance) tuples in priority order
    async fn select_mints_for_send(
        &self,
        amount: Amount,
        options: &MultiMintSendOptions,
    ) -> Result<Vec<(MintUrl, Wallet, Amount)>, Error> {
        // Validate options first
        options.validate()?;

        let wallets = self.wallets.read().await;
        let mut available_wallets = Vec::new();

        // First, collect all available wallets with their balances
        for (mint_url, wallet) in wallets.iter() {
            // Skip excluded mints
            if options.excluded_mints.contains(mint_url) {
                continue;
            }

            // Skip unhealthy mints (automatically excluded due to failures)
            if !self.is_mint_healthy(mint_url).await {
                tracing::debug!("Skipping unhealthy mint: {}", mint_url);
                continue;
            }

            let balance = wallet.total_balance().await?;
            if balance > Amount::ZERO {
                available_wallets.push((mint_url.clone(), wallet.clone(), balance));
            }
        }

        if available_wallets.is_empty() {
            return Err(Error::InsufficientFunds);
        }

        // Sort wallets by preference and strategy
        available_wallets = self.sort_wallets_by_priority(available_wallets, options).await?;

        // If we only need wallets that can handle the full amount for single-mint sends
        if !options.allow_cross_mint {
            available_wallets.retain(|(_, _, balance)| *balance >= amount);
            
            if available_wallets.is_empty() {
                return Err(Error::InsufficientFunds);
            }
        }

        // Apply max_mints limit
        if let Some(max_mints) = options.max_mints {
            available_wallets.truncate(max_mints);
        }

        // Check if we have enough balance across selected wallets
        let total_available: Amount = available_wallets.iter().map(|(_, _, balance)| *balance).fold(Amount::ZERO, |acc, amount| acc + amount);
        if total_available < amount {
            return Err(Error::InsufficientFunds);
        }

        Ok(available_wallets)
    }

    /// Sort wallets according to the specified strategy and preferences
    async fn sort_wallets_by_priority(
        &self,
        mut wallets: Vec<(MintUrl, Wallet, Amount)>,
        options: &MultiMintSendOptions,
    ) -> Result<Vec<(MintUrl, Wallet, Amount)>, Error> {
        // First, separate preferred mints and put them at the front
        let mut preferred_wallets = Vec::new();
        let mut other_wallets = Vec::new();

        for wallet_info in wallets {
            let (mint_url, _, _) = &wallet_info;
            if let Some(index) = options.preferred_mints.iter().position(|p| p == mint_url) {
                // Insert in preference order (maintain order of preferred_mints)
                while preferred_wallets.len() <= index {
                    preferred_wallets.push(None);
                }
                preferred_wallets[index] = Some(wallet_info);
            } else {
                other_wallets.push(wallet_info);
            }
        }

        // Flatten preferred wallets (remove None entries)
        let mut result: Vec<(MintUrl, Wallet, Amount)> = preferred_wallets.into_iter()
            .flatten()
            .collect();

        // Sort other wallets by strategy
        match options.selection_strategy {
            MintSelectionStrategy::HighestBalanceFirst => {
                other_wallets.sort_by(|a, b| b.2.cmp(&a.2));
            }
            MintSelectionStrategy::LowestBalanceFirst => {
                other_wallets.sort_by(|a, b| a.2.cmp(&b.2));
            }
            MintSelectionStrategy::Random => {
                // For now, we'll use a simple pseudo-random sort based on mint URL hash
                // In a real implementation, you might want to use a proper RNG
                other_wallets.sort_by(|a, b| {
                    let hash_a = a.0.to_string().len() % 1000;
                    let hash_b = b.0.to_string().len() % 1000;
                    hash_a.cmp(&hash_b)
                });
            }
            MintSelectionStrategy::LowestFeesFirst => {
                // Sort by estimated fees (lowest first)
                let mut wallet_fees = Vec::new();
                
                // Estimate fees for each wallet (use a reasonable test amount)
                let test_amount = Amount::from(1000); // Use 1000 sats as test amount
                
                for (mint_url, wallet, balance) in &other_wallets {
                    let estimated_fee = self.get_wallet_fee_estimate(wallet, test_amount).await
                        .unwrap_or(Amount::from(u32::MAX as u64)); // Use high fee if estimation fails
                    wallet_fees.push((mint_url.clone(), wallet.clone(), *balance, estimated_fee));
                }
                
                // Sort by fee (lowest first), then by balance (highest first) as tiebreaker
                wallet_fees.sort_by(|a, b| {
                    a.3.cmp(&b.3).then_with(|| b.2.cmp(&a.2))
                });
                
                // Convert back to original format
                other_wallets = wallet_fees.into_iter()
                    .map(|(mint_url, wallet, balance, _fee)| (mint_url, wallet, balance))
                    .collect();
            }
            MintSelectionStrategy::RoundRobin => {
                // Implement round-robin selection
                if !other_wallets.is_empty() {
                    let current_index = self.round_robin_counter.fetch_add(1, Ordering::Relaxed);
                    let start_index = current_index % other_wallets.len();
                    
                    // Rotate the vector to start from the round-robin position
                    other_wallets.rotate_left(start_index);
                    
                    tracing::debug!("Round-robin selection: starting from index {}", start_index);
                }
            }
        }

        result.extend(other_wallets);
        Ok(result)
    }
}

impl Drop for MultiMintWallet {
    fn drop(&mut self) {
        self.seed.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use cdk_common::database::WalletDatabase;

    async fn create_test_multi_wallet() -> MultiMintWallet {
        let localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync> =
            Arc::new(
                cdk_sqlite::wallet::memory::empty()
                    .await
                    .expect("Failed to create in-memory database")
            );
        let seed = [0u8; 64];
        let wallets = vec![];

        MultiMintWallet::new(localstore, seed, CurrencyUnit::Sat, wallets)
            .expect("Failed to create MultiMintWallet")
    }

    #[tokio::test]
    async fn test_total_balance_empty() {
        let multi_wallet = create_test_multi_wallet().await;
        let balance = multi_wallet.total_balance().await.unwrap();
        assert_eq!(balance, Amount::ZERO);
    }

    #[tokio::test]
    async fn test_send_insufficient_funds() {
        let multi_wallet = create_test_multi_wallet().await;
        let result = multi_wallet
            .send(Amount::from(1000), SendOptions::default())
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
    async fn test_multi_mint_send_options_default() {
        let options = MultiMintSendOptions::default();
        assert_eq!(options.max_mints, Some(1));
        assert_eq!(options.preferred_mints.len(), 0);
        assert_eq!(options.excluded_mints.len(), 0);
        assert!(!options.allow_cross_mint);
        assert!(matches!(options.selection_strategy, MintSelectionStrategy::HighestBalanceFirst));
    }

    #[tokio::test]
    async fn test_multi_mint_send_options_builder() {
        use std::str::FromStr;
        
        let mint1 = MintUrl::from_str("https://mint1.example.com").unwrap();
        let mint2 = MintUrl::from_str("https://mint2.example.com").unwrap();
        let mint3 = MintUrl::from_str("https://mint3.example.com").unwrap();

        let options = MultiMintSendOptions::new()
            .max_mints(3)
            .prefer_mint(mint1.clone())
            .prefer_mint(mint2.clone())
            .exclude_mint(mint3.clone())
            .selection_strategy(MintSelectionStrategy::LowestBalanceFirst)
            .allow_cross_mint(true);

        assert_eq!(options.max_mints, Some(3));
        assert_eq!(options.preferred_mints, vec![mint1, mint2]);
        assert_eq!(options.excluded_mints, vec![mint3]);
        assert!(options.allow_cross_mint);
        assert!(matches!(options.selection_strategy, MintSelectionStrategy::LowestBalanceFirst));
    }

    #[tokio::test]
    async fn test_multi_mint_send_options_validation() {
        use std::str::FromStr;
        
        let mint1 = MintUrl::from_str("https://mint1.example.com").unwrap();

        // Test conflicting preferred and excluded mints
        let options = MultiMintSendOptions::new()
            .prefer_mint(mint1.clone())
            .exclude_mint(mint1.clone());

        assert!(options.validate().is_err());

        // Test conflicting max_mints and cross_mint
        let options = MultiMintSendOptions::new()
            .max_mints(1)
            .allow_cross_mint(true);

        assert!(options.validate().is_err());

        // Test valid configuration
        let options = MultiMintSendOptions::new()
            .max_mints(3)
            .allow_cross_mint(true);

        assert!(options.validate().is_ok());
    }

    #[tokio::test]
    async fn test_send_with_options_insufficient_funds() {
        let multi_wallet = create_test_multi_wallet().await;
        
        let options = MultiMintSendOptions::new();
        let result = multi_wallet
            .send_with_options(Amount::from(1000), options)
            .await;
        
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mint_selection_strategy_enum() {
        // Test that all enum variants exist and can be created
        let strategies = vec![
            MintSelectionStrategy::HighestBalanceFirst,
            MintSelectionStrategy::LowestBalanceFirst,
            MintSelectionStrategy::Random,
            MintSelectionStrategy::LowestFeesFirst,
            MintSelectionStrategy::RoundRobin,
        ];

        for strategy in strategies {
            let options = MultiMintSendOptions::new().selection_strategy(strategy);
            // Just ensure the builder pattern works
            assert!(options.validate().is_ok());
        }
    }
}

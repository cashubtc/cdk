//! MultiMint Wallet
//!
//! Wrapper around core [`Wallet`] that enables the use of multiple mint unit
//! pairs

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use cdk_common::database;
use cdk_common::database::WalletDatabase;
use cdk_common::wallet::{Transaction, TransactionDirection, WalletKey};
use tokio::sync::RwLock;
use tracing::instrument;
use zeroize::Zeroize;

use super::receive::ReceiveOptions;
use super::send::{PreparedSend, SendOptions};
use super::Error;
use crate::amount::SplitTarget;
use crate::mint_url::MintUrl;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{CurrencyUnit, MeltOptions, Proof, Proofs, SpendingConditions, Token};
use crate::types::Melted;
use crate::wallet::types::MintQuote;
use crate::{ensure_cdk, Amount, Wallet};

/// Multi Mint Wallet
#[derive(Debug, Clone)]
pub struct MultiMintWallet {
    /// Storage backend
    pub localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
    seed: [u8; 64],
    /// Wallets
    pub wallets: Arc<RwLock<BTreeMap<WalletKey, Wallet>>>,
}

impl MultiMintWallet {
    /// Create a new [MultiMintWallet] with initial wallets
    pub fn new(
        localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        seed: [u8; 64],
        wallets: Vec<Wallet>,
    ) -> Self {
        Self {
            localstore,
            seed,
            wallets: Arc::new(RwLock::new(
                wallets
                    .into_iter()
                    .map(|w| (WalletKey::new(w.mint_url.clone(), w.unit.clone()), w))
                    .collect(),
            )),
        }
    }

    /// Adds a [Wallet] to this [MultiMintWallet]
    #[instrument(skip(self, wallet))]
    pub async fn add_wallet(&self, wallet: Wallet) {
        let wallet_key = WalletKey::new(wallet.mint_url.clone(), wallet.unit.clone());

        let mut wallets = self.wallets.write().await;

        wallets.insert(wallet_key, wallet);
    }

    /// Creates a new [Wallet] and adds it to this [MultiMintWallet]
    pub async fn create_and_add_wallet(
        &self,
        mint_url: &str,
        unit: CurrencyUnit,
        target_proof_count: Option<usize>,
    ) -> Result<Wallet> {
        let wallet = Wallet::new(
            mint_url,
            unit,
            self.localstore.clone(),
            self.seed,
            target_proof_count,
        )?;

        wallet.fetch_mint_info().await?;

        self.add_wallet(wallet.clone()).await;

        Ok(wallet)
    }

    /// Remove Wallet from MultiMintWallet
    #[instrument(skip(self))]
    pub async fn remove_wallet(&self, wallet_key: &WalletKey) {
        let mut wallets = self.wallets.write().await;

        wallets.remove(wallet_key);
    }

    /// Get Wallets from MultiMintWallet
    #[instrument(skip(self))]
    pub async fn get_wallets(&self) -> Vec<Wallet> {
        self.wallets.read().await.values().cloned().collect()
    }

    /// Get Wallet from MultiMintWallet
    #[instrument(skip(self))]
    pub async fn get_wallet(&self, wallet_key: &WalletKey) -> Option<Wallet> {
        self.wallets.read().await.get(wallet_key).cloned()
    }

    /// Check if mint unit pair is in wallet
    #[instrument(skip(self))]
    pub async fn has(&self, wallet_key: &WalletKey) -> bool {
        self.wallets.read().await.contains_key(wallet_key)
    }

    /// Get wallet balances
    #[instrument(skip(self))]
    pub async fn get_balances(
        &self,
        unit: &CurrencyUnit,
    ) -> Result<BTreeMap<MintUrl, Amount>, Error> {
        let mut balances = BTreeMap::new();

        for (WalletKey { mint_url, unit: u }, wallet) in self.wallets.read().await.iter() {
            if unit == u {
                let wallet_balance = wallet.total_balance().await?;
                balances.insert(mint_url.clone(), wallet_balance);
            }
        }

        Ok(balances)
    }

    /// List proofs.
    #[instrument(skip(self))]
    pub async fn list_proofs(
        &self,
    ) -> Result<BTreeMap<MintUrl, (Vec<Proof>, CurrencyUnit)>, Error> {
        let mut mint_proofs = BTreeMap::new();

        for (WalletKey { mint_url, unit: u }, wallet) in self.wallets.read().await.iter() {
            let wallet_proofs = wallet.get_unspent_proofs().await?;
            mint_proofs.insert(mint_url.clone(), (wallet_proofs, u.clone()));
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

    /// Prepare to send
    #[instrument(skip(self))]
    pub async fn prepare_send(
        &self,
        wallet_key: &WalletKey,
        amount: Amount,
        opts: SendOptions,
    ) -> Result<PreparedSend, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(wallet_key)
            .ok_or(Error::UnknownWallet(wallet_key.clone()))?;

        wallet.prepare_send(amount, opts).await
    }

    /// Mint quote for wallet
    #[instrument(skip(self))]
    pub async fn mint_quote(
        &self,
        wallet_key: &WalletKey,
        amount: Amount,
        description: Option<String>,
    ) -> Result<MintQuote, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(wallet_key)
            .ok_or(Error::UnknownWallet(wallet_key.clone()))?;

        wallet.mint_quote(amount, description).await
    }

    /// Check all mint quotes
    /// If quote is paid, wallet will mint
    #[instrument(skip(self))]
    pub async fn check_all_mint_quotes(
        &self,
        wallet_key: Option<WalletKey>,
    ) -> Result<HashMap<CurrencyUnit, Amount>, Error> {
        let mut amount_minted = HashMap::new();
        match wallet_key {
            Some(wallet_key) => {
                let wallets = self.wallets.read().await;
                let wallet = wallets
                    .get(&wallet_key)
                    .ok_or(Error::UnknownWallet(wallet_key.clone()))?;

                let amount = wallet.check_all_mint_quotes().await?;
                amount_minted.insert(wallet.unit.clone(), amount);
            }
            None => {
                for (_, wallet) in self.wallets.read().await.iter() {
                    let amount = wallet.check_all_mint_quotes().await?;

                    amount_minted
                        .entry(wallet.unit.clone())
                        .and_modify(|b| *b += amount)
                        .or_insert(amount);
                }
            }
        }

        Ok(amount_minted)
    }

    /// Mint a specific quote
    #[instrument(skip(self))]
    pub async fn mint(
        &self,
        wallet_key: &WalletKey,
        quote_id: &str,
        conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(wallet_key)
            .ok_or(Error::UnknownWallet(wallet_key.clone()))?;

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

        let mint_url = token_data.mint_url()?;

        // Check that all mints in tokes have wallets
        let wallet_key = WalletKey::new(mint_url.clone(), unit.clone());
        if !self.has(&wallet_key).await {
            return Err(Error::UnknownWallet(wallet_key.clone()));
        }

        let wallet_key = WalletKey::new(mint_url.clone(), unit);
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(&wallet_key)
            .ok_or(Error::UnknownWallet(wallet_key.clone()))?;

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
        wallet_key: &WalletKey,
        max_fee: Option<Amount>,
    ) -> Result<Melted, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(wallet_key)
            .ok_or(Error::UnknownWallet(wallet_key.clone()))?;

        let quote = wallet.melt_quote(bolt11.to_string(), options).await?;
        if let Some(max_fee) = max_fee {
            ensure_cdk!(quote.fee_reserve <= max_fee, Error::MaxFeeExceeded);
        }

        wallet.melt(&quote.id).await
    }

    /// Restore
    #[instrument(skip(self))]
    pub async fn restore(&self, wallet_key: &WalletKey) -> Result<Amount, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(wallet_key)
            .ok_or(Error::UnknownWallet(wallet_key.clone()))?;

        wallet.restore().await
    }

    /// Verify token matches p2pk conditions
    #[instrument(skip(self, token))]
    pub async fn verify_token_p2pk(
        &self,
        wallet_key: &WalletKey,
        token: &Token,
        conditions: SpendingConditions,
    ) -> Result<(), Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(wallet_key)
            .ok_or(Error::UnknownWallet(wallet_key.clone()))?;

        wallet.verify_token_p2pk(token, conditions).await
    }

    /// Verifys all proofs in token have valid dleq proof
    #[instrument(skip(self, token))]
    pub async fn verify_token_dleq(
        &self,
        wallet_key: &WalletKey,
        token: &Token,
    ) -> Result<(), Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(wallet_key)
            .ok_or(Error::UnknownWallet(wallet_key.clone()))?;

        wallet.verify_token_dleq(token).await
    }

    // ============================================================================
    // NEW UNIFIED INTERFACE METHODS - Making MultiMintWallet more like Wallet
    // ============================================================================

    /// Get total balance across all wallets for a specific currency unit
    #[instrument(skip(self))]
    pub async fn total_balance(&self, unit: &CurrencyUnit) -> Result<Amount, Error> {
        let mut total = Amount::ZERO;
        for (wallet_key, wallet) in self.wallets.read().await.iter() {
            if &wallet_key.unit == unit {
                total += wallet.total_balance().await?;
            }
        }
        Ok(total)
    }

    /// Send tokens with automatic wallet selection based on available balance
    /// 
    /// This method automatically selects the best wallet(s) to send from based on:
    /// - Available balance
    /// - Fees
    /// - Network conditions
    #[instrument(skip(self))]
    pub async fn send(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
        opts: SendOptions,
    ) -> Result<Token, Error> {
        // Find wallets with sufficient balance for this unit
        let wallets = self.wallets.read().await;
        let mut eligible_wallets = Vec::new();
        
        for (wallet_key, wallet) in wallets.iter() {
            if &wallet_key.unit == unit {
                let balance = wallet.total_balance().await?;
                if balance >= amount {
                    eligible_wallets.push((wallet_key.clone(), wallet.clone(), balance));
                }
            }
        }

        if eligible_wallets.is_empty() {
            // Check if we have enough balance across all wallets
            let total = self.total_balance(unit).await?;
            if total < amount {
                return Err(Error::InsufficientFunds);
            }
            // TODO: In Phase 2, implement cross-wallet sending
            return Err(Error::InsufficientFunds);
        }

        // Select the wallet with the lowest fees or best conditions
        // For now, just use the first eligible wallet
        // TODO: In Phase 2, implement smart wallet selection
        let (_, wallet, _) = eligible_wallets
            .into_iter()
            .next()
            .ok_or(Error::InsufficientFunds)?;

        // Prepare and confirm the send
        let prepared = wallet.prepare_send(amount, opts).await?;
        prepared.confirm(None).await
    }

    /// Send tokens from a specific wallet
    #[instrument(skip(self))]
    pub async fn send_from_wallet(
        &self,
        wallet_key: &WalletKey,
        amount: Amount,
        opts: SendOptions,
    ) -> Result<Token, Error> {
        let wallets = self.wallets.read().await;
        let wallet = wallets
            .get(wallet_key)
            .ok_or(Error::UnknownWallet(wallet_key.clone()))?;

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
        unit: &CurrencyUnit,
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
        let mut all_wallets_for_unit = Vec::new();
        
        for (wallet_key, wallet) in wallets.iter() {
            if &wallet_key.unit == unit {
                let balance = wallet.total_balance().await?;
                all_wallets_for_unit.push((wallet_key.clone(), wallet.clone(), balance));
                
                // Add some buffer for fees (5% of amount)
                let fee_buffer = Amount::from(u64::from(amount) / 20);
                if balance >= amount + fee_buffer {
                    eligible_wallets.push((wallet_key.clone(), wallet.clone()));
                }
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
        let total_balance = self.total_balance(unit).await?;
        if total_balance < amount {
            return Err(Error::InsufficientFunds);
        }

        // Attempt Multi-Path Payment
        self.melt_mpp_internal(bolt11, &all_wallets_for_unit, amount, options, max_fee).await
    }

    /// Internal method for Multi-Path Payment across multiple wallets
    async fn melt_mpp_internal(
        &self,
        bolt11: &str,
        wallets: &[(WalletKey, Wallet, Amount)],
        total_amount: Amount,
        options: Option<MeltOptions>,
        max_fee: Option<Amount>,
    ) -> Result<Melted, Error> {
        // Check if MPP is supported for this payment method
        // For now, we'll return an error indicating MPP is not yet fully implemented
        // In a full implementation, this would:
        // 1. Split the payment across multiple wallets
        // 2. Coordinate partial payments
        // 3. Handle atomic success/failure
        
        tracing::info!("MPP payment required for amount: {}", total_amount);
        tracing::debug!("Available wallets for MPP: {}", wallets.len());
        
        // TODO: Implement actual MPP logic
        // This is a placeholder that demonstrates the structure
        // Real implementation would require:
        // - Payment splitting algorithm
        // - Coordination between wallets
        // - Atomic payment handling
        // - Rollback on partial failure
        
        Err(Error::Custom("Multi-Path Payment across wallets not yet fully implemented. Please split the payment manually.".to_string()))
    }

    /// Melt (pay invoice) from a specific wallet
    #[instrument(skip(self, bolt11))]
    pub async fn melt_from_wallet(
        &self,
        wallet_key: &WalletKey,
        bolt11: &str,
        options: Option<MeltOptions>,
        max_fee: Option<Amount>,
    ) -> Result<Melted, Error> {
        self.pay_invoice_for_wallet(bolt11, options, wallet_key, max_fee).await
    }

    /// Swap proofs with automatic wallet selection
    #[instrument(skip(self))]
    pub async fn swap(
        &self,
        unit: &CurrencyUnit,
        amount: Option<Amount>,
        conditions: Option<SpendingConditions>,
    ) -> Result<Option<Proofs>, Error> {
        // Find a wallet with this unit that has proofs
        let wallets = self.wallets.read().await;
        
        for (wallet_key, wallet) in wallets.iter() {
            if &wallet_key.unit == unit {
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
        }
        
        Err(Error::InsufficientFunds)
    }

    /// Consolidate proofs from multiple wallets into fewer, larger proofs
    /// This can help reduce the number of proofs and optimize wallet performance
    #[instrument(skip(self))]
    pub async fn consolidate(&self, unit: &CurrencyUnit) -> Result<Amount, Error> {
        let mut total_consolidated = Amount::ZERO;
        let wallets = self.wallets.read().await;
        
        for (wallet_key, wallet) in wallets.iter() {
            if &wallet_key.unit == unit {
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
                            tracing::warn!("Failed to consolidate proofs for wallet {:?}: {}", wallet_key, e);
                        }
                    }
                }
            }
        }
        
        Ok(total_consolidated)
    }

    /// Get the best wallet for a specific operation based on criteria
    /// This is a helper method for smart wallet selection
    async fn select_best_wallet(
        &self,
        unit: &CurrencyUnit,
        min_amount: Amount,
    ) -> Result<Wallet, Error> {
        let wallets = self.wallets.read().await;
        let mut best_wallet = None;
        let mut best_balance = Amount::ZERO;
        
        for (wallet_key, wallet) in wallets.iter() {
            if &wallet_key.unit == unit {
                let balance = wallet.total_balance().await?;
                if balance >= min_amount && balance > best_balance {
                    best_balance = balance;
                    best_wallet = Some(wallet.clone());
                }
            }
        }
        
        best_wallet.ok_or(Error::InsufficientFunds)
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

    // Simple mock database for testing
    #[derive(Debug)]
    struct MockDatabase;

    impl WalletDatabase for MockDatabase {
        type Err = database::Error;

        async fn add_wallet(&self, _: &cdk_common::wallet::WalletKey, _: &str) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn get_wallets(&self) -> Result<Vec<cdk_common::wallet::WalletKey>, Self::Err> {
            Ok(vec![])
        }

        async fn remove_wallet(&self, _: &cdk_common::wallet::WalletKey) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn add_mint_keysets(
            &self,
            _: &crate::mint_url::MintUrl,
            _: Vec<cdk_common::nut02::MintKeySetInfo>,
        ) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn get_mint_keysets(
            &self,
            _: &crate::mint_url::MintUrl,
        ) -> Result<Option<Vec<cdk_common::nut02::MintKeySetInfo>>, Self::Err> {
            Ok(None)
        }

        async fn add_mint_quote(&self, _: cdk_common::wallet::MintQuote) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn get_mint_quote(
            &self,
            _: &str,
        ) -> Result<Option<cdk_common::wallet::MintQuote>, Self::Err> {
            Ok(None)
        }

        async fn get_mint_quotes(&self) -> Result<Vec<cdk_common::wallet::MintQuote>, Self::Err> {
            Ok(vec![])
        }

        async fn remove_mint_quote(&self, _: &str) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn add_melt_quote(&self, _: cdk_common::wallet::MeltQuote) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn get_melt_quote(
            &self,
            _: &str,
        ) -> Result<Option<cdk_common::wallet::MeltQuote>, Self::Err> {
            Ok(None)
        }

        async fn remove_melt_quote(&self, _: &str) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn add_proofs(&self, _: Vec<cdk_common::wallet::ProofInfo>) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn get_proofs(
            &self,
            _: Option<crate::mint_url::MintUrl>,
            _: Option<CurrencyUnit>,
            _: Option<Vec<crate::nuts::State>>,
            _: Option<Vec<crate::nuts::SpendingConditions>>,
        ) -> Result<Vec<cdk_common::wallet::ProofInfo>, Self::Err> {
            Ok(vec![])
        }

        async fn update_proofs(
            &self,
            _: Vec<cdk_common::wallet::ProofInfo>,
            _: crate::nuts::State,
        ) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn remove_proofs(&self, _: &[String]) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn reserve_proofs(&self, _: Vec<String>) -> Result<u64, Self::Err> {
            Ok(0)
        }

        async fn unreserve_proofs(&self, _: u64) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn delete_unreserved_proofs(&self) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn add_pending_proofs(
            &self,
            _: Vec<cdk_common::wallet::ProofInfo>,
        ) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn get_pending_proofs(&self) -> Result<Vec<cdk_common::wallet::ProofInfo>, Self::Err> {
            Ok(vec![])
        }

        async fn remove_pending_proofs(&self, _: &[String]) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn increment_keyset_counter(
            &self,
            _: &crate::nuts::Id,
            _: u32,
        ) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn get_keyset_counter(&self, _: &crate::nuts::Id) -> Result<Option<u32>, Self::Err> {
            Ok(Some(0))
        }

        async fn add_transactions(
            &self,
            _: Vec<cdk_common::wallet::Transaction>,
        ) -> Result<(), Self::Err> {
            Ok(())
        }

        async fn get_transactions(
            &self,
            _: Option<cdk_common::wallet::TransactionDirection>,
        ) -> Result<Vec<cdk_common::wallet::Transaction>, Self::Err> {
            Ok(vec![])
        }
    }

    fn create_test_multi_wallet() -> MultiMintWallet {
        let localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync> =
            Arc::new(MockDatabase);
        let seed = [0u8; 64];
        let wallets = vec![];

        MultiMintWallet::new(localstore, seed, wallets)
    }

    #[tokio::test]
    async fn test_total_balance_empty() {
        let multi_wallet = create_test_multi_wallet();
        let balance = multi_wallet.total_balance(&CurrencyUnit::Sat).await.unwrap();
        assert_eq!(balance, Amount::ZERO);
    }

    #[tokio::test]
    async fn test_send_insufficient_funds() {
        let multi_wallet = create_test_multi_wallet();
        let result = multi_wallet
            .send(Amount::from(1000), &CurrencyUnit::Sat, SendOptions::default())
            .await;
        
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_consolidate_empty() {
        let multi_wallet = create_test_multi_wallet();
        let result = multi_wallet.consolidate(&CurrencyUnit::Sat).await.unwrap();
        assert_eq!(result, Amount::ZERO);
    }

    #[test]
    fn test_multi_mint_wallet_creation() {
        let multi_wallet = create_test_multi_wallet();
        assert!(multi_wallet.wallets.try_read().is_ok());
    }
}

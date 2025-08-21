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
}

impl Drop for MultiMintWallet {
    fn drop(&mut self) {
        self.seed.zeroize();
    }
}

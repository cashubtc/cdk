//! MultiMint Wallet
//!
//! Wrapper around core [`Wallet`] that enables the use of multiple mint unit pairs

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::instrument;

use super::Error;
use crate::amount::SplitTarget;
use crate::nuts::{CurrencyUnit, SecretKey, SpendingConditions, Token};
use crate::types::Melted;
use crate::wallet::types::MintQuote;
use crate::{Amount, UncheckedUrl, Wallet};

/// Multi Mint Wallet
#[derive(Debug, Clone)]
pub struct MultiMintWallet {
    /// Wallets
    pub wallets: Arc<Mutex<HashMap<WalletKey, Wallet>>>,
}

/// Wallet Key
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletKey {
    mint_url: UncheckedUrl,
    unit: CurrencyUnit,
}

impl fmt::Display for WalletKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mint_url: {}, unit: {}", self.mint_url, self.unit,)
    }
}

impl WalletKey {
    /// Create new [`WalletKey`]
    pub fn new(mint_url: UncheckedUrl, unit: CurrencyUnit) -> Self {
        Self { mint_url, unit }
    }
}

impl MultiMintWallet {
    /// New Multimint wallet
    pub fn new(wallets: Vec<Wallet>) -> Self {
        Self {
            wallets: Arc::new(Mutex::new(
                wallets
                    .into_iter()
                    .map(|w| (WalletKey::new(w.mint_url.clone(), w.unit), w))
                    .collect(),
            )),
        }
    }

    /// Add wallet to MultiMintWallet
    #[instrument(skip(self, wallet))]
    pub async fn add_wallet(&self, wallet: Wallet) {
        let wallet_key = WalletKey::new(wallet.mint_url.clone(), wallet.unit);

        let mut wallets = self.wallets.lock().await;

        wallets.insert(wallet_key, wallet);
    }

    /// Remove Wallet from MultiMintWallet
    #[instrument(skip(self))]
    pub async fn remove_wallet(&self, wallet_key: &WalletKey) {
        let mut wallets = self.wallets.lock().await;

        wallets.remove(wallet_key);
    }

    /// Get Wallets from MultiMintWallet
    #[instrument(skip(self))]
    pub async fn get_wallets(&self) -> Vec<Wallet> {
        self.wallets.lock().await.values().cloned().collect()
    }

    /// Get Wallet from MultiMintWallet
    #[instrument(skip(self))]
    pub async fn get_wallet(&self, wallet_key: &WalletKey) -> Option<Wallet> {
        let wallets = self.wallets.lock().await;

        wallets.get(wallet_key).cloned()
    }

    /// Check if mint unit pair is in wallet
    #[instrument(skip(self))]
    pub async fn has(&self, wallet_key: &WalletKey) -> bool {
        self.wallets.lock().await.contains_key(wallet_key)
    }

    /// Get wallet balances
    #[instrument(skip(self))]
    pub async fn get_balances(
        &self,
        unit: &CurrencyUnit,
    ) -> Result<HashMap<UncheckedUrl, Amount>, Error> {
        let mut balances = HashMap::new();

        for (WalletKey { mint_url, unit: u }, wallet) in self.wallets.lock().await.iter() {
            if unit == u {
                let wallet_balance = wallet.total_balance().await?;
                balances.insert(mint_url.clone(), wallet_balance);
            }
        }

        Ok(balances)
    }

    /// Create cashu token
    #[instrument(skip(self))]
    pub async fn send(
        &self,
        wallet_key: &WalletKey,
        amount: Amount,
        memo: Option<String>,
        conditions: Option<SpendingConditions>,
    ) -> Result<String, Error> {
        let wallet = self
            .get_wallet(wallet_key)
            .await
            .ok_or(Error::UnknownWallet(wallet_key.to_string()))?;

        wallet
            .send(amount, memo, conditions, &SplitTarget::default())
            .await
    }

    /// Mint quote for wallet
    #[instrument(skip(self))]
    pub async fn mint_quote(
        &self,
        wallet_key: &WalletKey,
        amount: Amount,
    ) -> Result<MintQuote, Error> {
        let wallet = self
            .get_wallet(wallet_key)
            .await
            .ok_or(Error::UnknownWallet(wallet_key.to_string()))?;

        wallet.mint_quote(amount).await
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
                let wallet = self
                    .get_wallet(&wallet_key)
                    .await
                    .ok_or(Error::UnknownWallet(wallet_key.to_string()))?;

                let amount = wallet.check_all_mint_quotes().await?;
                amount_minted.insert(wallet.unit, amount);
            }
            None => {
                for (_, wallet) in self.wallets.lock().await.iter() {
                    let amount = wallet.check_all_mint_quotes().await?;

                    amount_minted
                        .entry(wallet.unit)
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
    ) -> Result<Amount, Error> {
        let wallet = self
            .get_wallet(wallet_key)
            .await
            .ok_or(Error::UnknownWallet(wallet_key.to_string()))?;
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
        p2pk_signing_keys: &[SecretKey],
        preimages: &[String],
    ) -> Result<Amount, Error> {
        let token_data = Token::from_str(encoded_token)?;
        let unit = token_data.unit().unwrap_or_default();

        let mint_proofs = token_data.proofs();

        let mut amount_received = Amount::ZERO;

        // Check that all mints in tokes have wallets
        for (mint_url, proofs) in mint_proofs {
            let wallet_key = WalletKey::new(mint_url.clone(), unit);
            if !self.has(&wallet_key).await {
                return Err(Error::UnknownWallet(wallet_key.to_string()));
            }

            let wallet_key = WalletKey::new(mint_url, unit);
            let wallet = self
                .get_wallet(&wallet_key)
                .await
                .ok_or(Error::UnknownWallet(wallet_key.to_string()))?;

            let amount = wallet
                .receive_proofs(
                    proofs,
                    &SplitTarget::default(),
                    p2pk_signing_keys,
                    preimages,
                )
                .await?;

            amount_received += amount;
        }

        Ok(amount_received)
    }

    /// Pay an bolt11 invoice from specific wallet
    #[instrument(skip(self, bolt11))]
    pub async fn pay_invoice_for_wallet(
        &self,
        bolt11: &str,
        wallet_key: &WalletKey,
        max_fee: Option<Amount>,
    ) -> Result<Melted, Error> {
        let wallet = self
            .get_wallet(wallet_key)
            .await
            .ok_or(Error::UnknownWallet(wallet_key.to_string()))?;

        let quote = wallet.melt_quote(bolt11.to_string(), None).await?;
        if let Some(max_fee) = max_fee {
            if quote.fee_reserve > max_fee {
                return Err(Error::MaxFeeExceeded);
            }
        }

        wallet.melt(&quote.id, SplitTarget::default()).await
    }

    /// Restore
    #[instrument(skip(self))]
    pub async fn restore(&self, wallet_key: &WalletKey) -> Result<Amount, Error> {
        let wallet = self
            .get_wallet(wallet_key)
            .await
            .ok_or(Error::UnknownWallet(wallet_key.to_string()))?;

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
        let wallet = self
            .get_wallet(wallet_key)
            .await
            .ok_or(Error::UnknownWallet(wallet_key.to_string()))?;

        wallet.verify_token_p2pk(token, conditions)
    }

    /// Verifys all proofs in toke have valid dleq proof
    #[instrument(skip(self, token))]
    pub async fn verify_token_dleq(
        &self,
        wallet_key: &WalletKey,
        token: &Token,
    ) -> Result<(), Error> {
        let wallet = self
            .get_wallet(wallet_key)
            .await
            .ok_or(Error::UnknownWallet(wallet_key.to_string()))?;

        wallet.verify_token_dleq(token).await
    }
}

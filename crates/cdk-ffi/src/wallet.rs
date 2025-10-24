//! FFI Wallet bindings

use std::str::FromStr;
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::{Wallet as CdkWallet, WalletBuilder as CdkWalletBuilder};

use crate::error::FfiError;
use crate::token::Token;
use crate::types::*;

/// FFI-compatible Wallet
#[derive(uniffi::Object)]
pub struct Wallet {
    inner: Arc<CdkWallet>,
}

#[uniffi::export(async_runtime = "tokio")]
impl Wallet {
    /// Create a new Wallet from mnemonic using WalletDatabase trait
    #[uniffi::constructor]
    pub fn new(
        mint_url: String,
        unit: CurrencyUnit,
        mnemonic: String,
        db: Arc<dyn crate::database::WalletDatabase>,
        config: WalletConfig,
    ) -> Result<Self, FfiError> {
        // Parse mnemonic and generate seed without passphrase
        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| FfiError::InvalidMnemonic { msg: e.to_string() })?;
        let seed = m.to_seed_normalized("");

        // Convert the FFI database trait to a CDK database implementation
        let localstore = crate::database::create_cdk_database_from_ffi(db);

        let wallet =
            CdkWalletBuilder::new()
                .mint_url(mint_url.parse().map_err(|e: cdk::mint_url::Error| {
                    FfiError::InvalidUrl { msg: e.to_string() }
                })?)
                .unit(unit.into())
                .localstore(localstore)
                .seed(seed)
                .target_proof_count(config.target_proof_count.unwrap_or(3) as usize)
                .build()
                .map_err(FfiError::from)?;

        Ok(Self {
            inner: Arc::new(wallet),
        })
    }

    /// Get the mint URL
    pub fn mint_url(&self) -> MintUrl {
        self.inner.mint_url.clone().into()
    }

    /// Get the currency unit
    pub fn unit(&self) -> CurrencyUnit {
        self.inner.unit.clone().into()
    }

    /// Get total balance
    pub async fn total_balance(&self) -> Result<Amount, FfiError> {
        let balance = self.inner.total_balance().await?;
        Ok(balance.into())
    }

    /// Get total pending balance
    pub async fn total_pending_balance(&self) -> Result<Amount, FfiError> {
        let balance = self.inner.total_pending_balance().await?;
        Ok(balance.into())
    }

    /// Get total reserved balance
    pub async fn total_reserved_balance(&self) -> Result<Amount, FfiError> {
        let balance = self.inner.total_reserved_balance().await?;
        Ok(balance.into())
    }

    /// Get mint info
    pub async fn get_mint_info(&self) -> Result<Option<MintInfo>, FfiError> {
        let info = self.inner.fetch_mint_info().await?;
        Ok(info.map(Into::into))
    }

    /// Receive tokens
    pub async fn receive(
        &self,
        token: std::sync::Arc<Token>,
        options: ReceiveOptions,
    ) -> Result<Amount, FfiError> {
        let amount = self
            .inner
            .receive(&token.to_string(), options.into())
            .await?;
        Ok(amount.into())
    }

    /// Restore wallet from seed
    pub async fn restore(&self) -> Result<Amount, FfiError> {
        let amount = self.inner.restore().await?;
        Ok(amount.into())
    }

    /// Verify token DLEQ proofs
    pub async fn verify_token_dleq(&self, token: std::sync::Arc<Token>) -> Result<(), FfiError> {
        let cdk_token = token.inner.clone();
        self.inner.verify_token_dleq(&cdk_token).await?;
        Ok(())
    }

    /// Receive proofs directly
    pub async fn receive_proofs(
        &self,
        proofs: Proofs,
        options: ReceiveOptions,
        memo: Option<String>,
    ) -> Result<Amount, FfiError> {
        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            proofs.into_iter().map(|p| p.try_into()).collect();
        let cdk_proofs = cdk_proofs?;

        let amount = self
            .inner
            .receive_proofs(cdk_proofs, options.into(), memo)
            .await?;
        Ok(amount.into())
    }

    /// Prepare a send operation
    pub async fn prepare_send(
        &self,
        amount: Amount,
        options: SendOptions,
    ) -> Result<std::sync::Arc<PreparedSend>, FfiError> {
        let prepared = self
            .inner
            .prepare_send(amount.into(), options.into())
            .await?;
        Ok(std::sync::Arc::new(prepared.into()))
    }

    /// Get a mint quote
    pub async fn mint_quote(
        &self,
        amount: Amount,
        description: Option<String>,
    ) -> Result<MintQuote, FfiError> {
        let quote = self.inner.mint_quote(amount.into(), description).await?;
        Ok(quote.into())
    }

    /// Mint tokens
    pub async fn mint(
        &self,
        quote_id: String,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, FfiError> {
        // Convert spending conditions if provided
        let conditions = spending_conditions.map(|sc| sc.try_into()).transpose()?;

        let proofs = self
            .inner
            .mint(&quote_id, amount_split_target.into(), conditions)
            .await?;
        Ok(proofs.into_iter().map(|p| p.into()).collect())
    }

    /// Get a melt quote
    pub async fn melt_quote(
        &self,
        request: String,
        options: Option<MeltOptions>,
    ) -> Result<MeltQuote, FfiError> {
        let cdk_options = options.map(Into::into);
        let quote = self.inner.melt_quote(request, cdk_options).await?;
        Ok(quote.into())
    }

    /// Melt tokens
    pub async fn melt(&self, quote_id: String) -> Result<Melted, FfiError> {
        let melted = self.inner.melt(&quote_id).await?;
        Ok(melted.into())
    }

    /// Get a quote for a bolt12 mint
    pub async fn mint_bolt12_quote(
        &self,
        amount: Option<Amount>,
        description: Option<String>,
    ) -> Result<MintQuote, FfiError> {
        let quote = self
            .inner
            .mint_bolt12_quote(amount.map(Into::into), description)
            .await?;
        Ok(quote.into())
    }

    /// Mint tokens using bolt12
    pub async fn mint_bolt12(
        &self,
        quote_id: String,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, FfiError> {
        let conditions = spending_conditions.map(|sc| sc.try_into()).transpose()?;

        let proofs = self
            .inner
            .mint_bolt12(
                &quote_id,
                amount.map(Into::into),
                amount_split_target.into(),
                conditions,
            )
            .await?;

        Ok(proofs.into_iter().map(|p| p.into()).collect())
    }

    /// Get a quote for a bolt12 melt
    pub async fn melt_bolt12_quote(
        &self,
        request: String,
        options: Option<MeltOptions>,
    ) -> Result<MeltQuote, FfiError> {
        let cdk_options = options.map(Into::into);
        let quote = self.inner.melt_bolt12_quote(request, cdk_options).await?;
        Ok(quote.into())
    }

    /// Swap proofs
    pub async fn swap(
        &self,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
    ) -> Result<Option<Proofs>, FfiError> {
        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            input_proofs.into_iter().map(|p| p.try_into()).collect();
        let cdk_proofs = cdk_proofs?;

        // Convert spending conditions if provided
        let conditions = spending_conditions.map(|sc| sc.try_into()).transpose()?;

        let result = self
            .inner
            .swap(
                amount.map(Into::into),
                amount_split_target.into(),
                cdk_proofs,
                conditions,
                include_fees,
            )
            .await?;

        Ok(result.map(|proofs| proofs.into_iter().map(|p| p.into()).collect()))
    }

    /// Get proofs by states
    pub async fn get_proofs_by_states(&self, states: Vec<ProofState>) -> Result<Proofs, FfiError> {
        let mut all_proofs = Vec::new();

        for state in states {
            let proofs = match state {
                ProofState::Unspent => self.inner.get_unspent_proofs().await?,
                ProofState::Pending => self.inner.get_pending_proofs().await?,
                ProofState::Reserved => self.inner.get_reserved_proofs().await?,
                ProofState::PendingSpent => self.inner.get_pending_spent_proofs().await?,
                ProofState::Spent => {
                    // CDK doesn't have a method to get spent proofs directly
                    // They are removed from the database when spent
                    continue;
                }
            };

            for proof in proofs {
                all_proofs.push(proof.into());
            }
        }

        Ok(all_proofs)
    }

    /// Check if proofs are spent
    pub async fn check_proofs_spent(&self, proofs: Proofs) -> Result<Vec<bool>, FfiError> {
        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            proofs.into_iter().map(|p| p.try_into()).collect();
        let cdk_proofs = cdk_proofs?;

        let proof_states = self.inner.check_proofs_spent(cdk_proofs).await?;
        // Convert ProofState to bool (spent = true, unspent = false)
        let spent_bools = proof_states
            .into_iter()
            .map(|proof_state| {
                matches!(
                    proof_state.state,
                    cdk::nuts::State::Spent | cdk::nuts::State::PendingSpent
                )
            })
            .collect();
        Ok(spent_bools)
    }

    /// List transactions
    pub async fn list_transactions(
        &self,
        direction: Option<TransactionDirection>,
    ) -> Result<Vec<Transaction>, FfiError> {
        let cdk_direction = direction.map(Into::into);
        let transactions = self.inner.list_transactions(cdk_direction).await?;
        Ok(transactions.into_iter().map(Into::into).collect())
    }

    /// Get transaction by ID
    pub async fn get_transaction(
        &self,
        id: TransactionId,
    ) -> Result<Option<Transaction>, FfiError> {
        let cdk_id = id.try_into()?;
        let transaction = self.inner.get_transaction(cdk_id).await?;
        Ok(transaction.map(Into::into))
    }

    /// Revert a transaction
    pub async fn revert_transaction(&self, id: TransactionId) -> Result<(), FfiError> {
        let cdk_id = id.try_into()?;
        self.inner.revert_transaction(cdk_id).await?;
        Ok(())
    }

    /// Subscribe to wallet events
    pub async fn subscribe(
        &self,
        params: SubscribeParams,
    ) -> Result<std::sync::Arc<ActiveSubscription>, FfiError> {
        let cdk_params: cdk::nuts::nut17::Params<Arc<String>> = params.clone().into();
        let sub_id = cdk_params.id.to_string();
        let active_sub = self.inner.subscribe(cdk_params).await;
        Ok(std::sync::Arc::new(ActiveSubscription::new(
            active_sub, sub_id,
        )))
    }

    /// Refresh keysets from the mint
    pub async fn refresh_keysets(&self) -> Result<Vec<KeySetInfo>, FfiError> {
        let keysets = self.inner.refresh_keysets().await?;
        Ok(keysets.into_iter().map(Into::into).collect())
    }

    /// Get the active keyset for the wallet's unit
    pub async fn get_active_keyset(&self) -> Result<KeySetInfo, FfiError> {
        let keyset = self.inner.get_active_keyset().await?;
        Ok(keyset.into())
    }

    /// Get fees for a specific keyset ID
    pub async fn get_keyset_fees_by_id(&self, keyset_id: String) -> Result<u64, FfiError> {
        let id = cdk::nuts::Id::from_str(&keyset_id)
            .map_err(|e| FfiError::Generic { msg: e.to_string() })?;
        Ok(self
            .inner
            .get_keyset_fees_and_amounts_by_id(id)
            .await?
            .fee())
    }

    /// Reclaim unspent proofs (mark them as unspent in the database)
    pub async fn reclaim_unspent(&self, proofs: Proofs) -> Result<(), FfiError> {
        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            proofs.iter().map(|p| p.clone().try_into()).collect();
        let cdk_proofs = cdk_proofs?;
        self.inner.reclaim_unspent(cdk_proofs).await?;
        Ok(())
    }

    /// Check all pending proofs and return the total amount reclaimed
    pub async fn check_all_pending_proofs(&self) -> Result<Amount, FfiError> {
        let amount = self.inner.check_all_pending_proofs().await?;
        Ok(amount.into())
    }

    /// Calculate fee for a given number of proofs with the specified keyset
    pub async fn calculate_fee(
        &self,
        proof_count: u32,
        keyset_id: String,
    ) -> Result<Amount, FfiError> {
        let id = cdk::nuts::Id::from_str(&keyset_id)
            .map_err(|e| FfiError::Generic { msg: e.to_string() })?;
        let fee = self
            .inner
            .get_keyset_count_fee(&id, proof_count as u64)
            .await?;
        Ok(fee.into())
    }
}

/// BIP353 methods for Wallet
#[cfg(not(target_arch = "wasm32"))]
#[uniffi::export(async_runtime = "tokio")]
impl Wallet {
    /// Get a quote for a BIP353 melt
    ///
    /// This method resolves a BIP353 address (e.g., "alice@example.com") to a Lightning offer
    /// and then creates a melt quote for that offer.
    pub async fn melt_bip353_quote(
        &self,
        bip353_address: String,
        amount_msat: Amount,
    ) -> Result<MeltQuote, FfiError> {
        let cdk_amount: cdk::Amount = amount_msat.into();
        let quote = self
            .inner
            .melt_bip353_quote(&bip353_address, cdk_amount)
            .await?;
        Ok(quote.into())
    }
}

/// Auth methods for Wallet
#[uniffi::export(async_runtime = "tokio")]
impl Wallet {
    /// Set Clear Auth Token (CAT) for authentication
    pub async fn set_cat(&self, cat: String) -> Result<(), FfiError> {
        self.inner.set_cat(cat).await?;
        Ok(())
    }

    /// Set refresh token for authentication
    pub async fn set_refresh_token(&self, refresh_token: String) -> Result<(), FfiError> {
        self.inner.set_refresh_token(refresh_token).await?;
        Ok(())
    }

    /// Refresh access token using the stored refresh token
    pub async fn refresh_access_token(&self) -> Result<(), FfiError> {
        self.inner.refresh_access_token().await?;
        Ok(())
    }

    /// Mint blind auth tokens
    pub async fn mint_blind_auth(&self, amount: Amount) -> Result<Proofs, FfiError> {
        let proofs = self.inner.mint_blind_auth(amount.into()).await?;
        Ok(proofs.into_iter().map(|p| p.into()).collect())
    }

    /// Get unspent auth proofs
    pub async fn get_unspent_auth_proofs(&self) -> Result<Vec<AuthProof>, FfiError> {
        let auth_proofs = self.inner.get_unspent_auth_proofs().await?;
        Ok(auth_proofs.into_iter().map(Into::into).collect())
    }
}

/// Configuration for creating wallets
#[derive(Debug, Clone, uniffi::Record)]
pub struct WalletConfig {
    pub target_proof_count: Option<u32>,
}

/// Generates a new random mnemonic phrase
#[uniffi::export]
pub fn generate_mnemonic() -> Result<String, FfiError> {
    let mnemonic =
        Mnemonic::generate(12).map_err(|e| FfiError::InvalidMnemonic { msg: e.to_string() })?;
    Ok(mnemonic.to_string())
}

/// Converts a mnemonic phrase to its entropy bytes
#[uniffi::export]
pub fn mnemonic_to_entropy(mnemonic: String) -> Result<Vec<u8>, FfiError> {
    let m =
        Mnemonic::parse(&mnemonic).map_err(|e| FfiError::InvalidMnemonic { msg: e.to_string() })?;
    Ok(m.to_entropy())
}

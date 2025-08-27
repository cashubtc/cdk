//! FFI Wallet bindings

use std::str::FromStr;
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::{Wallet as CdkWallet, WalletBuilder as CdkWalletBuilder};

use crate::error::FfiError;
use crate::runtime;
use crate::types::*;

/// FFI-compatible Wallet
#[derive(uniffi::Object)]
pub struct Wallet {
    inner: Arc<CdkWallet>,
}

#[uniffi::export]
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
    pub fn total_balance(&self) -> Result<Amount, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let balance = inner.total_balance().await?;
            Ok::<Amount, FfiError>(balance.into())
        })
    }

    /// Get total pending balance
    pub fn total_pending_balance(&self) -> Result<Amount, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let balance = inner.total_pending_balance().await?;
            Ok::<Amount, FfiError>(balance.into())
        })
    }

    /// Get total reserved balance
    pub fn total_reserved_balance(&self) -> Result<Amount, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let balance = inner.total_reserved_balance().await?;
            Ok::<Amount, FfiError>(balance.into())
        })
    }

    /// Get mint info
    pub fn get_mint_info(&self) -> Result<Option<MintInfo>, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let info = inner.fetch_mint_info().await?;
            Ok::<Option<MintInfo>, FfiError>(info.map(Into::into))
        })
    }

    /// Receive tokens
    pub fn receive(
        &self,
        token: std::sync::Arc<Token>,
        options: ReceiveOptions,
    ) -> Result<Amount, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let amount = inner.receive(&token.to_string(), options.into()).await?;
            Ok::<Amount, FfiError>(amount.into())
        })
    }

    /// Restore wallet from seed
    pub fn restore(&self) -> Result<Amount, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let amount = inner.restore().await?;
            Ok::<Amount, FfiError>(amount.into())
        })
    }

    /// Verify token DLEQ proofs
    pub fn verify_token_dleq(&self, token: std::sync::Arc<Token>) -> Result<(), FfiError> {
        let inner = self.inner.clone();
        let cdk_token = token.inner.clone();
        runtime::block_on(async move {
            inner.verify_token_dleq(&cdk_token).await?;
            Ok::<(), FfiError>(())
        })
    }

    /// Receive proofs directly
    pub fn receive_proofs(
        &self,
        proofs: Proofs,
        options: ReceiveOptions,
        memo: Option<String>,
    ) -> Result<Amount, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let cdk_proofs: Vec<cdk::nuts::Proof> =
                proofs.into_iter().map(|p| p.inner.clone()).collect();

            let amount = inner
                .receive_proofs(cdk_proofs, options.into(), memo)
                .await?;
            Ok::<Amount, FfiError>(amount.into())
        })
    }

    /// Prepare a send operation
    pub fn prepare_send(
        &self,
        amount: Amount,
        options: SendOptions,
    ) -> Result<std::sync::Arc<PreparedSend>, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let prepared = inner.prepare_send(amount.into(), options.into()).await?;
            Ok::<std::sync::Arc<PreparedSend>, FfiError>(std::sync::Arc::new(prepared.into()))
        })
    }

    /// Get a mint quote
    pub fn mint_quote(
        &self,
        amount: Amount,
        description: Option<String>,
    ) -> Result<MintQuote, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let quote = inner.mint_quote(amount.into(), description).await?;
            Ok::<MintQuote, FfiError>(quote.into())
        })
    }

    /// Mint tokens
    pub fn mint(
        &self,
        quote_id: String,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            // Convert spending conditions if provided
            let conditions = spending_conditions.map(|sc| sc.try_into()).transpose()?;

            let proofs = inner
                .mint(&quote_id, amount_split_target.into(), conditions)
                .await?;
            Ok::<Proofs, FfiError>(
                proofs
                    .into_iter()
                    .map(|p| std::sync::Arc::new(p.into()))
                    .collect(),
            )
        })
    }

    /// Get a melt quote
    pub fn melt_quote(
        &self,
        request: String,
        options: Option<MeltOptions>,
    ) -> Result<MeltQuote, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let cdk_options = options.map(Into::into);
            let quote = inner.melt_quote(request, cdk_options).await?;
            Ok::<MeltQuote, FfiError>(quote.into())
        })
    }

    /// Melt tokens
    pub fn melt(&self, quote_id: String) -> Result<Melted, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let melted = inner.melt(&quote_id).await?;
            Ok::<Melted, FfiError>(melted.into())
        })
    }

    /// Get a quote for a bolt12 mint
    pub fn mint_bolt12_quote(
        &self,
        amount: Option<Amount>,
        description: Option<String>,
    ) -> Result<MintQuote, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let quote = inner
                .mint_bolt12_quote(amount.map(Into::into), description)
                .await?;
            Ok::<MintQuote, FfiError>(quote.into())
        })
    }

    /// Mint tokens using bolt12
    pub fn mint_bolt12(
        &self,
        quote_id: String,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let conditions = spending_conditions.map(|sc| sc.try_into()).transpose()?;

            let proofs = inner
                .mint_bolt12(
                    &quote_id,
                    amount.map(Into::into),
                    amount_split_target.into(),
                    conditions,
                )
                .await?;

            Ok::<Proofs, FfiError>(
                proofs
                    .into_iter()
                    .map(|p| std::sync::Arc::new(p.into()))
                    .collect(),
            )
        })
    }

    /// Get a quote for a bolt12 melt
    pub fn melt_bolt12_quote(
        &self,
        request: String,
        options: Option<MeltOptions>,
    ) -> Result<MeltQuote, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let cdk_options = options.map(Into::into);
            let quote = inner.melt_bolt12_quote(request, cdk_options).await?;
            Ok::<MeltQuote, FfiError>(quote.into())
        })
    }

    /// Swap proofs
    pub fn swap(
        &self,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
    ) -> Result<Option<Proofs>, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let cdk_proofs: Vec<cdk::nuts::Proof> =
                input_proofs.into_iter().map(|p| p.inner.clone()).collect();

            // Convert spending conditions if provided
            let conditions = spending_conditions.map(|sc| sc.try_into()).transpose()?;

            let result = inner
                .swap(
                    amount.map(Into::into),
                    amount_split_target.into(),
                    cdk_proofs,
                    conditions,
                    include_fees,
                )
                .await?;

            Ok::<Option<Proofs>, FfiError>(result.map(|proofs| {
                proofs
                    .into_iter()
                    .map(|p| std::sync::Arc::new(p.into()))
                    .collect()
            }))
        })
    }

    /// Get proofs by states
    pub fn get_proofs_by_states(&self, states: Vec<ProofState>) -> Result<Proofs, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let mut all_proofs = Vec::new();

            for state in states {
                let proofs = match state {
                    ProofState::Unspent => inner.get_unspent_proofs().await?,
                    ProofState::Pending => inner.get_pending_proofs().await?,
                    ProofState::Reserved => inner.get_reserved_proofs().await?,
                    ProofState::PendingSpent => inner.get_pending_spent_proofs().await?,
                    ProofState::Spent => {
                        // CDK doesn't have a method to get spent proofs directly
                        // They are removed from the database when spent
                        continue;
                    }
                };

                for proof in proofs {
                    all_proofs.push(std::sync::Arc::new(proof.into()));
                }
            }

            Ok::<Proofs, FfiError>(all_proofs)
        })
    }

    /// Check if proofs are spent
    pub fn check_proofs_spent(&self, proofs: Proofs) -> Result<Vec<bool>, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let cdk_proofs: Vec<cdk::nuts::Proof> =
                proofs.into_iter().map(|p| p.inner.clone()).collect();

            let proof_states = inner.check_proofs_spent(cdk_proofs).await?;
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
            Ok::<Vec<bool>, FfiError>(spent_bools)
        })
    }

    /// List transactions
    pub fn list_transactions(
        &self,
        direction: Option<TransactionDirection>,
    ) -> Result<Vec<Transaction>, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let cdk_direction = direction.map(Into::into);
            let transactions = inner.list_transactions(cdk_direction).await?;
            Ok::<Vec<Transaction>, FfiError>(transactions.into_iter().map(Into::into).collect())
        })
    }

    /// Get transaction by ID
    pub fn get_transaction(&self, id: TransactionId) -> Result<Option<Transaction>, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let cdk_id = id.try_into()?;
            let transaction = inner.get_transaction(cdk_id).await?;
            Ok::<Option<Transaction>, FfiError>(transaction.map(Into::into))
        })
    }

    /// Revert a transaction
    pub fn revert_transaction(&self, id: TransactionId) -> Result<(), FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let cdk_id = id.try_into()?;
            inner.revert_transaction(cdk_id).await?;
            Ok::<(), FfiError>(())
        })
    }

    /// Set Clear Auth Token (CAT) for authentication
    #[cfg(feature = "auth")]
    pub fn set_cat(&self, cat: String) -> Result<(), FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            inner.set_cat(cat).await?;
            Ok::<(), FfiError>(())
        })
    }

    /// Set refresh token for authentication
    #[cfg(feature = "auth")]
    pub fn set_refresh_token(&self, refresh_token: String) -> Result<(), FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            inner.set_refresh_token(refresh_token).await?;
            Ok::<(), FfiError>(())
        })
    }

    /// Refresh access token using the stored refresh token
    #[cfg(feature = "auth")]
    pub fn refresh_access_token(&self) -> Result<(), FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            inner.refresh_access_token().await?;
            Ok::<(), FfiError>(())
        })
    }

    /// Mint blind auth tokens
    #[cfg(feature = "auth")]
    pub fn mint_blind_auth(&self, amount: Amount) -> Result<Proofs, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let proofs = inner.mint_blind_auth(amount.into()).await?;
            Ok::<Proofs, FfiError>(
                proofs
                    .into_iter()
                    .map(|p| std::sync::Arc::new(p.into()))
                    .collect(),
            )
        })
    }

    /// Get unspent auth proofs
    #[cfg(feature = "auth")]
    pub fn get_unspent_auth_proofs(&self) -> Result<Vec<AuthProof>, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let auth_proofs = inner.get_unspent_auth_proofs().await?;
            Ok::<Vec<AuthProof>, FfiError>(auth_proofs.into_iter().map(Into::into).collect())
        })
    }

    /// Subscribe to wallet events
    pub fn subscribe(
        &self,
        params: SubscribeParams,
    ) -> Result<std::sync::Arc<ActiveSubscription>, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let cdk_params: cdk_common::subscription::Params = params.clone().into();
            let sub_id = cdk_params.id.to_string();
            let active_sub = inner.subscribe(cdk_params).await;
            Ok::<std::sync::Arc<ActiveSubscription>, FfiError>(std::sync::Arc::new(
                ActiveSubscription::new(active_sub, sub_id),
            ))
        })
    }

    /// Refresh keysets from the mint
    pub fn refresh_keysets(&self) -> Result<Vec<KeySetInfo>, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let keysets = inner.refresh_keysets().await?;
            Ok::<Vec<KeySetInfo>, FfiError>(keysets.into_iter().map(Into::into).collect())
        })
    }

    /// Get the active keyset for the wallet's unit
    pub fn get_active_keyset(&self) -> Result<KeySetInfo, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let keyset = inner.get_active_keyset().await?;
            Ok::<KeySetInfo, FfiError>(keyset.into())
        })
    }

    /// Get fees for a specific keyset ID
    pub fn get_keyset_fees_by_id(&self, keyset_id: String) -> Result<u64, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let id = cdk::nuts::Id::from_str(&keyset_id)
                .map_err(|e| FfiError::Generic { msg: e.to_string() })?;
            let fees = inner.get_keyset_fees_by_id(id).await?;
            Ok::<u64, FfiError>(fees)
        })
    }

    /// Reclaim unspent proofs (mark them as unspent in the database)
    pub fn reclaim_unspent(&self, proofs: Proofs) -> Result<(), FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let cdk_proofs: Vec<cdk::nuts::Proof> =
                proofs.iter().map(|p| p.inner.clone()).collect();
            inner.reclaim_unspent(cdk_proofs).await?;
            Ok::<(), FfiError>(())
        })
    }

    /// Check all pending proofs and return the total amount reclaimed
    pub fn check_all_pending_proofs(&self) -> Result<Amount, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let amount = inner.check_all_pending_proofs().await?;
            Ok::<Amount, FfiError>(amount.into())
        })
    }

    /// Calculate fee for a given number of proofs with the specified keyset
    pub fn calculate_fee(&self, proof_count: u32, keyset_id: String) -> Result<Amount, FfiError> {
        let inner = self.inner.clone();
        runtime::block_on(async move {
            let id = cdk::nuts::Id::from_str(&keyset_id)
                .map_err(|e| FfiError::Generic { msg: e.to_string() })?;
            let fee_ppk = inner.get_keyset_fees_by_id(id).await?;
            let total_fee = (proof_count as u64 * fee_ppk) / 1000; // fee is per thousand
            Ok::<Amount, FfiError>(Amount::new(total_fee))
        })
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

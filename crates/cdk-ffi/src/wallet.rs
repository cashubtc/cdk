//! FFI Wallet bindings

use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::{Wallet as CdkWallet, WalletBuilder as CdkWalletBuilder};
use cdk_sqlite::wallet::WalletSqliteDatabase;

use crate::error::FfiError;
use crate::types::*;

/// FFI-compatible Wallet
#[derive(uniffi::Object)]
pub struct Wallet {
    inner: Arc<CdkWallet>,
}

#[uniffi::export]
impl Wallet {
    /// Create a new Wallet from mnemonic
    #[uniffi::constructor]
    pub async fn new(
        mint_url: String,
        unit: CurrencyUnit,
        mnemonic: String,
        passphrase: Option<String>,
        config: WalletConfig,
    ) -> Result<Self, FfiError> {
        // Parse mnemonic and generate seed
        let m = Mnemonic::parse(&mnemonic).map_err(|e| FfiError::Generic {
            msg: format!("Invalid mnemonic: {}", e),
        })?;
        let seed = m.to_seed_normalized(passphrase.as_deref().unwrap_or_default());

        let localstore: Arc<
            dyn cdk_common::database::WalletDatabase<Err = cdk_common::database::Error>
                + Send
                + Sync,
        > = Arc::new(
            WalletSqliteDatabase::new(config.work_dir.as_str())
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?,
        );

        let wallet = CdkWalletBuilder::new()
            .mint_url(
                mint_url
                    .parse()
                    .map_err(|e: cdk::mint_url::Error| FfiError::Generic { msg: e.to_string() })?,
            )
            .unit(unit.into())
            .localstore(localstore)
            .seed(&seed)
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
        let info = self.inner.get_mint_info().await?;
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
        let cdk_proofs: Vec<cdk::nuts::Proof> =
            proofs.into_iter().map(|p| p.inner.clone()).collect();

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
    ) -> Result<std::sync::Arc<MintQuote>, FfiError> {
        let quote = self.inner.mint_quote(amount.into(), description).await?;
        Ok(std::sync::Arc::new(quote.into()))
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
        Ok(proofs
            .into_iter()
            .map(|p| std::sync::Arc::new(p.into()))
            .collect())
    }

    /// Get a melt quote
    pub async fn melt_quote(
        &self,
        request: String,
        options: Option<MeltOptions>,
    ) -> Result<std::sync::Arc<MeltQuote>, FfiError> {
        let cdk_options = options.map(Into::into);
        let quote = self.inner.melt_quote(request, cdk_options).await?;
        Ok(std::sync::Arc::new(quote.into()))
    }

    /// Melt tokens
    pub async fn melt(&self, quote_id: String) -> Result<Melted, FfiError> {
        let melted = self.inner.melt(&quote_id).await?;
        Ok(melted.into())
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
        let cdk_proofs: Vec<cdk::nuts::Proof> =
            input_proofs.into_iter().map(|p| p.inner.clone()).collect();

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

        Ok(result.map(|proofs| {
            proofs
                .into_iter()
                .map(|p| std::sync::Arc::new(p.into()))
                .collect()
        }))
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
                all_proofs.push(std::sync::Arc::new(proof.into()));
            }
        }

        Ok(all_proofs)
    }

    /// Check if proofs are spent
    pub async fn check_proofs_spent(&self, proofs: Proofs) -> Result<Vec<bool>, FfiError> {
        let cdk_proofs: Vec<cdk::nuts::Proof> =
            proofs.into_iter().map(|p| p.inner.clone()).collect();

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
}

/// Configuration for creating wallets
#[derive(Debug, Clone, uniffi::Record)]
pub struct WalletConfig {
    pub work_dir: String,
    pub target_proof_count: Option<u32>,
}

/// Generates a new random mnemonic phrase
#[uniffi::export]
pub fn generate_mnemonic() -> Result<String, FfiError> {
    let mnemonic = Mnemonic::generate(12).map_err(|e| FfiError::Generic { msg: e.to_string() })?;
    Ok(mnemonic.to_string())
}

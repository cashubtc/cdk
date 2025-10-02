//! FFI MultiMintWallet bindings

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::multi_mint_wallet::{
    MultiMintReceiveOptions as CdkMultiMintReceiveOptions,
    MultiMintSendOptions as CdkMultiMintSendOptions, MultiMintWallet as CdkMultiMintWallet,
    TransferMode as CdkTransferMode, TransferResult as CdkTransferResult,
};

use crate::error::FfiError;
use crate::token::Token;
use crate::types::*;

/// FFI-compatible MultiMintWallet
#[derive(uniffi::Object)]
pub struct MultiMintWallet {
    inner: Arc<CdkMultiMintWallet>,
}

#[uniffi::export(async_runtime = "tokio")]
impl MultiMintWallet {
    /// Create a new MultiMintWallet from mnemonic using WalletDatabase trait
    #[uniffi::constructor]
    pub fn new(
        unit: CurrencyUnit,
        mnemonic: String,
        db: Arc<dyn crate::database::WalletDatabase>,
    ) -> Result<Self, FfiError> {
        // Parse mnemonic and generate seed without passphrase
        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| FfiError::InvalidMnemonic { msg: e.to_string() })?;
        let seed = m.to_seed_normalized("");

        // Convert the FFI database trait to a CDK database implementation
        let localstore = crate::database::create_cdk_database_from_ffi(db);

        let wallet = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle.block_on(async move {
                    CdkMultiMintWallet::new(localstore, seed, unit.into()).await
                })
            }),
            Err(_) => {
                // No current runtime, create a new one
                tokio::runtime::Runtime::new()
                    .map_err(|e| FfiError::Database {
                        msg: format!("Failed to create runtime: {}", e),
                    })?
                    .block_on(async move {
                        CdkMultiMintWallet::new(localstore, seed, unit.into()).await
                    })
            }
        }?;

        Ok(Self {
            inner: Arc::new(wallet),
        })
    }

    /// Create a new MultiMintWallet with proxy configuration
    #[uniffi::constructor]
    pub fn new_with_proxy(
        unit: CurrencyUnit,
        mnemonic: String,
        db: Arc<dyn crate::database::WalletDatabase>,
        proxy_url: String,
    ) -> Result<Self, FfiError> {
        // Parse mnemonic and generate seed without passphrase
        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| FfiError::InvalidMnemonic { msg: e.to_string() })?;
        let seed = m.to_seed_normalized("");

        // Convert the FFI database trait to a CDK database implementation
        let localstore = crate::database::create_cdk_database_from_ffi(db);

        // Parse proxy URL
        let proxy_url =
            url::Url::parse(&proxy_url).map_err(|e| FfiError::InvalidUrl { msg: e.to_string() })?;

        let wallet = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle.block_on(async move {
                    CdkMultiMintWallet::new_with_proxy(localstore, seed, unit.into(), proxy_url)
                        .await
                })
            }),
            Err(_) => {
                // No current runtime, create a new one
                tokio::runtime::Runtime::new()
                    .map_err(|e| FfiError::Database {
                        msg: format!("Failed to create runtime: {}", e),
                    })?
                    .block_on(async move {
                        CdkMultiMintWallet::new_with_proxy(localstore, seed, unit.into(), proxy_url)
                            .await
                    })
            }
        }?;

        Ok(Self {
            inner: Arc::new(wallet),
        })
    }

    /// Get the currency unit for this wallet
    pub fn unit(&self) -> CurrencyUnit {
        self.inner.unit().clone().into()
    }

    /// Add a mint to this MultiMintWallet
    pub async fn add_mint(
        &self,
        mint_url: MintUrl,
        target_proof_count: Option<u32>,
    ) -> Result<(), FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        self.inner
            .add_mint(cdk_mint_url, target_proof_count.map(|c| c as usize))
            .await?;
        Ok(())
    }

    /// Remove mint from MultiMintWallet
    pub async fn remove_mint(&self, mint_url: MintUrl) {
        let url_str = mint_url.url.clone();
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into().unwrap_or_else(|_| {
            // If conversion fails, we can't remove the mint, but we shouldn't panic
            // This is a best-effort operation
            cdk::mint_url::MintUrl::from_str(&url_str).unwrap_or_else(|_| {
                // Last resort: create a dummy URL that won't match anything
                cdk::mint_url::MintUrl::from_str("https://invalid.mint").unwrap()
            })
        });
        self.inner.remove_mint(&cdk_mint_url).await;
    }

    /// Check if mint is in wallet
    pub async fn has_mint(&self, mint_url: MintUrl) -> bool {
        if let Ok(cdk_mint_url) = mint_url.try_into() {
            self.inner.has_mint(&cdk_mint_url).await
        } else {
            false
        }
    }

    /// Get wallet balances for all mints
    pub async fn get_balances(&self) -> Result<BalanceMap, FfiError> {
        let balances = self.inner.get_balances().await?;
        let mut balance_map = HashMap::new();
        for (mint_url, amount) in balances {
            balance_map.insert(mint_url.to_string(), amount.into());
        }
        Ok(balance_map)
    }

    /// Get total balance across all mints
    pub async fn total_balance(&self) -> Result<Amount, FfiError> {
        let total = self.inner.total_balance().await?;
        Ok(total.into())
    }

    /// List proofs for all mints
    pub async fn list_proofs(&self) -> Result<ProofsByMint, FfiError> {
        let proofs = self.inner.list_proofs().await?;
        let mut proofs_by_mint = HashMap::new();
        for (mint_url, mint_proofs) in proofs {
            let ffi_proofs: Vec<Arc<Proof>> = mint_proofs
                .into_iter()
                .map(|p| Arc::new(p.into()))
                .collect();
            proofs_by_mint.insert(mint_url.to_string(), ffi_proofs);
        }
        Ok(proofs_by_mint)
    }

    /// Receive token
    pub async fn receive(
        &self,
        token: Arc<Token>,
        options: MultiMintReceiveOptions,
    ) -> Result<Amount, FfiError> {
        let amount = self
            .inner
            .receive(&token.to_string(), options.into())
            .await?;
        Ok(amount.into())
    }

    /// Restore wallets for a specific mint
    pub async fn restore(&self, mint_url: MintUrl) -> Result<Amount, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let amount = self.inner.restore(&cdk_mint_url).await?;
        Ok(amount.into())
    }

    /// Prepare a send operation from a specific mint
    pub async fn prepare_send(
        &self,
        mint_url: MintUrl,
        amount: Amount,
        options: MultiMintSendOptions,
    ) -> Result<Arc<PreparedSend>, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let prepared = self
            .inner
            .prepare_send(cdk_mint_url, amount.into(), options.into())
            .await?;
        Ok(Arc::new(prepared.into()))
    }

    /// Get a mint quote from a specific mint
    pub async fn mint_quote(
        &self,
        mint_url: MintUrl,
        amount: Amount,
        description: Option<String>,
    ) -> Result<MintQuote, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let quote = self
            .inner
            .mint_quote(&cdk_mint_url, amount.into(), description)
            .await?;
        Ok(quote.into())
    }

    /// Check a specific mint quote status
    pub async fn check_mint_quote(
        &self,
        mint_url: MintUrl,
        quote_id: String,
    ) -> Result<MintQuote, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let quote = self
            .inner
            .check_mint_quote(&cdk_mint_url, &quote_id)
            .await?;
        Ok(quote.into())
    }

    /// Mint tokens at a specific mint
    pub async fn mint(
        &self,
        mint_url: MintUrl,
        quote_id: String,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let conditions = spending_conditions.map(|sc| sc.try_into()).transpose()?;

        let proofs = self
            .inner
            .mint(&cdk_mint_url, &quote_id, conditions)
            .await?;
        Ok(proofs.into_iter().map(|p| Arc::new(p.into())).collect())
    }

    /// Wait for a mint quote to be paid and automatically mint the proofs
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn wait_for_mint_quote(
        &self,
        mint_url: MintUrl,
        quote_id: String,
        split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
        timeout_secs: u64,
    ) -> Result<Proofs, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let conditions = spending_conditions.map(|sc| sc.try_into()).transpose()?;

        let proofs = self
            .inner
            .wait_for_mint_quote(
                &cdk_mint_url,
                &quote_id,
                split_target.into(),
                conditions,
                timeout_secs,
            )
            .await?;
        Ok(proofs.into_iter().map(|p| Arc::new(p.into())).collect())
    }

    /// Get a melt quote from a specific mint
    pub async fn melt_quote(
        &self,
        mint_url: MintUrl,
        request: String,
        options: Option<MeltOptions>,
    ) -> Result<MeltQuote, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let cdk_options = options.map(Into::into);
        let quote = self
            .inner
            .melt_quote(&cdk_mint_url, request, cdk_options)
            .await?;
        Ok(quote.into())
    }

    /// Melt tokens (pay a bolt11 invoice)
    pub async fn melt(
        &self,
        bolt11: String,
        options: Option<MeltOptions>,
        max_fee: Option<Amount>,
    ) -> Result<Melted, FfiError> {
        let cdk_options = options.map(Into::into);
        let cdk_max_fee = max_fee.map(Into::into);
        let melted = self.inner.melt(&bolt11, cdk_options, cdk_max_fee).await?;
        Ok(melted.into())
    }

    /// Transfer funds between mints
    pub async fn transfer(
        &self,
        source_mint: MintUrl,
        target_mint: MintUrl,
        transfer_mode: TransferMode,
    ) -> Result<TransferResult, FfiError> {
        let source_cdk: cdk::mint_url::MintUrl = source_mint.try_into()?;
        let target_cdk: cdk::mint_url::MintUrl = target_mint.try_into()?;
        let result = self
            .inner
            .transfer(&source_cdk, &target_cdk, transfer_mode.into())
            .await?;
        Ok(result.into())
    }

    /// Swap proofs with automatic wallet selection
    pub async fn swap(
        &self,
        amount: Option<Amount>,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Option<Proofs>, FfiError> {
        let conditions = spending_conditions.map(|sc| sc.try_into()).transpose()?;

        let result = self.inner.swap(amount.map(Into::into), conditions).await?;

        Ok(result.map(|proofs| proofs.into_iter().map(|p| Arc::new(p.into())).collect()))
    }

    /// List transactions from all mints
    pub async fn list_transactions(
        &self,
        direction: Option<TransactionDirection>,
    ) -> Result<Vec<Transaction>, FfiError> {
        let cdk_direction = direction.map(Into::into);
        let transactions = self.inner.list_transactions(cdk_direction).await?;
        Ok(transactions.into_iter().map(Into::into).collect())
    }

    /// Check all mint quotes and mint if paid
    pub async fn check_all_mint_quotes(
        &self,
        mint_url: Option<MintUrl>,
    ) -> Result<Amount, FfiError> {
        let cdk_mint_url = mint_url.map(|url| url.try_into()).transpose()?;
        let amount = self.inner.check_all_mint_quotes(cdk_mint_url).await?;
        Ok(amount.into())
    }

    /// Consolidate proofs across all mints
    pub async fn consolidate(&self) -> Result<Amount, FfiError> {
        let amount = self.inner.consolidate().await?;
        Ok(amount.into())
    }

    /// Get list of mint URLs
    pub async fn get_mint_urls(&self) -> Vec<String> {
        let wallets = self.inner.get_wallets().await;
        wallets.iter().map(|w| w.mint_url.to_string()).collect()
    }

    /// Verify token DLEQ proofs
    pub async fn verify_token_dleq(&self, token: Arc<Token>) -> Result<(), FfiError> {
        let cdk_token = token.inner.clone();
        self.inner.verify_token_dleq(&cdk_token).await?;
        Ok(())
    }
}

/// Transfer mode for mint-to-mint transfers
#[derive(Debug, Clone, uniffi::Enum)]
pub enum TransferMode {
    /// Transfer exact amount to target (target receives specified amount)
    ExactReceive { amount: Amount },
    /// Transfer all available balance (source will be emptied)
    FullBalance,
}

impl From<TransferMode> for CdkTransferMode {
    fn from(mode: TransferMode) -> Self {
        match mode {
            TransferMode::ExactReceive { amount } => CdkTransferMode::ExactReceive(amount.into()),
            TransferMode::FullBalance => CdkTransferMode::FullBalance,
        }
    }
}

/// Result of a transfer operation with detailed breakdown
#[derive(Debug, Clone, uniffi::Record)]
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

impl From<CdkTransferResult> for TransferResult {
    fn from(result: CdkTransferResult) -> Self {
        Self {
            amount_sent: result.amount_sent.into(),
            amount_received: result.amount_received.into(),
            fees_paid: result.fees_paid.into(),
            source_balance_after: result.source_balance_after.into(),
            target_balance_after: result.target_balance_after.into(),
        }
    }
}

/// Options for receiving tokens in multi-mint context
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct MultiMintReceiveOptions {
    /// Whether to allow receiving from untrusted (not yet added) mints
    pub allow_untrusted: bool,
    /// Mint URL to transfer tokens to from untrusted mints (None means keep in original mint)
    pub transfer_to_mint: Option<MintUrl>,
    /// Base receive options to apply to the wallet receive
    pub receive_options: ReceiveOptions,
}

impl From<MultiMintReceiveOptions> for CdkMultiMintReceiveOptions {
    fn from(options: MultiMintReceiveOptions) -> Self {
        let mut opts = CdkMultiMintReceiveOptions::new();
        opts.allow_untrusted = options.allow_untrusted;
        opts.transfer_to_mint = options.transfer_to_mint.and_then(|url| url.try_into().ok());
        opts.receive_options = options.receive_options.into();
        opts
    }
}

/// Options for sending tokens in multi-mint context
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct MultiMintSendOptions {
    /// Whether to allow transferring funds from other mints if needed
    pub allow_transfer: bool,
    /// Maximum amount to transfer from other mints (optional limit)
    pub max_transfer_amount: Option<Amount>,
    /// Specific mint URLs allowed for transfers (empty means all mints allowed)
    pub allowed_mints: Vec<MintUrl>,
    /// Specific mint URLs to exclude from transfers
    pub excluded_mints: Vec<MintUrl>,
    /// Base send options to apply to the wallet send
    pub send_options: SendOptions,
}

impl From<MultiMintSendOptions> for CdkMultiMintSendOptions {
    fn from(options: MultiMintSendOptions) -> Self {
        let mut opts = CdkMultiMintSendOptions::new();
        opts.allow_transfer = options.allow_transfer;
        opts.max_transfer_amount = options.max_transfer_amount.map(Into::into);
        opts.allowed_mints = options
            .allowed_mints
            .into_iter()
            .filter_map(|url| url.try_into().ok())
            .collect();
        opts.excluded_mints = options
            .excluded_mints
            .into_iter()
            .filter_map(|url| url.try_into().ok())
            .collect();
        opts.send_options = options.send_options.into();
        opts
    }
}

/// Type alias for balances by mint URL
pub type BalanceMap = HashMap<String, Amount>;

/// Type alias for proofs by mint URL
pub type ProofsByMint = HashMap<String, Vec<Arc<Proof>>>;

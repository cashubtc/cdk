//! FFI MultiMintWallet bindings

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::multi_mint_wallet::{
    MultiMintReceiveOptions as CdkMultiMintReceiveOptions,
    MultiMintSendOptions as CdkMultiMintSendOptions, MultiMintWallet as CdkMultiMintWallet,
    TokenData as CdkTokenData, TransferMode as CdkTransferMode,
    TransferResult as CdkTransferResult,
};

use crate::error::FfiError;
use crate::token::Token;
use crate::types::payment_request::{
    CreateRequestParams, CreateRequestResult, NostrWaitInfo, PaymentRequest,
};
use crate::types::*;

/// FFI-compatible MultiMintWallet
#[derive(uniffi::Object)]
pub struct MultiMintWallet {
    inner: Arc<CdkMultiMintWallet>,
}

#[uniffi::export(async_runtime = "tokio")]
impl MultiMintWallet {
    /// Create a new MultiMintWallet from mnemonic using WalletDatabaseFfi trait
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

    /// Set metadata cache TTL (time-to-live) in seconds for a specific mint
    ///
    /// Controls how long cached mint metadata (keysets, keys, mint info) is considered fresh
    /// before requiring a refresh from the mint server for a specific mint.
    ///
    /// # Arguments
    ///
    /// * `mint_url` - The mint URL to set the TTL for
    /// * `ttl_secs` - Optional TTL in seconds. If None, cache never expires.
    pub async fn set_metadata_cache_ttl_for_mint(
        &self,
        mint_url: MintUrl,
        ttl_secs: Option<u64>,
    ) -> Result<(), FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let wallets = self.inner.get_wallets().await;

        if let Some(wallet) = wallets.iter().find(|w| w.mint_url == cdk_mint_url) {
            let ttl = ttl_secs.map(std::time::Duration::from_secs);
            wallet.set_metadata_cache_ttl(ttl);
            Ok(())
        } else {
            Err(FfiError::Generic {
                msg: format!("Mint not found: {}", cdk_mint_url),
            })
        }
    }

    /// Set metadata cache TTL (time-to-live) in seconds for all mints
    ///
    /// Controls how long cached mint metadata is considered fresh for all mints
    /// in this MultiMintWallet.
    ///
    /// # Arguments
    ///
    /// * `ttl_secs` - Optional TTL in seconds. If None, cache never expires for any mint.
    pub async fn set_metadata_cache_ttl_for_all_mints(&self, ttl_secs: Option<u64>) {
        let wallets = self.inner.get_wallets().await;
        let ttl = ttl_secs.map(std::time::Duration::from_secs);

        for wallet in wallets.iter() {
            wallet.set_metadata_cache_ttl(ttl);
        }
    }

    /// Add a mint to this MultiMintWallet
    pub async fn add_mint(
        &self,
        mint_url: MintUrl,
        target_proof_count: Option<u32>,
    ) -> Result<(), FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;

        if let Some(count) = target_proof_count {
            let config = cdk::wallet::multi_mint_wallet::WalletConfig::new()
                .with_target_proof_count(count as usize);
            self.inner
                .add_mint_with_config(cdk_mint_url, config)
                .await?;
        } else {
            self.inner.add_mint(cdk_mint_url).await?;
        }
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
                cdk::mint_url::MintUrl::from_str("https://invalid.mint")
                    .expect("Valid hardcoded URL")
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

    pub async fn get_mint_keysets(&self, mint_url: MintUrl) -> Result<Vec<KeySetInfo>, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let keysets = self.inner.get_mint_keysets(&cdk_mint_url).await?;

        let keysets = keysets.into_iter().map(|k| k.into()).collect();

        Ok(keysets)
    }

    /// Get token data (mint URL and proofs) from a token
    ///
    /// This method extracts the mint URL and proofs from a token. It will automatically
    /// fetch the keysets from the mint if needed to properly decode the proofs.
    ///
    /// The mint must already be added to the wallet. If the mint is not in the wallet,
    /// use `add_mint` first.
    pub async fn get_token_data(&self, token: Arc<Token>) -> Result<TokenData, FfiError> {
        let token_data = self.inner.get_token_data(&token.inner).await?;
        Ok(token_data.into())
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
            let ffi_proofs: Vec<Proof> = mint_proofs.into_iter().map(|p| p.into()).collect();
            proofs_by_mint.insert(mint_url.to_string(), ffi_proofs);
        }
        Ok(proofs_by_mint)
    }

    /// Check the state of proofs at a specific mint
    pub async fn check_proofs_state(
        &self,
        mint_url: MintUrl,
        proofs: Proofs,
    ) -> Result<Vec<ProofState>, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            proofs.into_iter().map(|p| p.try_into()).collect();
        let cdk_proofs = cdk_proofs?;

        let states = self
            .inner
            .check_proofs_state(&cdk_mint_url, cdk_proofs)
            .await?;

        Ok(states.into_iter().map(|s| s.into()).collect())
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
        Ok(proofs.into_iter().map(|p| p.into()).collect())
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
        Ok(proofs.into_iter().map(|p| p.into()).collect())
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

    /// Get a melt quote for a BIP353 human-readable address
    ///
    /// This method resolves a BIP353 address (e.g., "alice@example.com") to a Lightning offer
    /// and then creates a melt quote for that offer at the specified mint.
    ///
    /// # Arguments
    ///
    /// * `mint_url` - The mint to use for creating the melt quote
    /// * `bip353_address` - Human-readable address in the format "user@domain.com"
    /// * `amount_msat` - Amount to pay in millisatoshis
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn melt_bip353_quote(
        &self,
        mint_url: MintUrl,
        bip353_address: String,
        amount_msat: u64,
    ) -> Result<MeltQuote, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let cdk_amount = cdk::Amount::from(amount_msat);
        let quote = self
            .inner
            .melt_bip353_quote(&cdk_mint_url, &bip353_address, cdk_amount)
            .await?;
        Ok(quote.into())
    }

    /// Get a melt quote for a Lightning address
    ///
    /// This method resolves a Lightning address (e.g., "alice@example.com") to a Lightning invoice
    /// and then creates a melt quote for that invoice at the specified mint.
    ///
    /// # Arguments
    ///
    /// * `mint_url` - The mint to use for creating the melt quote
    /// * `lightning_address` - Lightning address in the format "user@domain.com"
    /// * `amount_msat` - Amount to pay in millisatoshis
    pub async fn melt_lightning_address_quote(
        &self,
        mint_url: MintUrl,
        lightning_address: String,
        amount_msat: u64,
    ) -> Result<MeltQuote, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let cdk_amount = cdk::Amount::from(amount_msat);
        let quote = self
            .inner
            .melt_lightning_address_quote(&cdk_mint_url, &lightning_address, cdk_amount)
            .await?;
        Ok(quote.into())
    }

    /// Get a melt quote for a human-readable address
    ///
    /// This method accepts a human-readable address that could be either a BIP353 address
    /// or a Lightning address. It intelligently determines which to try based on mint support:
    ///
    /// 1. If the mint supports Bolt12, it tries BIP353 first
    /// 2. Falls back to Lightning address only if BIP353 DNS resolution fails
    /// 3. If BIP353 resolves but fails at the mint, it does NOT fall back to Lightning address
    /// 4. If the mint doesn't support Bolt12, it tries Lightning address directly
    ///
    /// # Arguments
    ///
    /// * `mint_url` - The mint to use for creating the melt quote
    /// * `address` - Human-readable address (BIP353 or Lightning address)
    /// * `amount_msat` - Amount to pay in millisatoshis
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn melt_human_readable_quote(
        &self,
        mint_url: MintUrl,
        address: String,
        amount_msat: u64,
    ) -> Result<MeltQuote, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let cdk_amount = cdk::Amount::from(amount_msat);
        let quote = self
            .inner
            .melt_human_readable_quote(&cdk_mint_url, &address, cdk_amount)
            .await?;
        Ok(quote.into())
    }

    /// Melt tokens
    pub async fn melt_with_mint(
        &self,
        mint_url: MintUrl,
        quote_id: String,
    ) -> Result<Melted, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let melted = self.inner.melt_with_mint(&cdk_mint_url, &quote_id).await?;
        Ok(melted.into())
    }

    /// Melt specific proofs from a specific mint
    ///
    /// This method allows melting proofs that may not be in the wallet's database,
    /// similar to how `receive_proofs` handles external proofs. The proofs will be
    /// added to the database and used for the melt operation.
    ///
    /// # Arguments
    ///
    /// * `mint_url` - The mint to use for the melt operation
    /// * `quote_id` - The melt quote ID (obtained from `melt_quote`)
    /// * `proofs` - The proofs to melt (can be external proofs not in the wallet's database)
    ///
    /// # Returns
    ///
    /// A `Melted` result containing the payment details and any change proofs
    pub async fn melt_proofs(
        &self,
        mint_url: MintUrl,
        quote_id: String,
        proofs: Proofs,
    ) -> Result<Melted, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            proofs.into_iter().map(|p| p.try_into()).collect();
        let cdk_proofs = cdk_proofs?;

        let melted = self
            .inner
            .melt_proofs(&cdk_mint_url, &quote_id, cdk_proofs)
            .await?;
        Ok(melted.into())
    }

    /// Check melt quote status
    pub async fn check_melt_quote(
        &self,
        mint_url: MintUrl,
        quote_id: String,
    ) -> Result<MeltQuote, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let melted = self
            .inner
            .check_melt_quote(&cdk_mint_url, &quote_id)
            .await?;
        Ok(melted.into())
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

        Ok(result.map(|proofs| proofs.into_iter().map(|p| p.into()).collect()))
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

    /// Get proofs for a transaction by transaction ID
    ///
    /// This retrieves all proofs associated with a transaction. If `mint_url` is provided,
    /// it will only check that specific mint's wallet. Otherwise, it searches across all
    /// wallets to find which mint the transaction belongs to.
    ///
    /// # Arguments
    ///
    /// * `id` - The transaction ID
    /// * `mint_url` - Optional mint URL to check directly, avoiding iteration over all wallets
    pub async fn get_proofs_for_transaction(
        &self,
        id: TransactionId,
        mint_url: Option<MintUrl>,
    ) -> Result<Vec<Proof>, FfiError> {
        let cdk_id = id.try_into()?;
        let cdk_mint_url = mint_url.map(|url| url.try_into()).transpose()?;
        let proofs = self
            .inner
            .get_proofs_for_transaction(cdk_id, cdk_mint_url)
            .await?;
        Ok(proofs.into_iter().map(Into::into).collect())
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

    /// Get all wallets from MultiMintWallet
    pub async fn get_wallets(&self) -> Vec<Arc<crate::wallet::Wallet>> {
        let wallets = self.inner.get_wallets().await;
        wallets
            .into_iter()
            .map(|w| Arc::new(crate::wallet::Wallet::from_inner(Arc::new(w))))
            .collect()
    }

    /// Get a specific wallet from MultiMintWallet by mint URL
    pub async fn get_wallet(&self, mint_url: MintUrl) -> Option<Arc<crate::wallet::Wallet>> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into().ok()?;
        let wallet = self.inner.get_wallet(&cdk_mint_url).await?;
        Some(Arc::new(crate::wallet::Wallet::from_inner(Arc::new(
            wallet,
        ))))
    }

    /// Verify token DLEQ proofs
    pub async fn verify_token_dleq(&self, token: Arc<Token>) -> Result<(), FfiError> {
        let cdk_token = token.inner.clone();
        self.inner.verify_token_dleq(&cdk_token).await?;
        Ok(())
    }

    /// Query mint for current mint information
    pub async fn fetch_mint_info(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let mint_info = self.inner.fetch_mint_info(&cdk_mint_url).await?;
        Ok(mint_info.map(Into::into))
    }

    /// Get mint info for all wallets
    ///
    /// This method loads the mint info for each wallet in the MultiMintWallet
    /// and returns a map of mint URLs to their corresponding mint info.
    ///
    /// Uses cached mint info when available, only fetching from the mint if the cache
    /// has expired.
    pub async fn get_all_mint_info(&self) -> Result<MintInfoMap, FfiError> {
        let mint_infos = self.inner.get_all_mint_info().await?;
        let mut result = HashMap::new();
        for (mint_url, mint_info) in mint_infos {
            result.insert(mint_url.to_string(), mint_info.into());
        }
        Ok(result)
    }
}

/// Payment request methods for MultiMintWallet
#[uniffi::export(async_runtime = "tokio")]
impl MultiMintWallet {
    /// Pay a NUT-18 PaymentRequest
    ///
    /// This method handles paying a payment request by selecting an appropriate mint:
    /// - If `mint_url` is provided, it verifies the payment request accepts that mint
    ///   and uses it to pay.
    /// - If `mint_url` is None, it automatically selects the mint that:
    ///   1. Is accepted by the payment request (matches one of the request's mints, or request accepts any mint)
    ///   2. Has the highest balance among matching mints
    ///
    /// # Arguments
    ///
    /// * `payment_request` - The NUT-18 payment request to pay
    /// * `mint_url` - Optional specific mint to use. If None, automatically selects the best matching mint.
    /// * `custom_amount` - Custom amount to pay (required if payment request has no amount)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The payment request has no amount and no custom amount is provided
    /// - The specified mint is not accepted by the payment request
    /// - No matching mint has sufficient balance
    /// - No transport is available in the payment request
    pub async fn pay_request(
        &self,
        payment_request: Arc<PaymentRequest>,
        mint_url: Option<MintUrl>,
        custom_amount: Option<Amount>,
    ) -> Result<(), FfiError> {
        let cdk_mint_url = mint_url.map(|url| url.try_into()).transpose()?;
        let cdk_amount = custom_amount.map(Into::into);

        self.inner
            .pay_request(payment_request.inner().clone(), cdk_mint_url, cdk_amount)
            .await?;

        Ok(())
    }

    /// Create a NUT-18 payment request
    ///
    /// Creates a payment request that can be shared to receive Cashu tokens.
    /// The request can include optional amount, description, and spending conditions.
    ///
    /// # Arguments
    ///
    /// * `params` - Parameters for creating the payment request
    ///
    /// # Transport Options
    ///
    /// - `"nostr"` - Uses Nostr relays for privacy-preserving delivery (requires nostr_relays)
    /// - `"http"` - Uses HTTP POST for delivery (requires http_url)
    /// - `"none"` - No transport; token must be delivered out-of-band
    ///
    /// # Example
    ///
    /// ```ignore
    /// let params = CreateRequestParams {
    ///     amount: Some(100),
    ///     unit: "sat".to_string(),
    ///     description: Some("Coffee payment".to_string()),
    ///     transport: "http".to_string(),
    ///     http_url: Some("https://example.com/callback".to_string()),
    ///     ..Default::default()
    /// };
    /// let result = wallet.create_request(params).await?;
    /// println!("Share this request: {}", result.payment_request.to_string_encoded());
    ///
    /// // If using Nostr transport, wait for payment:
    /// if let Some(nostr_info) = result.nostr_wait_info {
    ///     let amount = wallet.wait_for_nostr_payment(nostr_info).await?;
    ///     println!("Received {} sats", amount);
    /// }
    /// ```
    pub async fn create_request(
        &self,
        params: CreateRequestParams,
    ) -> Result<CreateRequestResult, FfiError> {
        let (payment_request, nostr_wait_info) = self.inner.create_request(params.into()).await?;
        Ok(CreateRequestResult {
            payment_request: Arc::new(PaymentRequest::from_inner(payment_request)),
            nostr_wait_info: nostr_wait_info.map(|info| Arc::new(NostrWaitInfo::from_inner(info))),
        })
    }

    /// Wait for a Nostr payment and receive it into the wallet
    ///
    /// This method connects to the Nostr relays specified in the `NostrWaitInfo`,
    /// subscribes for incoming payment events, and receives the first valid
    /// payment into the wallet.
    ///
    /// # Arguments
    ///
    /// * `info` - The Nostr wait info returned from `create_request` when using Nostr transport
    ///
    /// # Returns
    ///
    /// The amount received from the payment.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let result = wallet.create_request(params).await?;
    /// if let Some(nostr_info) = result.nostr_wait_info {
    ///     let amount = wallet.wait_for_nostr_payment(nostr_info).await?;
    ///     println!("Received {} sats", amount);
    /// }
    /// ```
    pub async fn wait_for_nostr_payment(
        &self,
        info: Arc<NostrWaitInfo>,
    ) -> Result<Amount, FfiError> {
        // We need to clone the inner NostrWaitInfo since we can't consume the Arc
        let info_inner = cdk::wallet::payment_request::NostrWaitInfo {
            keys: info.inner().keys.clone(),
            relays: info.inner().relays.clone(),
            pubkey: info.inner().pubkey,
        };
        let amount = self
            .inner
            .wait_for_nostr_payment(info_inner)
            .await
            .map_err(|e| FfiError::Generic { msg: e.to_string() })?;
        Ok(amount.into())
    }
}

/// Auth methods for MultiMintWallet
#[uniffi::export(async_runtime = "tokio")]
impl MultiMintWallet {
    /// Set Clear Auth Token (CAT) for a specific mint
    pub async fn set_cat(&self, mint_url: MintUrl, cat: String) -> Result<(), FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        self.inner.set_cat(&cdk_mint_url, cat).await?;
        Ok(())
    }

    /// Set refresh token for a specific mint
    pub async fn set_refresh_token(
        &self,
        mint_url: MintUrl,
        refresh_token: String,
    ) -> Result<(), FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        self.inner
            .set_refresh_token(&cdk_mint_url, refresh_token)
            .await?;
        Ok(())
    }

    /// Refresh access token for a specific mint using the stored refresh token
    pub async fn refresh_access_token(&self, mint_url: MintUrl) -> Result<(), FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        self.inner.refresh_access_token(&cdk_mint_url).await?;
        Ok(())
    }

    /// Mint blind auth tokens at a specific mint
    pub async fn mint_blind_auth(
        &self,
        mint_url: MintUrl,
        amount: Amount,
    ) -> Result<Proofs, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let proofs = self
            .inner
            .mint_blind_auth(&cdk_mint_url, amount.into())
            .await?;
        Ok(proofs.into_iter().map(|p| p.into()).collect())
    }

    /// Get unspent auth proofs for a specific mint
    pub async fn get_unspent_auth_proofs(
        &self,
        mint_url: MintUrl,
    ) -> Result<Vec<AuthProof>, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let auth_proofs = self.inner.get_unspent_auth_proofs(&cdk_mint_url).await?;
        Ok(auth_proofs.into_iter().map(Into::into).collect())
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

/// Data extracted from a token including mint URL, proofs, and memo
#[derive(Debug, Clone, uniffi::Record)]
pub struct TokenData {
    /// The mint URL from the token
    pub mint_url: MintUrl,
    /// The proofs contained in the token
    pub proofs: Proofs,
    /// The memo from the token, if present
    pub memo: Option<String>,
    /// Value of token
    pub value: Amount,
    /// Unit of token
    pub unit: CurrencyUnit,
    /// Fee to redeem
    ///
    /// If the token is for a proof that we do not know, we cannot get the fee.
    /// To avoid just erroring and still allow decoding, this is an option.
    /// None does not mean there is no fee, it means we do not know the fee.
    pub redeem_fee: Option<Amount>,
}

impl From<CdkTokenData> for TokenData {
    fn from(data: CdkTokenData) -> Self {
        Self {
            mint_url: data.mint_url.into(),
            proofs: data.proofs.into_iter().map(|p| p.into()).collect(),
            memo: data.memo,
            value: data.value.into(),
            unit: data.unit.into(),
            redeem_fee: data.redeem_fee.map(|a| a.into()),
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
pub type ProofsByMint = HashMap<String, Vec<Proof>>;

/// Type alias for mint info by mint URL
pub type MintInfoMap = HashMap<String, MintInfo>;

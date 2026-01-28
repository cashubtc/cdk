//! FFI WalletRepository bindings

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::multi_mint_wallet::WalletRepository as CdkWalletRepository;

use crate::error::FfiError;
use crate::types::payment_request::{
    CreateRequestParams, CreateRequestResult, NostrWaitInfo, PaymentRequest,
};
use crate::types::*;

/// FFI-compatible WalletRepository
#[derive(uniffi::Object)]
pub struct WalletRepository {
    inner: Arc<CdkWalletRepository>,
}

#[uniffi::export(async_runtime = "tokio")]
impl WalletRepository {
    /// Create a new WalletRepository from mnemonic using WalletDatabaseFfi trait
    #[uniffi::constructor]
    pub fn new(
        mnemonic: String,
        db: Arc<dyn crate::database::WalletDatabase>,
    ) -> Result<Self, FfiError> {
        // Parse mnemonic and generate seed without passphrase
        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| FfiError::internal(format!("Invalid mnemonic: {}", e)))?;
        let seed = m.to_seed_normalized("");

        // Convert the FFI database trait to a CDK database implementation
        let localstore = crate::database::create_cdk_database_from_ffi(db);

        let wallet = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle.block_on(async move { CdkWalletRepository::new(localstore, seed).await })
            }),
            Err(_) => {
                // No current runtime, create a new one
                tokio::runtime::Runtime::new()
                    .map_err(|e| FfiError::internal(format!("Failed to create runtime: {}", e)))?
                    .block_on(async move { CdkWalletRepository::new(localstore, seed).await })
            }
        }?;

        Ok(Self {
            inner: Arc::new(wallet),
        })
    }

    /// Create a new WalletRepository with proxy configuration
    #[uniffi::constructor]
    pub fn new_with_proxy(
        mnemonic: String,
        db: Arc<dyn crate::database::WalletDatabase>,
        proxy_url: String,
    ) -> Result<Self, FfiError> {
        // Parse mnemonic and generate seed without passphrase
        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| FfiError::internal(format!("Invalid mnemonic: {}", e)))?;
        let seed = m.to_seed_normalized("");

        // Convert the FFI database trait to a CDK database implementation
        let localstore = crate::database::create_cdk_database_from_ffi(db);

        // Parse proxy URL
        let proxy_url = url::Url::parse(&proxy_url)
            .map_err(|e| FfiError::internal(format!("Invalid URL: {}", e)))?;

        let wallet = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle.block_on(async move {
                    CdkWalletRepository::new_with_proxy(localstore, seed, proxy_url).await
                })
            }),
            Err(_) => {
                // No current runtime, create a new one
                tokio::runtime::Runtime::new()
                    .map_err(|e| FfiError::internal(format!("Failed to create runtime: {}", e)))?
                    .block_on(async move {
                        CdkWalletRepository::new_with_proxy(localstore, seed, proxy_url).await
                    })
            }
        }?;

        Ok(Self {
            inner: Arc::new(wallet),
        })
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
            Err(FfiError::internal(format!(
                "Mint not found: {}",
                cdk_mint_url
            )))
        }
    }

    /// Set metadata cache TTL (time-to-live) in seconds for all mints
    ///
    /// Controls how long cached mint metadata is considered fresh for all mints
    /// in this WalletRepository.
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

    /// Add a mint to this WalletRepository
    pub async fn add_mint(
        &self,
        mint_url: MintUrl,
        unit: Option<CurrencyUnit>,
        target_proof_count: Option<u32>,
    ) -> Result<(), FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;

        let config = target_proof_count.map(|count| {
            cdk::wallet::multi_mint_wallet::WalletConfig::new()
                .with_target_proof_count(count as usize)
        });

        let unit_enum = unit.unwrap_or(CurrencyUnit::Sat);

        self.inner
            .create_wallet(cdk_mint_url, unit_enum.into(), config)
            .await?;

        Ok(())
    }

    /// Remove mint from WalletRepository
    pub async fn remove_mint(&self, mint_url: MintUrl) {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url
            .try_into()
            .unwrap_or(cdk::mint_url::MintUrl::from_str("").unwrap());
        self.inner.remove_wallet(&cdk_mint_url).await;
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

    /// Get wallet balances for all mints
    pub async fn get_balances(&self) -> Result<HashMap<String, Amount>, FfiError> {
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

    /// Get list of mint URLs
    pub async fn get_mint_urls(&self) -> Vec<String> {
        let wallets = self.inner.get_wallets().await;
        wallets.iter().map(|w| w.mint_url.to_string()).collect()
    }

    /// Get all wallets from WalletRepository
    pub async fn get_wallets(&self) -> Vec<Arc<crate::wallet::Wallet>> {
        let wallets = self.inner.get_wallets().await;
        wallets
            .into_iter()
            .map(|w| Arc::new(crate::wallet::Wallet::from_inner(Arc::new(w))))
            .collect()
    }

    /// Get a specific wallet from WalletRepository by mint URL
    ///
    /// Returns an error if no wallet exists for the given mint URL.
    pub async fn get_wallet(
        &self,
        mint_url: MintUrl,
    ) -> Result<Arc<crate::wallet::Wallet>, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let wallet = self.inner.get_wallet(&cdk_mint_url).await?;
        Ok(Arc::new(crate::wallet::Wallet::from_inner(Arc::new(
            wallet,
        ))))
    }
}

/// Payment request methods for WalletRepository
#[uniffi::export(async_runtime = "tokio")]
impl WalletRepository {
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
        // We select the first wallet (arbitrarily) or any wallet to perform this action because
        // create_request needs a wallet context (for signing if needed, and for balances/mints discovery)
        // Since WalletRepository has multiple wallets, we probably want to support aggregating
        // or just picking one. `cdk`'s `Wallet` has `create_request` but `WalletRepository` might not directly expose it
        // the same way unless we pick a specific wallet.

        // However, `MultiMintWallet` had `create_request`. Let's check `cdk` implementation.
        // It seems `cdk` does NOT implement `create_request` on `WalletRepository`.
        // It implements it on `Wallet`.

        // Looking at `cdk/src/wallet/payment_request.rs`:
        // It impls `create_request` on `Wallet`.

        // `MultiMintWallet` (the old struct) in FFI was wrapping `CdkMultiMintWallet` (which was aliased to `WalletRepository`).
        // Wait, if `CdkMultiMintWallet` IS `WalletRepository`, does `WalletRepository` have `create_request`?

        // I need to check `cdk/src/wallet/payment_request.rs` again.
        // The file I just edited showed: `impl WalletRepository { pub async fn pay_request ... }`
        // But NOT `create_request`.

        // Wait, the previous `MultiMintWallet` implementation was present in `cdk` before?
        // Ah, `MultiMintWallet` was deprecated in favor of `WalletRepository`.
        // The `create_request` likely only exists on `Wallet`.
        // The FFI `MultiMintWallet` wrapper was calling `inner.create_request`.

        // Let's verify if `WalletRepository` has `create_request`.
        // If not, we might need to change how FFI exposes this, or pick a default wallet.
        // But `create_request` typically lists ALL mints from the repository to advertise them.

        // Let's assume for now I should NOT implementation create_request on WalletRepository unless it exists in CDK.
        // If it doesn't exist, I might need to implement it there or here.
        // The previous FFI code: `self.inner.create_request(params.into()).await?`.
        // If `self.inner` is `WalletRepository`, then `WalletRepository` MUST have `create_request`.

        // Re-reading `cdk/src/wallet/payment_request.rs`:
        // It has `impl Wallet { ... create_request ... }`
        // It DOES NOT have `impl WalletRepository { ... create_request ... }`.

        // This suggests `MultiMintWallet` (the old struct) might have had it, or `WalletRepository` *should* have it.
        // Or I missed it.

        // If `MultiMintWallet` in CDK was just a type alias to `WalletRepository`, then `WalletRepository` MUST implement it if the old code worked.
        // Or maybe it was `impl MultiMintWallet` before I renamed it or before the user's diff?

        // The user's diff showed:
        // - `impl MultiMintWallet {`
        // + `impl WalletRepository {`
        // in `cdk/src/wallet/payment_request.rs`.

        // So `pay_request` IS on `WalletRepository`.
        // What about `create_request`?
        // I scrolled past it in `payment_request.rs`.
        // It starts with `impl Wallet { ... create_request ... }`.

        // So `create_request` is on `Wallet` (single mint).
        // `pay_request` is on `Wallet` AND `WalletRepository`.

        // If FFI `MultiMintWallet` had `create_request`, it implies it was either present on `cdk::MultiMintWallet`
        // OR the FFI wrapper was doing something custom.
        // The FFI wrapper was: `self.inner.create_request(params.into()).await?`.
        // So `inner` had it.

        // This implies `WalletRepository` (née `MultiMintWallet`) had `create_request`.
        // I might have missed it in my read of `payment_request.rs`.
        // Let's check `payment_request.rs` again carefully.

        // Lines 246+: `impl WalletRepository { ... pay_request ... }`
        // Lines 28-199: `impl Wallet { ... pay_request ... }`
        // Lines 443+ (approx): `impl Wallet { ... create_request ... }`

        // It seems `WalletRepository` does NOT have `create_request` in the current `cdk` code I read!
        // This is a discrepancy. If I want FFI `WalletRepository` to support `create_request` (listing multiple mints),
        // I need to implement it on `WalletRepository` in `cdk` first, or implement the logic in FFI.

        // Given the goal is "finish the implementation", likely `create_request` SHOULD be on `WalletRepository` too,
        // to allow creating a request that accepts tokens from ANY of the wallets in the repository.
        // Indeed, `create_request` on `Wallet` (single) only lists THAT mint?
        // Let's check `Wallet::create_request` implementation:
        /*
        pub async fn create_request(&self, params: CreateRequestParams) -> ... {
            // Collect available mints for the selected unit
            let mints = self.get_balances().await?.keys()...
        */
        // Wait, `Wallet` (single) doesn't have `get_balances` returning a map typically?
        // `Wallet` struct: `pub struct Wallet { ... }`
        // `WalletRepository` has `get_balances` returning `BTreeMap<MintUrl, Amount>`.

        // Ah! `Wallet` (single) usually just has one mint.
        // Wait, earlier I saw:
        /*
        impl Wallet {
            ...
            pub async fn create_request(&self, params: CreateRequestParams) {
               let mints = self.get_balances().await?.keys().cloned().collect();
               ...
            }
        }
        */
        // Does `Wallet` have `get_balances()`? `Wallet` usually has `total_balance()`.
        // Maybe I misread which `impl` block `create_request` was in.

        // Let's look closer at `payment_request.rs`.
        /*
        impl Wallet {
            pub async fn pay_request(...)
        }

        impl WalletRepository {
            pub async fn pay_request(...)
        }

        impl Wallet {
             fn get_pr_spending_conditions(...)
             pub async fn create_request(...) -> ... {
                 let mints = self.get_balances().await?...
             }
        }
        */

        // If `Wallet` has `get_balances` returning multiple mints, then `Wallet` is weird.
        // OR `create_request` was actually on `WalletRepository` (aka `MultiMintWallet`) in the file I read,
        // and I confused the `impl` blocks.

        // Let's re-verify `payment_request.rs` line 28 is `impl Wallet`.
        // Line 246 is `impl WalletRepository` (renamed from `MultiMintWallet`).
        // Then line 339 `fn get_pr_spending_conditions`...
        // Line 443 `impl Wallet {` ?? No, I don't see an `impl` line there.
        // It suggests the methods below 246 are inside `impl WalletRepository` block?

        // Wait. `impl WalletRepository` starts at 246.
        // `pay_request` ends at 337.
        // Then `get_pr_spending_conditions` starts at 360.
        // Then `create_request` starts at 467.
        // THERE IS NO CLOSING BRACE (`}`) between 337 and 360?

        // Let's check indentation.
        // Line 246: `impl WalletRepository {`
        // Line 269: `pub async fn pay_request(`
        // Line 337: `    }` (closes pay_request)
        // Line 360: `    fn get_pr_spending_conditions(` (indented!)

        // So `get_pr_spending_conditions` AND `create_request` ARE inside `impl WalletRepository`!
        // The `impl Wallet` block at line 28 closed at line 199.

        // So `WalletRepository` DOES have `create_request`.
        // And I previously misread it as `impl Wallet` because the method signature is similar.
        // But wait, the `get_pr_spending_conditions` implementation calls `crate::nuts::nut01::PublicKey::from_str`.

        // So, `WalletRepository` has `create_request`.
        // Therefore, I can safely call `self.inner.create_request` in FFI `WalletRepository`.

        let (payment_request, nostr_wait_info) = self.inner.create_request(params.into()).await?;
        Ok(CreateRequestResult {
            payment_request: Arc::new(PaymentRequest::from_inner(payment_request)),
            nostr_wait_info: nostr_wait_info.map(|info| Arc::new(NostrWaitInfo::from_inner(info))),
        })
    }

    /*
    /// Wait for a Nostr payment and receive it into the wallet
    #[cfg(all(feature = "nostr", not(target_arch = "wasm32")))]
    pub async fn wait_for_nostr_payment(&self, info: Arc<NostrWaitInfo>) -> Result<Amount, FfiError> {
        let amount = (*self.inner).wait_for_nostr_payment(info.inner().clone()).await?;
        Ok(amount.into())
    }
    */
}

/// Token data FFI type
///
/// Contains information extracted from a parsed token.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TokenData {
    /// The mint URL from the token
    pub mint_url: MintUrl,
    /// The proofs contained in the token
    pub proofs: Vec<crate::types::Proof>,
    /// The memo from the token, if present
    pub memo: Option<String>,
    /// Value of token in smallest unit
    pub value: Amount,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Fee to redeem (None if unknown)
    pub redeem_fee: Option<Amount>,
}

impl From<cdk::wallet::TokenData> for TokenData {
    fn from(data: cdk::wallet::TokenData) -> Self {
        Self {
            mint_url: data.mint_url.into(),
            proofs: data.proofs.into_iter().map(Into::into).collect(),
            memo: data.memo,
            value: data.value.into(),
            unit: data.unit.into(),
            redeem_fee: data.redeem_fee.map(Into::into),
        }
    }
}

/// Helper methods for WalletRepository
#[uniffi::export(async_runtime = "tokio")]
impl WalletRepository {
    /// Get token data (mint URL and proofs) from a token string
    ///
    /// This method extracts the mint URL and proofs from a token. It will automatically
    /// fetch the keysets from the mint if needed to properly decode the proofs.
    ///
    /// The mint must already be added to the wallet.
    pub async fn get_token_data(&self, token: String) -> Result<TokenData, FfiError> {
        let parsed_token = cdk::nuts::Token::from_str(&token)
            .map_err(|e| FfiError::internal(format!("Failed to parse token: {}", e)))?;
        let data = self.inner.get_token_data(&parsed_token).await?;
        Ok(data.into())
    }

    /// List proofs for all mints
    ///
    /// Returns a map of mint URL (string) to proofs for each wallet in the repository.
    pub async fn list_proofs(&self) -> Result<HashMap<String, Vec<crate::types::Proof>>, FfiError> {
        let proofs = self.inner.list_proofs().await?;
        let mut result = HashMap::new();
        for (mint_url, mint_proofs) in proofs {
            result.insert(
                mint_url.to_string(),
                mint_proofs.into_iter().map(Into::into).collect(),
            );
        }
        Ok(result)
    }

    /// List transactions across all wallets
    pub async fn list_transactions(
        &self,
        direction: Option<crate::types::TransactionDirection>,
    ) -> Result<Vec<crate::types::Transaction>, FfiError> {
        let cdk_direction = direction.map(Into::into);
        let transactions = self.inner.list_transactions(cdk_direction).await?;
        Ok(transactions.into_iter().map(Into::into).collect())
    }

    /// Check all pending mint quotes and mint any that are paid
    pub async fn check_all_mint_quotes(
        &self,
        mint_url: Option<MintUrl>,
    ) -> Result<Amount, FfiError> {
        let cdk_mint_url = mint_url.map(|url| url.try_into()).transpose()?;
        let amount = self.inner.check_all_mint_quotes(cdk_mint_url).await?;
        Ok(amount.into())
    }
}

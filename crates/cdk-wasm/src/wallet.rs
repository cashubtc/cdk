//! WASM Wallet bindings

use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::{Wallet as CdkWallet, WalletBuilder as CdkWalletBuilder};
use wasm_bindgen::prelude::*;

use crate::error::WasmError;
use crate::local_storage::LocalStorageDatabase;
use crate::types::*;

/// WASM-compatible Wallet
#[wasm_bindgen]
pub struct Wallet {
    #[allow(missing_debug_implementations)]
    inner: Arc<CdkWallet>,
}

impl std::fmt::Debug for Wallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wallet")
            .field("mint_url", &self.inner.mint_url)
            .finish()
    }
}

impl Wallet {
    /// Create a Wallet from an existing CDK wallet (internal use only)
    pub(crate) fn from_inner(inner: Arc<CdkWallet>) -> Self {
        Self { inner }
    }
}

#[wasm_bindgen]
impl Wallet {
    /// Create a new Wallet
    #[wasm_bindgen(constructor)]
    pub fn new(
        mint_url: String,
        unit: JsValue,
        mnemonic: String,
        db: JsValue,
        target_proof_count: Option<u32>,
    ) -> Result<Wallet, WasmError> {
        let unit: CurrencyUnit =
            serde_wasm_bindgen::from_value(unit).map_err(WasmError::internal)?;

        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| WasmError::internal(format!("Invalid mnemonic: {}", e)))?;
        let seed = m.to_seed_normalized("");

        // Use provided JS database or fall back to in-memory store
        // TODO: Accept a JS database implementation via WalletDatabaseBridge
        let _ = db;
        let localstore = LocalStorageDatabase::new().into_arc();

        let wallet = CdkWalletBuilder::new()
            .mint_url(mint_url.parse().map_err(|e: cdk::mint_url::Error| {
                WasmError::internal(format!("Invalid URL: {}", e))
            })?)
            .unit(unit.into())
            .seed(seed)
            .localstore(localstore)
            .target_proof_count(target_proof_count.unwrap_or(3) as usize)
            .build()
            .map_err(WasmError::from)?;

        Ok(Self {
            inner: Arc::new(wallet),
        })
    }

    /// Get the mint URL
    #[wasm_bindgen(js_name = "mintUrl")]
    pub fn mint_url(&self) -> JsValue {
        let url: MintUrl = self.inner.mint_url.clone().into();
        serde_wasm_bindgen::to_value(&url).unwrap_or(JsValue::NULL)
    }

    /// Get the currency unit
    pub fn unit(&self) -> JsValue {
        let unit: CurrencyUnit = self.inner.unit.clone().into();
        serde_wasm_bindgen::to_value(&unit).unwrap_or(JsValue::NULL)
    }

    /// Set metadata cache TTL in seconds
    #[wasm_bindgen(js_name = "setMetadataCacheTtl")]
    pub fn set_metadata_cache_ttl(&self, ttl_secs: Option<u64>) {
        let ttl = ttl_secs.map(std::time::Duration::from_secs);
        self.inner.set_metadata_cache_ttl(ttl);
    }

    /// Get total balance
    #[wasm_bindgen(js_name = "totalBalance")]
    pub async fn total_balance(&self) -> Result<Amount, WasmError> {
        let balance = self.inner.total_balance().await?;
        Ok(balance.into())
    }

    /// Get total pending balance
    #[wasm_bindgen(js_name = "totalPendingBalance")]
    pub async fn total_pending_balance(&self) -> Result<Amount, WasmError> {
        let balance = self.inner.total_pending_balance().await?;
        Ok(balance.into())
    }

    /// Get total reserved balance
    #[wasm_bindgen(js_name = "totalReservedBalance")]
    pub async fn total_reserved_balance(&self) -> Result<Amount, WasmError> {
        let balance = self.inner.total_reserved_balance().await?;
        Ok(balance.into())
    }

    /// Fetch mint info from mint
    #[wasm_bindgen(js_name = "fetchMintInfo")]
    pub async fn fetch_mint_info(&self) -> Result<JsValue, WasmError> {
        let info = self.inner.fetch_mint_info().await?;
        let wasm_info: Option<MintInfo> = info.map(Into::into);
        serde_wasm_bindgen::to_value(&wasm_info).map_err(WasmError::internal)
    }

    /// Load mint info (from cache if fresh)
    #[wasm_bindgen(js_name = "loadMintInfo")]
    pub async fn load_mint_info(&self) -> Result<JsValue, WasmError> {
        let info = self.inner.load_mint_info().await?;
        let wasm_info: MintInfo = info.into();
        serde_wasm_bindgen::to_value(&wasm_info).map_err(WasmError::internal)
    }

    /// Receive tokens
    pub async fn receive(&self, token_str: String, options: JsValue) -> Result<Amount, WasmError> {
        let opts: ReceiveOptions =
            serde_wasm_bindgen::from_value(options).map_err(WasmError::internal)?;
        let amount = self.inner.receive(&token_str, opts.into()).await?;
        Ok(amount.into())
    }

    /// Restore wallet from seed
    pub async fn restore(&self) -> Result<JsValue, WasmError> {
        let restored = self.inner.restore().await?;
        let wasm_restored: Restored = restored.into();
        serde_wasm_bindgen::to_value(&wasm_restored).map_err(WasmError::internal)
    }

    /// Get a mint quote
    #[wasm_bindgen(js_name = "mintQuote")]
    pub async fn mint_quote(
        &self,
        payment_method: JsValue,
        amount: Option<Amount>,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<JsValue, WasmError> {
        let method: PaymentMethod =
            serde_wasm_bindgen::from_value(payment_method).map_err(WasmError::internal)?;
        let quote = self
            .inner
            .mint_quote(method, amount.map(Into::into), description, extra)
            .await?;
        let wasm_quote: MintQuote = quote.into();
        serde_wasm_bindgen::to_value(&wasm_quote).map_err(WasmError::internal)
    }

    /// Mint tokens
    pub async fn mint(
        &self,
        quote_id: String,
        amount_split_target: JsValue,
        spending_conditions: JsValue,
    ) -> Result<JsValue, WasmError> {
        let split: SplitTarget =
            serde_wasm_bindgen::from_value(amount_split_target).map_err(WasmError::internal)?;
        let conditions: Option<SpendingConditions> = if spending_conditions.is_null()
            || spending_conditions.is_undefined()
        {
            None
        } else {
            Some(serde_wasm_bindgen::from_value(spending_conditions).map_err(WasmError::internal)?)
        };

        let cdk_conditions = conditions.map(|sc| sc.try_into()).transpose()?;

        let proofs = self
            .inner
            .mint(&quote_id, split.into(), cdk_conditions)
            .await?;
        let wasm_proofs: Proofs = proofs.into_iter().map(|p| p.into()).collect();
        serde_wasm_bindgen::to_value(&wasm_proofs).map_err(WasmError::internal)
    }

    /// Get a melt quote
    #[wasm_bindgen(js_name = "meltQuote")]
    pub async fn melt_quote(
        &self,
        method: JsValue,
        request: String,
        options: JsValue,
        extra: Option<String>,
    ) -> Result<JsValue, WasmError> {
        let payment_method: PaymentMethod =
            serde_wasm_bindgen::from_value(method).map_err(WasmError::internal)?;
        let cdk_options: Option<MeltOptions> = if options.is_null() || options.is_undefined() {
            None
        } else {
            Some(serde_wasm_bindgen::from_value(options).map_err(WasmError::internal)?)
        };

        let quote = self
            .inner
            .melt_quote::<cdk::nuts::PaymentMethod, _>(
                payment_method.into(),
                request,
                cdk_options.map(Into::into),
                extra,
            )
            .await?;
        let wasm_quote: MeltQuote = quote.into();
        serde_wasm_bindgen::to_value(&wasm_quote).map_err(WasmError::internal)
    }

    /// Swap proofs
    pub async fn swap(
        &self,
        amount: JsValue,
        amount_split_target: JsValue,
        input_proofs: JsValue,
        spending_conditions: JsValue,
        include_fees: bool,
    ) -> Result<JsValue, WasmError> {
        let amt: Option<Amount> = if amount.is_null() || amount.is_undefined() {
            None
        } else {
            Some(serde_wasm_bindgen::from_value(amount).map_err(WasmError::internal)?)
        };
        let split: SplitTarget =
            serde_wasm_bindgen::from_value(amount_split_target).map_err(WasmError::internal)?;
        let proofs: Proofs =
            serde_wasm_bindgen::from_value(input_proofs).map_err(WasmError::internal)?;
        let conditions: Option<SpendingConditions> = if spending_conditions.is_null()
            || spending_conditions.is_undefined()
        {
            None
        } else {
            Some(serde_wasm_bindgen::from_value(spending_conditions).map_err(WasmError::internal)?)
        };

        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            proofs.into_iter().map(|p| p.try_into()).collect();
        let cdk_proofs = cdk_proofs?;
        let cdk_conditions = conditions.map(|sc| sc.try_into()).transpose()?;

        let result = self
            .inner
            .swap(
                amt.map(Into::into),
                split.into(),
                cdk_proofs,
                cdk_conditions,
                include_fees,
            )
            .await?;

        let wasm_result: Option<Proofs> =
            result.map(|proofs| proofs.into_iter().map(|p| p.into()).collect());
        serde_wasm_bindgen::to_value(&wasm_result).map_err(WasmError::internal)
    }

    /// List transactions
    #[wasm_bindgen(js_name = "listTransactions")]
    pub async fn list_transactions(&self, direction: JsValue) -> Result<JsValue, WasmError> {
        let dir: Option<TransactionDirection> = if direction.is_null() || direction.is_undefined() {
            None
        } else {
            Some(serde_wasm_bindgen::from_value(direction).map_err(WasmError::internal)?)
        };
        let cdk_direction = dir.map(Into::into);
        let transactions = self.inner.list_transactions(cdk_direction).await?;
        let wasm_txs: Vec<Transaction> = transactions.into_iter().map(Into::into).collect();
        serde_wasm_bindgen::to_value(&wasm_txs).map_err(WasmError::internal)
    }

    /// Check proofs spent status
    #[wasm_bindgen(js_name = "checkProofsSpent")]
    pub async fn check_proofs_spent(&self, proofs: JsValue) -> Result<Vec<u8>, WasmError> {
        let wasm_proofs: Proofs =
            serde_wasm_bindgen::from_value(proofs).map_err(WasmError::internal)?;
        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            wasm_proofs.into_iter().map(|p| p.try_into()).collect();
        let cdk_proofs = cdk_proofs?;

        let proof_states = self.inner.check_proofs_spent(cdk_proofs).await?;
        Ok(proof_states
            .into_iter()
            .map(|ps| {
                matches!(
                    ps.state,
                    cdk::nuts::State::Spent | cdk::nuts::State::PendingSpent
                ) as u8
            })
            .collect())
    }

    /// Refresh keysets from the mint
    #[wasm_bindgen(js_name = "refreshKeysets")]
    pub async fn refresh_keysets(&self) -> Result<JsValue, WasmError> {
        let keysets = self.inner.refresh_keysets().await?;
        let wasm_keysets: Vec<KeySetInfo> = keysets.into_iter().map(Into::into).collect();
        serde_wasm_bindgen::to_value(&wasm_keysets).map_err(WasmError::internal)
    }

    /// Get the active keyset for the wallet's unit
    #[wasm_bindgen(js_name = "getActiveKeyset")]
    pub async fn get_active_keyset(&self) -> Result<JsValue, WasmError> {
        let keyset = self.inner.get_active_keyset().await?;
        let wasm_keyset: KeySetInfo = keyset.into();
        serde_wasm_bindgen::to_value(&wasm_keyset).map_err(WasmError::internal)
    }

    /// Check all pending proofs
    #[wasm_bindgen(js_name = "checkAllPendingProofs")]
    pub async fn check_all_pending_proofs(&self) -> Result<Amount, WasmError> {
        let amount = self.inner.check_all_pending_proofs().await?;
        Ok(amount.into())
    }

    /// Set Clear Auth Token (CAT)
    #[wasm_bindgen(js_name = "setCat")]
    pub async fn set_cat(&self, cat: String) -> Result<(), WasmError> {
        self.inner.set_cat(cat).await?;
        Ok(())
    }

    /// Set refresh token
    #[wasm_bindgen(js_name = "setRefreshToken")]
    pub async fn set_refresh_token(&self, refresh_token: String) -> Result<(), WasmError> {
        self.inner.set_refresh_token(refresh_token).await?;
        Ok(())
    }

    /// Refresh access token
    #[wasm_bindgen(js_name = "refreshAccessToken")]
    pub async fn refresh_access_token(&self) -> Result<(), WasmError> {
        self.inner.refresh_access_token().await?;
        Ok(())
    }

    /// Check the status of a mint quote from the mint
    #[wasm_bindgen(js_name = "checkMintQuoteStatus")]
    pub async fn check_mint_quote_status(&self, quote_id: String) -> Result<JsValue, WasmError> {
        let quote = self.inner.check_mint_quote_status(&quote_id).await?;
        let wasm_quote: MintQuote = quote.into();
        serde_wasm_bindgen::to_value(&wasm_quote).map_err(WasmError::internal)
    }
}

/// Generates a new random mnemonic phrase
#[wasm_bindgen(js_name = "generateMnemonic")]
pub fn generate_mnemonic() -> Result<String, WasmError> {
    let mnemonic = Mnemonic::generate(12)
        .map_err(|e| WasmError::internal(format!("Failed to generate mnemonic: {}", e)))?;
    Ok(mnemonic.to_string())
}

/// Converts a mnemonic phrase to its entropy bytes
#[wasm_bindgen(js_name = "mnemonicToEntropy")]
pub fn mnemonic_to_entropy(mnemonic: String) -> Result<Vec<u8>, WasmError> {
    let m = Mnemonic::parse(&mnemonic)
        .map_err(|e| WasmError::internal(format!("Invalid mnemonic: {}", e)))?;
    Ok(m.to_entropy())
}

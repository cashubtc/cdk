//! WASM wallet exports via wasm-bindgen

use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::WalletBuilder as CdkWalletBuilder;
use wasm_bindgen::prelude::*;

use crate::error::FfiError;
use crate::types::*;
use crate::wallet::{generate_mnemonic, mnemonic_to_entropy, Wallet, WalletConfig};

/// Helper to convert FfiError to JsValue
fn to_js_err(e: FfiError) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// Helper to serialize a value to JsValue via serde_wasm_bindgen
fn to_js<T: serde::Serialize>(val: &T) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(val).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Helper to deserialize a JsValue via serde_wasm_bindgen
fn from_js<T: serde::de::DeserializeOwned>(val: JsValue) -> Result<T, JsValue> {
    serde_wasm_bindgen::from_value(val).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen]
impl Wallet {
    /// Create a new Wallet with localStorage backend
    ///
    /// # Arguments
    /// * `mint_url` - The mint URL string
    /// * `unit` - Currency unit as JsValue (e.g. "sat", "msat")
    /// * `mnemonic` - BIP39 mnemonic phrase
    /// * `config` - Optional WalletConfig as JsValue
    #[wasm_bindgen(constructor)]
    pub fn js_new(
        mint_url: String,
        unit: JsValue,
        mnemonic: String,
        config: JsValue,
    ) -> Result<Wallet, JsValue> {
        let unit: CurrencyUnit = from_js(unit)?;
        let config: WalletConfig = from_js(config)?;

        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| JsValue::from_str(&format!("Invalid mnemonic: {e}")))?;
        let seed = m.to_seed_normalized("");

        let localstore = super::localstorage::LocalStorageDatabase::new()
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        let wallet = CdkWalletBuilder::new()
            .mint_url(
                mint_url
                    .parse()
                    .map_err(|e: cdk::mint_url::Error| JsValue::from_str(&e.to_string()))?,
            )
            .unit(unit.into())
            .localstore(Arc::new(localstore))
            .seed(seed)
            .target_proof_count(config.target_proof_count.unwrap_or(3) as usize)
            .build()
            .map_err(|e| to_js_err(FfiError::from(e)))?;

        Ok(Wallet::from_inner(Arc::new(wallet)))
    }

    /// Get the mint URL
    #[wasm_bindgen(js_name = "mintUrl")]
    pub fn js_mint_url(&self) -> Result<JsValue, JsValue> {
        to_js(&self.mint_url())
    }

    /// Get the currency unit
    #[wasm_bindgen(js_name = "unit")]
    pub fn js_unit(&self) -> Result<JsValue, JsValue> {
        to_js(&self.unit())
    }

    /// Set metadata cache TTL in seconds
    #[wasm_bindgen(js_name = "setMetadataCacheTtl")]
    pub fn js_set_metadata_cache_ttl(&self, ttl_secs: Option<u64>) {
        self.set_metadata_cache_ttl(ttl_secs);
    }

    /// Get total balance
    #[wasm_bindgen(js_name = "totalBalance")]
    pub async fn js_total_balance(&self) -> Result<JsValue, JsValue> {
        let balance = self.total_balance().await.map_err(to_js_err)?;
        to_js(&balance)
    }

    /// Get total pending balance
    #[wasm_bindgen(js_name = "totalPendingBalance")]
    pub async fn js_total_pending_balance(&self) -> Result<JsValue, JsValue> {
        let balance = self.total_pending_balance().await.map_err(to_js_err)?;
        to_js(&balance)
    }

    /// Get total reserved balance
    #[wasm_bindgen(js_name = "totalReservedBalance")]
    pub async fn js_total_reserved_balance(&self) -> Result<JsValue, JsValue> {
        let balance = self.total_reserved_balance().await.map_err(to_js_err)?;
        to_js(&balance)
    }

    /// Fetch mint info from the mint server
    #[wasm_bindgen(js_name = "fetchMintInfo")]
    pub async fn js_fetch_mint_info(&self) -> Result<JsValue, JsValue> {
        let info = self.fetch_mint_info().await.map_err(to_js_err)?;
        to_js(&info)
    }

    /// Load mint info (from cache if fresh)
    #[wasm_bindgen(js_name = "loadMintInfo")]
    pub async fn js_load_mint_info(&self) -> Result<JsValue, JsValue> {
        let info = self.load_mint_info().await.map_err(to_js_err)?;
        to_js(&info)
    }

    /// Receive a cashu token string
    #[wasm_bindgen(js_name = "receive")]
    pub async fn js_receive(&self, token: &str, options: JsValue) -> Result<JsValue, JsValue> {
        let opts: ReceiveOptions = from_js(options)?;
        let amount = self
            .inner()
            .receive(token, opts.into())
            .await
            .map_err(|e| to_js_err(FfiError::from(e)))?;
        to_js(&Amount::from(amount))
    }

    /// Restore wallet from seed
    #[wasm_bindgen(js_name = "restore")]
    pub async fn js_restore(&self) -> Result<JsValue, JsValue> {
        let restored = self.restore().await.map_err(to_js_err)?;
        to_js(&restored)
    }

    /// Get pending send operation IDs
    #[wasm_bindgen(js_name = "getPendingSends")]
    pub async fn js_get_pending_sends(&self) -> Result<JsValue, JsValue> {
        let sends = self.get_pending_sends().await.map_err(to_js_err)?;
        to_js(&sends)
    }

    /// Revoke a pending send operation
    #[wasm_bindgen(js_name = "revokeSend")]
    pub async fn js_revoke_send(&self, operation_id: String) -> Result<JsValue, JsValue> {
        let amount = self.revoke_send(operation_id).await.map_err(to_js_err)?;
        to_js(&amount)
    }

    /// Check if a pending send has been claimed
    #[wasm_bindgen(js_name = "checkSendStatus")]
    pub async fn js_check_send_status(&self, operation_id: String) -> Result<bool, JsValue> {
        self.check_send_status(operation_id)
            .await
            .map_err(to_js_err)
    }

    /// Get a mint quote
    #[wasm_bindgen(js_name = "mintQuote")]
    pub async fn js_mint_quote(
        &self,
        payment_method: JsValue,
        amount: JsValue,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let method: PaymentMethod = from_js(payment_method)?;
        let amt: Option<Amount> = if amount.is_null() || amount.is_undefined() {
            None
        } else {
            Some(from_js(amount)?)
        };
        let quote = self
            .mint_quote(method, amt, description, extra)
            .await
            .map_err(to_js_err)?;
        to_js(&quote)
    }

    /// Check a mint quote status
    #[wasm_bindgen(js_name = "checkMintQuote")]
    pub async fn js_check_mint_quote(&self, quote_id: String) -> Result<JsValue, JsValue> {
        let quote = self.check_mint_quote(quote_id).await.map_err(to_js_err)?;
        to_js(&quote)
    }

    /// Fetch a mint quote from the mint
    #[wasm_bindgen(js_name = "fetchMintQuote")]
    pub async fn js_fetch_mint_quote(
        &self,
        quote_id: String,
        payment_method: JsValue,
    ) -> Result<JsValue, JsValue> {
        let method: Option<PaymentMethod> =
            if payment_method.is_null() || payment_method.is_undefined() {
                None
            } else {
                Some(from_js(payment_method)?)
            };
        let quote = self
            .fetch_mint_quote(quote_id, method)
            .await
            .map_err(to_js_err)?;
        to_js(&quote)
    }

    /// Mint tokens from a paid quote
    #[wasm_bindgen(js_name = "mint")]
    pub async fn js_mint(
        &self,
        quote_id: String,
        amount_split_target: JsValue,
        spending_conditions: JsValue,
    ) -> Result<JsValue, JsValue> {
        let split: SplitTarget = from_js(amount_split_target)?;
        let conditions: Option<SpendingConditions> =
            if spending_conditions.is_null() || spending_conditions.is_undefined() {
                None
            } else {
                Some(from_js(spending_conditions)?)
            };
        let proofs = self
            .mint(quote_id, split, conditions)
            .await
            .map_err(to_js_err)?;
        to_js(&proofs)
    }

    /// Get a melt quote
    #[wasm_bindgen(js_name = "meltQuote")]
    pub async fn js_melt_quote(
        &self,
        method: JsValue,
        request: String,
        options: JsValue,
        extra: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let payment_method: PaymentMethod = from_js(method)?;
        let melt_options: Option<MeltOptions> = if options.is_null() || options.is_undefined() {
            None
        } else {
            Some(from_js(options)?)
        };
        let quote = self
            .melt_quote(payment_method, request, melt_options, extra)
            .await
            .map_err(to_js_err)?;
        to_js(&quote)
    }

    /// Send tokens: prepare and confirm in one step, returns the cashu token string
    #[wasm_bindgen(js_name = "send")]
    pub async fn js_send(
        &self,
        amount: JsValue,
        options: JsValue,
        memo: Option<String>,
    ) -> Result<String, JsValue> {
        let amt: Amount = from_js(amount)?;
        let send_opts: SendOptions = from_js(options)?;
        let prepared = self.prepare_send(amt, send_opts).await.map_err(to_js_err)?;
        let token = prepared.confirm(memo).await.map_err(to_js_err)?;
        Ok(token.to_string())
    }

    /// Melt tokens: prepare and confirm in one step
    #[wasm_bindgen(js_name = "melt")]
    pub async fn js_melt(&self, quote_id: String) -> Result<JsValue, JsValue> {
        let prepared = self.prepare_melt(quote_id).await.map_err(to_js_err)?;
        let finalized = prepared.confirm().await.map_err(to_js_err)?;
        to_js(&finalized)
    }

    /// Swap proofs
    #[wasm_bindgen(js_name = "swap")]
    pub async fn js_swap(
        &self,
        amount: JsValue,
        amount_split_target: JsValue,
        input_proofs: JsValue,
        spending_conditions: JsValue,
        include_fees: bool,
    ) -> Result<JsValue, JsValue> {
        let amt: Option<Amount> = if amount.is_null() || amount.is_undefined() {
            None
        } else {
            Some(from_js(amount)?)
        };
        let split: SplitTarget = from_js(amount_split_target)?;
        let proofs: Proofs = from_js(input_proofs)?;
        let conditions: Option<SpendingConditions> =
            if spending_conditions.is_null() || spending_conditions.is_undefined() {
                None
            } else {
                Some(from_js(spending_conditions)?)
            };
        let result = self
            .swap(amt, split, proofs, conditions, include_fees)
            .await
            .map_err(to_js_err)?;
        to_js(&result)
    }

    /// Get proofs filtered by state
    #[wasm_bindgen(js_name = "getProofsByStates")]
    pub async fn js_get_proofs_by_states(&self, states: JsValue) -> Result<JsValue, JsValue> {
        let state_list: Vec<ProofState> = from_js(states)?;
        let proofs = self
            .get_proofs_by_states(state_list)
            .await
            .map_err(to_js_err)?;
        to_js(&proofs)
    }

    /// Check if proofs are spent
    #[wasm_bindgen(js_name = "checkProofsSpent")]
    pub async fn js_check_proofs_spent(&self, proofs: JsValue) -> Result<JsValue, JsValue> {
        let proof_list: Proofs = from_js(proofs)?;
        let spent = self
            .check_proofs_spent(proof_list)
            .await
            .map_err(to_js_err)?;
        to_js(&spent)
    }

    /// List transactions
    #[wasm_bindgen(js_name = "listTransactions")]
    pub async fn js_list_transactions(&self, direction: JsValue) -> Result<JsValue, JsValue> {
        let dir: Option<TransactionDirection> = if direction.is_null() || direction.is_undefined() {
            None
        } else {
            Some(from_js(direction)?)
        };
        let txs = self.list_transactions(dir).await.map_err(to_js_err)?;
        to_js(&txs)
    }

    /// Get a transaction by ID
    #[wasm_bindgen(js_name = "getTransaction")]
    pub async fn js_get_transaction(&self, id: JsValue) -> Result<JsValue, JsValue> {
        let tx_id: TransactionId = from_js(id)?;
        let tx = self.get_transaction(tx_id).await.map_err(to_js_err)?;
        to_js(&tx)
    }

    /// Get proofs for a transaction
    #[wasm_bindgen(js_name = "getProofsForTransaction")]
    pub async fn js_get_proofs_for_transaction(&self, id: JsValue) -> Result<JsValue, JsValue> {
        let tx_id: TransactionId = from_js(id)?;
        let proofs = self
            .get_proofs_for_transaction(tx_id)
            .await
            .map_err(to_js_err)?;
        to_js(&proofs)
    }

    /// Revert a transaction
    #[wasm_bindgen(js_name = "revertTransaction")]
    pub async fn js_revert_transaction(&self, id: JsValue) -> Result<(), JsValue> {
        let tx_id: TransactionId = from_js(id)?;
        self.revert_transaction(tx_id).await.map_err(to_js_err)
    }

    /// Refresh keysets from the mint
    #[wasm_bindgen(js_name = "refreshKeysets")]
    pub async fn js_refresh_keysets(&self) -> Result<JsValue, JsValue> {
        let keysets = self.refresh_keysets().await.map_err(to_js_err)?;
        to_js(&keysets)
    }

    /// Get the active keyset
    #[wasm_bindgen(js_name = "getActiveKeyset")]
    pub async fn js_get_active_keyset(&self) -> Result<JsValue, JsValue> {
        let keyset = self.get_active_keyset().await.map_err(to_js_err)?;
        to_js(&keyset)
    }

    /// Get fees for a keyset by ID
    #[wasm_bindgen(js_name = "getKeysetFeesById")]
    pub async fn js_get_keyset_fees_by_id(&self, keyset_id: String) -> Result<u64, JsValue> {
        self.get_keyset_fees_by_id(keyset_id)
            .await
            .map_err(to_js_err)
    }

    /// Check all pending proofs
    #[wasm_bindgen(js_name = "checkAllPendingProofs")]
    pub async fn js_check_all_pending_proofs(&self) -> Result<JsValue, JsValue> {
        let amount = self.check_all_pending_proofs().await.map_err(to_js_err)?;
        to_js(&amount)
    }

    /// Calculate fee for proofs
    #[wasm_bindgen(js_name = "calculateFee")]
    pub async fn js_calculate_fee(
        &self,
        proof_count: u32,
        keyset_id: String,
    ) -> Result<JsValue, JsValue> {
        let fee = self
            .calculate_fee(proof_count, keyset_id)
            .await
            .map_err(to_js_err)?;
        to_js(&fee)
    }

    /// Set Clear Auth Token
    #[wasm_bindgen(js_name = "setCat")]
    pub async fn js_set_cat(&self, cat: String) -> Result<(), JsValue> {
        self.set_cat(cat).await.map_err(to_js_err)
    }

    /// Set refresh token
    #[wasm_bindgen(js_name = "setRefreshToken")]
    pub async fn js_set_refresh_token(&self, refresh_token: String) -> Result<(), JsValue> {
        self.set_refresh_token(refresh_token)
            .await
            .map_err(to_js_err)
    }

    /// Refresh access token
    #[wasm_bindgen(js_name = "refreshAccessToken")]
    pub async fn js_refresh_access_token(&self) -> Result<(), JsValue> {
        self.refresh_access_token().await.map_err(to_js_err)
    }

    /// Mint blind auth tokens
    #[wasm_bindgen(js_name = "mintBlindAuth")]
    pub async fn js_mint_blind_auth(&self, amount: JsValue) -> Result<JsValue, JsValue> {
        let amt: Amount = from_js(amount)?;
        let proofs = self.mint_blind_auth(amt).await.map_err(to_js_err)?;
        to_js(&proofs)
    }

    /// Get unspent auth proofs
    #[wasm_bindgen(js_name = "getUnspentAuthProofs")]
    pub async fn js_get_unspent_auth_proofs(&self) -> Result<JsValue, JsValue> {
        let proofs = self.get_unspent_auth_proofs().await.map_err(to_js_err)?;
        to_js(&proofs)
    }
}

/// Generate a new random BIP39 mnemonic (12 words)
#[wasm_bindgen(js_name = "generateMnemonic")]
pub fn js_generate_mnemonic() -> Result<String, JsValue> {
    generate_mnemonic().map_err(to_js_err)
}

/// Convert a mnemonic phrase to its entropy bytes
#[wasm_bindgen(js_name = "mnemonicToEntropy")]
pub fn js_mnemonic_to_entropy(mnemonic: String) -> Result<Vec<u8>, JsValue> {
    mnemonic_to_entropy(mnemonic).map_err(to_js_err)
}

use std::ops::Deref;

use cashu_js::nuts::nut00::{JsBlindedMessages, JsToken};
use cashu_js::nuts::nut01::JsKeys;
use cashu_js::nuts::nut03::JsRequestMintResponse;
#[cfg(feature = "nut07")]
use cashu_js::JsProofsStatus;
use cashu_js::{JsAmount, JsBolt11Invoice};
use cashu_sdk::client::gloo_client::HttpClient;
use cashu_sdk::wallet::Wallet;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::types::{JsMelted, JsSendProofs};

#[wasm_bindgen(js_name = Wallet)]
pub struct JsWallet {
    inner: Wallet<HttpClient>,
}

impl Deref for JsWallet {
    type Target = Wallet<HttpClient>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Wallet<HttpClient>> for JsWallet {
    fn from(inner: Wallet<HttpClient>) -> JsWallet {
        JsWallet { inner }
    }
}

#[wasm_bindgen(js_class = Wallet)]
impl JsWallet {
    #[wasm_bindgen(constructor)]
    pub fn new(mint_url: String, mint_keys: JsKeys) -> JsWallet {
        let client = HttpClient {};
        JsWallet {
            inner: Wallet::new(client, mint_url.into(), mint_keys.deref().clone()),
        }
    }

    /// Check Proofs spent
    #[cfg(feature = "nut07")]
    #[wasm_bindgen(js_name = checkProofsSpent)]
    pub async fn check_proofs_spent(&self, proofs: JsValue) -> Result<JsProofsStatus> {
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;

        Ok(self
            .inner
            .check_proofs_spent(&proofs)
            .await
            .map_err(into_err)?
            .into())
    }

    /// Request Mint
    #[wasm_bindgen(js_name = requestMint)]
    pub async fn request_mint(&self, amount: JsAmount) -> Result<JsRequestMintResponse> {
        Ok(self
            .inner
            .request_mint(*amount.deref())
            .await
            .map_err(into_err)?
            .into())
    }

    /// Mint Token
    #[wasm_bindgen(js_name = mintToken)]
    pub async fn mint_token(
        &self,
        amount: JsAmount,
        hash: String,
        unit: Option<String>,
        memo: Option<String>,
    ) -> Result<JsToken> {
        Ok(self
            .inner
            .mint_token(*amount.deref(), &hash, unit, memo)
            .await
            .map_err(into_err)?
            .into())
    }

    /// Mint
    #[wasm_bindgen(js_name = mint)]
    pub async fn mint(&self, amount: JsAmount, hash: String) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(
            &self
                .inner
                .mint(*amount.deref(), &hash)
                .await
                .map_err(into_err)?,
        )
        .map_err(into_err)
    }

    /// Check Fee
    #[wasm_bindgen(js_name = checkFee)]
    pub async fn check_fee(&self, invoice: JsBolt11Invoice) -> Result<JsAmount> {
        Ok(self
            .inner
            .check_fee(invoice.deref().clone())
            .await
            .map_err(into_err)?
            .into())
    }

    /// Receive
    #[wasm_bindgen(js_name = receive)]
    pub async fn receive(&self, token: String) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.receive(&token).await.map_err(into_err)?)
            .map_err(into_err)
    }

    /// Process Split
    #[wasm_bindgen(js_name = processSplitResponse)]
    pub fn process_split_response(
        &self,
        blinded_messages: JsBlindedMessages,
        promises: JsValue,
    ) -> Result<JsValue> {
        let promises = serde_wasm_bindgen::from_value(promises).map_err(into_err)?;

        serde_wasm_bindgen::to_value(
            &self
                .inner
                .process_split_response(blinded_messages.deref().clone(), promises)
                .map_err(into_err)?,
        )
        .map_err(into_err)
    }

    /// Send
    #[wasm_bindgen(js_name = send)]
    pub async fn send(&self, amount: JsAmount, proofs: JsValue) -> Result<JsSendProofs> {
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;

        Ok(self
            .inner
            .send(*amount.deref(), proofs)
            .await
            .map_err(into_err)?
            .into())
    }

    /// Melt
    #[wasm_bindgen(js_name = melt)]
    pub async fn melt(
        &self,
        invoice: JsBolt11Invoice,
        proofs: JsValue,
        fee_reserve: JsAmount,
    ) -> Result<JsMelted> {
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;

        Ok(self
            .inner
            .melt(invoice.deref().clone(), proofs, *fee_reserve.deref())
            .await
            .map_err(into_err)?
            .into())
    }

    /// Proofs to token
    #[wasm_bindgen(js_name = proofsToToken)]
    pub fn proofs_to_token(
        &self,
        proofs: JsValue,
        unit: Option<String>,
        memo: Option<String>,
    ) -> Result<String> {
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;

        self.inner
            .proofs_to_token(proofs, unit, memo)
            .map_err(into_err)
    }
}

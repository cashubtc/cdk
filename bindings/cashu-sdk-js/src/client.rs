use std::ops::Deref;

use cashu_js::nuts::nut00::JsBlindedMessages;
use cashu_js::nuts::nut01::JsKeys;
use cashu_js::nuts::nut02::JsKeySetsResponse;
use cashu_js::nuts::nut03::JsRequestMintResponse;
use cashu_js::nuts::nut04::JsPostMintResponse;
use cashu_js::nuts::nut05::JsCheckFeesResponse;
use cashu_js::nuts::nut06::{JsSplitRequest, JsSplitResponse};
#[cfg(feature = "nut07")]
use cashu_js::nuts::nut07::JsCheckSpendableResponse;
use cashu_js::nuts::nut08::JsMeltResponse;
#[cfg(feature = "nut09")]
use cashu_js::nuts::nut09::JsMintInfo;
use cashu_js::{JsAmount, JsBolt11Invoice};
use cashu_sdk::client::Client;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = Client)]
pub struct JsClient {
    inner: Client,
}

impl Deref for JsClient {
    type Target = Client;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Client> for JsClient {
    fn from(inner: Client) -> JsClient {
        JsClient { inner }
    }
}

#[wasm_bindgen(js_class = Client)]
impl JsClient {
    #[wasm_bindgen(constructor)]
    pub fn new(mint_url: String) -> Result<JsClient> {
        Ok(JsClient {
            inner: Client::new(&mint_url).map_err(into_err)?,
        })
    }

    /// Get Keys [NUT-01]
    #[wasm_bindgen(js_name = getKeys)]
    pub async fn get_keys(&self) -> Result<JsKeys> {
        Ok(self.inner.get_keys().await.map_err(into_err)?.into())
    }

    /// Get Keys [NUT-01]
    #[wasm_bindgen(js_name = getKeysets)]
    pub async fn get_keysets(&self) -> Result<JsKeySetsResponse> {
        Ok(self.inner.get_keysets().await.map_err(into_err)?.into())
    }

    /// Request Mint [NUT-03]
    #[wasm_bindgen(js_name = requestMint)]
    pub async fn request_mint(&self, amount: JsAmount) -> Result<JsRequestMintResponse> {
        Ok(self
            .inner
            .request_mint(*amount.deref())
            .await
            .map_err(into_err)?
            .into())
    }

    /// Mint [NUT-04]
    #[wasm_bindgen(js_name = mint)]
    pub async fn mint(
        &self,
        blinded_messages: JsBlindedMessages,
        hash: String,
    ) -> Result<JsPostMintResponse> {
        Ok(self
            .inner
            .mint(blinded_messages.deref().clone(), &hash)
            .await
            .map_err(into_err)?
            .into())
    }

    /// Check Max expected fee [NUT-05]
    #[wasm_bindgen(js_name = check_fees)]
    pub async fn check_fees(&self, invoice: JsBolt11Invoice) -> Result<JsCheckFeesResponse> {
        Ok(self
            .inner
            .check_fees(invoice.deref().clone())
            .await
            .map_err(into_err)?
            .into())
    }

    /// Melt [NUT-05]
    #[wasm_bindgen(js_name = melt)]
    pub async fn melt(
        &self,
        proofs: JsValue,
        invoice: JsBolt11Invoice,
        outputs: JsValue,
    ) -> Result<JsMeltResponse> {
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;
        let outputs = if outputs.is_null() {
            None
        } else {
            Some(serde_wasm_bindgen::from_value(outputs).map_err(into_err)?)
        };
        Ok(self
            .inner
            .melt(proofs, invoice.deref().clone(), outputs)
            .await
            .map_err(into_err)?
            .into())
    }

    /// Split [NUT-06]
    #[wasm_bindgen(js_name = split)]
    pub async fn split(&self, split_request: JsSplitRequest) -> Result<JsSplitResponse> {
        Ok(self
            .inner
            .split(split_request.deref().clone())
            .await
            .map_err(into_err)?
            .into())
    }

    #[cfg(feature = "nut07")]
    #[wasm_bindgen(js_name = checkSpendable)]
    pub async fn check_spendable(&self, proofs: JsValue) -> Result<JsCheckSpendableResponse> {
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;

        Ok(self
            .inner
            .check_spendable(&proofs)
            .await
            .map_err(into_err)?
            .into())
    }

    #[cfg(feature = "nut09")]
    #[wasm_bindgen(js_name = getInfo)]
    pub async fn get_info(&self) -> Result<JsMintInfo> {
        Ok(self.inner.get_info().await.map_err(into_err)?.into())
    }
}

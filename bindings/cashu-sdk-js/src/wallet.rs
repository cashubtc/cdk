use std::ops::Deref;
use std::str::FromStr;

use cashu_js::nuts::nut00::{JsBlindedMessages, JsToken};
use cashu_js::nuts::nut01::JsKeys;
use cashu_js::JsAmount;
#[cfg(feature = "nut07")]
use cashu_js::JsProofsStatus;
use cashu_sdk::client::gloo_client::HttpClient;
use cashu_sdk::nuts::CurrencyUnit;
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
    // TODO: Quotes
    #[wasm_bindgen(constructor)]
    pub fn new(mint_url: String, mint_keys: JsKeys) -> JsWallet {
        let client = HttpClient {};

        JsWallet {
            inner: Wallet::new(client, mint_url.into(), vec![], mint_keys.deref().clone()),
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

    /// Mint Token
    #[wasm_bindgen(js_name = mintToken)]
    pub async fn mint_token(
        &mut self,
        amount: JsAmount,
        memo: Option<String>,
        unit: Option<String>,
    ) -> Result<JsToken> {
        let unit = unit.map(|u| CurrencyUnit::from_str(&u).unwrap_or_default());

        Ok(self
            .inner
            .mint_token(*amount.deref(), memo, unit)
            .await
            .map_err(into_err)?
            .into())
    }

    /// Mint
    #[wasm_bindgen(js_name = mint)]
    pub async fn mint(&mut self, quote: String) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.mint(&quote).await.map_err(into_err)?)
            .map_err(into_err)
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
        quote: String,
        proofs: JsValue,
        fee_reserve: JsAmount,
    ) -> Result<JsMelted> {
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;

        Ok(self
            .inner
            .melt(quote, proofs, *fee_reserve.deref())
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

        let unit = unit.map(|u| {
            CurrencyUnit::from_str(&u)
                .map_err(into_err)
                .unwrap_or_default()
        });

        self.inner
            .proofs_to_token(proofs, memo, unit)
            .map_err(into_err)
    }
}

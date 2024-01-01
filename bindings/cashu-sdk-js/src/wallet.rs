use std::collections::HashMap;
use std::ops::Deref;
use std::str::FromStr;

use cashu_js::nuts::nut00::{JsBlindedMessages, JsToken};
use cashu_js::nuts::nut01::JsKeys;
use cashu_js::JsAmount;
#[cfg(feature = "nut07")]
use cashu_js::JsProofsStatus;
use cashu_sdk::client::gloo_client::HttpClient;
use cashu_sdk::nuts::{CurrencyUnit, Id};
use cashu_sdk::url::UncheckedUrl;
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
    pub fn new(mint_urls: Vec<String>, mint_keys: Vec<JsKeys>) -> JsWallet {
        let client = HttpClient {};

        let mints = mint_urls
            .iter()
            .map(|u| (UncheckedUrl::from_str(u).unwrap(), None))
            .collect();

        let keys = mint_keys
            .iter()
            .map(|k| (Id::from(k.deref()), k.deref().clone()))
            .collect();

        JsWallet {
            inner: Wallet::new(client, mints, HashMap::new(), vec![], vec![], None, keys),
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
        mint_url: String,
        amount: JsAmount,
        memo: Option<String>,
        unit: Option<String>,
    ) -> Result<JsToken> {
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;
        let unit = unit.map(|u| CurrencyUnit::from_str(&u).unwrap_or_default());

        Ok(self
            .inner
            .mint_token(mint_url, *amount.deref(), memo, unit)
            .await
            .map_err(into_err)?
            .into())
    }

    /// Mint
    #[wasm_bindgen(js_name = mint)]
    pub async fn mint(&mut self, mint_url: String, quote: String) -> Result<JsValue> {
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;
        serde_wasm_bindgen::to_value(&self.inner.mint(mint_url, &quote).await.map_err(into_err)?)
            .map_err(into_err)
    }

    /// Receive
    #[wasm_bindgen(js_name = receive)]
    pub async fn receive(&mut self, token: String) -> Result<JsValue> {
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
    pub async fn send(
        &mut self,
        mint_url: String,
        amount: JsAmount,
        unit: String,
        proofs: JsValue,
    ) -> Result<JsSendProofs> {
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;
        let unit = CurrencyUnit::from_str(&unit).map_err(into_err)?;
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;

        Ok(self
            .inner
            .send(&mint_url, &unit, *amount.deref(), proofs)
            .await
            .map_err(into_err)?
            .into())
    }

    /// Melt
    #[wasm_bindgen(js_name = melt)]
    pub async fn melt(
        &mut self,
        mint_url: String,
        quote: String,
        proofs: JsValue,
    ) -> Result<JsMelted> {
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;

        Ok(self
            .inner
            .melt(&mint_url, &quote, proofs)
            .await
            .map_err(into_err)?
            .into())
    }

    /// Proofs to token
    #[wasm_bindgen(js_name = proofsToToken)]
    pub fn proofs_to_token(
        &self,
        mint_url: String,
        proofs: JsValue,
        unit: Option<String>,
        memo: Option<String>,
    ) -> Result<String> {
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;

        let unit = unit.map(|u| {
            CurrencyUnit::from_str(&u)
                .map_err(into_err)
                .unwrap_or_default()
        });

        self.inner
            .proofs_to_token(mint_url, proofs, memo, unit)
            .map_err(into_err)
    }
}

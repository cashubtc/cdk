use std::ops::Deref;

use cashu_js::nuts::nut00::{JsBlindedMessages, JsToken};
use cashu_js::nuts::nut01::JsKeys;
use cashu_js::nuts::nut03::JsRequestMintResponse;
use cashu_js::{JsAmount, JsBolt11Invoice, JsProofsStatus};
use cashu_sdk::wallet::Wallet;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::types::{JsMelted, JsSendProofs};
use crate::JsClient;

#[wasm_bindgen(js_name = Wallet)]
pub struct JsWallet {
    inner: Wallet,
}

impl Deref for JsWallet {
    type Target = Wallet;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Wallet> for JsWallet {
    fn from(inner: Wallet) -> JsWallet {
        JsWallet { inner }
    }
}

#[wasm_bindgen(js_class = Wallet)]
impl JsWallet {
    #[wasm_bindgen(constructor)]
    pub fn new(client: JsClient, mint_keys: JsKeys) -> JsWallet {
        JsWallet {
            inner: Wallet::new(client.deref().clone(), mint_keys.deref().clone()),
        }
    }

    /// Check Proofs spent
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
    pub async fn mint_token(&self, amount: JsAmount, hash: String) -> Result<JsToken> {
        Ok(self
            .inner
            .mint_token(*amount.deref(), &hash)
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
    pub fn proofs_to_token(&self, proofs: JsValue, memo: Option<String>) -> Result<String> {
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;

        Ok(self
            .inner
            .proofs_to_token(proofs, memo)
            .map_err(into_err)?
            .into())
    }
}

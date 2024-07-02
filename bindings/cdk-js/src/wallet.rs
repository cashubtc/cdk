//! Wallet Js Bindings

use std::ops::Deref;
use std::sync::Arc;

use cdk::amount::SplitTarget;
use cdk::nuts::{Proofs, SecretKey};
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_rexie::WalletRexieDatabase;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::nuts::nut01::JsSecretKey;
use crate::nuts::nut04::JsMintQuoteBolt11Response;
use crate::nuts::nut05::JsMeltQuoteBolt11Response;
use crate::nuts::nut11::JsP2PKSpendingConditions;
use crate::nuts::nut14::JsHTLCSpendingConditions;
use crate::nuts::{JsCurrencyUnit, JsMintInfo, JsProof};
use crate::types::melt_quote::JsMeltQuote;
use crate::types::{JsAmount, JsMelted, JsMintQuote};

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
    pub async fn new(mints_url: String, unit: JsCurrencyUnit, seed: Vec<u8>) -> Self {
        let db = WalletRexieDatabase::new().await.unwrap();

        Wallet::new(&mints_url, unit.into(), Arc::new(db), &seed).into()
    }

    #[wasm_bindgen(js_name = totalBalance)]
    pub async fn total_balance(&self) -> Result<JsValue> {
        Ok(serde_wasm_bindgen::to_value(
            &self.inner.total_balance().await.map_err(into_err)?,
        )?)
    }

    #[wasm_bindgen(js_name = totalPendingBalance)]
    pub async fn total_pending_balance(&self) -> Result<JsValue> {
        Ok(serde_wasm_bindgen::to_value(
            &self.inner.total_pending_balance().await.map_err(into_err)?,
        )?)
    }

    #[wasm_bindgen(js_name = checkAllPendingProofs)]
    pub async fn check_all_pending_proofs(&self) -> Result<JsAmount> {
        Ok(self
            .inner
            .check_all_pending_proofs()
            .await
            .map_err(into_err)?
            .into())
    }

    #[wasm_bindgen(js_name = getMintInfo)]
    pub async fn get_mint_info(&self) -> Result<Option<JsMintInfo>> {
        Ok(self
            .inner
            .get_mint_info()
            .await
            .map_err(into_err)?
            .map(|i| i.into()))
    }

    #[wasm_bindgen(js_name = refreshMint)]
    pub async fn refresh_mint_keys(&self) -> Result<()> {
        self.inner.refresh_mint_keys().await.map_err(into_err)?;
        Ok(())
    }

    #[wasm_bindgen(js_name = mintQuote)]
    pub async fn mint_quote(&mut self, amount: u64) -> Result<JsMintQuote> {
        let quote = self
            .inner
            .mint_quote(amount.into())
            .await
            .map_err(into_err)?;

        Ok(quote.into())
    }

    #[wasm_bindgen(js_name = mintQuoteStatus)]
    pub async fn mint_quote_status(&self, quote_id: String) -> Result<JsMintQuoteBolt11Response> {
        let quote = self
            .inner
            .mint_quote_state(&quote_id)
            .await
            .map_err(into_err)?;

        Ok(quote.into())
    }

    #[wasm_bindgen(js_name = checkAllMintQuotes)]
    pub async fn check_all_mint_quotes(&self) -> Result<JsAmount> {
        let amount = self.inner.check_all_mint_quotes().await.map_err(into_err)?;

        Ok(amount.into())
    }

    #[wasm_bindgen(js_name = mint)]
    pub async fn mint(
        &mut self,
        quote_id: String,
        p2pk_condition: Option<JsP2PKSpendingConditions>,
        htlc_condition: Option<JsHTLCSpendingConditions>,
        split_target_amount: Option<JsAmount>,
    ) -> Result<JsAmount> {
        let target = split_target_amount
            .map(|a| SplitTarget::Value(*a.deref()))
            .unwrap_or_default();
        let conditions = match (p2pk_condition, htlc_condition) {
            (Some(_), Some(_)) => {
                return Err(JsValue::from_str(
                    "Cannot define both p2pk and htlc conditions",
                ));
            }
            (None, Some(htlc_condition)) => Some(htlc_condition.deref().clone()),
            (Some(p2pk_condition), None) => Some(p2pk_condition.deref().clone()),
            (None, None) => None,
        };

        Ok(self
            .inner
            .mint(&quote_id, target, conditions)
            .await
            .map_err(into_err)?
            .into())
    }

    #[wasm_bindgen(js_name = meltQuote)]
    pub async fn melt_quote(
        &mut self,
        request: String,
        mpp_amount: Option<JsAmount>,
    ) -> Result<JsMeltQuote> {
        let melt_quote = self
            .inner
            .melt_quote(request, mpp_amount.map(|a| *a.deref()))
            .await
            .map_err(into_err)?;

        Ok(melt_quote.into())
    }

    #[wasm_bindgen(js_name = meltQuoteStatus)]
    pub async fn melt_quote_status(&self, quote_id: String) -> Result<JsMeltQuoteBolt11Response> {
        let quote = self
            .inner
            .melt_quote_status(&quote_id)
            .await
            .map_err(into_err)?;

        Ok(quote.into())
    }

    #[wasm_bindgen(js_name = melt)]
    pub async fn melt(
        &mut self,
        quote_id: String,
        split_target_amount: Option<JsAmount>,
    ) -> Result<JsMelted> {
        let target = split_target_amount
            .map(|a| SplitTarget::Value(*a.deref()))
            .unwrap_or_default();

        let melted = self.inner.melt(&quote_id, target).await.map_err(into_err)?;

        Ok(melted.into())
    }

    #[wasm_bindgen(js_name = receive)]
    pub async fn receive(
        &mut self,
        encoded_token: String,
        signing_keys: Vec<JsSecretKey>,
        preimages: Vec<String>,
    ) -> Result<JsAmount> {
        let signing_keys: Vec<SecretKey> = signing_keys.iter().map(|s| s.deref().clone()).collect();

        Ok(self
            .inner
            .receive(
                &encoded_token,
                &SplitTarget::default(),
                &signing_keys,
                &preimages,
            )
            .await
            .map_err(into_err)?
            .into())
    }

    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = send)]
    pub async fn send(
        &mut self,
        memo: Option<String>,
        amount: u64,
        p2pk_condition: Option<JsP2PKSpendingConditions>,
        htlc_condition: Option<JsHTLCSpendingConditions>,
        split_target_amount: Option<JsAmount>,
    ) -> Result<String> {
        let conditions = match (p2pk_condition, htlc_condition) {
            (Some(_), Some(_)) => {
                return Err(JsValue::from_str(
                    "Cannot define both p2pk and htlc conditions",
                ));
            }
            (None, Some(htlc_condition)) => Some(htlc_condition.deref().clone()),
            (Some(p2pk_condition), None) => Some(p2pk_condition.deref().clone()),
            (None, None) => None,
        };

        let target = split_target_amount
            .map(|a| SplitTarget::Value(*a.deref()))
            .unwrap_or_default();
        self.inner
            .send(Amount::from(amount), memo, conditions, &target)
            .await
            .map_err(into_err)
    }

    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = swap)]
    pub async fn swap(
        &mut self,
        amount: u64,
        input_proofs: Vec<JsProof>,
        p2pk_condition: Option<JsP2PKSpendingConditions>,
        htlc_condition: Option<JsHTLCSpendingConditions>,
        split_target_amount: Option<JsAmount>,
    ) -> Result<JsValue> {
        let conditions = match (p2pk_condition, htlc_condition) {
            (Some(_), Some(_)) => {
                return Err(JsValue::from_str(
                    "Cannot define both p2pk and htlc conditions",
                ));
            }
            (None, Some(htlc_condition)) => Some(htlc_condition.deref().clone()),
            (Some(p2pk_condition), None) => Some(p2pk_condition.deref().clone()),
            (None, None) => None,
        };

        let proofs: Proofs = input_proofs.iter().map(|p| p.deref()).cloned().collect();

        let target = split_target_amount
            .map(|a| SplitTarget::Value(*a.deref()))
            .unwrap_or_default();
        let post_swap_proofs = self
            .inner
            .swap(Some(Amount::from(amount)), &target, proofs, conditions)
            .await
            .map_err(into_err)?;

        Ok(serde_wasm_bindgen::to_value(&post_swap_proofs)?)
    }
}

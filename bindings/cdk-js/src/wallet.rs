use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cdk::amount::SplitTarget;
use cdk::nuts::Proofs;
use cdk::url::UncheckedUrl;
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_rexie::RexieWalletDatabase;
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
    pub async fn new(seed: Vec<u8>, p2pk_signing_keys: Vec<JsSecretKey>) -> Self {
        let db = RexieWalletDatabase::new().await.unwrap();

        Wallet::new(
            Arc::new(db),
            &seed,
            p2pk_signing_keys
                .into_iter()
                .map(|s| s.deref().clone())
                .collect(),
        )
        .into()
    }

    #[wasm_bindgen(js_name = unitBalance)]
    pub async fn unit_balance(&self, unit: JsCurrencyUnit) -> Result<JsAmount> {
        Ok(self
            .inner
            .unit_balance(unit.into())
            .await
            .map_err(into_err)?
            .into())
    }

    #[wasm_bindgen(js_name = pendingUnitBalance)]
    pub async fn pending_unit_balance(&self, unit: JsCurrencyUnit) -> Result<JsAmount> {
        Ok(self
            .inner
            .pending_unit_balance(unit.into())
            .await
            .map_err(into_err)?
            .into())
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
    pub async fn check_all_pending_proofs(&self, mint_url: Option<String>) -> Result<JsAmount> {
        let mint_url = match mint_url {
            Some(url) => Some(UncheckedUrl::from_str(&url).map_err(into_err)?),
            None => None,
        };

        Ok(self
            .inner
            .check_all_pending_proofs(mint_url)
            .await
            .map_err(into_err)?
            .into())
    }

    #[wasm_bindgen(js_name = mintBalances)]
    pub async fn mint_balances(&self) -> Result<JsValue> {
        let mint_balances = self.inner.mint_balances().await.map_err(into_err)?;

        Ok(serde_wasm_bindgen::to_value(&mint_balances)?)
    }

    #[wasm_bindgen(js_name = addMint)]
    pub async fn add_mint(&self, mint_url: String) -> Result<Option<JsMintInfo>> {
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;

        Ok(self
            .inner
            .add_mint(mint_url)
            .await
            .map_err(into_err)?
            .map(|i| i.into()))
    }

    #[wasm_bindgen(js_name = refreshMint)]
    pub async fn refresh_mint_keys(&self, mint_url: String) -> Result<()> {
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;
        self.inner
            .refresh_mint_keys(&mint_url)
            .await
            .map_err(into_err)?;
        Ok(())
    }

    #[wasm_bindgen(js_name = mintQuote)]
    pub async fn mint_quote(
        &mut self,
        mint_url: String,
        amount: u64,
        unit: JsCurrencyUnit,
    ) -> Result<JsMintQuote> {
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;
        let quote = self
            .inner
            .mint_quote(mint_url, amount.into(), unit.into())
            .await
            .map_err(into_err)?;

        Ok(quote.into())
    }

    #[wasm_bindgen(js_name = mintQuoteStatus)]
    pub async fn mint_quote_status(
        &self,
        mint_url: String,
        quote_id: String,
    ) -> Result<JsMintQuoteBolt11Response> {
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;

        let quote = self
            .inner
            .mint_quote_status(mint_url, &quote_id)
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
        mint_url: String,
        quote_id: String,
        p2pk_condition: Option<JsP2PKSpendingConditions>,
        htlc_condition: Option<JsHTLCSpendingConditions>,
        split_target_amount: Option<JsAmount>,
    ) -> Result<JsAmount> {
        let target = split_target_amount
            .map(|a| SplitTarget::Value(*a.deref()))
            .unwrap_or_default();
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;
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
            .mint(mint_url, &quote_id, target, conditions)
            .await
            .map_err(into_err)?
            .into())
    }

    #[wasm_bindgen(js_name = meltQuote)]
    pub async fn melt_quote(
        &mut self,
        mint_url: String,
        unit: JsCurrencyUnit,
        request: String,
    ) -> Result<JsMeltQuote> {
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;
        let melt_quote = self
            .inner
            .melt_quote(mint_url, unit.into(), request)
            .await
            .map_err(into_err)?;

        Ok(melt_quote.into())
    }

    #[wasm_bindgen(js_name = meltQuoteStatus)]
    pub async fn melt_quote_status(
        &self,
        mint_url: String,
        quote_id: String,
    ) -> Result<JsMeltQuoteBolt11Response> {
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;

        let quote = self
            .inner
            .melt_quote_status(mint_url, &quote_id)
            .await
            .map_err(into_err)?;

        Ok(quote.into())
    }

    #[wasm_bindgen(js_name = melt)]
    pub async fn melt(
        &mut self,
        mint_url: String,
        quote_id: String,
        split_target_amount: Option<JsAmount>,
    ) -> Result<JsMelted> {
        let target = split_target_amount
            .map(|a| SplitTarget::Value(*a.deref()))
            .unwrap_or_default();
        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;

        let melted = self
            .inner
            .melt(&mint_url, &quote_id, target)
            .await
            .map_err(into_err)?;

        Ok(melted.into())
    }

    #[wasm_bindgen(js_name = receive)]
    pub async fn receive(&mut self, encoded_token: String, preimages: JsValue) -> Result<JsAmount> {
        let preimages: Option<Vec<String>> = serde_wasm_bindgen::from_value(preimages)?;

        Ok(self
            .inner
            .receive(&encoded_token, &SplitTarget::default(), preimages)
            .await
            .map_err(into_err)?
            .into())
    }

    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = send)]
    pub async fn send(
        &mut self,
        mint_url: String,
        unit: JsCurrencyUnit,
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

        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;

        let target = split_target_amount
            .map(|a| SplitTarget::Value(*a.deref()))
            .unwrap_or_default();
        self.inner
            .send(
                &mint_url,
                unit.into(),
                memo,
                Amount::from(amount),
                &target,
                conditions,
            )
            .await
            .map_err(into_err)
    }

    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = swap)]
    pub async fn swap(
        &mut self,
        mint_url: String,
        unit: JsCurrencyUnit,
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

        let mint_url = UncheckedUrl::from_str(&mint_url).map_err(into_err)?;

        let proofs: Proofs = input_proofs.iter().map(|p| p.deref()).cloned().collect();

        let target = split_target_amount
            .map(|a| SplitTarget::Value(*a.deref()))
            .unwrap_or_default();
        let post_swap_proofs = self
            .inner
            .swap(
                &mint_url,
                &unit.into(),
                Some(Amount::from(amount)),
                &target,
                proofs,
                conditions,
            )
            .await
            .map_err(into_err)?;

        Ok(serde_wasm_bindgen::to_value(&post_swap_proofs)?)
    }
}

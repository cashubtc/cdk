use cdk::nuts::CurrencyUnit;
use wasm_bindgen::prelude::*;

// use crate::nuts::{JsHTLCWitness, JsP2PKWitness};

#[wasm_bindgen(js_name = CurrencyUnit)]
pub enum JsCurrencyUnit {
    Sat,
    Msat,
    Usd,
    Eur,
    Hash,
}

impl From<CurrencyUnit> for JsCurrencyUnit {
    fn from(inner: CurrencyUnit) -> JsCurrencyUnit {
        match inner {
            CurrencyUnit::Sat => JsCurrencyUnit::Sat,
            CurrencyUnit::Msat => JsCurrencyUnit::Msat,
            CurrencyUnit::Usd => JsCurrencyUnit::Usd,
            CurrencyUnit::Eur => JsCurrencyUnit::Eur,
            CurrencyUnit::Hash => JsCurrencyUnit::Hash,
        }
    }
}

impl From<JsCurrencyUnit> for CurrencyUnit {
    fn from(inner: JsCurrencyUnit) -> CurrencyUnit {
        match inner {
            JsCurrencyUnit::Sat => CurrencyUnit::Sat,
            JsCurrencyUnit::Msat => CurrencyUnit::Msat,
            JsCurrencyUnit::Usd => CurrencyUnit::Usd,
            JsCurrencyUnit::Eur => CurrencyUnit::Eur,
            JsCurrencyUnit::Hash => CurrencyUnit::Hash,
        }
    }
}

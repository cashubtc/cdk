use cdk::nuts::PaymentMethod;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = PaymentMethod)]
pub enum JsPaymentMethod {
    Bolt11,
}

impl From<PaymentMethod> for JsPaymentMethod {
    fn from(inner: PaymentMethod) -> JsPaymentMethod {
        match inner {
            PaymentMethod::Bolt11 => JsPaymentMethod::Bolt11,
            PaymentMethod::Custom(_) => todo!(),
        }
    }
}

impl From<JsPaymentMethod> for PaymentMethod {
    fn from(inner: JsPaymentMethod) -> PaymentMethod {
        match inner {
            JsPaymentMethod::Bolt11 => PaymentMethod::Bolt11,
        }
    }
}

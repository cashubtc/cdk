use wasm_bindgen::prelude::*;

pub mod error;
pub mod nuts;
mod types;

pub use self::types::{JsAmount, JsBolt11Invoice, JsProofsStatus, JsSecret};

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

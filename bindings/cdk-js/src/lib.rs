use wasm_bindgen::prelude::*;

pub mod error;
pub mod nuts;
pub mod types;
pub mod wallet;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

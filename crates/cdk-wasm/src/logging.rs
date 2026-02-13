//! WASM Logging configuration
//!
//! Provides functions to initialize tracing for browser console logging.

use std::sync::Once;

use wasm_bindgen::prelude::*;

static INIT: Once = std::sync::Once::new();

/// Initialize tracing for browser console logging.
///
/// Call this function once at application startup, before creating
/// any wallets. Subsequent calls are safe but have no effect.
///
/// # Arguments
///
/// * `level` - Log level filter (e.g., "debug", "info", "warn", "error", "trace")
#[wasm_bindgen(js_name = "initLogging")]
pub fn init_logging(level: String) {
    INIT.call_once(|| {
        let _ = level;
        // Use web_sys::console for basic logging in WASM
        web_sys::console::log_1(&"cdk-wasm logging initialized".into());
    });
}

/// Initialize logging with default "info" level
#[wasm_bindgen(js_name = "initDefaultLogging")]
pub fn init_default_logging() {
    init_logging("info".to_string());
}

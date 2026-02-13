//! CDK WASM bindings
//!
//! Web Assembly bindings for the Cashu Development Kit wallet.
//! Provides JavaScript/TypeScript access to the CDK wallet functionality.

#![allow(missing_docs)]

#[cfg(not(target_arch = "wasm32"))]
compile_error!("cdk-wasm only supports wasm32 targets");

#[cfg(target_arch = "wasm32")]
#[path = ""]
mod wasm_modules {
    pub mod database;
    pub mod error;
    pub mod local_storage;
    pub mod logging;
    pub mod token;
    pub mod types;
    pub mod wallet;
    pub mod wallet_repository;
}

#[cfg(target_arch = "wasm32")]
pub use wasm_modules::*;

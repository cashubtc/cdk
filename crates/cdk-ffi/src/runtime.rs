//! Runtime management for FFI bindings
//!
//! This module provides a global Tokio runtime for handling async operations
//! in the FFI bindings. The runtime is lazily initialized and shared across
//! all FFI function calls.
//!
//! # Example Usage (from client code)
//!
//! ```python
//! import cdk_ffi
//!
//! # Initialize the runtime once at application startup
//! cdk_ffi.init_runtime()
//!
//! # Now you can use async FFI functions
//! wallet = await cdk_ffi.Wallet.new(...)
//! ```

use std::sync::Arc;

use once_cell::sync::Lazy;
use tokio::runtime::Runtime;

use crate::error::FfiError;

/// Global Tokio runtime instance
static RUNTIME: Lazy<Arc<Runtime>> =
    Lazy::new(|| Arc::new(Runtime::new().expect("Failed to create Tokio runtime")));

/// Initialize the Tokio runtime for FFI usage
///
/// This function initializes a global Tokio runtime that will be used
/// for all async operations in the FFI bindings. It ensures that only
/// one runtime instance is created and reused across all calls.
///
/// This should be called once at application startup before any other
/// FFI functions are used.
#[uniffi::export]
pub fn init_runtime() -> Result<(), FfiError> {
    // Force lazy initialization of the runtime
    let _ = &*RUNTIME;
    Ok(())
}

/// Get the global runtime instance (for internal use)
///
/// This function is provided for future use cases where we might need
/// to access the runtime directly from within the FFI code.
#[allow(dead_code)]
pub(crate) fn get_runtime() -> Arc<Runtime> {
    RUNTIME.clone()
}

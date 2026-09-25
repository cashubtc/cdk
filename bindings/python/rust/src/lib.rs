//! Re-export everything from cdk-ffi.
//!
//! uniffi names the library it loads after the cdk-ffi namespace rather than
//! this crate, so the built `libcdk_ffi_python.*` is renamed to `libcdk_ffi.*`
//! on the way into the Python package.

/// Re-export cdk_ffi
pub use cdk_ffi::*;

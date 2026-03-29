//! Re-export everything from cdk-ffi
//! Note: The actual library with UniFFI metadata used for binding generation
//! is cdk-ffi itself (from target/release/deps), not this wrapper.
//! This wrapper is only used at runtime by Dart's native assets system.

/// Re-export cdk_ffi
pub use cdk_ffi::*;

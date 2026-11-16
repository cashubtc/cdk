//! Re-export everything from cdk-ffi.
//! The resulting cdk-ffi-dart library contains the UniFFI metadata used for
//! binding generation and is loaded at runtime by Dart's native assets system.

/// Re-export cdk_ffi
pub use cdk_ffi::*;

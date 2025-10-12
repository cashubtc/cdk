//! FFI-compatible types
//!
//! This module contains all the FFI types used by the UniFFI bindings.
//! Types are organized into logical submodules for better maintainability.

// Module declarations
pub mod amount;
pub mod keys;
pub mod mint;
pub mod proof;
pub mod quote;
pub mod subscription;
pub mod transaction;
pub mod wallet;

// Re-export all types for convenient access
pub use amount::*;
pub use keys::*;
pub use mint::*;
pub use proof::*;
pub use quote::*;
pub use subscription::*;
pub use transaction::*;
pub use wallet::*;

//! FFI-compatible types
//!
//! This module contains all the FFI types used by the UniFFI bindings.
//! Types are organized into logical submodules for better maintainability.

// Module declarations
pub mod amount;
pub mod invoice;
pub mod keys;
pub mod mint;
pub mod payment_request;
pub mod proof;
pub mod quote;
pub mod subscription;
pub mod transaction;
pub mod wallet;

// Re-export all types for convenient access
pub use amount::*;
pub use invoice::*;
pub use keys::*;
pub use mint::*;
pub use payment_request::*;
pub use proof::*;
pub use quote::*;
pub use subscription::*;
pub use transaction::*;
pub use wallet::*;

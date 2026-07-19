//! Federated Bark BOLT11 backend.
//!
//! The Bark dependency and every runtime type in this crate are inert unless
//! the explicit `federation-bark` feature is enabled.

#![cfg_attr(not(feature = "federation-bark"), allow(missing_docs))]

#[cfg(feature = "federation-bark")]
mod adapter;
#[cfg(feature = "federation-bark")]
mod config;
#[cfg(feature = "federation-bark")]
mod custody;
#[cfg(feature = "federation-bark")]
mod error;
#[cfg(feature = "federation-bark")]
mod runtime;

#[cfg(all(test, feature = "federation-bark"))]
mod tests;

#[cfg(feature = "federation-bark")]
pub use self::adapter::FederatedBarkPaymentAdapter;
#[cfg(feature = "federation-bark")]
pub use self::config::{
    FederatedBarkLimits, FederatedBarkLocalBindings, FederatedBarkLocalConfig, FederatedBarkScope,
    FederatedBarkStoreAvailability, ValidatedFederatedBarkConfig,
};
#[cfg(feature = "federation-bark")]
pub use self::custody::CdkFrostVtxoCustody;
#[cfg(feature = "federation-bark")]
pub use self::error::Error;
#[cfg(feature = "federation-bark")]
pub use self::runtime::{
    FederatedBarkDegradedReason, FederatedBarkReadiness, FederatedBarkRuntime,
    FederatedBarkRuntimeStatus,
};

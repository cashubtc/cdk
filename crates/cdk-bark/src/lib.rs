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
#[cfg(feature = "federation-bark")]
mod secret_store;

#[cfg(all(test, feature = "federation-bark"))]
mod tests;

#[cfg(feature = "federation-bark")]
pub use self::adapter::FederatedBarkPaymentAdapter;
#[cfg(feature = "federation-bark")]
pub use self::config::{
    FederatedBarkConnectionConfig, FederatedBarkLimits, FederatedBarkLocalBindings,
    FederatedBarkLocalConfig, FederatedBarkMaintenanceControls,
    FederatedBarkReceiveSecretStoreConfig, FederatedBarkScope, FederatedBarkStoreAvailability,
    ValidatedFederatedBarkConfig,
};
#[cfg(feature = "federation-bark")]
pub use self::custody::{CdkFrostVtxoCustody, FederatedBarkCustodyClient};
#[cfg(feature = "federation-bark")]
pub use self::error::Error;
#[cfg(feature = "federation-bark")]
pub use self::runtime::{
    FederatedBarkDegradedReason, FederatedBarkExitOutcome, FederatedBarkReadiness,
    FederatedBarkReceiveClaimOutcome, FederatedBarkReceiveClaimProposal,
    FederatedBarkReceiveClaimResult, FederatedBarkRefreshOutcome, FederatedBarkRuntime,
    FederatedBarkRuntimeStatus, FederatedBarkSendProposal, FederatedBarkSendTerminalStatus,
};
#[cfg(feature = "federation-bark")]
pub use self::secret_store::{
    FederatedBarkReceiveSecretAcceptedRef, FederatedBarkReceiveSecretEncryptionKey,
    FederatedBarkReceiveSecretStatus, FederatedBarkReceiveSecretStore,
    FederatedBarkReceiveSecretTerminalRef, FederatedBarkResumableReceiveSecret,
    FEDERATED_BARK_RECEIVE_SECRET_MINIMUM_RETENTION_DAYS,
    FEDERATED_BARK_RECEIVE_SECRET_ORPHAN_RETENTION_SECONDS,
};

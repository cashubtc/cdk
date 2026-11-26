//! The multi-operator setup ceremony as library code.
//!
//! - [`FederationSetupCoordinatorService`]: the creator's coordinator, with
//!   bounded admission and the durable effect of each request. `cdk-axum`
//!   serves it over HTTP.
//! - [`FederationSetupHttpClient`]: the typed operator client for that API.
//! - [`ceremony`]: each operator's local state machine, from `create` and
//!   `join` through proposal review, approval, the certified DKG handoff and
//!   activation.
//! - [`finalize`]: deriving the final member config from durable DKG records.
//!
//! Callers own the database, the listeners and all printing.

pub mod ceremony;
mod client;
mod coordinator;
mod error;
pub mod files;
pub mod finalize;

pub use self::client::{FederationSetupCoordinator, FederationSetupHttpClient};
#[doc(hidden)]
pub use self::coordinator::FederationSetupOperationFaults;
pub use self::coordinator::{
    FederationSetupCoordinatorService, FederationSetupCoordinatorServiceConfig,
    FederationSetupRequestAuthorization, FederationSetupRequestSource,
};
pub use self::error::{
    FederationSetupHttpError, FederationSetupHttpErrorBody, FederationSetupHttpErrorCode,
};

/// Route of the enrollment request.
pub const FEDERATION_SETUP_ENROLLMENTS_PATH: &str = "/federation/setup/v1/enrollments";
/// Route of the proposal request.
pub const FEDERATION_SETUP_PROPOSAL_PATH: &str = "/federation/setup/v1/proposal";
/// Route of the approval request.
pub const FEDERATION_SETUP_APPROVALS_PATH: &str = "/federation/setup/v1/approvals";
/// Route of the certified manifest request.
pub const FEDERATION_SETUP_MANIFEST_PATH: &str = "/federation/setup/v1/manifest";
/// Route of the status request.
pub const FEDERATION_SETUP_STATUS_PATH: &str = "/federation/setup/v1/status";
/// Largest approval request and acknowledgement.
pub const FEDERATION_SETUP_APPROVAL_REQUEST_BYTES: usize = 4 * 1024;
/// Largest error response body.
pub const FEDERATION_SETUP_ERROR_RESPONSE_BYTES: usize = 4 * 1024;

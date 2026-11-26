//! Redacted errors of the setup coordinator protocol.

use cdk_common::federation::FederationError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable structured error codes returned by the setup-only API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationSetupHttpErrorCode {
    /// Invitation is absent or malformed.
    InvalidInvitation,
    /// Invitation has expired.
    Expired,
    /// One-use authorization was replayed with different content.
    Replay,
    /// Immutable or compare-and-set state conflicts.
    Conflict,
    /// Requested setup artifact is not yet available.
    NotReady,
    /// A member or coordinator signature is invalid.
    InvalidSignature,
    /// Request or artifact uses an unsupported protocol version.
    UnsupportedVersion,
    /// Request is structurally invalid.
    InvalidRequest,
    /// Durable state could not be read or written.
    StorageFailure,
    /// Bounded admission was saturated.
    RateLimited,
    /// Request deadline elapsed while owned work continued safely.
    Timeout,
    /// Coordinator is shutting down.
    Unavailable,
}

impl FederationSetupHttpErrorCode {
    /// HTTP status code a setup endpoint answers this error with.
    pub const fn http_status(self) -> u16 {
        match self {
            Self::InvalidInvitation | Self::InvalidSignature => 401,
            Self::Expired => 410,
            Self::Replay | Self::Conflict => 409,
            Self::NotReady => 425,
            Self::UnsupportedVersion => 426,
            Self::InvalidRequest => 400,
            Self::StorageFailure => 500,
            Self::RateLimited => 429,
            Self::Timeout => 504,
            Self::Unavailable => 503,
        }
    }

    /// The fixed message a client reports for an error with this code.
    pub(crate) const fn safe_message(self) -> &'static str {
        match self {
            Self::InvalidInvitation => "setup invitation authentication failed",
            Self::Expired => "setup invitation is expired",
            Self::Replay => "setup request replay conflicts with durable state",
            Self::Conflict => "setup artifact conflicts with durable state",
            Self::NotReady => "setup artifact is not ready",
            Self::InvalidSignature => "setup signature validation failed",
            Self::UnsupportedVersion => "unsupported setup protocol version",
            Self::InvalidRequest => "setup request validation failed",
            Self::StorageFailure => "setup durable state is unavailable",
            Self::RateLimited => "setup request admission is saturated",
            Self::Timeout => "setup request deadline elapsed",
            Self::Unavailable => "setup coordinator is unavailable",
        }
    }
}

/// Redacted setup HTTP failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("federation setup HTTP error: {code:?}")]
pub struct FederationSetupHttpError {
    /// Stable machine-readable code.
    pub code: FederationSetupHttpErrorCode,
    message: &'static str,
}

/// Body of a setup endpoint's error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupHttpErrorBody {
    /// Stable machine-readable code.
    pub code: FederationSetupHttpErrorCode,
    /// Fixed, secret-free description.
    pub message: String,
}

impl FederationSetupHttpError {
    /// An error with a fixed, secret-free message.
    pub const fn new(code: FederationSetupHttpErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }

    /// The fixed, secret-free message.
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// HTTP status code a setup endpoint answers this error with.
    pub const fn http_status(&self) -> u16 {
        self.code.http_status()
    }

    /// The error response body.
    pub fn body(&self) -> FederationSetupHttpErrorBody {
        FederationSetupHttpErrorBody {
            code: self.code,
            message: self.message.to_owned(),
        }
    }

    /// The invitation did not authenticate.
    pub const fn invalid_invitation() -> Self {
        Self::new(
            FederationSetupHttpErrorCode::InvalidInvitation,
            "setup invitation authentication failed",
        )
    }

    /// Durable state could not be read or written.
    pub const fn storage() -> Self {
        Self::new(
            FederationSetupHttpErrorCode::StorageFailure,
            "setup durable state is unavailable",
        )
    }

    /// The request deadline elapsed.
    pub const fn timeout() -> Self {
        Self::new(
            FederationSetupHttpErrorCode::Timeout,
            "setup request deadline elapsed",
        )
    }

    pub(crate) const fn rate_limited() -> Self {
        Self::new(
            FederationSetupHttpErrorCode::RateLimited,
            "setup request admission is saturated",
        )
    }

    pub(crate) const fn not_ready() -> Self {
        Self::new(
            FederationSetupHttpErrorCode::NotReady,
            "setup artifact is not ready",
        )
    }

    pub(crate) fn from_federation(error: FederationError) -> Self {
        match error {
            FederationError::SetupVersionMismatch { .. } => Self::new(
                FederationSetupHttpErrorCode::UnsupportedVersion,
                "unsupported setup protocol version",
            ),
            FederationError::InvalidSetupSignature { .. }
            | FederationError::SetupSignatureVerificationFailed { .. } => Self::new(
                FederationSetupHttpErrorCode::InvalidSignature,
                "setup signature validation failed",
            ),
            FederationError::SetupPersistenceConflict { .. }
            | FederationError::SetupProposalHashMismatch { .. }
            | FederationError::SetupManifestHashMismatch { .. } => Self::new(
                FederationSetupHttpErrorCode::Conflict,
                "setup artifact conflicts with durable state",
            ),
            _ => Self::new(
                FederationSetupHttpErrorCode::InvalidRequest,
                "setup request validation failed",
            ),
        }
    }
}

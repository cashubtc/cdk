//! Shared authenticated private federation request admission.
//!
//! This module owns only the common, side-effect-free transport boundary:
//! bounded global admission plus signed-envelope verification. Route handlers
//! retain their message-class-specific validation and sequence-consumption
//! point, especially for DKG's durable atomic replay protection.

use axum::response::Response;
use cdk::federation::{FederationRestMessageKind, FederationSignedRequest};
use serde::Serialize;

use crate::MintState;

use super::{acquire_private_rest_admission, verify_federation_request, VerifiedFederationRequest};

/// Verified private request that retains its global admission permit.
#[derive(Debug)]
pub(crate) struct AdmittedFederationRequest {
    /// Signed sender and sequence, verified without consuming the sequence.
    pub(crate) verified: VerifiedFederationRequest,
    _admission: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl AdmittedFederationRequest {
    /// Verified sender identity.
    pub(crate) fn sender(&self) -> cdk::federation::MemberId {
        self.verified.sender
    }

    /// Verified request sequence.
    pub(crate) fn sequence(&self) -> u64 {
        self.verified.sequence
    }
}

/// Acquire global private-request admission and verify the signed envelope.
///
/// This deliberately does not validate the route payload or consume a
/// sequence. Those operations remain at the owning route or persistence
/// boundary, where their message-class-specific acceptance semantics are
/// visible.
pub(crate) async fn admit_private_request<T>(
    state: &MintState,
    request: &FederationSignedRequest<T>,
    kind: FederationRestMessageKind,
) -> Result<AdmittedFederationRequest, Response>
where
    T: Serialize,
{
    let admission = acquire_private_rest_admission(state, kind)?;
    let verified = verify_federation_request(state, request, kind).await?;
    Ok(AdmittedFederationRequest {
        verified,
        _admission: admission,
    })
}

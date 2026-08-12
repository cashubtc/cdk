//! Bark application protocol for CDK federations.
//!
//! Generic federation consensus carries a bounded application envelope. This
//! crate owns Bark's typed operations, deterministic projection records,
//! evidence certificates, protected receive-secret messages, and operator
//! status. Builds that omit `federation-bark` do not compile these types.

mod certificate;
mod config;
mod operation;
mod operator_status;
mod projection;
mod secret;
#[cfg(feature = "test-util")]
#[doc(hidden)]
pub mod test_support;

pub use self::certificate::{
    FederationBarkEvidenceAttestation, FederationBarkEvidenceAttestationRequest,
    FederationBarkEvidenceAttestationResponse, FederationBarkEvidenceBinding,
    FederationBarkEvidenceCertificate, FederationBarkEvidencePhase,
    FEDERATION_BARK_EVIDENCE_CERTIFICATE_VERSION,
    FEDERATION_MAX_BARK_EVIDENCE_ATTESTATION_REQUEST_BYTES,
};
pub use self::config::{
    federation_bark_application, federation_bark_setup_application, federation_bark_setup_config,
    federation_frost_wallet_with_bark, FederationBarkFrostApplicationConfig,
    FederationBarkFrostRegistrationAdapter, FederationBarkPolicyConfig,
    FederationBarkReceiveAntiDosPolicy, FederationBarkSetupConfig,
};
pub use self::operation::{
    federation_bark_application_id, federation_bark_application_operation,
    federation_bark_custody_transcript_hash, federation_bark_description_hash,
    federation_bark_exit_allocation_quote, federation_bark_mailbox_scope,
    federation_bark_receive_htlc_package_hash, federation_bark_receive_request_hash,
    federation_bark_refresh_requires_recovery, federation_bark_signer_set_for_attempt,
    register_federation_bark_wire_codec, validate_federation_bark_application_operation,
    validate_federation_bark_lightning_send_expiry, validate_federation_bark_operation,
    FederationBarkActionId, FederationBarkActionKind, FederationBarkActionPlan,
    FederationBarkActionTransition, FederationBarkArtifact, FederationBarkArtifactKind,
    FederationBarkAuthorizationReference, FederationBarkCertifiedReceiveHtlcs,
    FederationBarkConflictKey, FederationBarkCustodyProtocol, FederationBarkDigest,
    FederationBarkExitRequest, FederationBarkExternalId, FederationBarkFrostAuthorization,
    FederationBarkMemberObservation, FederationBarkObservationStatus, FederationBarkOperation,
    FederationBarkOperationKind, FederationBarkOperationRecord, FederationBarkReceiveIntent,
    FederationBarkReceivePrepared, FederationBarkReceiveProof,
    FederationBarkReceiveSecretAcknowledgement, FederationBarkRefreshRequest,
    FederationBarkSemanticPhase, FederationBarkUnit, FederationBarkVtxoId, FederationBarkVtxoInput,
    FederationBarkVtxoLocator, FederationBarkVtxoOutput, FederationBarkVtxoState,
    FederationBarkVtxoTransition, FEDERATION_BARK_AUTHORIZATION_VERSION,
    FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU, FEDERATION_BARK_OPERATION_VERSION,
    FEDERATION_MAX_BARK_ARTIFACT_AGGREGATE_BYTES, FEDERATION_MAX_BARK_ARTIFACT_BYTES,
    FEDERATION_MAX_BARK_AUTHORIZATION_BYTES, FEDERATION_MAX_BARK_COLLECTION_ENTRIES,
    FEDERATION_MAX_BARK_DESCRIPTION_BYTES, FEDERATION_MAX_BARK_INVOICE_BYTES,
    FEDERATION_MAX_BARK_LIGHTNING_SEND_EXPIRY_EXTENSION_BLOCKS,
    FEDERATION_MAX_BARK_OPERATION_BYTES, FEDERATION_MAX_BARK_REFRESH_INPUTS,
    FEDERATION_MAX_BARK_REFRESH_INPUT_ARTIFACT_BYTES,
};
pub use self::operator_status::{
    FederationBarkActionStatus, FederationBarkOperatorStatus, FederationBarkProtocolStatus,
    FederationBarkReceiveSecretCoverage, FederationBarkReconcileRequest,
    FederationBarkReconcileResponse, FederationBarkRetryRequest, FederationBarkRetryResponse,
    FederationBarkVtxoStatusCount, FEDERATION_BARK_OPERATOR_STATUS_VERSION,
};
pub use self::projection::{
    federation_bark_refresh_candidates, FederationBarkActionRecord,
    FederationBarkDerivationAllocationRecord, FederationBarkObservationRecord,
    FederationBarkProjectionSnapshot, FederationBarkReservationRecord,
    FederationBarkTransitionRecord, FederationBarkVtxoRecord,
};
pub use self::secret::{
    ark_server_commitment, payment_hash_for_preimage, preimage_commitment,
    FederationBarkPrepareReceiveSecretRequest, FederationBarkReceiveSecret,
    FederationBarkReceiveSecretBinding, FEDERATION_BARK_RECEIVE_SECRET_VERSION,
    FEDERATION_MAX_BARK_RECEIVE_SECRET_REQUEST_BYTES,
};
pub use cdk_common::federation::{
    FederationApplicationCustodyAbortRequest as FederationBarkCustodyAbortRequest,
    FederationApplicationThresholdDhRequest as FederationBarkThresholdDhRequest,
    FederationApplicationThresholdDhResponse as FederationBarkThresholdDhResponse,
    FederationApplicationVirtualMusigCommitmentRequest as FederationBarkVirtualMusigCommitmentRequest,
    FederationApplicationVirtualMusigCommitmentResponse as FederationBarkVirtualMusigCommitmentResponse,
    FederationApplicationVirtualMusigPartialRequest as FederationBarkVirtualMusigPartialRequest,
    FederationApplicationVirtualMusigPartialResponse as FederationBarkVirtualMusigPartialResponse,
};

use cdk_common::federation::{FederationError, Result};

fn invalid_bark(reason: impl Into<String>) -> FederationError {
    FederationError::ApplicationOperationInvalid {
        reason: format!("Bark: {}", reason.into()),
    }
}

fn canonical_json_bytes<T>(value: &T) -> Result<Vec<u8>>
where
    T: serde::Serialize + ?Sized,
{
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(serde_json::to_vec(&value)?)
}

//! Byte encodings of Lightning-send result and request artifacts.
//!
//! The executing member builds these artifacts and every member's projection
//! revalidates them, so both sides call the functions here instead of keeping
//! their own copy of each byte layout.

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;

use cdk_common::federation::{FederationError, FederationOperationId, Result};

use crate::{
    FederationBarkActionId, FederationBarkActionPlan, FederationBarkActionTransition,
    FederationBarkArtifact, FederationBarkArtifactKind, FederationBarkDigest,
    FederationBarkVtxoInput, FederationBarkVtxoLocator,
};

const SEND_PAYMENT_SUCCEEDED_DOMAIN: &[u8] = b"cdk/bark/send-payment-succeeded/v1";
const SEND_PAYMENT_FAILED_DOMAIN: &[u8] = b"cdk/bark/send-payment-failed/v1";
const SEND_REVOCATION_REQUEST_DOMAIN: &[u8] = b"cdk/bark/send-revocation-request/v1";

fn invalid_result(reason: &'static str) -> FederationError {
    FederationError::ApplicationOperationInvalid { reason }
}

/// SHA-256 digest of Bark artifact or result bytes.
pub fn federation_bark_digest(bytes: &[u8]) -> Result<FederationBarkDigest> {
    FederationBarkDigest::from_bytes(Sha256Hash::hash(bytes).to_byte_array())
}

/// Result hash committed by a successful Lightning send.
pub fn federation_bark_send_succeeded_result_hash(
    action_id: FederationBarkActionId,
    htlc_result_hash: FederationBarkDigest,
    preimage: &[u8; 32],
) -> Result<FederationBarkDigest> {
    let mut bytes = SEND_PAYMENT_SUCCEEDED_DOMAIN.to_vec();
    bytes.extend_from_slice(&action_id.to_bytes());
    bytes.extend_from_slice(&htlc_result_hash.to_bytes());
    bytes.extend_from_slice(preimage);
    federation_bark_digest(&bytes)
}

/// Check a payment-success transition against its plan and return the preimage.
///
/// The transition must carry exactly one 32-byte proof whose hash is the
/// plan's payment hash, and its result hash must commit to that preimage and
/// the accepted HTLC result (`transition.request_hash`).
pub fn validate_federation_bark_send_succeeded_result(
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<[u8; 32]> {
    let mut proofs = transition
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Proof);
    let proof = proofs
        .next()
        .ok_or_else(|| invalid_result("payment success proof is missing"))?;
    let preimage: [u8; 32] = proof
        .bytes
        .as_slice()
        .try_into()
        .map_err(|_| invalid_result("payment success proof is not a 32-byte preimage"))?;
    let payment_hash = plan
        .payment_hash
        .ok_or_else(|| invalid_result("send plan payment hash is missing"))?;
    if proofs.next().is_some()
        || proof.hash != federation_bark_digest(&proof.bytes)?
        || federation_bark_digest(&preimage)? != payment_hash
        || transition.result_hash
            != federation_bark_send_succeeded_result_hash(
                plan.action_id,
                transition.request_hash,
                &preimage,
            )?
    {
        return Err(invalid_result(
            "payment success proof differs from the accepted invoice or result",
        ));
    }
    Ok(preimage)
}

/// Result artifact recorded when a Lightning send fails after its HTLCs were cosigned.
pub fn federation_bark_send_failed_result(
    plan: &FederationBarkActionPlan,
    htlc_result_hash: FederationBarkDigest,
) -> Result<FederationBarkArtifact> {
    let payment_hash = plan
        .payment_hash
        .ok_or_else(|| invalid_result("send plan payment hash is missing"))?;
    let mut bytes = SEND_PAYMENT_FAILED_DOMAIN.to_vec();
    bytes.extend_from_slice(&plan.action_id.to_bytes());
    bytes.extend_from_slice(&htlc_result_hash.to_bytes());
    bytes.extend_from_slice(&payment_hash.to_bytes());
    Ok(FederationBarkArtifact {
        kind: FederationBarkArtifactKind::ArkResult,
        hash: federation_bark_digest(&bytes)?,
        bytes,
    })
}

/// Check that a payment-failure transition carries exactly the expected result.
pub fn validate_federation_bark_send_failed_result(
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<()> {
    let expected = federation_bark_send_failed_result(plan, transition.request_hash)?;
    let mut results = transition
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult);
    if results.next() != Some(&expected)
        || results.next().is_some()
        || transition.result_hash != expected.hash
    {
        return Err(invalid_result(
            "payment failure result differs from its accepted HTLC result",
        ));
    }
    Ok(())
}

/// Request bytes of the revocation that returns a failed send's HTLC inputs.
pub fn federation_bark_send_revocation_request_bytes(
    action_id: FederationBarkActionId,
    failed_operation_id: FederationOperationId,
    payment_hash: FederationBarkDigest,
    output_locator: FederationBarkVtxoLocator,
    inputs: &[FederationBarkVtxoInput],
) -> Result<Vec<u8>> {
    let mut bytes = SEND_REVOCATION_REQUEST_DOMAIN.to_vec();
    bytes.extend_from_slice(&action_id.to_bytes());
    bytes.extend_from_slice(&failed_operation_id.to_bytes());
    bytes.extend_from_slice(&payment_hash.to_bytes());
    append_vtxo_locator(&mut bytes, output_locator);
    bytes.extend_from_slice(
        &u32::try_from(inputs.len())
            .map_err(|_| invalid_result("too many revocation inputs"))?
            .to_be_bytes(),
    );
    for input in inputs {
        bytes.extend_from_slice(&input.vtxo_id.to_bytes());
        append_vtxo_locator(&mut bytes, input.locator);
        bytes.extend_from_slice(&u64::from(input.amount).to_be_bytes());
    }
    Ok(bytes)
}

fn append_vtxo_locator(bytes: &mut Vec<u8>, locator: FederationBarkVtxoLocator) {
    bytes.push(locator.role);
    bytes.extend_from_slice(&locator.index.to_be_bytes());
    bytes.extend_from_slice(&locator.version.to_be_bytes());
}

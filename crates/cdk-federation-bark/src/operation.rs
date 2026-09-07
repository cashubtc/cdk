//! Stable federation-domain operations for Bark custody and Lightning actions.
//!
//! These types deliberately contain no Bark wallet checkpoints or member-local
//! database state. They are the bounded, canonical facts ordered by federation
//! consensus before a Bark or Ark side effect may occur.

use core::fmt;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::PublicKey;
use bitcoin::Network;
use lightning_invoice::{Bolt11Invoice, Bolt11InvoiceDescriptionRef};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use cdk_common::federation::{
    register_external_application_codec, FederationAllocatedAddressRef,
    FederationApplicationConflictKey, FederationApplicationOperation,
    FederationApplicationStateKey, FederationConfig, FederationError,
    FederationExternalApplicationCodec, FederationExternalApplicationDecoded,
    FederationExternalApplicationMaterializedStateKey, FederationExternalApplicationOperation,
    FederationFrostApplicationId, FederationId, FederationJournalEntry, FederationOperation,
    FederationOperationId, FederationOperationSignature, FederationOperationStatus, MemberId,
    Result,
};
use cdk_common::Amount;

use crate::{canonical_json_bytes, FederationBarkEvidenceCertificate, FederationBarkEvidencePhase};

/// Current nested Bark operation format.
///
/// Same-action recovery plans reuse reservations. Older formats are rejected.
pub const FEDERATION_BARK_OPERATION_VERSION: u16 = 2;
/// Current typed Bark participant-authorization format.
pub const FEDERATION_BARK_AUTHORIZATION_VERSION: u16 = 1;
/// Maximum canonical size of one nested Bark operation.
pub const FEDERATION_MAX_BARK_OPERATION_BYTES: usize = 512 * 1024;
/// Maximum raw bytes in one typed Bark artifact.
pub const FEDERATION_MAX_BARK_ARTIFACT_BYTES: usize = 64 * 1024;
/// Maximum aggregate raw artifact bytes in one operation.
pub const FEDERATION_MAX_BARK_ARTIFACT_AGGREGATE_BYTES: usize = 512 * 1024;
/// Maximum entries in each bounded Bark collection.
pub const FEDERATION_MAX_BARK_COLLECTION_ENTRIES: usize = 64;
/// Maximum canonical size of one participant authorization reference.
pub const FEDERATION_MAX_BARK_AUTHORIZATION_BYTES: usize = 16 * 1024;
/// Maximum VTXOs in one refresh round.
///
/// Bark's round-attempt authorization repeats every serialized input VTXO.
/// Keeping this below the general collection bound ensures one accepted plan
/// cannot create an authorization that exceeds the fixed 16 KiB participant
/// boundary.
pub const FEDERATION_MAX_BARK_REFRESH_INPUTS: usize = 8;
/// Aggregate serialized input-VTXO budget for one refresh round.
///
/// The remaining authorization space is reserved for the round challenge,
/// output request, signer context, and canonical FROST authorization fields.
pub const FEDERATION_MAX_BARK_REFRESH_INPUT_ARTIFACT_BYTES: usize = 12 * 1024;
/// Maximum canonical BOLT11 invoice bytes retained in an operation.
pub const FEDERATION_MAX_BARK_INVOICE_BYTES: usize = 64 * 1024;
/// Maximum UTF-8 bytes in a direct federated Bark invoice description.
pub const FEDERATION_MAX_BARK_DESCRIPTION_BYTES: usize = 4 * 1024;
/// Frozen v1 emergency-exit drain fee rate (10 sat/vB).
pub const FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU: u64 = 2_500;
/// Largest accepted outgoing HTLC extension beyond the earliest input expiry.
///
/// The Ark server chooses the lower bound from its live tip and advertised
/// send delta. This two-week upper bound keeps an actor-proposed absolute
/// height bounded without consulting external state during deterministic
/// replay.
pub const FEDERATION_MAX_BARK_LIGHTNING_SEND_EXPIRY_EXTENSION_BLOCKS: u32 = 2_016;

const BARK_OPERATION_HASH_DOMAIN: &[u8] = b"cdk/federation/bark-operation/v1";
const BARK_AUTHORIZATION_HASH_DOMAIN: &[u8] = b"cdk/federation/bark-authorization/v1";
const BARK_SIGNING_SLOT_DOMAIN: &[u8] = b"cdk/federation/bark-signing-slot/v1";
const BARK_SESSION_ID_DOMAIN: &[u8] = b"cdk/federation/bark-session-id/v1";
const BARK_CUSTODY_TRANSCRIPT_DOMAIN: &[u8] = b"cdk/federation/bark-custody-transcript/v1";
const BARK_ACTION_ID_DOMAIN: &[u8] = b"cdk/federation/bark-action-id/v1";
const BARK_RECEIVE_ALLOCATION_DOMAIN: &[u8] = b"cdk/federation/bark-receive-allocation/v1";
const BARK_DESCRIPTION_DOMAIN: &[u8] = b"cdk/federation/bark-description/v1";
const BARK_MAILBOX_SCOPE_DOMAIN: &[u8] = b"cdk/federation/bark-mailbox-scope/v1";
const BARK_RECEIVE_REQUEST_DOMAIN: &[u8] = b"cdk/federation/bark-receive-request/v1";
const BARK_RECEIVE_RESULT_DOMAIN: &[u8] = b"cdk/federation/bark-receive-result/v1";
const BARK_RECEIVE_HTLC_PACKAGE_DOMAIN: &[u8] = b"cdk/federation/bark-receive-htlc-package/v1";
const BARK_RECEIVE_EXTERNAL_ID_DOMAIN: &[u8] = b"cdk/federation/bark-receive-external-id/v1";
const BARK_SEND_REQUEST_DOMAIN: &str = "cdk/federation/bark-send-request/v1";
const BARK_SEND_EXTERNAL_ID_DOMAIN: &[u8] = b"cdk/federation/bark-send-external-id/v1";
const BARK_REVOCATION_EXTERNAL_ID_DOMAIN: &[u8] = b"cdk/federation/bark-revocation-external-id/v1";
const BARK_REFRESH_EXTERNAL_ID_DOMAIN: &[u8] = b"cdk/federation/bark-refresh-external-id/v1";
const BARK_EXIT_EXTERNAL_ID_DOMAIN: &[u8] = b"cdk/federation/bark-exit-external-id/v1";
const BARK_SIGNING_ATTEMPT_RESULT_DOMAIN: &[u8] = b"cdk/federation/bark-signing-attempt-result/v1";
const BARK_REFRESH_REQUEST_DOMAIN: &str = "cdk/federation/bark-refresh-request/v1";
const BARK_EXIT_REQUEST_DOMAIN: &str = "cdk/federation/bark-exit-request/v1";
const BARK_EXIT_ALLOCATION_QUOTE_PREFIX: &str = "bark-exit-";
const BARK_CPFP_ALLOCATION_QUOTE_PREFIX: &str = "bark-cpfp-";

/// Commit to the exact bounded transcript presented to a Bark custody method.
pub fn federation_bark_custody_transcript_hash(bytes: &[u8]) -> Result<FederationBarkDigest> {
    if bytes.is_empty() || bytes.len() > FEDERATION_MAX_BARK_AUTHORIZATION_BYTES {
        return Err(invalid_bark(
            "Bark custody transcript is empty or oversized",
        ));
    }
    domain_hash(BARK_CUSTODY_TRANSCRIPT_DOMAIN, bytes)
}

/// Validate the bounded absolute expiry proposed from Bark's live server view.
pub fn validate_federation_bark_lightning_send_expiry(
    minimum_input_expiry: u32,
    proposed_expiry: u32,
) -> Result<()> {
    let maximum_expiry = minimum_input_expiry
        .checked_add(FEDERATION_MAX_BARK_LIGHTNING_SEND_EXPIRY_EXTENSION_BLOCKS)
        .ok_or_else(|| invalid_bark("Bark Lightning send expiry bound overflows"))?;
    if minimum_input_expiry == 0 || proposed_expiry == 0 || proposed_expiry > maximum_expiry {
        return Err(invalid_bark(
            "Bark Lightning send expiry is outside its accepted bound",
        ));
    }
    Ok(())
}

/// Deterministic address-allocation label for an emergency-exit destination.
#[must_use]
pub fn federation_bark_exit_allocation_quote(action_id: FederationBarkActionId) -> String {
    format!("{BARK_EXIT_ALLOCATION_QUOTE_PREFIX}{action_id}")
}

/// Deterministic internal-change allocation label for one exit CPFP parent.
#[must_use]
pub fn federation_bark_cpfp_allocation_quote(
    action_id: FederationBarkActionId,
    parent_txid: bitcoin::Txid,
) -> String {
    format!("{BARK_CPFP_ALLOCATION_QUOTE_PREFIX}{action_id}-{parent_txid}")
}

fn invalid_bark(reason: impl Into<String>) -> FederationError {
    FederationError::ApplicationOperationInvalid {
        reason: reason.into(),
    }
}

macro_rules! bark_hash_type {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 32]);

        impl $name {
            /// Construct a canonical non-zero identifier.
            pub fn from_bytes(bytes: [u8; 32]) -> Result<Self> {
                if bytes == [0; 32] {
                    return Err(invalid_bark(concat!(
                        stringify!($name),
                        " must be non-zero"
                    )));
                }
                Ok(Self(bytes))
            }

            /// Parse a lowercase or uppercase hexadecimal identifier.
            pub fn from_hex(value: &str) -> Result<Self> {
                let bytes = cdk_common::util::hex::decode(value)?;
                let bytes: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
                    invalid_bark(format!(
                        "{} must contain 32 bytes, found {}",
                        stringify!($name),
                        bytes.len()
                    ))
                })?;
                Self::from_bytes(bytes)
            }

            /// Return the canonical bytes.
            #[must_use]
            pub fn to_bytes(self) -> [u8; 32] {
                self.0
            }

            /// Return lowercase hexadecimal form.
            #[must_use]
            pub fn to_hex(self) -> String {
                cdk_common::util::hex::encode(self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.to_hex())
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_hex())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::from_hex(&value).map_err(serde::de::Error::custom)
            }
        }
    };
}

bark_hash_type!(
    FederationBarkActionId,
    "Stable identifier for one federated Bark economic action."
);
bark_hash_type!(
    FederationBarkDigest,
    "Domain-specific commitment to canonical Bark data."
);
/// Canonical 36-byte Bark virtual transaction output identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationBarkVtxoId([u8; 36]);

impl FederationBarkVtxoId {
    /// Construct an exact non-zero Bark VTXO identifier.
    pub fn from_bytes(bytes: [u8; 36]) -> Result<Self> {
        if bytes == [0; 36] {
            return Err(invalid_bark("FederationBarkVtxoId must be non-zero"));
        }
        Ok(Self(bytes))
    }

    /// Parse the canonical hexadecimal identifier.
    pub fn from_hex(value: &str) -> Result<Self> {
        let bytes = cdk_common::util::hex::decode(value)?;
        let bytes: [u8; 36] = bytes.try_into().map_err(|bytes: Vec<u8>| {
            invalid_bark(format!(
                "FederationBarkVtxoId must contain 36 bytes, found {}",
                bytes.len()
            ))
        })?;
        Self::from_bytes(bytes)
    }

    /// Return exact Bark VTXO identifier bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 36] {
        self.0
    }

    /// Return lowercase hexadecimal form.
    #[must_use]
    pub fn to_hex(self) -> String {
        cdk_common::util::hex::encode(self.0)
    }
}

impl fmt::Display for FederationBarkVtxoId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl Serialize for FederationBarkVtxoId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationBarkVtxoId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(serde::de::Error::custom)
    }
}
bark_hash_type!(
    FederationBarkExternalId,
    "Federation-internal identity for one accepted external side-effect request."
);

impl FederationBarkActionId {
    /// Derive the frozen v1 action ID for one object in an accepted operation.
    pub fn derive(
        federation_id: FederationId,
        accepted_operation_id: FederationOperationId,
        phase: FederationBarkSemanticPhase,
        object_ordinal: u32,
    ) -> Result<Self> {
        let mut transcript = Vec::new();
        transcript.extend_from_slice(BARK_ACTION_ID_DOMAIN);
        transcript.extend_from_slice(&federation_id.to_bytes());
        transcript.extend_from_slice(&accepted_operation_id.to_bytes());
        transcript.extend_from_slice(&phase.code().to_be_bytes());
        transcript.extend_from_slice(&object_ordinal.to_be_bytes());
        Self::from_bytes(Sha256Hash::hash(&transcript).to_byte_array())
    }

    /// Derive the initial receive action from its preallocated Cashu quote.
    ///
    /// Receive intent is the first accepted operation, so it has no prior
    /// journal operation to reference. The frozen action derivation is fed a
    /// domain-separated synthetic allocation reference bound to federation
    /// and quote, avoiding a circular operation-ID dependency.
    pub fn derive_receive_intent(federation_id: FederationId, quote: &str) -> Result<Self> {
        ensure_quote(quote)?;
        let mut allocation = Vec::new();
        append_hash_bytes(&mut allocation, BARK_RECEIVE_ALLOCATION_DOMAIN);
        append_hash_bytes(&mut allocation, &federation_id.to_bytes());
        append_hash_bytes(&mut allocation, quote.as_bytes());
        let allocation =
            FederationOperationId::from_bytes(Sha256Hash::hash(&allocation).to_byte_array());
        Self::derive(
            federation_id,
            allocation,
            FederationBarkSemanticPhase::ReceiveIntent,
            0,
        )
    }
}

impl FederationBarkExternalId {
    /// Derive the frozen execution identity for one outgoing Lightning action.
    pub fn derive_lightning_send(
        action_id: FederationBarkActionId,
        predecessor_operation_id: FederationOperationId,
        attempt: u32,
    ) -> Result<Self> {
        if attempt == 0 {
            return Err(invalid_bark("Bark send attempt must be non-zero"));
        }
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, BARK_SEND_EXTERNAL_ID_DOMAIN);
        append_hash_bytes(&mut transcript, &action_id.to_bytes());
        append_hash_bytes(&mut transcript, &predecessor_operation_id.to_bytes());
        append_hash_bytes(&mut transcript, &attempt.to_be_bytes());
        Self::from_bytes(Sha256Hash::hash(&transcript).to_byte_array())
    }

    /// Derive the frozen execution identity for one failed-send revocation.
    pub fn derive_failed_send_revocation(
        action_id: FederationBarkActionId,
        predecessor_operation_id: FederationOperationId,
        attempt: u32,
    ) -> Result<Self> {
        if attempt == 0 {
            return Err(invalid_bark("Bark revocation attempt must be non-zero"));
        }
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, BARK_REVOCATION_EXTERNAL_ID_DOMAIN);
        append_hash_bytes(&mut transcript, &action_id.to_bytes());
        append_hash_bytes(&mut transcript, &predecessor_operation_id.to_bytes());
        append_hash_bytes(&mut transcript, &attempt.to_be_bytes());
        Self::from_bytes(Sha256Hash::hash(&transcript).to_byte_array())
    }

    /// Derive the frozen execution identity for one refresh round.
    pub fn derive_refresh(
        action_id: FederationBarkActionId,
        predecessor_operation_id: FederationOperationId,
        attempt: u32,
    ) -> Result<Self> {
        if attempt == 0 {
            return Err(invalid_bark("Bark refresh attempt must be non-zero"));
        }
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, BARK_REFRESH_EXTERNAL_ID_DOMAIN);
        append_hash_bytes(&mut transcript, &action_id.to_bytes());
        append_hash_bytes(&mut transcript, &predecessor_operation_id.to_bytes());
        append_hash_bytes(&mut transcript, &attempt.to_be_bytes());
        Self::from_bytes(Sha256Hash::hash(&transcript).to_byte_array())
    }

    /// Derive the frozen execution identity for one emergency exit.
    pub fn derive_emergency_exit(
        action_id: FederationBarkActionId,
        predecessor_operation_id: FederationOperationId,
        attempt: u32,
    ) -> Result<Self> {
        if attempt == 0 {
            return Err(invalid_bark("Bark exit attempt must be non-zero"));
        }
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, BARK_EXIT_EXTERNAL_ID_DOMAIN);
        append_hash_bytes(&mut transcript, &action_id.to_bytes());
        append_hash_bytes(&mut transcript, &predecessor_operation_id.to_bytes());
        append_hash_bytes(&mut transcript, &attempt.to_be_bytes());
        Self::from_bytes(Sha256Hash::hash(&transcript).to_byte_array())
    }
}

/// Only currency unit admitted by the v1 Bark backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationBarkUnit {
    /// Bitcoin satoshis.
    Sat,
}

/// CDK-owned deterministic locator for one Bark custody public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkVtxoLocator {
    /// Frozen v1 custody role. Only `1` is currently valid.
    pub role: u8,
    /// Consensus-allocated locator index.
    pub index: u32,
    /// Locator derivation format. Only `1` is currently valid.
    pub version: u16,
}

impl FederationBarkVtxoLocator {
    /// Construct the current VTXO-custody locator.
    #[must_use]
    pub const fn vtxo_custody(index: u32) -> Self {
        Self {
            role: 1,
            index,
            version: 1,
        }
    }

    fn validate(self) -> Result<()> {
        if self.role != 1 || self.version != 1 {
            return Err(invalid_bark("unsupported Bark VTXO locator"));
        }
        Ok(())
    }
}

impl fmt::Display for FederationBarkVtxoLocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}:{}", self.role, self.index, self.version)
    }
}

/// Signed proof that one member durably stored an encrypted receive secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkReceiveSecretAcknowledgement {
    /// Member that persisted and acknowledged the protected record.
    pub member: MemberId,
    /// Commitment to the exact protected record and receive scope.
    pub record_hash: FederationBarkDigest,
    /// Member identity signature over the acknowledgement transcript.
    pub signature: FederationOperationSignature,
}

/// First-receive anti-DoS proof fixed by the accepted receive intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationBarkReceiveProof {
    /// The pinned Ark capability does not require a proof.
    NoProof,
}

/// Intent accepted before invoice creation or preimage disclosure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkReceiveIntent {
    /// Nested operation format.
    pub version: u16,
    /// Deterministic action lineage.
    pub action_id: FederationBarkActionId,
    /// Cashu mint quote reserved by this receive.
    pub quote: String,
    /// Member eligible to execute invoice preparation first.
    pub actor: MemberId,
    /// Cashu liability requested by the client.
    pub cashu_amount: Amount,
    /// Exact invoice amount accepted for creation.
    pub gross_amount: Amount,
    /// Exact Ark receive fee.
    pub receive_fee: Amount,
    /// Minimum federation-custodied value required after claim.
    pub minimum_net_amount: Amount,
    /// Explicit v1 currency unit.
    pub unit: FederationBarkUnit,
    /// Commitment to the bounded user description.
    pub description_hash: FederationBarkDigest,
    /// Exact bounded direct BOLT11 description required for actor failover.
    pub description: Option<String>,
    /// BOLT11 payment hash.
    pub payment_hash: FederationBarkDigest,
    /// Commitment to the protected preimage.
    pub preimage_commitment: FederationBarkDigest,
    /// Persist-before-acknowledgement proofs, sorted by member.
    pub secret_acknowledgements: Vec<FederationBarkReceiveSecretAcknowledgement>,
    /// Consensus-owned destination key locator.
    pub destination: FederationBarkVtxoLocator,
    /// Commitment to the member-independent mailbox scope.
    pub mailbox_scope: FederationBarkDigest,
    /// Exact notification-only mailbox identifier supplied to the Ark server.
    #[serde(with = "hex_array_33")]
    pub mailbox_id: [u8; 33],
    /// Commitment to the pinned Ark server identity and capability snapshot.
    pub ark_server: FederationBarkDigest,
    /// Immutable Bitcoin network.
    pub network: Network,
    /// Smallest final-CLTV delta allowed by the accepted policy.
    pub minimum_cltv_delta: u32,
    /// Accepted receive fee policy version.
    pub fee_policy_version: u16,
    /// Accepted anti-DoS policy version.
    pub anti_dos_policy_version: u16,
    /// Exact anti-DoS proof mode.
    pub receive_proof: FederationBarkReceiveProof,
    /// Consensus-visible invoice intent expiry.
    pub expires_at: u64,
}

/// Exact canonical BOLT11 invoice accepted after an intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkReceivePrepared {
    /// Nested operation format.
    pub version: u16,
    /// Receive action lineage.
    pub action_id: FederationBarkActionId,
    /// Cashu quote from the receive intent.
    pub quote: String,
    /// Member that obtained and validated the idempotent Ark result.
    pub actor: MemberId,
    /// Accepted receive-intent operation.
    pub intent_operation_id: FederationOperationId,
    /// Exact canonical BOLT11 invoice.
    pub invoice: String,
    /// Invoice payment hash.
    pub payment_hash: FederationBarkDigest,
    /// Exact invoice amount.
    pub gross_amount: Amount,
    /// Minimum net VTXO value proven by invoice preparation.
    pub expected_net_amount: Amount,
    /// Immutable Bitcoin network.
    pub network: Network,
    /// Exact invoice expiry encoded in the accepted result.
    pub expires_at: u64,
    /// Actual final-CLTV delta returned by the Ark server.
    pub final_cltv_delta: u32,
    /// Exact fee returned by the Ark server.
    pub receive_fee: Amount,
    /// Commitment to the exact outgoing Ark request.
    pub request_hash: FederationBarkDigest,
    /// Commitment to the canonical Ark result.
    pub result_hash: FederationBarkDigest,
    /// Stable external retry identifier.
    pub external_id: FederationBarkExternalId,
}

/// Exact input value reserved by one accepted Bark action plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkVtxoInput {
    /// Canonical VTXO identifier.
    pub vtxo_id: FederationBarkVtxoId,
    /// Exact consensus-owned custody locator.
    pub locator: FederationBarkVtxoLocator,
    /// Accepted value in satoshis.
    pub amount: Amount,
}

/// Exact output value and locator allocated by one Bark action plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkVtxoOutput {
    /// Consensus-owned destination locator.
    pub locator: FederationBarkVtxoLocator,
    /// Exact output value in satoshis.
    pub amount: Amount,
    /// Commitment to the canonical output policy or VTXO template.
    pub artifact_hash: FederationBarkDigest,
}

/// Consensus-authoritative lifecycle state of one Bark VTXO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationBarkVtxoState {
    /// The VTXO was observed but is not yet safe to spend.
    Observed,
    /// The VTXO is accepted as spendable federation collateral.
    Spendable,
    /// An accepted action plan exclusively reserved the VTXO.
    Reserved,
    /// The accepted plan may have spent the VTXO externally.
    PendingSpend,
    /// The VTXO was terminally spent.
    Spent,
    /// The VTXO is committed to an emergency exit.
    Exiting,
    /// The emergency exit is sufficiently confirmed.
    Exited,
}

impl FederationBarkVtxoState {
    /// Stable operator and metrics label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Spendable => "spendable",
            Self::Reserved => "reserved",
            Self::PendingSpend => "pending_spend",
            Self::Spent => "spent",
            Self::Exiting => "exiting",
            Self::Exited => "exited",
        }
    }
}

/// Exact VTXO state mutation proven by one accepted Bark transition.
///
/// The canonical VTXO bytes are carried once in the transition's artifact set
/// and selected by `artifact_hash`. This mapping lets the CDK projection remain
/// independent of Bark's internal serialization and database schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkVtxoTransition {
    /// Canonical VTXO identifier.
    pub vtxo_id: FederationBarkVtxoId,
    /// Consensus-owned custody locator.
    pub locator: FederationBarkVtxoLocator,
    /// Exact VTXO value in satoshis.
    pub amount: Amount,
    /// Absolute block height at which the VTXO expires.
    pub expiry_height: u32,
    /// Required prior state, or `None` when this transition creates the row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_state: Option<FederationBarkVtxoState>,
    /// New authoritative state.
    pub new_state: FederationBarkVtxoState,
    /// Hash of the canonical `Vtxo` artifact in this transition.
    pub artifact_hash: FederationBarkDigest,
}

/// Stable category of a bounded canonical Bark artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationBarkArtifactKind {
    /// Canonical VTXO bytes.
    Vtxo,
    /// Canonical Ark request bytes.
    ArkRequest,
    /// Canonical Ark result bytes.
    ArkResult,
    /// Canonical BOLT11 invoice bytes.
    Invoice,
    /// Canonical transaction bytes.
    Transaction,
    /// Canonical payment or chain proof bytes.
    Proof,
}

/// Bounded exact artifact needed to validate or rebuild Bark state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkArtifact {
    /// Artifact category.
    pub kind: FederationBarkArtifactKind,
    /// Canonical artifact commitment.
    pub hash: FederationBarkDigest,
    /// Exact canonical bytes, hex-encoded on the wire.
    #[serde(with = "hex_bytes")]
    pub bytes: Vec<u8>,
}

/// Certified incoming HTLC evidence carried by the claim authorization that
/// immediately consumes it.
///
/// Canonical VTXO bytes remain in the parent action plan's artifact set. This
/// nested record carries only the exact state transitions and their signed
/// evidence commitment, avoiding a second copy of the potentially large
/// artifacts while preserving the former readiness boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkCertifiedReceiveHtlcs {
    /// Nested evidence format.
    pub version: u16,
    /// Earliest HTLC policy expiry authenticated by Bark.
    pub minimum_expiry: u32,
    /// Commitment to the exact Bark hydration request.
    pub request_hash: FederationBarkDigest,
    /// Exact observed VTXO introductions, sorted by canonical identity.
    pub vtxo_transitions: Vec<FederationBarkVtxoTransition>,
    /// Independent-member certificate over the canonical HTLC evidence.
    pub certificate: FederationBarkEvidenceCertificate,
}

/// Commit to the complete backend-independent incoming HTLC package.
///
/// This digest is suitable for member attestations because every member can
/// recompute it from the canonical operation alone. It binds the hydration
/// request, every introduced VTXO transition, and the exact VTXO bytes without
/// requiring CDK's federation layer to decode Bark's private serialization.
pub fn federation_bark_receive_htlc_package_hash(
    minimum_expiry: u32,
    request_hash: FederationBarkDigest,
    transitions: &[FederationBarkVtxoTransition],
    artifacts: &[FederationBarkArtifact],
) -> Result<FederationBarkDigest> {
    #[derive(Serialize)]
    #[serde(deny_unknown_fields)]
    struct PackageCommitment<'a> {
        minimum_expiry: u32,
        request_hash: FederationBarkDigest,
        transitions: &'a [FederationBarkVtxoTransition],
        artifacts: Vec<&'a FederationBarkArtifact>,
    }

    if minimum_expiry == 0 {
        return Err(invalid_bark("receive HTLC package expiry is zero"));
    }
    ensure_count("receive HTLC transitions", transitions.len(), false)?;
    ensure_strictly_sorted("receive HTLC transitions", transitions)?;
    let vtxo_artifacts = artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
        .collect::<Vec<_>>();
    if vtxo_artifacts.len() != transitions.len()
        || transitions.iter().any(|transition| {
            transition.previous_state.is_some()
                || transition.new_state != FederationBarkVtxoState::Observed
                || transition.amount == Amount::ZERO
                || transition.expiry_height == 0
                || vtxo_artifacts
                    .iter()
                    .filter(|artifact| artifact.hash == transition.artifact_hash)
                    .count()
                    != 1
        })
        || vtxo_artifacts.iter().any(|artifact| {
            transitions
                .iter()
                .filter(|transition| transition.artifact_hash == artifact.hash)
                .count()
                != 1
        })
    {
        return Err(invalid_bark(
            "receive HTLC package does not map one transition to each VTXO artifact",
        ));
    }
    validate_artifacts(
        &vtxo_artifacts
            .iter()
            .map(|artifact| (*artifact).clone())
            .collect::<Vec<_>>(),
    )?;
    domain_hash(
        BARK_RECEIVE_HTLC_PACKAGE_DOMAIN,
        &canonical_json_bytes(&PackageCommitment {
            minimum_expiry,
            request_hash,
            transitions,
            artifacts: vtxo_artifacts,
        })?,
    )
}

/// Side-effecting Bark action fixed before execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationBarkActionKind {
    /// Pay one outgoing BOLT11 invoice.
    LightningSend,
    /// Claim incoming HTLC VTXOs with the protected preimage.
    IncomingClaim,
    /// Revoke failed outgoing HTLC VTXOs.
    FailedSendRevocation,
    /// Refresh expiring federation VTXOs.
    Refresh,
    /// Exit Bark custody into the accepted federated on-chain wallet.
    EmergencyExit,
}

impl FederationBarkActionKind {
    /// Stable operator and metrics label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LightningSend => "lightning_send",
            Self::IncomingClaim => "incoming_claim",
            Self::FailedSendRevocation => "failed_send_revocation",
            Self::Refresh => "refresh",
            Self::EmergencyExit => "emergency_exit",
        }
    }
}

/// Exact side-effecting plan accepted before Bark or Ark execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkActionPlan {
    /// Nested operation format.
    pub version: u16,
    /// Stable economic action lineage.
    pub action_id: FederationBarkActionId,
    /// Cashu quote authorizing a receive/send/revocation action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// Plan category.
    pub kind: FederationBarkActionKind,
    /// Member selected to execute the first attempt.
    pub actor: MemberId,
    /// Accepted predecessor operation or quote observation.
    pub predecessor_operation_id: FederationOperationId,
    /// Payment hash for send and claim actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_hash: Option<FederationBarkDigest>,
    /// Exact sorted input reservation set.
    pub inputs: Vec<FederationBarkVtxoInput>,
    /// Exact sorted output allocation set.
    pub outputs: Vec<FederationBarkVtxoOutput>,
    /// Exact revocation key allocation for an outgoing Lightning send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_locator: Option<FederationBarkVtxoLocator>,
    /// Value leaving Bark custody as an external Lightning payment.
    pub external_amount: Amount,
    /// Exact planned fee.
    pub fee_amount: Amount,
    /// Maximum accepted fee under Cashu and Bark policy.
    pub fee_bound: Amount,
    /// Exact sorted canonical artifacts needed by the plan.
    pub artifacts: Vec<FederationBarkArtifact>,
    /// Certified HTLC evidence combined with an incoming claim authorization.
    ///
    /// Required for incoming claims; absent for other action kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certified_receive_htlcs: Option<Box<FederationBarkCertifiedReceiveHtlcs>>,
    /// Stable accepted execution identity; never sent as a custom Ark field.
    pub external_id: FederationBarkExternalId,
    /// Initial deterministic signer set, sorted by member.
    pub signer_set: Vec<MemberId>,
    /// Initial accepted custody attempt.
    pub attempt: u32,
    /// Consensus-visible action expiry or chain-height bound.
    pub expires_at: u64,
}

/// Select the deterministic threshold-sized Bark signer window for one
/// consensus-authorized custody attempt.
///
/// Attempt one starts at the first canonical member. Each later attempt moves
/// the same fixed-size window forward by one member, wrapping at the end. The
/// schedule depends only on accepted public configuration and never on a
/// member's local health view. One pass over the roster is deliberately
/// bounded; a later retry requires an explicit recovery protocol rather than
/// silently cycling back to an old signer set.
pub fn federation_bark_signer_set_for_attempt(
    config: &FederationConfig,
    attempt: u32,
) -> Result<Vec<MemberId>> {
    let application = crate::federation_bark_application(config)?
        .ok_or_else(|| invalid_bark("accepted Bark application is missing"))?;
    let members = config.members.keys().copied().collect::<Vec<_>>();
    let threshold = usize::from(application.threshold);
    let attempt_index = usize::try_from(
        attempt
            .checked_sub(1)
            .ok_or_else(|| invalid_bark("Bark signing attempt must be non-zero"))?,
    )
    .map_err(|_| invalid_bark("Bark signing attempt does not fit this platform"))?;
    if members.is_empty()
        || threshold == 0
        || threshold > members.len()
        || attempt_index >= members.len()
    {
        return Err(invalid_bark(
            "Bark signing attempt is outside the deterministic roster pass",
        ));
    }
    let mut signer_set = (0..threshold)
        .map(|offset| members[(attempt_index + offset) % members.len()])
        .collect::<Vec<_>>();
    signer_set.sort_unstable();
    Ok(signer_set)
}

/// Select the next deterministic execution actor after an accepted actor.
///
/// This rotates once through the canonical member roster. It changes only who
/// may drive an already accepted action; the plan, external id, inputs,
/// outputs, fees, and package commitments remain unchanged.
pub fn federation_bark_next_actor(
    config: &FederationConfig,
    current_actor: MemberId,
) -> Result<MemberId> {
    let members = config.members.keys().copied().collect::<Vec<_>>();
    let current_index = members
        .iter()
        .position(|member| *member == current_actor)
        .ok_or_else(|| invalid_bark("current Bark actor is not a federation member"))?;
    if members.len() < 2 {
        return Err(invalid_bark("Bark actor failover requires another member"));
    }
    Ok(members[(current_index + 1) % members.len()])
}

#[derive(Serialize)]
struct FederationBarkLightningSendRequestBinding<'a> {
    domain: &'static str,
    version: u16,
    action_id: FederationBarkActionId,
    quote: &'a str,
    actor: MemberId,
    predecessor_operation_id: FederationOperationId,
    payment_hash: FederationBarkDigest,
    inputs: &'a [FederationBarkVtxoInput],
    output_locator: FederationBarkVtxoLocator,
    output_amount: Amount,
    revocation_locator: FederationBarkVtxoLocator,
    external_amount: Amount,
    fee_amount: Amount,
    fee_bound: Amount,
    external_id: FederationBarkExternalId,
    signer_set: &'a [MemberId],
    attempt: u32,
    expires_at: u64,
}

/// Canonical round request bound by an accepted refresh plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkRefreshRequest {
    /// Domain separator for the canonical request.
    pub domain: String,
    /// Request schema version.
    pub version: u16,
    /// Stable maintenance action.
    pub action_id: FederationBarkActionId,
    /// Exact predecessor used for deterministic selection.
    pub predecessor_operation_id: FederationOperationId,
    /// Exact sorted refresh inputs.
    pub inputs: Vec<FederationBarkVtxoInput>,
    /// Consensus-allocated round output locator.
    pub output_locator: FederationBarkVtxoLocator,
    /// Exact replacement value.
    pub output_amount: Amount,
    /// Accepted refresh fee.
    pub fee: Amount,
    /// First height at which refresh execution is permitted.
    pub trigger_height: u32,
    /// Height at which the plan must escalate to emergency exit.
    pub emergency_height: u32,
    /// Earliest expiry among the exact inputs.
    pub minimum_expiry: u32,
    /// Stable Ark retry identifier.
    pub external_id: FederationBarkExternalId,
    /// Accepted signer attempt.
    pub attempt: u32,
}

/// Canonical emergency-exit request into an allocated Bark recovery script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkExitRequest {
    /// Domain separator for the canonical request.
    pub domain: String,
    /// Request schema version.
    pub version: u16,
    /// Stable recovery action.
    pub action_id: FederationBarkActionId,
    /// Exact predecessor used for deterministic selection.
    pub predecessor_operation_id: FederationOperationId,
    /// Exact sorted exit inputs.
    pub inputs: Vec<FederationBarkVtxoInput>,
    /// Consensus allocation owned by Bark's recovery application.
    pub destination: FederationAllocatedAddressRef,
    /// Exact destination script committed by the allocation.
    #[serde(with = "hex_bytes")]
    pub destination_script: Vec<u8>,
    /// Immutable Bitcoin network.
    pub network: Network,
    /// Exact fee rate used for the final drain, in sat/kwu.
    pub fee_rate_sat_per_kwu: u64,
    /// Maximum fee accepted for the final drain.
    pub fee_bound: Amount,
    /// Minimum value that must arrive under Bark recovery custody.
    pub minimum_recovered: Amount,
    /// First height at which emergency exit execution is permitted.
    pub trigger_height: u32,
    /// Earliest expiry among the exact inputs.
    pub minimum_expiry: u32,
    /// Required drain-transaction confirmation depth.
    pub minimum_confirmations: u32,
    /// Stable publication retry identifier.
    pub external_id: FederationBarkExternalId,
    /// Accepted signer attempt.
    pub attempt: u32,
}

/// Semantic Bark state fixed by an action plan or accepted transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationBarkSemanticPhase {
    /// Receive intent accepted.
    ReceiveIntent,
    /// Canonical invoice accepted.
    ReceivePrepared,
    /// Outgoing Lightning plan accepted.
    SendPlanned,
    /// Incoming claim plan accepted.
    ClaimPlanned,
    /// Failed-send revocation plan accepted.
    RevocationPlanned,
    /// Refresh plan accepted.
    RefreshPlanned,
    /// Emergency-exit plan accepted.
    ExitPlanned,
    /// A higher deterministic signer attempt was accepted.
    SigningAttemptAdvanced,
    /// Incoming HTLC VTXOs are available for claim.
    ReceiveHtlcsReady,
    /// The receive preimage was disclosed after claim authorization.
    ReceivePreimageDisclosed,
    /// Incoming value became spendable under federation custody.
    ReceiveClaimed,
    /// Outgoing HTLC package was co-signed.
    SendHtlcsCosigned,
    /// Outgoing payment succeeded.
    PaymentSucceeded,
    /// Outgoing payment terminally failed but may still require recovery.
    PaymentFailed,
    /// Failed outgoing VTXOs were safely revoked.
    RevocationCompleted,
    /// Refresh produced accepted replacement VTXOs.
    RefreshCompleted,
    /// Emergency exit was registered.
    ExitRegistered,
    /// Emergency exit was published.
    ExitPublished,
    /// Emergency exit was sufficiently confirmed.
    ExitConfirmed,
    /// Bounded member evidence was recorded.
    Observation,
}

/// Return whether one accepted action head authorizes the exact custody attempt.
#[must_use]
pub const fn federation_bark_custody_attempt_is_current(
    kind: FederationBarkActionKind,
    phase: FederationBarkSemanticPhase,
    signing_retry_phase: Option<FederationBarkSemanticPhase>,
    current_attempt: u32,
    authorization_attempt: u32,
) -> bool {
    if current_attempt != authorization_attempt {
        return false;
    }
    let effective_phase = match signing_retry_phase {
        Some(phase) => phase,
        None => phase,
    };
    match kind {
        FederationBarkActionKind::IncomingClaim => matches!(
            effective_phase,
            FederationBarkSemanticPhase::ClaimPlanned
                | FederationBarkSemanticPhase::ReceivePreimageDisclosed
        ),
        FederationBarkActionKind::LightningSend => {
            matches!(effective_phase, FederationBarkSemanticPhase::SendPlanned)
        }
        FederationBarkActionKind::FailedSendRevocation => matches!(
            effective_phase,
            FederationBarkSemanticPhase::RevocationPlanned
        ),
        FederationBarkActionKind::Refresh => {
            matches!(effective_phase, FederationBarkSemanticPhase::RefreshPlanned)
        }
        FederationBarkActionKind::EmergencyExit => matches!(
            effective_phase,
            FederationBarkSemanticPhase::ExitPlanned | FederationBarkSemanticPhase::ExitRegistered
        ),
    }
}

impl FederationBarkSemanticPhase {
    /// Complete fixed phase domain for bounded metrics.
    pub const ALL: [Self; 20] = [
        Self::ReceiveIntent,
        Self::ReceivePrepared,
        Self::SendPlanned,
        Self::ClaimPlanned,
        Self::RevocationPlanned,
        Self::RefreshPlanned,
        Self::ExitPlanned,
        Self::SigningAttemptAdvanced,
        Self::ReceiveHtlcsReady,
        Self::ReceivePreimageDisclosed,
        Self::ReceiveClaimed,
        Self::SendHtlcsCosigned,
        Self::PaymentSucceeded,
        Self::PaymentFailed,
        Self::RevocationCompleted,
        Self::RefreshCompleted,
        Self::ExitRegistered,
        Self::ExitPublished,
        Self::ExitConfirmed,
        Self::Observation,
    ];

    /// Stable operator and metrics label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReceiveIntent => "receive_intent",
            Self::ReceivePrepared => "receive_prepared",
            Self::SendPlanned => "send_planned",
            Self::ClaimPlanned => "claim_planned",
            Self::RevocationPlanned => "revocation_planned",
            Self::RefreshPlanned => "refresh_planned",
            Self::ExitPlanned => "exit_planned",
            Self::SigningAttemptAdvanced => "signing_attempt_advanced",
            Self::ReceiveHtlcsReady => "receive_htlcs_ready",
            Self::ReceivePreimageDisclosed => "receive_preimage_disclosed",
            Self::ReceiveClaimed => "receive_claimed",
            Self::SendHtlcsCosigned => "send_htlcs_cosigned",
            Self::PaymentSucceeded => "payment_succeeded",
            Self::PaymentFailed => "payment_failed",
            Self::RevocationCompleted => "revocation_completed",
            Self::RefreshCompleted => "refresh_completed",
            Self::ExitRegistered => "exit_registered",
            Self::ExitPublished => "exit_published",
            Self::ExitConfirmed => "exit_confirmed",
            Self::Observation => "observation",
        }
    }

    /// Stable v1 numeric code used by action and session derivation.
    #[must_use]
    pub const fn code(self) -> u16 {
        match self {
            Self::ReceiveIntent => 1,
            Self::ReceivePrepared => 2,
            Self::SendPlanned => 3,
            Self::ClaimPlanned => 4,
            Self::RevocationPlanned => 5,
            Self::RefreshPlanned => 6,
            Self::ExitPlanned => 7,
            Self::SigningAttemptAdvanced => 8,
            Self::ReceiveHtlcsReady => 9,
            Self::ReceivePreimageDisclosed => 10,
            Self::ReceiveClaimed => 11,
            Self::SendHtlcsCosigned => 12,
            Self::PaymentSucceeded => 14,
            Self::PaymentFailed => 15,
            Self::RevocationCompleted => 16,
            Self::RefreshCompleted => 17,
            Self::ExitRegistered => 18,
            Self::ExitPublished => 19,
            Self::ExitConfirmed => 20,
            Self::Observation => 21,
        }
    }
}

/// Accepted externally observed transition of one Bark action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkActionTransition {
    /// Nested operation format.
    pub version: u16,
    /// Stable action lineage.
    pub action_id: FederationBarkActionId,
    /// Cashu quote bound to the action, when quote-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// New semantic phase.
    pub phase: FederationBarkSemanticPhase,
    /// Member that obtained and validated the result.
    pub actor: MemberId,
    /// Accepted plan operation.
    pub plan_operation_id: FederationOperationId,
    /// Exact predecessor transition or plan.
    pub predecessor_operation_id: FederationOperationId,
    /// Payment hash for payment-scoped transitions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_hash: Option<FederationBarkDigest>,
    /// Attempt that produced this result.
    pub attempt: u32,
    /// Exact signer set for this attempt, sorted by member.
    pub signer_set: Vec<MemberId>,
    /// Commitment to the exact external request.
    pub request_hash: FederationBarkDigest,
    /// Commitment to the complete canonical result.
    pub result_hash: FederationBarkDigest,
    /// Stable external retry identifier.
    pub external_id: FederationBarkExternalId,
    /// Exact sorted result artifacts.
    pub artifacts: Vec<FederationBarkArtifact>,
    /// Exact sorted VTXO state mutations proven by the result.
    ///
    /// Defaulting this additive field preserves decoding of the frozen v1 Bark
    /// operation shape emitted before the SQL projection was installed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vtxo_transitions: Vec<FederationBarkVtxoTransition>,
    /// Independent-member evidence certificate when this transition replaces
    /// separately ordered member observations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_certificate: Option<Box<FederationBarkEvidenceCertificate>>,
}

#[derive(Serialize)]
struct FederationBarkSigningAttemptResultBinding<'a> {
    domain: &'static str,
    action_id: FederationBarkActionId,
    plan_operation_id: FederationOperationId,
    predecessor_operation_id: FederationOperationId,
    actor: MemberId,
    attempt: u32,
    signer_set: &'a [MemberId],
    request_hash: FederationBarkDigest,
    external_id: FederationBarkExternalId,
}

/// Conservative status reported by one federation member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationBarkObservationStatus {
    /// External state remains pending or ambiguous.
    Pending,
    /// Exact external request is known to have been accepted.
    Accepted,
    /// External operation is terminally successful.
    Succeeded,
    /// External operation is terminally failed.
    Failed,
    /// Recovery is required before a terminal Cashu observation is safe.
    RecoveryRequired,
}

/// Bounded member evidence used by Bark transition and payment policies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkMemberObservation {
    /// Nested operation format.
    pub version: u16,
    /// Stable action lineage.
    pub action_id: FederationBarkActionId,
    /// Cashu quote bound to the action, when quote-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// Reporting federation member.
    pub observer: MemberId,
    /// Monotonic source-scoped observation epoch.
    pub observation_epoch: u64,
    /// Commitment to the configured Ark or chain source scope.
    pub source_scope: FederationBarkDigest,
    /// Payment hash when the evidence is payment-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_hash: Option<FederationBarkDigest>,
    /// Conservative observed status.
    pub status: FederationBarkObservationStatus,
    /// Exact observed value.
    pub amount: Amount,
    /// Exact observed fee.
    pub fee: Amount,
    /// Commitment to the bounded evidence artifact.
    pub result_hash: FederationBarkDigest,
}

/// Stable nested Bark journal operation family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FederationBarkOperation {
    /// Accept a receive secret, locator, amounts, and policy before invoice creation.
    ReceiveIntent(FederationBarkReceiveIntent),
    /// Accept the exact idempotent invoice preparation result.
    ReceivePrepared(FederationBarkReceivePrepared),
    /// Accept one exact send, claim, revocation, refresh, or exit plan.
    ActionPlan(FederationBarkActionPlan),
    /// Accept one exact semantic action transition.
    ActionTransition(FederationBarkActionTransition),
    /// Accept bounded member-local Ark or chain evidence.
    MemberObservation(FederationBarkMemberObservation),
    /// Accept a quorum certificate over independently verified Bark evidence.
    EvidenceCertificate(FederationBarkEvidenceCertificate),
}

/// Stable family-level Bark operation kind used in authorization references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationBarkOperationKind {
    /// Receive intent.
    ReceiveIntent,
    /// Prepared receive invoice.
    ReceivePrepared,
    /// Exact action plan.
    ActionPlan,
    /// Semantic action transition.
    ActionTransition,
    /// Member observation.
    MemberObservation,
    /// Independent-member evidence certificate.
    EvidenceCertificate,
}

/// Typed Bark semantic reservation used by the generic federation journal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum FederationBarkConflictKey {
    /// A Cashu quote may bind only one initial Bark action.
    Quote {
        /// Cashu quote identifier.
        quote: String,
    },
    /// A BOLT11 payment hash may belong to only one receive or send lineage.
    PaymentHash {
        /// Exact payment hash.
        payment_hash: FederationBarkDigest,
    },
    /// One VTXO may be reserved by only one accepted plan.
    VtxoReservation {
        /// Reserved VTXO.
        vtxo_id: FederationBarkVtxoId,
    },
    /// One derivation locator may be allocated only once.
    Locator {
        /// Reserved destination locator.
        locator: FederationBarkVtxoLocator,
    },
    /// One action phase and attempt may contain only one exact payload.
    ActionPhase {
        /// Stable action lineage.
        action_id: FederationBarkActionId,
        /// Semantic phase.
        phase: FederationBarkSemanticPhase,
        /// Custody attempt where applicable.
        attempt: Option<u32>,
    },
    /// One action predecessor may admit only one next transition.
    Predecessor {
        /// Stable action lineage owning the predecessor edge.
        action_id: FederationBarkActionId,
        /// Exact predecessor operation.
        operation_id: FederationOperationId,
    },
    /// One accepted external-execution identity may bind only one request/result.
    ExternalId {
        /// Stable federation-internal execution identifier.
        external_id: FederationBarkExternalId,
    },
    /// One deterministic custody slot and attempt may bind one signer context.
    SigningAttempt {
        /// Stable action lineage.
        action_id: FederationBarkActionId,
        /// Semantic phase using custody.
        phase: FederationBarkSemanticPhase,
        /// Accepted attempt.
        attempt: u32,
    },
    /// One observer may submit one source-scoped view per epoch.
    Observation {
        /// Stable action lineage.
        action_id: FederationBarkActionId,
        /// Reporting member.
        observer: MemberId,
        /// Source-scoped epoch.
        observation_epoch: u64,
    },
    /// One evidence phase per action may accept only one exact certificate.
    EvidenceCertificate {
        /// Stable action lineage.
        action_id: FederationBarkActionId,
        /// Independently verified evidence family.
        phase: FederationBarkEvidencePhase,
    },
}

impl FederationBarkOperation {
    pub(crate) fn preserves_action_actor(&self) -> bool {
        matches!(
            self,
            Self::ActionPlan(plan) if plan.certified_receive_htlcs.is_some()
        ) || matches!(
            self,
            Self::ActionTransition(transition)
                if transition.evidence_certificate.is_some()
                    || transition.phase == FederationBarkSemanticPhase::PaymentSucceeded
        )
    }

    /// Return the family-level kind.
    #[must_use]
    pub const fn kind(&self) -> FederationBarkOperationKind {
        match self {
            Self::ReceiveIntent(_) => FederationBarkOperationKind::ReceiveIntent,
            Self::ReceivePrepared(_) => FederationBarkOperationKind::ReceivePrepared,
            Self::ActionPlan(_) => FederationBarkOperationKind::ActionPlan,
            Self::ActionTransition(_) => FederationBarkOperationKind::ActionTransition,
            Self::MemberObservation(_) => FederationBarkOperationKind::MemberObservation,
            Self::EvidenceCertificate(_) => FederationBarkOperationKind::EvidenceCertificate,
        }
    }

    /// Return the member whose envelope authorization must match this payload.
    #[must_use]
    pub fn claimed_author(&self) -> Option<MemberId> {
        if self.preserves_action_actor() {
            // Certificates and payment preimages provide objective authority;
            // the action keeps the actor selected by its accepted origin.
            return None;
        }
        match self {
            Self::ReceiveIntent(operation) => Some(operation.actor),
            Self::ReceivePrepared(operation) => Some(operation.actor),
            Self::ActionPlan(operation) => Some(operation.actor),
            Self::ActionTransition(operation) => Some(operation.actor),
            Self::MemberObservation(operation) => Some(operation.observer),
            Self::EvidenceCertificate(_) => None,
        }
    }

    /// Return the stable action identifier.
    #[must_use]
    pub const fn action_id(&self) -> FederationBarkActionId {
        match self {
            Self::ReceiveIntent(operation) => operation.action_id,
            Self::ReceivePrepared(operation) => operation.action_id,
            Self::ActionPlan(operation) => operation.action_id,
            Self::ActionTransition(operation) => operation.action_id,
            Self::MemberObservation(operation) => operation.action_id,
            Self::EvidenceCertificate(certificate) => certificate.binding.action_id,
        }
    }

    /// Return the Cashu quote bound to this Bark action lineage.
    #[must_use]
    pub fn quote(&self) -> Option<&str> {
        match self {
            Self::ReceiveIntent(operation) => Some(operation.quote.as_str()),
            Self::ReceivePrepared(operation) => Some(operation.quote.as_str()),
            Self::ActionPlan(operation) => operation.quote.as_deref(),
            Self::ActionTransition(operation) => operation.quote.as_deref(),
            Self::MemberObservation(operation) => operation.quote.as_deref(),
            Self::EvidenceCertificate(certificate) => Some(certificate.binding.quote.as_str()),
        }
    }

    /// Return the Lightning payment hash bound to this action lineage.
    #[must_use]
    pub const fn payment_hash(&self) -> Option<FederationBarkDigest> {
        match self {
            Self::ReceiveIntent(operation) => Some(operation.payment_hash),
            Self::ReceivePrepared(operation) => Some(operation.payment_hash),
            Self::ActionPlan(operation) => operation.payment_hash,
            Self::ActionTransition(operation) => operation.payment_hash,
            Self::MemberObservation(operation) => operation.payment_hash,
            Self::EvidenceCertificate(certificate) => Some(certificate.binding.payment_hash),
        }
    }

    /// Return the semantic phase represented by this operation.
    #[must_use]
    pub const fn phase(&self) -> FederationBarkSemanticPhase {
        match self {
            Self::ReceiveIntent(_) => FederationBarkSemanticPhase::ReceiveIntent,
            Self::ReceivePrepared(_) => FederationBarkSemanticPhase::ReceivePrepared,
            Self::ActionPlan(operation) => match operation.kind {
                FederationBarkActionKind::LightningSend => FederationBarkSemanticPhase::SendPlanned,
                FederationBarkActionKind::IncomingClaim => {
                    FederationBarkSemanticPhase::ClaimPlanned
                }
                FederationBarkActionKind::FailedSendRevocation => {
                    FederationBarkSemanticPhase::RevocationPlanned
                }
                FederationBarkActionKind::Refresh => FederationBarkSemanticPhase::RefreshPlanned,
                FederationBarkActionKind::EmergencyExit => FederationBarkSemanticPhase::ExitPlanned,
            },
            Self::ActionTransition(operation) => operation.phase,
            Self::MemberObservation(_) => FederationBarkSemanticPhase::Observation,
            Self::EvidenceCertificate(_) => FederationBarkSemanticPhase::Observation,
        }
    }

    /// Return the canonical result commitment, if this record carries a result.
    #[must_use]
    pub const fn result_hash(&self) -> Option<FederationBarkDigest> {
        match self {
            Self::ReceivePrepared(operation) => Some(operation.result_hash),
            Self::ActionTransition(operation) => Some(operation.result_hash),
            Self::MemberObservation(operation) => Some(operation.result_hash),
            Self::EvidenceCertificate(certificate) => Some(certificate.binding.evidence_digest),
            Self::ReceiveIntent(_) | Self::ActionPlan(_) => None,
        }
    }

    /// Derive semantic conflict keys from typed fields only.
    #[must_use]
    pub fn conflict_keys(&self) -> Vec<FederationBarkConflictKey> {
        match self {
            Self::ReceiveIntent(operation) => vec![
                FederationBarkConflictKey::Quote {
                    quote: operation.quote.clone(),
                },
                FederationBarkConflictKey::PaymentHash {
                    payment_hash: operation.payment_hash,
                },
                FederationBarkConflictKey::Locator {
                    locator: operation.destination,
                },
                FederationBarkConflictKey::ActionPhase {
                    action_id: operation.action_id,
                    phase: FederationBarkSemanticPhase::ReceiveIntent,
                    attempt: None,
                },
            ],
            Self::ReceivePrepared(operation) => vec![
                FederationBarkConflictKey::ActionPhase {
                    action_id: operation.action_id,
                    phase: FederationBarkSemanticPhase::ReceivePrepared,
                    attempt: None,
                },
                FederationBarkConflictKey::ExternalId {
                    external_id: operation.external_id,
                },
            ],
            Self::ActionPlan(operation) => {
                let phase = self.phase();
                let mut keys =
                    Vec::with_capacity(operation.inputs.len() + operation.outputs.len() + 6);
                if matches!(operation.kind, FederationBarkActionKind::LightningSend) {
                    if let Some(quote) = &operation.quote {
                        keys.push(FederationBarkConflictKey::Quote {
                            quote: quote.clone(),
                        });
                    }
                    if let Some(payment_hash) = operation.payment_hash {
                        keys.push(FederationBarkConflictKey::PaymentHash { payment_hash });
                    }
                }
                keys.push(FederationBarkConflictKey::ActionPhase {
                    action_id: operation.action_id,
                    phase,
                    attempt: Some(operation.attempt),
                });
                keys.push(FederationBarkConflictKey::Predecessor {
                    action_id: operation.action_id,
                    operation_id: operation.predecessor_operation_id,
                });
                keys.push(FederationBarkConflictKey::ExternalId {
                    external_id: operation.external_id,
                });
                keys.push(FederationBarkConflictKey::SigningAttempt {
                    action_id: operation.action_id,
                    phase,
                    attempt: operation.attempt,
                });
                if let Some(certified) = &operation.certified_receive_htlcs {
                    keys.push(FederationBarkConflictKey::ActionPhase {
                        action_id: operation.action_id,
                        phase: FederationBarkSemanticPhase::ReceiveHtlcsReady,
                        attempt: Some(operation.attempt),
                    });
                    keys.push(FederationBarkConflictKey::EvidenceCertificate {
                        action_id: operation.action_id,
                        phase: certified.certificate.binding.phase,
                    });
                }
                let reuses_existing_reservations = matches!(
                    operation.kind,
                    FederationBarkActionKind::FailedSendRevocation
                        | FederationBarkActionKind::EmergencyExit
                );
                if !reuses_existing_reservations {
                    keys.extend(operation.inputs.iter().map(|input| {
                        FederationBarkConflictKey::VtxoReservation {
                            vtxo_id: input.vtxo_id,
                        }
                    }));
                }
                if operation.kind != FederationBarkActionKind::FailedSendRevocation {
                    keys.extend(operation.outputs.iter().map(|output| {
                        FederationBarkConflictKey::Locator {
                            locator: output.locator,
                        }
                    }));
                }
                if let Some(locator) = operation.revocation_locator {
                    keys.push(FederationBarkConflictKey::Locator { locator });
                }
                keys
            }
            Self::ActionTransition(operation) => {
                let mut keys = vec![
                    FederationBarkConflictKey::ActionPhase {
                        action_id: operation.action_id,
                        phase: operation.phase,
                        attempt: Some(operation.attempt),
                    },
                    FederationBarkConflictKey::Predecessor {
                        action_id: operation.action_id,
                        operation_id: operation.predecessor_operation_id,
                    },
                ];
                if !operation.signer_set.is_empty() {
                    keys.push(FederationBarkConflictKey::SigningAttempt {
                        action_id: operation.action_id,
                        phase: operation.phase,
                        attempt: operation.attempt,
                    });
                }
                if let Some(certificate) = &operation.evidence_certificate {
                    keys.push(FederationBarkConflictKey::EvidenceCertificate {
                        action_id: operation.action_id,
                        phase: certificate.binding.phase,
                    });
                }
                keys
            }
            Self::MemberObservation(operation) => {
                vec![FederationBarkConflictKey::Observation {
                    action_id: operation.action_id,
                    observer: operation.observer,
                    observation_epoch: operation.observation_epoch,
                }]
            }
            Self::EvidenceCertificate(certificate) => {
                vec![FederationBarkConflictKey::EvidenceCertificate {
                    action_id: certificate.binding.action_id,
                    phase: certificate.binding.phase,
                }]
            }
        }
    }

    /// Validate deterministic operation-local rules and frozen v1 bounds.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::ReceiveIntent(operation) => operation.validate()?,
            Self::ReceivePrepared(operation) => operation.validate()?,
            Self::ActionPlan(operation) => operation.validate()?,
            Self::ActionTransition(operation) => operation.validate()?,
            Self::MemberObservation(operation) => operation.validate()?,
            Self::EvidenceCertificate(certificate) => {
                if certificate.attestations.is_empty()
                    || certificate.attestations.len() > FEDERATION_MAX_BARK_COLLECTION_ENTRIES
                    || certificate
                        .attestations
                        .windows(2)
                        .any(|pair| pair[0].signer >= pair[1].signer)
                {
                    return Err(invalid_bark(
                        "Bark evidence certificate signer set is invalid",
                    ));
                }
            }
        }
        let actual = self.canonical_bytes()?.len();
        if actual > FEDERATION_MAX_BARK_OPERATION_BYTES {
            return Err(invalid_bark(format!(
                "canonical Bark operation is {actual} bytes, maximum is {FEDERATION_MAX_BARK_OPERATION_BYTES}"
            )));
        }
        Ok(())
    }

    /// Return sorted-key canonical JSON bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(self)
    }

    /// Commit to the complete canonical nested operation.
    pub fn commitment(&self) -> Result<FederationBarkDigest> {
        domain_hash(BARK_OPERATION_HASH_DOMAIN, &self.canonical_bytes()?)
    }
}

impl FederationBarkReceiveIntent {
    fn validate(&self) -> Result<()> {
        ensure_version(self.version)?;
        ensure_quote(&self.quote)?;
        self.destination.validate()?;
        if self.cashu_amount == Amount::ZERO
            || self.gross_amount != self.cashu_amount
            || self.receive_fee != Amount::ZERO
            || self.minimum_net_amount < self.cashu_amount
        {
            return Err(invalid_bark(
                "v1 receive amounts must be non-zero, fee-free, and fully collateralized",
            ));
        }
        if self.minimum_cltv_delta == 0
            || self.fee_policy_version != 1
            || self.anti_dos_policy_version != 1
            || self.expires_at == 0
        {
            return Err(invalid_bark("unsupported receive policy or expiry"));
        }
        let description = self.description.as_deref().unwrap_or_default();
        if description.len() > FEDERATION_MAX_BARK_DESCRIPTION_BYTES
            || self.description_hash
                != federation_bark_description_hash(self.description.as_deref())?
        {
            return Err(invalid_bark(
                "receive description exceeds 4 KiB or differs from its commitment",
            ));
        }
        PublicKey::from_slice(&self.mailbox_id)
            .map_err(|_| invalid_bark("invalid Bark mailbox identifier"))?;
        if self.mailbox_scope != federation_bark_mailbox_scope(&self.mailbox_id)? {
            return Err(invalid_bark(
                "receive mailbox identifier differs from its scope commitment",
            ));
        }
        ensure_count(
            "receive-secret acknowledgements",
            self.secret_acknowledgements.len(),
            false,
        )?;
        ensure_members_sorted_unique(
            &self
                .secret_acknowledgements
                .iter()
                .map(|acknowledgement| acknowledgement.member)
                .collect::<Vec<_>>(),
        )?;
        let expected_record = self.secret_acknowledgements[0].record_hash;
        if self
            .secret_acknowledgements
            .iter()
            .any(|acknowledgement| acknowledgement.record_hash != expected_record)
        {
            return Err(invalid_bark(
                "receive-secret acknowledgements do not bind one record",
            ));
        }
        if !self
            .secret_acknowledgements
            .iter()
            .any(|acknowledgement| acknowledgement.member == self.actor)
        {
            return Err(invalid_bark(
                "receive actor must have acknowledged the protected secret",
            ));
        }
        Ok(())
    }
}

impl FederationBarkReceivePrepared {
    fn validate(&self) -> Result<()> {
        ensure_version(self.version)?;
        ensure_quote(&self.quote)?;
        if self.invoice.is_empty() || self.invoice.len() > FEDERATION_MAX_BARK_INVOICE_BYTES {
            return Err(invalid_bark("Bark invoice is empty or exceeds 64 KiB"));
        }
        if self.gross_amount == Amount::ZERO
            || self.receive_fee != Amount::ZERO
            || self.expected_net_amount < self.gross_amount
            || self.expires_at == 0
            || self.final_cltv_delta == 0
        {
            return Err(invalid_bark(
                "prepared receive violates v1 amount or expiry rules",
            ));
        }
        Ok(())
    }

    /// Construct and fully validate the exact invoice result for an accepted intent.
    pub fn from_invoice(
        actor: MemberId,
        intent_operation_id: FederationOperationId,
        intent: &FederationBarkReceiveIntent,
        invoice: Bolt11Invoice,
    ) -> Result<Self> {
        let expires_at = invoice
            .expires_at()
            .ok_or_else(|| invalid_bark("Bark invoice expiry overflows"))?
            .as_secs();
        let final_cltv_delta = u32::try_from(invoice.min_final_cltv_expiry_delta())
            .map_err(|_| invalid_bark("Bark invoice final CLTV exceeds u32"))?;
        let expected_net_amount = intent
            .gross_amount
            .checked_sub(intent.receive_fee)
            .ok_or_else(|| invalid_bark("Bark receive fee exceeds gross amount"))?;
        let mut prepared = Self {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: intent.action_id,
            quote: intent.quote.clone(),
            actor,
            intent_operation_id,
            invoice: invoice.to_string(),
            payment_hash: FederationBarkDigest::from_bytes(invoice.payment_hash().to_byte_array())?,
            gross_amount: intent.gross_amount,
            expected_net_amount,
            network: invoice.network(),
            expires_at,
            final_cltv_delta,
            receive_fee: intent.receive_fee,
            request_hash: federation_bark_receive_request_hash(intent_operation_id, intent)?,
            result_hash: FederationBarkDigest::from_bytes([1; 32])?,
            external_id: receive_external_id(intent_operation_id, intent.action_id)?,
        };
        prepared.result_hash = receive_result_hash(&prepared)?;
        prepared.validate_against_intent(intent)?;
        Ok(prepared)
    }

    /// Validate every prepared field and the canonical invoice against its accepted intent.
    pub fn validate_against_intent(&self, intent: &FederationBarkReceiveIntent) -> Result<()> {
        self.validate()?;
        intent.validate()?;
        if self.action_id != intent.action_id
            || self.quote != intent.quote
            || self.payment_hash != intent.payment_hash
            || self.gross_amount != intent.gross_amount
            || self.receive_fee != intent.receive_fee
            || self.expected_net_amount
                != intent
                    .gross_amount
                    .checked_sub(intent.receive_fee)
                    .ok_or_else(|| invalid_bark("Bark receive fee exceeds gross amount"))?
            || self.network != intent.network
        {
            return Err(invalid_bark(
                "prepared receive differs from accepted intent bindings",
            ));
        }
        if !intent
            .secret_acknowledgements
            .iter()
            .any(|acknowledgement| acknowledgement.member == self.actor)
        {
            return Err(invalid_bark(
                "prepared receive actor did not acknowledge the protected secret",
            ));
        }
        let invoice = self
            .invoice
            .parse::<Bolt11Invoice>()
            .map_err(|_| invalid_bark("prepared receive contains an invalid BOLT11 invoice"))?;
        invoice
            .check_signature()
            .map_err(|_| invalid_bark("prepared receive invoice signature is invalid"))?;
        if invoice.to_string() != self.invoice
            || invoice.network() != intent.network
            || invoice.payment_hash().to_byte_array() != intent.payment_hash.to_bytes()
        {
            return Err(invalid_bark(
                "prepared receive invoice identity, network, or payment hash differs",
            ));
        }
        let expected_msat = u64::from(intent.gross_amount)
            .checked_mul(1_000)
            .ok_or_else(|| invalid_bark("Bark invoice amount overflows millisatoshis"))?;
        if invoice.amount_milli_satoshis() != Some(expected_msat) {
            return Err(invalid_bark(
                "prepared receive invoice amount differs from accepted gross amount",
            ));
        }
        match invoice.description() {
            Bolt11InvoiceDescriptionRef::Direct(description)
                if description.to_string() == intent.description.as_deref().unwrap_or_default() => {
            }
            Bolt11InvoiceDescriptionRef::Direct(_) | Bolt11InvoiceDescriptionRef::Hash(_) => {
                return Err(invalid_bark(
                    "prepared receive requires the exact accepted direct description",
                ));
            }
        }
        let invoice_expiry = invoice
            .expires_at()
            .ok_or_else(|| invalid_bark("Bark invoice expiry overflows"))?
            .as_secs();
        let invoice_cltv = u32::try_from(invoice.min_final_cltv_expiry_delta())
            .map_err(|_| invalid_bark("Bark invoice final CLTV exceeds u32"))?;
        if self.expires_at != invoice_expiry
            || self.expires_at > intent.expires_at
            || self.final_cltv_delta != invoice_cltv
            || self.final_cltv_delta < intent.minimum_cltv_delta
        {
            return Err(invalid_bark(
                "prepared receive invoice expiry or final CLTV violates accepted policy",
            ));
        }
        if self.request_hash
            != federation_bark_receive_request_hash(self.intent_operation_id, intent)?
            || self.external_id != receive_external_id(self.intent_operation_id, intent.action_id)?
            || self.result_hash != receive_result_hash(self)?
        {
            return Err(invalid_bark(
                "prepared receive request, result, or retry commitment differs",
            ));
        }
        Ok(())
    }
}

/// Commit to a bounded direct BOLT11 description, treating absence as the empty string.
pub fn federation_bark_description_hash(description: Option<&str>) -> Result<FederationBarkDigest> {
    let description = description.unwrap_or_default();
    if description.len() > FEDERATION_MAX_BARK_DESCRIPTION_BYTES {
        return Err(invalid_bark("Bark receive description exceeds 4 KiB"));
    }
    domain_hash(BARK_DESCRIPTION_DOMAIN, description.as_bytes())
}

/// Commit to the exact notification-only Ark mailbox identifier.
pub fn federation_bark_mailbox_scope(mailbox_id: &[u8; 33]) -> Result<FederationBarkDigest> {
    PublicKey::from_slice(mailbox_id)
        .map_err(|_| invalid_bark("invalid Bark mailbox identifier"))?;
    domain_hash(BARK_MAILBOX_SCOPE_DOMAIN, mailbox_id)
}

/// Commit to the exact accepted receive request sent to the Ark adapter.
pub fn federation_bark_receive_request_hash(
    intent_operation_id: FederationOperationId,
    intent: &FederationBarkReceiveIntent,
) -> Result<FederationBarkDigest> {
    #[derive(Serialize)]
    struct RequestCommitment<'a> {
        intent_operation_id: FederationOperationId,
        intent: &'a FederationBarkReceiveIntent,
    }
    domain_hash(
        BARK_RECEIVE_REQUEST_DOMAIN,
        &canonical_json_bytes(&RequestCommitment {
            intent_operation_id,
            intent,
        })?,
    )
}

fn receive_result_hash(prepared: &FederationBarkReceivePrepared) -> Result<FederationBarkDigest> {
    #[derive(Serialize)]
    struct ResultCommitment<'a> {
        action_id: FederationBarkActionId,
        intent_operation_id: FederationOperationId,
        invoice: &'a str,
        payment_hash: FederationBarkDigest,
        gross_amount: Amount,
        expected_net_amount: Amount,
        network: Network,
        expires_at: u64,
        final_cltv_delta: u32,
        receive_fee: Amount,
        request_hash: FederationBarkDigest,
        external_id: FederationBarkExternalId,
    }
    domain_hash(
        BARK_RECEIVE_RESULT_DOMAIN,
        &canonical_json_bytes(&ResultCommitment {
            action_id: prepared.action_id,
            intent_operation_id: prepared.intent_operation_id,
            invoice: &prepared.invoice,
            payment_hash: prepared.payment_hash,
            gross_amount: prepared.gross_amount,
            expected_net_amount: prepared.expected_net_amount,
            network: prepared.network,
            expires_at: prepared.expires_at,
            final_cltv_delta: prepared.final_cltv_delta,
            receive_fee: prepared.receive_fee,
            request_hash: prepared.request_hash,
            external_id: prepared.external_id,
        })?,
    )
}

fn receive_external_id(
    intent_operation_id: FederationOperationId,
    action_id: FederationBarkActionId,
) -> Result<FederationBarkExternalId> {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(&intent_operation_id.to_bytes());
    transcript.extend_from_slice(&action_id.to_bytes());
    FederationBarkExternalId::from_bytes(
        Sha256Hash::hash(&[BARK_RECEIVE_EXTERNAL_ID_DOMAIN, transcript.as_slice()].concat())
            .to_byte_array(),
    )
}

impl FederationBarkActionPlan {
    fn validate_certified_receive_htlcs(&self) -> Result<()> {
        let Some(certified) = self.certified_receive_htlcs.as_deref() else {
            return if self.kind == FederationBarkActionKind::IncomingClaim {
                Err(invalid_bark(
                    "incoming claim requires certified HTLC evidence",
                ))
            } else {
                Ok(())
            };
        };
        if self.kind != FederationBarkActionKind::IncomingClaim {
            return Err(invalid_bark(
                "only an incoming claim may carry certified HTLC evidence",
            ));
        }
        ensure_version(certified.version)?;
        ensure_count(
            "certified receive HTLC transitions",
            certified.vtxo_transitions.len(),
            false,
        )?;
        ensure_strictly_sorted(
            "certified receive HTLC transitions",
            &certified.vtxo_transitions,
        )?;
        if certified.vtxo_transitions.len() != self.inputs.len() {
            return Err(invalid_bark(
                "certified receive HTLC count differs from claim inputs",
            ));
        }
        let mut certified_amount = Amount::ZERO;
        for (input, transition) in self.inputs.iter().zip(&certified.vtxo_transitions) {
            transition.locator.validate()?;
            if transition.previous_state.is_some()
                || transition.new_state != FederationBarkVtxoState::Observed
                || transition.vtxo_id != input.vtxo_id
                || transition.locator != input.locator
                || transition.amount != input.amount
                || transition.amount == Amount::ZERO
                || transition.expiry_height == 0
                || !self.artifacts.iter().any(|artifact| {
                    artifact.kind == FederationBarkArtifactKind::Vtxo
                        && artifact.hash == transition.artifact_hash
                })
            {
                return Err(invalid_bark(
                    "certified receive HTLC differs from its claim input or artifact",
                ));
            }
            certified_amount = certified_amount
                .checked_add(transition.amount)
                .ok_or_else(|| invalid_bark("certified receive HTLC amount overflow"))?;
        }
        let binding = &certified.certificate.binding;
        if u64::from(certified.minimum_expiry) != self.expires_at {
            return Err(invalid_bark(
                "certified receive HTLC expiry differs from its claim plan",
            ));
        }
        if binding.phase != FederationBarkEvidencePhase::ReceiveHtlcsReady
            || binding.action_id != self.action_id
            || binding.quote != self.quote.as_deref().unwrap_or_default()
            || binding.evidence_operation_id != self.predecessor_operation_id
            || Some(binding.payment_hash) != self.payment_hash
            || binding.status != FederationBarkObservationStatus::Accepted
        {
            return Err(invalid_bark(
                "certified receive HTLC identity differs from its claim plan",
            ));
        }
        if binding.amount != certified_amount || binding.attempt != self.attempt {
            return Err(invalid_bark(
                "certified receive HTLC amount or attempt differs from its claim plan",
            ));
        }
        if binding.evidence_digest
            != federation_bark_receive_htlc_package_hash(
                certified.minimum_expiry,
                certified.request_hash,
                &certified.vtxo_transitions,
                &self.artifacts,
            )?
        {
            return Err(invalid_bark(
                "certified receive HTLC digest differs from its exact artifact package",
            ));
        }
        Ok(())
    }

    /// Decode the exact canonical invoice bound to an outgoing plan.
    pub fn lightning_send_invoice(&self) -> Result<Bolt11Invoice> {
        if self.kind != FederationBarkActionKind::LightningSend {
            return Err(invalid_bark("Bark action is not a Lightning send"));
        }
        let invoice = self
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == FederationBarkArtifactKind::Invoice)
            .ok_or_else(|| invalid_bark("Bark Lightning send invoice artifact is missing"))?;
        let invoice_text = std::str::from_utf8(&invoice.bytes)
            .map_err(|_| invalid_bark("Bark Lightning send invoice is not UTF-8"))?;
        let parsed = invoice_text
            .parse::<Bolt11Invoice>()
            .map_err(|_| invalid_bark("Bark Lightning send invoice is invalid"))?;
        if parsed.to_string() != invoice_text {
            return Err(invalid_bark("Bark Lightning send invoice is not canonical"));
        }
        Ok(parsed)
    }

    /// Bind the exact canonical BOLT11 invoice and Ark request transcript to
    /// an outgoing plan. The request transcript excludes only its own artifact
    /// hash, avoiding a circular commitment.
    pub fn bind_lightning_send_artifacts(&mut self, invoice: &Bolt11Invoice) -> Result<()> {
        if self.kind != FederationBarkActionKind::LightningSend {
            return Err(invalid_bark(
                "only a Lightning send may bind send artifacts",
            ));
        }
        let invoice_bytes = invoice.to_string().into_bytes();
        if invoice_bytes.is_empty() || invoice_bytes.len() > FEDERATION_MAX_BARK_INVOICE_BYTES {
            return Err(invalid_bark("Bark send invoice is empty or oversized"));
        }
        let request_bytes = self.lightning_send_request_bytes()?;
        let request = bark_artifact(FederationBarkArtifactKind::ArkRequest, request_bytes)?;
        let invoice = bark_artifact(FederationBarkArtifactKind::Invoice, invoice_bytes)?;
        let output = self
            .outputs
            .first_mut()
            .ok_or_else(|| invalid_bark("Bark Lightning send has no HTLC/change allocation"))?;
        output.artifact_hash = request.hash;
        self.artifacts = vec![request, invoice];
        self.artifacts.sort();
        Ok(())
    }

    fn lightning_send_request_bytes(&self) -> Result<Vec<u8>> {
        let quote = self
            .quote
            .as_deref()
            .ok_or_else(|| invalid_bark("Bark Lightning send has no quote"))?;
        let payment_hash = self
            .payment_hash
            .ok_or_else(|| invalid_bark("Bark Lightning send has no payment hash"))?;
        let output = self
            .outputs
            .first()
            .filter(|_| self.outputs.len() == 1)
            .ok_or_else(|| invalid_bark("Bark Lightning send must allocate one HTLC/change key"))?;
        let revocation_locator = self
            .revocation_locator
            .ok_or_else(|| invalid_bark("Bark Lightning send has no revocation locator"))?;
        canonical_json_bytes(&FederationBarkLightningSendRequestBinding {
            domain: BARK_SEND_REQUEST_DOMAIN,
            version: self.version,
            action_id: self.action_id,
            quote,
            actor: self.actor,
            predecessor_operation_id: self.predecessor_operation_id,
            payment_hash,
            inputs: &self.inputs,
            output_locator: output.locator,
            output_amount: output.amount,
            revocation_locator,
            external_amount: self.external_amount,
            fee_amount: self.fee_amount,
            fee_bound: self.fee_bound,
            external_id: self.external_id,
            signer_set: &self.signer_set,
            attempt: self.attempt,
            expires_at: self.expires_at,
        })
    }

    fn validate_lightning_send_artifacts(&self) -> Result<()> {
        if self.artifacts.len() != 2 {
            return Err(invalid_bark(
                "Bark Lightning send must bind one request and one invoice artifact",
            ));
        }
        let request = self
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == FederationBarkArtifactKind::ArkRequest)
            .ok_or_else(|| invalid_bark("Bark Lightning send request artifact is missing"))?;
        let invoice = self.lightning_send_invoice()?;
        if request.bytes != self.lightning_send_request_bytes()?
            || self.outputs[0].artifact_hash != request.hash
        {
            return Err(invalid_bark(
                "Bark Lightning send request artifact differs from its plan",
            ));
        }
        invoice
            .check_signature()
            .map_err(|_| invalid_bark("Bark Lightning send invoice signature is invalid"))?;
        let expected_msat = u64::from(self.external_amount)
            .checked_mul(1_000)
            .ok_or_else(|| invalid_bark("Bark Lightning send amount overflow"))?;
        if invoice.payment_hash().to_byte_array()
            != self
                .payment_hash
                .ok_or_else(|| invalid_bark("Bark Lightning send has no payment hash"))?
                .to_bytes()
            || invoice.amount_milli_satoshis() != Some(expected_msat)
        {
            return Err(invalid_bark(
                "Bark Lightning send invoice differs from its payment bindings",
            ));
        }
        Ok(())
    }

    /// Bind the exact round request and immutable input VTXOs to a refresh
    /// plan. The output allocation points at the request commitment.
    pub fn bind_refresh_artifacts(
        &mut self,
        trigger_height: u32,
        minimum_expiry: u32,
        input_vtxos: impl IntoIterator<Item = FederationBarkArtifact>,
    ) -> Result<()> {
        if self.kind != FederationBarkActionKind::Refresh {
            return Err(invalid_bark("only a refresh may bind a round request"));
        }
        let output = self
            .outputs
            .first()
            .filter(|_| self.outputs.len() == 1)
            .ok_or_else(|| invalid_bark("Bark refresh must allocate one output"))?;
        let emergency_height = u32::try_from(self.expires_at)
            .map_err(|_| invalid_bark("Bark refresh deadline exceeds u32"))?;
        let request = FederationBarkRefreshRequest {
            domain: BARK_REFRESH_REQUEST_DOMAIN.to_string(),
            version: self.version,
            action_id: self.action_id,
            predecessor_operation_id: self.predecessor_operation_id,
            inputs: self.inputs.clone(),
            output_locator: output.locator,
            output_amount: output.amount,
            fee: self.fee_amount,
            trigger_height,
            emergency_height,
            minimum_expiry,
            external_id: self.external_id,
            attempt: self.attempt,
        };
        request.validate()?;
        let request = bark_artifact(
            FederationBarkArtifactKind::ArkRequest,
            canonical_json_bytes(&request)?,
        )?;
        self.outputs[0].artifact_hash = request.hash;
        self.artifacts = input_vtxos.into_iter().collect();
        self.artifacts.push(request);
        self.artifacts.sort();
        self.artifacts.dedup();
        Ok(())
    }

    /// Decode and canonically verify the exact refresh request artifact.
    pub fn refresh_request(&self) -> Result<FederationBarkRefreshRequest> {
        if self.kind != FederationBarkActionKind::Refresh {
            return Err(invalid_bark("Bark action is not a refresh"));
        }
        let artifact = self
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == FederationBarkArtifactKind::ArkRequest)
            .ok_or_else(|| invalid_bark("Bark refresh request artifact is missing"))?;
        let request = serde_json::from_slice::<FederationBarkRefreshRequest>(&artifact.bytes)
            .map_err(|_| invalid_bark("Bark refresh request artifact is invalid"))?;
        if canonical_json_bytes(&request)? != artifact.bytes {
            return Err(invalid_bark("Bark refresh request is not canonical"));
        }
        request.validate()?;
        Ok(request)
    }

    /// Bind the exact Bark recovery allocation, final-drain policy,
    /// immutable input VTXOs, and precomputed exit transaction packages.
    #[allow(clippy::too_many_arguments)]
    pub fn bind_emergency_exit_artifacts(
        &mut self,
        destination: FederationAllocatedAddressRef,
        destination_script: Vec<u8>,
        network: Network,
        fee_rate_sat_per_kwu: u64,
        trigger_height: u32,
        minimum_confirmations: u32,
        input_vtxos: impl IntoIterator<Item = FederationBarkArtifact>,
        exit_transactions: impl IntoIterator<Item = Vec<u8>>,
    ) -> Result<()> {
        if self.kind != FederationBarkActionKind::EmergencyExit {
            return Err(invalid_bark(
                "only an emergency exit may bind a recovery destination",
            ));
        }
        let minimum_expiry = u32::try_from(self.expires_at)
            .map_err(|_| invalid_bark("Bark exit expiry exceeds u32"))?;
        let request = FederationBarkExitRequest {
            domain: BARK_EXIT_REQUEST_DOMAIN.to_string(),
            version: self.version,
            action_id: self.action_id,
            predecessor_operation_id: self.predecessor_operation_id,
            inputs: self.inputs.clone(),
            destination,
            destination_script,
            network,
            fee_rate_sat_per_kwu,
            fee_bound: self.fee_bound,
            minimum_recovered: self.external_amount,
            trigger_height,
            minimum_expiry,
            minimum_confirmations,
            external_id: self.external_id,
            attempt: self.attempt,
        };
        request.validate()?;
        let request = bark_artifact(
            FederationBarkArtifactKind::ArkRequest,
            canonical_json_bytes(&request)?,
        )?;
        self.artifacts = input_vtxos.into_iter().collect();
        self.artifacts.push(request);
        for transaction in exit_transactions {
            self.artifacts.push(bark_artifact(
                FederationBarkArtifactKind::Transaction,
                transaction,
            )?);
        }
        self.artifacts.sort();
        self.artifacts.dedup();
        Ok(())
    }

    /// Decode and canonically verify the exact emergency-exit request.
    pub fn emergency_exit_request(&self) -> Result<FederationBarkExitRequest> {
        if self.kind != FederationBarkActionKind::EmergencyExit {
            return Err(invalid_bark("Bark action is not an emergency exit"));
        }
        let artifact = self
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == FederationBarkArtifactKind::ArkRequest)
            .ok_or_else(|| invalid_bark("Bark exit request artifact is missing"))?;
        let request = serde_json::from_slice::<FederationBarkExitRequest>(&artifact.bytes)
            .map_err(|_| invalid_bark("Bark exit request artifact is invalid"))?;
        if canonical_json_bytes(&request)? != artifact.bytes {
            return Err(invalid_bark("Bark exit request is not canonical"));
        }
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<()> {
        ensure_version(self.version)?;
        ensure_optional_quote(self.quote.as_deref())?;
        ensure_count("Bark inputs", self.inputs.len(), false)?;
        ensure_count(
            "Bark outputs",
            self.outputs.len(),
            self.kind == FederationBarkActionKind::EmergencyExit,
        )?;
        ensure_count("Bark signer set", self.signer_set.len(), false)?;
        ensure_count("Bark artifacts", self.artifacts.len(), true)?;
        ensure_strictly_sorted("Bark inputs", &self.inputs)?;
        ensure_strictly_sorted("Bark outputs", &self.outputs)?;
        ensure_strictly_sorted("Bark artifacts", &self.artifacts)?;
        ensure_members_sorted_unique(&self.signer_set)?;
        for input in &self.inputs {
            input.locator.validate()?;
        }
        if self
            .inputs
            .windows(2)
            .any(|pair| pair[0].vtxo_id == pair[1].vtxo_id)
        {
            return Err(invalid_bark("duplicate Bark action input VTXO"));
        }
        for output in &self.outputs {
            output.locator.validate()?;
            if output.amount == Amount::ZERO && self.kind != FederationBarkActionKind::LightningSend
            {
                return Err(invalid_bark("Bark action output must be non-zero"));
            }
            if !self
                .artifacts
                .iter()
                .any(|artifact| artifact.hash == output.artifact_hash)
            {
                return Err(invalid_bark(
                    "Bark action output does not reference a canonical artifact",
                ));
            }
        }
        if self.inputs.iter().any(|input| input.amount == Amount::ZERO) {
            return Err(invalid_bark("Bark action input must be non-zero"));
        }
        validate_artifacts(&self.artifacts)?;
        self.validate_certified_receive_htlcs()?;
        if self.attempt == 0 || self.expires_at == 0 || self.fee_amount > self.fee_bound {
            return Err(invalid_bark("invalid Bark attempt, expiry, or fee bound"));
        }
        let quote_and_payment_required = matches!(
            self.kind,
            FederationBarkActionKind::LightningSend
                | FederationBarkActionKind::IncomingClaim
                | FederationBarkActionKind::FailedSendRevocation
        );
        if quote_and_payment_required && (self.quote.is_none() || self.payment_hash.is_none()) {
            return Err(invalid_bark(
                "payment action is missing its Cashu quote or payment hash",
            ));
        }
        if matches!(self.kind, FederationBarkActionKind::LightningSend) {
            let revocation_locator = self
                .revocation_locator
                .ok_or_else(|| invalid_bark("Lightning send has no revocation locator"))?;
            revocation_locator.validate()?;
            if self.external_amount == Amount::ZERO
                || self.outputs.len() != 1
                || self.outputs[0].locator == revocation_locator
                || self.inputs.iter().any(|input| {
                    input.locator == self.outputs[0].locator || input.locator == revocation_locator
                })
            {
                return Err(invalid_bark(
                    "Lightning send amount or key allocations are invalid",
                ));
            }
            if self.external_id
                != FederationBarkExternalId::derive_lightning_send(
                    self.action_id,
                    self.predecessor_operation_id,
                    self.attempt,
                )?
            {
                return Err(invalid_bark(
                    "Lightning send external id differs from its accepted action",
                ));
            }
            self.validate_lightning_send_artifacts()?;
        } else if matches!(self.kind, FederationBarkActionKind::FailedSendRevocation) {
            if self.outputs.len() != 1
                || self.external_amount != Amount::ZERO
                || self.fee_amount != Amount::ZERO
                || self.fee_bound != Amount::ZERO
                || self.revocation_locator.is_some()
                || self
                    .inputs
                    .iter()
                    .any(|input| input.locator == self.outputs[0].locator)
                || self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkRequest)
                    .count()
                    != 1
                || self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
                    .count()
                    < self.inputs.len()
                || self.external_id
                    != FederationBarkExternalId::derive_failed_send_revocation(
                        self.action_id,
                        self.predecessor_operation_id,
                        self.attempt,
                    )?
            {
                return Err(invalid_bark(
                    "failed-send revocation must bind exact HTLC inputs and one recovery output",
                ));
            }
        } else if !matches!(self.kind, FederationBarkActionKind::EmergencyExit)
            && (self.external_amount != Amount::ZERO || self.revocation_locator.is_some())
        {
            return Err(invalid_bark(
                "only Lightning send may move value outside Bark outputs or reserve a revocation key",
            ));
        }
        if matches!(self.kind, FederationBarkActionKind::IncomingClaim)
            && (self.outputs.len() != 1
                || self.fee_amount != Amount::ZERO
                || self.fee_bound != Amount::ZERO
                || self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
                    .count()
                    < self.inputs.len())
        {
            return Err(invalid_bark(
                "incoming claim must bind every input, one output allocation, and zero claim fee",
            ));
        }
        if self.kind == FederationBarkActionKind::Refresh {
            let request = self.refresh_request()?;
            let request_artifact = self
                .artifacts
                .iter()
                .find(|artifact| artifact.kind == FederationBarkArtifactKind::ArkRequest)
                .ok_or_else(|| invalid_bark("refresh request artifact is missing"))?;
            let output = self
                .outputs
                .first()
                .filter(|_| self.outputs.len() == 1)
                .ok_or_else(|| invalid_bark("refresh must allocate one output"))?;
            if self.quote.is_some()
                || self.payment_hash.is_some()
                || self.revocation_locator.is_some()
                || self.external_amount != Amount::ZERO
                || request.action_id != self.action_id
                || request.predecessor_operation_id != self.predecessor_operation_id
                || request.inputs != self.inputs
                || request.output_locator != output.locator
                || request.output_amount != output.amount
                || request.fee != self.fee_amount
                || request.emergency_height != u32::try_from(self.expires_at).unwrap_or(u32::MAX)
                || request.external_id != self.external_id
                || request.attempt != self.attempt
                || output.artifact_hash != request_artifact.hash
                || self.external_id
                    != FederationBarkExternalId::derive_refresh(
                        self.action_id,
                        self.predecessor_operation_id,
                        self.attempt,
                    )?
                || self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
                    .count()
                    < self.inputs.len()
            {
                return Err(invalid_bark(
                    "refresh must bind exact inputs, timing, fee, and one round output",
                ));
            }
        }
        if self.kind == FederationBarkActionKind::EmergencyExit {
            let request = self.emergency_exit_request()?;
            if self.quote.is_some()
                || self.payment_hash.is_some()
                || self.revocation_locator.is_some()
                || !self.outputs.is_empty()
                || self.external_amount == Amount::ZERO
                || self.fee_amount != self.fee_bound
                || request.action_id != self.action_id
                || request.predecessor_operation_id != self.predecessor_operation_id
                || request.inputs != self.inputs
                || request.fee_bound != self.fee_bound
                || request.minimum_recovered != self.external_amount
                || request.minimum_expiry != u32::try_from(self.expires_at).unwrap_or(u32::MAX)
                || request.external_id != self.external_id
                || request.attempt != self.attempt
                || self.external_id
                    != FederationBarkExternalId::derive_emergency_exit(
                        self.action_id,
                        self.predecessor_operation_id,
                        self.attempt,
                    )?
                || self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
                    .count()
                    < self.inputs.len()
                || self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Transaction)
                    .count()
                    == 0
            {
                return Err(invalid_bark(
                    "emergency exit must bind exact inputs, BDK destination, transactions, and fee policy",
                ));
            }
        }
        let inputs = checked_sum(self.inputs.iter().map(|input| input.amount))?;
        let outputs = checked_sum(self.outputs.iter().map(|output| output.amount))?;
        let accounted = outputs
            .checked_add(self.external_amount)
            .and_then(|amount| amount.checked_add(self.fee_amount))
            .ok_or_else(|| invalid_bark("Bark action accounting overflow"))?;
        if inputs != accounted {
            return Err(invalid_bark(
                "Bark action inputs do not equal outputs, payment, and fee",
            ));
        }
        Ok(())
    }
}

/// Return whether an accepted refresh cannot fit the fixed FROST
/// authorization budget and must retain its reservations for emergency exit.
///
/// This is deliberately separate from historical operation validation so a
/// previously accepted oversized plan remains replayable and recoverable.
#[must_use]
pub fn federation_bark_refresh_requires_recovery(plan: &FederationBarkActionPlan) -> bool {
    if plan.kind != FederationBarkActionKind::Refresh
        || plan.inputs.len() > FEDERATION_MAX_BARK_REFRESH_INPUTS
    {
        return true;
    }
    let mut input_artifact_count = 0_usize;
    let mut input_artifact_bytes = 0_usize;
    for artifact in &plan.artifacts {
        if artifact.kind != FederationBarkArtifactKind::Vtxo {
            continue;
        }
        input_artifact_count = input_artifact_count.saturating_add(1);
        let Some(next) = input_artifact_bytes.checked_add(artifact.bytes.len()) else {
            return true;
        };
        input_artifact_bytes = next;
    }
    input_artifact_count != plan.inputs.len()
        || input_artifact_bytes > FEDERATION_MAX_BARK_REFRESH_INPUT_ARTIFACT_BYTES
}

impl FederationBarkRefreshRequest {
    fn validate(&self) -> Result<()> {
        ensure_version(self.version)?;
        if self.domain != BARK_REFRESH_REQUEST_DOMAIN {
            return Err(invalid_bark("Bark refresh request domain differs"));
        }
        ensure_count("Bark refresh inputs", self.inputs.len(), false)?;
        ensure_strictly_sorted("Bark refresh inputs", &self.inputs)?;
        self.output_locator.validate()?;
        if self.inputs.iter().any(|input| input.amount == Amount::ZERO)
            || self
                .inputs
                .iter()
                .any(|input| input.locator == self.output_locator)
            || self.output_amount == Amount::ZERO
            || self.attempt == 0
            || self.trigger_height == 0
            || self.trigger_height >= self.emergency_height
            || self.emergency_height >= self.minimum_expiry
        {
            return Err(invalid_bark("Bark refresh request is unsafe or incomplete"));
        }
        let input_amount = checked_sum(self.inputs.iter().map(|input| input.amount))?;
        if input_amount.checked_sub(self.fee) != Some(self.output_amount)
            || self.external_id
                != FederationBarkExternalId::derive_refresh(
                    self.action_id,
                    self.predecessor_operation_id,
                    self.attempt,
                )?
        {
            return Err(invalid_bark(
                "Bark refresh request amount or external id differs",
            ));
        }
        Ok(())
    }
}

impl FederationBarkExitRequest {
    fn validate(&self) -> Result<()> {
        ensure_version(self.version)?;
        if self.domain != BARK_EXIT_REQUEST_DOMAIN {
            return Err(invalid_bark("Bark exit request domain differs"));
        }
        ensure_count("Bark exit inputs", self.inputs.len(), false)?;
        ensure_strictly_sorted("Bark exit inputs", &self.inputs)?;
        ensure_quote(&self.destination.quote)?;
        let destination_script = bitcoin::ScriptBuf::from_bytes(self.destination_script.clone());
        if self.destination.application_key_epoch == 0
            || self.destination.quote != federation_bark_exit_allocation_quote(self.action_id)
            || self.destination.derivation_index
                > cdk_common::federation::FEDERATION_MAX_DERIVATION_INDEX
            || !destination_script.is_p2tr()
            || self.inputs.iter().any(|input| input.amount == Amount::ZERO)
            || self.fee_rate_sat_per_kwu == 0
            || self.fee_bound == Amount::ZERO
            || self.minimum_recovered == Amount::ZERO
            || self.minimum_confirmations == 0
            || self.attempt == 0
            || self.trigger_height == 0
            || self.trigger_height >= self.minimum_expiry
        {
            return Err(invalid_bark("Bark exit request is unsafe or incomplete"));
        }
        let input_amount = checked_sum(self.inputs.iter().map(|input| input.amount))?;
        if input_amount.checked_sub(self.fee_bound) != Some(self.minimum_recovered)
            || self.external_id
                != FederationBarkExternalId::derive_emergency_exit(
                    self.action_id,
                    self.predecessor_operation_id,
                    self.attempt,
                )?
        {
            return Err(invalid_bark(
                "Bark exit request amount or external id differs",
            ));
        }
        Ok(())
    }
}

impl FederationBarkActionTransition {
    /// Construct the sole canonical transition that authorizes the next
    /// deterministic custody signer attempt for an unchanged action plan.
    pub fn signing_attempt_advanced(
        plan_operation_id: FederationOperationId,
        predecessor_operation_id: FederationOperationId,
        plan: &FederationBarkActionPlan,
        actor: MemberId,
        attempt: u32,
        signer_set: Vec<MemberId>,
    ) -> Result<Self> {
        if attempt <= plan.attempt {
            return Err(invalid_bark("Bark signing attempt did not advance"));
        }
        ensure_count("Bark signer set", signer_set.len(), false)?;
        ensure_members_sorted_unique(&signer_set)?;
        let plan_operation = FederationBarkOperation::ActionPlan(plan.clone());
        let request_hash = plan_operation.commitment()?;
        let result_binding = FederationBarkSigningAttemptResultBinding {
            domain: "cdk/federation/bark-signing-attempt-result/v1",
            action_id: plan.action_id,
            plan_operation_id,
            predecessor_operation_id,
            actor,
            attempt,
            signer_set: &signer_set,
            request_hash,
            external_id: plan.external_id,
        };
        let result_hash = domain_hash(
            BARK_SIGNING_ATTEMPT_RESULT_DOMAIN,
            &canonical_json_bytes(&result_binding)?,
        )?;
        Ok(Self {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: plan.quote.clone(),
            phase: FederationBarkSemanticPhase::SigningAttemptAdvanced,
            actor,
            plan_operation_id,
            predecessor_operation_id,
            payment_hash: plan.payment_hash,
            attempt,
            signer_set,
            request_hash,
            result_hash,
            external_id: plan.external_id,
            artifacts: Vec::new(),
            vtxo_transitions: Vec::new(),
            evidence_certificate: None,
        })
    }

    fn validate(&self) -> Result<()> {
        ensure_version(self.version)?;
        ensure_optional_quote(self.quote.as_deref())?;
        if matches!(
            self.phase,
            FederationBarkSemanticPhase::ReceiveIntent
                | FederationBarkSemanticPhase::ReceivePrepared
                | FederationBarkSemanticPhase::SendPlanned
                | FederationBarkSemanticPhase::ClaimPlanned
                | FederationBarkSemanticPhase::RevocationPlanned
                | FederationBarkSemanticPhase::RefreshPlanned
                | FederationBarkSemanticPhase::ExitPlanned
                | FederationBarkSemanticPhase::Observation
        ) {
            return Err(invalid_bark("phase is not a Bark action transition"));
        }
        if self.attempt == 0 {
            return Err(invalid_bark("Bark transition attempt must be non-zero"));
        }
        ensure_count("Bark signer set", self.signer_set.len(), true)?;
        ensure_count("Bark artifacts", self.artifacts.len(), true)?;
        ensure_count("Bark VTXO transitions", self.vtxo_transitions.len(), true)?;
        ensure_members_sorted_unique(&self.signer_set)?;
        ensure_strictly_sorted("Bark artifacts", &self.artifacts)?;
        ensure_strictly_sorted("Bark VTXO transitions", &self.vtxo_transitions)?;
        validate_artifacts(&self.artifacts)?;
        match &self.evidence_certificate {
            Some(certificate)
                if self.phase == FederationBarkSemanticPhase::ReceiveHtlcsReady
                    && certificate.binding.phase
                        == FederationBarkEvidencePhase::ReceiveHtlcsReady
                    && certificate.binding.action_id == self.action_id
                    && certificate.binding.quote == self.quote.as_deref().unwrap_or_default()
                    && Some(certificate.binding.payment_hash) == self.payment_hash
                    && certificate.binding.evidence_operation_id
                        == self.predecessor_operation_id
                    && certificate.binding.attempt == self.attempt
                    && certificate.binding.evidence_digest == self.result_hash =>
            {
                let certified_amount = checked_sum(
                    self.vtxo_transitions
                        .iter()
                        .filter(|transition| transition.previous_state.is_none())
                        .map(|transition| transition.amount),
                )?;
                if certified_amount != certificate.binding.amount {
                    return Err(invalid_bark(
                        "Bark HTLC certificate amount differs from its artifacts",
                    ));
                }
            }
            Some(certificate)
                if self.phase == FederationBarkSemanticPhase::PaymentSucceeded
                    && certificate.binding.phase == FederationBarkEvidencePhase::SendSucceeded
                    && certificate.binding.action_id == self.action_id
                    && certificate.binding.quote == self.quote.as_deref().unwrap_or_default()
                    && Some(certificate.binding.payment_hash) == self.payment_hash
                    && certificate.binding.evidence_operation_id
                        == self.predecessor_operation_id
                    && certificate.binding.attempt == self.attempt
                    && certificate.binding.evidence_digest == self.result_hash =>
            {
                let certified_total = certificate
                    .binding
                    .amount
                    .checked_add(certificate.binding.fee)
                    .ok_or_else(|| invalid_bark("Bark send certificate amount overflow"))?;
                let spent_amount = checked_sum(
                    self.vtxo_transitions
                        .iter()
                        .filter(|transition| {
                            transition.previous_state == Some(FederationBarkVtxoState::PendingSpend)
                                && transition.new_state == FederationBarkVtxoState::Spent
                        })
                        .map(|transition| transition.amount),
                )?;
                if spent_amount != certified_total {
                    return Err(invalid_bark(
                        "Bark send certificate amount differs from its spent value",
                    ));
                }
            }
            Some(_) => {
                return Err(invalid_bark(
                    "Bark evidence certificate does not match its transition",
                ));
            }
            None => {}
        }
        if self.phase == FederationBarkSemanticPhase::SigningAttemptAdvanced
            && (!self.artifacts.is_empty() || !self.vtxo_transitions.is_empty())
        {
            return Err(invalid_bark(
                "Bark signing-attempt transition cannot carry result artifacts",
            ));
        }
        for transition in &self.vtxo_transitions {
            transition.locator.validate()?;
            if transition.amount == Amount::ZERO || transition.expiry_height == 0 {
                return Err(invalid_bark(
                    "Bark VTXO transition amount and expiry must be non-zero",
                ));
            }
            let artifact_matches = self.artifacts.iter().any(|artifact| {
                artifact.kind == FederationBarkArtifactKind::Vtxo
                    && artifact.hash == transition.artifact_hash
            });
            if !artifact_matches {
                return Err(invalid_bark(
                    "Bark VTXO transition does not reference a canonical VTXO artifact",
                ));
            }
            if !valid_vtxo_state_transition(transition.previous_state, transition.new_state) {
                return Err(invalid_bark("invalid Bark VTXO state transition"));
            }
        }
        match self.phase {
            FederationBarkSemanticPhase::ReceiveHtlcsReady
                if self.vtxo_transitions.is_empty()
                    || self.vtxo_transitions.iter().any(|transition| {
                        transition.previous_state.is_some()
                            || transition.new_state != FederationBarkVtxoState::Observed
                    }) =>
            {
                return Err(invalid_bark(
                    "HTLC-ready evidence must introduce observed, unclaimed VTXOs",
                ));
            }
            FederationBarkSemanticPhase::ReceivePreimageDisclosed
                if self.vtxo_transitions.is_empty()
                    || self.vtxo_transitions.iter().any(|transition| {
                        transition.previous_state != Some(FederationBarkVtxoState::Reserved)
                            || transition.new_state != FederationBarkVtxoState::PendingSpend
                    }) =>
            {
                return Err(invalid_bark(
                    "preimage disclosure must leave every claim input pending spend",
                ));
            }
            FederationBarkSemanticPhase::ReceiveClaimed => {
                let has_spent_input = self.vtxo_transitions.iter().any(|transition| {
                    transition.previous_state == Some(FederationBarkVtxoState::PendingSpend)
                        && transition.new_state == FederationBarkVtxoState::Spent
                });
                let has_spendable_output = self.vtxo_transitions.iter().any(|transition| {
                    transition.previous_state.is_none()
                        && transition.new_state == FederationBarkVtxoState::Spendable
                });
                if !has_spent_input || !has_spendable_output {
                    return Err(invalid_bark(
                        "claimed receive must spend pending HTLCs and create spendable outputs",
                    ));
                }
            }
            FederationBarkSemanticPhase::SendHtlcsCosigned => {
                let has_spent_input = self.vtxo_transitions.iter().any(|transition| {
                    transition.previous_state == Some(FederationBarkVtxoState::Reserved)
                        && transition.new_state == FederationBarkVtxoState::Spent
                });
                let has_pending_htlc = self.vtxo_transitions.iter().any(|transition| {
                    transition.previous_state.is_none()
                        && transition.new_state == FederationBarkVtxoState::PendingSpend
                });
                let valid_shape = self.vtxo_transitions.iter().all(|transition| {
                    (transition.previous_state == Some(FederationBarkVtxoState::Reserved)
                        && transition.new_state == FederationBarkVtxoState::Spent)
                        || (transition.previous_state.is_none()
                            && transition.new_state == FederationBarkVtxoState::PendingSpend)
                        || (transition.previous_state.is_none()
                            && transition.new_state == FederationBarkVtxoState::Spendable)
                });
                let result_count = self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
                    .count();
                if !has_spent_input || !has_pending_htlc || !valid_shape || result_count != 1 {
                    return Err(invalid_bark(
                        "outgoing HTLC result must spend inputs, retain HTLC value, and bind one Ark result",
                    ));
                }
            }
            FederationBarkSemanticPhase::PaymentSucceeded => {
                let valid_spends = !self.vtxo_transitions.is_empty()
                    && self.vtxo_transitions.iter().all(|transition| {
                        transition.previous_state == Some(FederationBarkVtxoState::PendingSpend)
                            && transition.new_state == FederationBarkVtxoState::Spent
                    });
                let proofs = self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Proof)
                    .collect::<Vec<_>>();
                if !valid_spends || proofs.len() != 1 || proofs[0].bytes.len() != 32 {
                    return Err(invalid_bark(
                        "payment success must spend pending inputs and bind one 32-byte proof",
                    ));
                }
            }
            FederationBarkSemanticPhase::PaymentFailed
                if !self.vtxo_transitions.is_empty()
                    || self
                        .artifacts
                        .iter()
                        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
                        .count()
                        != 1 =>
            {
                return Err(invalid_bark(
                    "payment failure must retain HTLC value and bind one exact result",
                ));
            }
            FederationBarkSemanticPhase::RevocationCompleted => {
                let has_spent_input = self.vtxo_transitions.iter().any(|transition| {
                    transition.previous_state == Some(FederationBarkVtxoState::Reserved)
                        && transition.new_state == FederationBarkVtxoState::Spent
                });
                let has_spendable_output = self.vtxo_transitions.iter().any(|transition| {
                    transition.previous_state.is_none()
                        && transition.new_state == FederationBarkVtxoState::Spendable
                });
                let valid_shape = self.vtxo_transitions.iter().all(|transition| {
                    (transition.previous_state == Some(FederationBarkVtxoState::Reserved)
                        && transition.new_state == FederationBarkVtxoState::Spent)
                        || (transition.previous_state.is_none()
                            && transition.new_state == FederationBarkVtxoState::Spendable)
                });
                let result_count = self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
                    .count();
                if !has_spent_input || !has_spendable_output || !valid_shape || result_count != 1 {
                    return Err(invalid_bark(
                        "revocation must spend exact HTLCs and restore spendable value",
                    ));
                }
            }
            FederationBarkSemanticPhase::RefreshCompleted => {
                let has_spent_input = self.vtxo_transitions.iter().any(|transition| {
                    transition.previous_state == Some(FederationBarkVtxoState::Reserved)
                        && transition.new_state == FederationBarkVtxoState::Spent
                });
                let has_spendable_output = self.vtxo_transitions.iter().any(|transition| {
                    transition.previous_state.is_none()
                        && transition.new_state == FederationBarkVtxoState::Spendable
                });
                let valid_shape = self.vtxo_transitions.iter().all(|transition| {
                    (transition.previous_state == Some(FederationBarkVtxoState::Reserved)
                        && transition.new_state == FederationBarkVtxoState::Spent)
                        || (transition.previous_state.is_none()
                            && transition.new_state == FederationBarkVtxoState::Spendable)
                });
                let result_count = self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
                    .count();
                if !has_spent_input || !has_spendable_output || !valid_shape || result_count != 1 {
                    return Err(invalid_bark(
                        "refresh must spend reserved inputs, create spendable outputs, and bind one Ark result",
                    ));
                }
            }
            FederationBarkSemanticPhase::ExitRegistered => {
                let valid_shape = !self.vtxo_transitions.is_empty()
                    && self.vtxo_transitions.iter().all(|transition| {
                        transition.previous_state == Some(FederationBarkVtxoState::Reserved)
                            && transition.new_state == FederationBarkVtxoState::Exiting
                    });
                let transaction_count = self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Transaction)
                    .count();
                let result_count = self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
                    .count();
                let recovery_packages = self
                    .artifacts
                    .iter()
                    .filter_map(|artifact| {
                        crate::recovery::FederationBarkCpfpPackageSet::from_artifact(artifact).ok()
                    })
                    .collect::<Vec<_>>();
                let recovery_is_bound = match recovery_packages.as_slice() {
                    [packages] => {
                        packages.action_id == self.action_id
                            && packages.plan_operation_id == self.plan_operation_id
                            && packages.actor == self.actor
                            && packages.attempt == self.attempt
                            && packages.signer_set == self.signer_set
                            && packages.packages.iter().all(|package| {
                                self.artifacts.iter().any(|artifact| {
                                    artifact.kind == FederationBarkArtifactKind::Transaction
                                        && artifact.bytes
                                            == bitcoin::consensus::serialize(&package.parent)
                                })
                            })
                    }
                    _ => false,
                };
                if !valid_shape || transaction_count == 0 || result_count != 1 || !recovery_is_bound
                {
                    return Err(invalid_bark(
                        "exit registration must move reserved inputs to exiting and bind exact transactions and recovery packages",
                    ));
                }
            }
            FederationBarkSemanticPhase::ExitPublished
                if !self.vtxo_transitions.is_empty()
                    || self
                        .artifacts
                        .iter()
                        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Transaction)
                        .count()
                        == 0
                    || self
                        .artifacts
                        .iter()
                        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
                        .count()
                        != 1
                    || self
                        .artifacts
                        .iter()
                        .filter_map(|artifact| {
                            crate::recovery::FederationBarkCpfpPublication::from_artifact(artifact)
                                .ok()
                        })
                        .count()
                        != 1 =>
            {
                return Err(invalid_bark(
                    "exit publication must retain VTXO state and bind exact transaction and CPFP evidence",
                ));
            }
            FederationBarkSemanticPhase::ExitConfirmed => {
                let valid_shape = !self.vtxo_transitions.is_empty()
                    && self.vtxo_transitions.iter().all(|transition| {
                        transition.previous_state == Some(FederationBarkVtxoState::Exiting)
                            && transition.new_state == FederationBarkVtxoState::Exited
                    });
                let transaction_count = self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Transaction)
                    .count();
                let result_count = self
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
                    .count();
                if !valid_shape || transaction_count == 0 || result_count != 1 {
                    return Err(invalid_bark(
                        "exit confirmation must finalize every exiting input with exact chain evidence",
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn valid_vtxo_state_transition(
    previous: Option<FederationBarkVtxoState>,
    new: FederationBarkVtxoState,
) -> bool {
    use FederationBarkVtxoState::{
        Exited, Exiting, Observed, PendingSpend, Reserved, Spendable, Spent,
    };

    matches!(
        (previous, new),
        (None, Observed | PendingSpend | Spendable)
            | (Some(Observed), Reserved | Spendable | Exiting)
            | (Some(Reserved), PendingSpend | Spent | Spendable | Exiting)
            | (Some(PendingSpend), Spent | Spendable | Exiting)
            | (Some(Exiting), Exited)
    )
}

impl FederationBarkMemberObservation {
    fn validate(&self) -> Result<()> {
        ensure_version(self.version)?;
        ensure_optional_quote(self.quote.as_deref())?;
        if self.observation_epoch == 0 {
            return Err(invalid_bark("Bark observation epoch must be non-zero"));
        }
        Ok(())
    }
}

/// Custody protocol authorized by an accepted Bark operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationBarkCustodyProtocol {
    /// Plain BIP340 threshold signature.
    Frost,
    /// Federation-produced virtual user MuSig2 partial signature.
    Musig2,
    /// Threshold Diffie-Hellman mailbox blinding contribution.
    ThresholdDh,
}

/// Complete typed reference authorizing one Bark custody protocol request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkAuthorizationReference {
    /// Authorization format.
    pub version: u16,
    /// Owning federation.
    pub federation_id: FederationId,
    /// Registered Bark application.
    pub application_id: FederationFrostApplicationId,
    /// Referenced reusable-root epoch.
    pub root_key_epoch: u64,
    /// Bark application key epoch.
    pub application_key_epoch: u64,
    /// Bark application derivation contract version.
    pub application_derivation_version: u16,
    /// Accepted operation authorizing this request.
    pub accepted_operation_id: FederationOperationId,
    /// Accepted journal position.
    pub accepted_operation_index: u64,
    /// Stable economic action lineage.
    pub action_id: FederationBarkActionId,
    /// Authorized semantic phase.
    pub phase: FederationBarkSemanticPhase,
    /// Object ordinal within the accepted operation.
    pub object_ordinal: u32,
    /// Family-level accepted operation kind.
    pub operation_kind: FederationBarkOperationKind,
    /// Exact accepted plan or result commitment.
    pub plan_or_result_hash: FederationBarkDigest,
    /// Exact VTXO custody locator.
    pub locator: FederationBarkVtxoLocator,
    /// Disjoint custody protocol namespace.
    pub protocol: FederationBarkCustodyProtocol,
    /// Exact message or protocol transcript commitment.
    pub transcript_hash: FederationBarkDigest,
    /// Accepted signer subset, sorted by member.
    pub signer_set: Vec<MemberId>,
    /// Accepted protocol attempt.
    pub attempt: u32,
    /// Immutable Bitcoin network.
    pub network: Network,
    /// Exact compressed public key of the pinned Ark server.
    #[serde(with = "hex_array_33")]
    pub ark_server_public_key: [u8; 33],
}

impl FederationBarkAuthorizationReference {
    /// Validate stable fields, ordering, and the 16 KiB authorization limit.
    pub fn validate(&self) -> Result<()> {
        if self.version != FEDERATION_BARK_AUTHORIZATION_VERSION
            || self.root_key_epoch == 0
            || self.application_key_epoch == 0
            || self.application_derivation_version == 0
            || self.attempt == 0
        {
            return Err(invalid_bark(
                "unsupported Bark authorization version or epoch",
            ));
        }
        self.locator.validate()?;
        PublicKey::from_slice(&self.ark_server_public_key)
            .map_err(|_| invalid_bark("Bark authorization Ark server key is invalid"))?;
        ensure_count(
            "Bark authorization signer set",
            self.signer_set.len(),
            false,
        )?;
        ensure_members_sorted_unique(&self.signer_set)?;
        if !authorization_kind_matches_phase(self.operation_kind, self.phase) {
            return Err(invalid_bark(
                "Bark authorization operation kind and phase are incompatible",
            ));
        }
        let actual = self.canonical_bytes()?.len();
        if actual > FEDERATION_MAX_BARK_AUTHORIZATION_BYTES {
            return Err(invalid_bark(format!(
                "Bark authorization is {actual} bytes, maximum is {FEDERATION_MAX_BARK_AUTHORIZATION_BYTES}"
            )));
        }
        Ok(())
    }

    /// Validate the accepted journal and public-configuration portion of custody authorization.
    ///
    /// The Bark SQL projection must additionally reconstruct the referenced
    /// VTXO locator and exact message/transcript before a participant releases
    /// cryptographic material. This method intentionally performs no database
    /// or network access.
    pub fn validate_accepted_operation(
        &self,
        config: &FederationConfig,
        entry: &FederationJournalEntry,
    ) -> Result<()> {
        self.validate()?;
        entry.envelope.verify_authorization(config)?;
        if entry.status() != FederationOperationStatus::Applied {
            return Err(invalid_bark(
                "Bark custody authorization requires an applied operation",
            ));
        }
        if self.federation_id != config.federation_id
            || self.accepted_operation_id != entry.operation_id
            || self.accepted_operation_index != entry.index
        {
            return Err(invalid_bark(
                "Bark authorization references a different accepted operation",
            ));
        }
        let application = crate::federation_bark_application(config)?
            .ok_or_else(|| invalid_bark("accepted Bark application is missing"))?;
        if self.application_id != application.application_id
            || self.root_key_epoch != application.root_key_epoch
            || self.application_key_epoch != application.application_key_epoch
            || self.application_derivation_version != application.derivation_version
            || self.network != application.network
            || self.ark_server_public_key != application.ark_server_public_key
        {
            return Err(invalid_bark(
                "Bark authorization application scope differs from public config",
            ));
        }
        let operation = validate_federation_bark_operation(&entry.envelope.operation)?;
        if self.action_id != operation.action_id()
            || self.operation_kind != operation.kind()
            || self.phase != operation.phase()
            || self.plan_or_result_hash != operation.commitment()?
        {
            return Err(invalid_bark(
                "Bark authorization differs from the accepted operation",
            ));
        }
        let (accepted_signers, accepted_attempt) = match &operation {
            FederationBarkOperation::ActionPlan(plan) => (&plan.signer_set, plan.attempt),
            FederationBarkOperation::ActionTransition(transition)
                if !transition.signer_set.is_empty() =>
            {
                (&transition.signer_set, transition.attempt)
            }
            FederationBarkOperation::ReceiveIntent(_)
            | FederationBarkOperation::ReceivePrepared(_)
            | FederationBarkOperation::ActionTransition(_)
            | FederationBarkOperation::MemberObservation(_)
            | FederationBarkOperation::EvidenceCertificate(_) => {
                return Err(invalid_bark(
                    "accepted operation does not authorize a custody signer set",
                ));
            }
        };
        if &self.signer_set != accepted_signers || self.attempt != accepted_attempt {
            return Err(invalid_bark(
                "Bark authorization signer set or attempt differs from accepted state",
            ));
        }
        if self.signer_set.len() < usize::from(application.threshold)
            || self
                .signer_set
                .iter()
                .any(|member| !config.members.contains_key(member))
        {
            return Err(invalid_bark(
                "Bark authorization signer set cannot satisfy the accepted application",
            ));
        }
        Ok(())
    }

    /// Return sorted-key canonical JSON bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(self)
    }

    /// Commit to every authorization field.
    pub fn commitment(&self) -> Result<FederationBarkDigest> {
        domain_hash(BARK_AUTHORIZATION_HASH_DOMAIN, &self.canonical_bytes()?)
    }

    /// Derive the stable session slot, excluding signer set and attempt.
    pub fn signing_slot(&self) -> Result<FederationBarkDigest> {
        #[derive(Serialize)]
        struct SigningSlot {
            version: u16,
            federation_id: FederationId,
            application_id: FederationFrostApplicationId,
            root_key_epoch: u64,
            application_key_epoch: u64,
            application_derivation_version: u16,
            accepted_operation_id: FederationOperationId,
            accepted_operation_index: u64,
            action_id: FederationBarkActionId,
            phase: FederationBarkSemanticPhase,
            object_ordinal: u32,
            operation_kind: FederationBarkOperationKind,
            plan_or_result_hash: FederationBarkDigest,
            locator: FederationBarkVtxoLocator,
            protocol: FederationBarkCustodyProtocol,
            transcript_hash: FederationBarkDigest,
            network: Network,
            #[serde(with = "hex_array_33")]
            ark_server_public_key: [u8; 33],
        }

        self.validate()?;
        let slot = SigningSlot {
            version: self.version,
            federation_id: self.federation_id,
            application_id: self.application_id,
            root_key_epoch: self.root_key_epoch,
            application_key_epoch: self.application_key_epoch,
            application_derivation_version: self.application_derivation_version,
            accepted_operation_id: self.accepted_operation_id,
            accepted_operation_index: self.accepted_operation_index,
            action_id: self.action_id,
            phase: self.phase,
            object_ordinal: self.object_ordinal,
            operation_kind: self.operation_kind,
            plan_or_result_hash: self.plan_or_result_hash,
            locator: self.locator,
            protocol: self.protocol,
            transcript_hash: self.transcript_hash,
            network: self.network,
            ark_server_public_key: self.ark_server_public_key,
        };
        domain_hash(BARK_SIGNING_SLOT_DOMAIN, &canonical_json_bytes(&slot)?)
    }

    /// Derive the full protocol session ID, including attempt and signer set.
    pub fn session_id(&self) -> Result<FederationBarkDigest> {
        self.validate()?;
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, &self.signing_slot()?.to_bytes());
        append_hash_bytes(&mut transcript, &self.attempt.to_be_bytes());
        for member in &self.signer_set {
            append_hash_bytes(&mut transcript, &member.get().to_be_bytes());
        }
        domain_hash(BARK_SESSION_ID_DOMAIN, &transcript)
    }
}

/// Complete application payload consumed by the Bark FROST registration.
///
/// The accepted-operation reference is revalidated against durable consensus
/// before this payload reaches the generic key manager. Keeping the exact
/// message and derivation in the payload lets the application adapter remain
/// deterministic while avoiding an unrestricted digest-signing boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkFrostAuthorization {
    /// Authorization payload format.
    pub version: u16,
    /// Accepted Bark operation and custody-object reference.
    pub reference: FederationBarkAuthorizationReference,
    /// Exact BIP340 message authorized by the accepted request.
    pub message: [u8; 32],
    /// Canonical [`cdk_frost::SigningDerivation`] wire bytes.
    pub signing_derivation: Vec<u8>,
    /// Exact bounded Bark request transcript independently checked by members.
    ///
    /// The participant authorization wire uses a compact CBOR byte string.
    #[serde(with = "compact_bytes")]
    pub request_transcript: Vec<u8>,
}

impl FederationBarkFrostAuthorization {
    /// Construct a bounded authorization payload from reviewed inputs.
    pub fn new(
        reference: FederationBarkAuthorizationReference,
        message: [u8; 32],
        signing_derivation: &cdk_frost::SigningDerivation,
        request_transcript: Vec<u8>,
    ) -> Result<Self> {
        let value = Self {
            version: FEDERATION_BARK_AUTHORIZATION_VERSION,
            reference,
            message,
            signing_derivation: signing_derivation.encode(),
            request_transcript,
        };
        value.validate()?;
        Ok(value)
    }

    /// Validate the accepted reference, message, derivation, and size bound.
    pub fn validate(&self) -> Result<()> {
        if self.version != FEDERATION_BARK_AUTHORIZATION_VERSION || self.message == [0; 32] {
            return Err(invalid_bark(
                "unsupported Bark FROST authorization version or empty message",
            ));
        }
        self.reference.validate()?;
        if self.request_transcript.is_empty()
            || self.reference.transcript_hash != self.request_transcript_hash()?
        {
            return Err(invalid_bark(
                "Bark request transcript is empty or differs from its accepted reference",
            ));
        }
        let derivation = self.signing_derivation()?;
        if derivation.binding() == [0; 32] {
            return Err(invalid_bark("Bark signing derivation binding is empty"));
        }
        let actual = self.canonical_bytes()?.len();
        if actual > FEDERATION_MAX_BARK_AUTHORIZATION_BYTES {
            return Err(invalid_bark(format!(
                "Bark FROST authorization is {actual} bytes, maximum is {FEDERATION_MAX_BARK_AUTHORIZATION_BYTES}"
            )));
        }
        Ok(())
    }

    /// Decode and require the canonical compact participant wire format.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > FEDERATION_MAX_BARK_AUTHORIZATION_BYTES {
            return Err(invalid_bark("Bark FROST authorization exceeds its bound"));
        }
        let value: Self = ciborium::from_reader(bytes)
            .map_err(|error| invalid_bark(format!("invalid Bark FROST authorization: {error}")))?;
        value.validate()?;
        if value.canonical_bytes()? != bytes {
            return Err(invalid_bark(
                "Bark FROST authorization encoding is not canonical",
            ));
        }
        Ok(value)
    }

    /// Return canonical compact participant wire bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        ciborium::into_writer(self, &mut bytes)
            .map_err(|error| invalid_bark(format!("invalid Bark FROST authorization: {error}")))?;
        Ok(bytes)
    }

    /// Decode the exact signing derivation.
    pub fn signing_derivation(&self) -> Result<cdk_frost::SigningDerivation> {
        cdk_frost::SigningDerivation::decode(&self.signing_derivation)
            .map_err(|error| invalid_bark(format!("invalid Bark signing derivation: {error}")))
    }

    /// Commit to the exact Bark custody request transcript.
    pub fn request_transcript_hash(&self) -> Result<FederationBarkDigest> {
        federation_bark_custody_transcript_hash(&self.request_transcript)
    }
}

/// Compact operation metadata returned to routes and workers waiting for a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkOperationRecord {
    /// Accepted operation ID.
    pub operation_id: FederationOperationId,
    /// Accepted journal index.
    pub operation_index: u64,
    /// Local deterministic application status.
    pub status: FederationOperationStatus,
    /// Nested operation kind.
    pub kind: FederationBarkOperationKind,
    /// Stable action lineage.
    pub action_id: FederationBarkActionId,
    /// Semantic phase represented by this record.
    pub phase: FederationBarkSemanticPhase,
    /// Canonical result commitment when present.
    pub result_hash: Option<FederationBarkDigest>,
}

impl FederationBarkOperationRecord {
    /// Extract Bark metadata from one journal entry.
    #[must_use]
    pub fn from_journal_entry(entry: &FederationJournalEntry) -> Option<Self> {
        let operation = validate_federation_bark_operation(&entry.envelope.operation).ok()?;
        Some(Self {
            operation_id: entry.operation_id,
            operation_index: entry.index,
            status: entry.status(),
            kind: operation.kind(),
            action_id: operation.action_id(),
            phase: operation.phase(),
            result_hash: operation.result_hash(),
        })
    }

    /// Find the latest retained record for one exact action phase.
    #[must_use]
    pub fn latest_for_action_phase(
        entries: &[FederationJournalEntry],
        action_id: FederationBarkActionId,
        phase: FederationBarkSemanticPhase,
    ) -> Option<Self> {
        entries
            .iter()
            .filter_map(Self::from_journal_entry)
            .filter(|record| record.action_id == action_id && record.phase == phase)
            .max_by_key(|record| record.operation_index)
    }
}

fn ensure_version(version: u16) -> Result<()> {
    if version != FEDERATION_BARK_OPERATION_VERSION {
        return Err(invalid_bark(format!(
            "unsupported Bark operation version {version}"
        )));
    }
    Ok(())
}

fn authorization_kind_matches_phase(
    kind: FederationBarkOperationKind,
    phase: FederationBarkSemanticPhase,
) -> bool {
    match kind {
        FederationBarkOperationKind::ReceiveIntent => {
            phase == FederationBarkSemanticPhase::ReceiveIntent
        }
        FederationBarkOperationKind::ReceivePrepared => {
            phase == FederationBarkSemanticPhase::ReceivePrepared
        }
        FederationBarkOperationKind::ActionPlan => matches!(
            phase,
            FederationBarkSemanticPhase::SendPlanned
                | FederationBarkSemanticPhase::ClaimPlanned
                | FederationBarkSemanticPhase::RevocationPlanned
                | FederationBarkSemanticPhase::RefreshPlanned
                | FederationBarkSemanticPhase::ExitPlanned
        ),
        FederationBarkOperationKind::ActionTransition => !matches!(
            phase,
            FederationBarkSemanticPhase::ReceiveIntent
                | FederationBarkSemanticPhase::ReceivePrepared
                | FederationBarkSemanticPhase::SendPlanned
                | FederationBarkSemanticPhase::ClaimPlanned
                | FederationBarkSemanticPhase::RevocationPlanned
                | FederationBarkSemanticPhase::RefreshPlanned
                | FederationBarkSemanticPhase::ExitPlanned
                | FederationBarkSemanticPhase::Observation
        ),
        FederationBarkOperationKind::MemberObservation => {
            phase == FederationBarkSemanticPhase::Observation
        }
        FederationBarkOperationKind::EvidenceCertificate => false,
    }
}

fn ensure_quote(quote: &str) -> Result<()> {
    if quote.is_empty() || quote.len() > 256 {
        return Err(invalid_bark("Bark quote identifier is empty or too long"));
    }
    Ok(())
}

fn ensure_optional_quote(quote: Option<&str>) -> Result<()> {
    if let Some(quote) = quote {
        ensure_quote(quote)?;
    }
    Ok(())
}

fn ensure_count(label: &str, actual: usize, allow_empty: bool) -> Result<()> {
    if (!allow_empty && actual == 0) || actual > FEDERATION_MAX_BARK_COLLECTION_ENTRIES {
        return Err(invalid_bark(format!(
            "{label} count {actual} is outside the accepted bound"
        )));
    }
    Ok(())
}

fn ensure_members_sorted_unique(members: &[MemberId]) -> Result<()> {
    ensure_strictly_sorted("Bark member set", members)
}

fn ensure_strictly_sorted<T>(label: &str, values: &[T]) -> Result<()>
where
    T: Ord,
{
    if values.windows(2).any(|window| window[0] >= window[1]) {
        return Err(invalid_bark(format!(
            "{label} must be strictly sorted and unique"
        )));
    }
    Ok(())
}

fn checked_sum<I>(amounts: I) -> Result<Amount>
where
    I: IntoIterator<Item = Amount>,
{
    Amount::try_sum(amounts).map_err(|_| invalid_bark("Bark amount sum overflow"))
}

fn validate_artifacts(artifacts: &[FederationBarkArtifact]) -> Result<()> {
    let mut aggregate = 0usize;
    for artifact in artifacts {
        if artifact.bytes.len() > FEDERATION_MAX_BARK_ARTIFACT_BYTES {
            return Err(invalid_bark("one Bark artifact exceeds 64 KiB"));
        }
        aggregate = aggregate
            .checked_add(artifact.bytes.len())
            .ok_or_else(|| invalid_bark("Bark artifact aggregate overflow"))?;
        if Sha256Hash::hash(&artifact.bytes).to_byte_array() != artifact.hash.to_bytes() {
            return Err(invalid_bark("Bark artifact hash mismatch"));
        }
    }
    if aggregate > FEDERATION_MAX_BARK_ARTIFACT_AGGREGATE_BYTES {
        return Err(invalid_bark("Bark artifact aggregate exceeds 512 KiB"));
    }
    Ok(())
}

fn bark_artifact(
    kind: FederationBarkArtifactKind,
    bytes: Vec<u8>,
) -> Result<FederationBarkArtifact> {
    if bytes.is_empty() || bytes.len() > FEDERATION_MAX_BARK_ARTIFACT_BYTES {
        return Err(invalid_bark("Bark artifact is empty or oversized"));
    }
    Ok(FederationBarkArtifact {
        kind,
        hash: FederationBarkDigest::from_bytes(Sha256Hash::hash(&bytes).to_byte_array())?,
        bytes,
    })
}

fn domain_hash(domain: &[u8], value: &[u8]) -> Result<FederationBarkDigest> {
    let mut transcript = Vec::new();
    append_hash_bytes(&mut transcript, domain);
    append_hash_bytes(&mut transcript, value);
    FederationBarkDigest::from_bytes(Sha256Hash::hash(&transcript).to_byte_array())
}

fn append_hash_bytes(transcript: &mut Vec<u8>, value: &[u8]) {
    transcript.extend_from_slice(&(value.len() as u32).to_be_bytes());
    transcript.extend_from_slice(value);
}

/// Return Bark's stable application identifier independent of key epoch.
pub fn federation_bark_application_id() -> Result<FederationFrostApplicationId> {
    let registration = cdk_frost_bark::application_registration_for_epoch(1)
        .map_err(|error| invalid_bark(error.to_string()))?;
    FederationFrostApplicationId::from_bytes(registration.id().to_bytes())
}

const FEDERATION_BARK_WIRE_TAG: &str = "bark";

/// Register Bark's application-owned established federation wire codec.
pub fn register_federation_bark_wire_codec() -> Result<()> {
    register_external_application_codec(
        FEDERATION_BARK_WIRE_TAG,
        FederationExternalApplicationCodec::new(
            decode_federation_bark_wire_operation,
            federation_bark_materialized_state_keys,
            decode_federation_bark_materialized_state_key,
        ),
    )
}

fn federation_bark_application_metadata(
    operation: &FederationBarkOperation,
) -> Result<FederationApplicationOperation> {
    operation.validate()?;
    let conflict_keys = operation
        .conflict_keys()
        .into_iter()
        .map(|key| {
            FederationApplicationConflictKey::new(
                FEDERATION_BARK_WIRE_TAG,
                cdk_common::util::hex::encode(canonical_json_bytes(&key)?),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    FederationApplicationOperation::new(
        federation_bark_application_id()?,
        bark_operation_kind_label(operation.kind()),
        operation,
        conflict_keys,
        bark_state_keys(operation)?,
        operation.quote().map(ToOwned::to_owned),
        operation.claimed_author(),
    )
}

#[derive(Serialize)]
struct FederationBarkWireConflictKey {
    key: FederationBarkConflictKey,
}

fn federation_bark_external_decoded(
    operation: &FederationBarkOperation,
) -> Result<FederationExternalApplicationDecoded> {
    let conflict_key_data = operation
        .conflict_keys()
        .into_iter()
        .map(|key| serde_json::to_value(FederationBarkWireConflictKey { key }))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    FederationExternalApplicationDecoded::new(
        federation_bark_application_metadata(operation)?,
        conflict_key_data,
    )
}

fn decode_federation_bark_wire_operation(
    value: &serde_json::Value,
) -> Result<FederationExternalApplicationDecoded> {
    let operation: FederationBarkOperation = serde_json::from_value(value.clone())?;
    federation_bark_external_decoded(&operation)
}

fn federation_bark_materialized_state_keys(
    application: &FederationApplicationOperation,
    operation_id: FederationOperationId,
) -> Result<Vec<FederationExternalApplicationMaterializedStateKey>> {
    let operation = validate_federation_bark_application_operation(application)?;
    let mut keys = Vec::new();
    let action_id = operation.action_id();
    match &operation {
        FederationBarkOperation::ReceiveIntent(intent) => {
            keys.push(federation_bark_materialized_key(
                "bark_action",
                "action_id",
                &action_id,
                action_id.to_string(),
            )?);
            keys.push(federation_bark_materialized_key(
                "bark_derivation_allocation",
                "locator",
                &intent.destination,
                intent.destination.to_string(),
            )?);
        }
        FederationBarkOperation::ReceivePrepared(_) => {
            keys.push(federation_bark_materialized_key(
                "bark_action",
                "action_id",
                &action_id,
                action_id.to_string(),
            )?);
            keys.push(federation_bark_materialized_key(
                "bark_transition",
                "operation_id",
                &operation_id,
                operation_id.to_string(),
            )?);
        }
        FederationBarkOperation::ActionPlan(plan) => {
            keys.push(federation_bark_materialized_key(
                "bark_action",
                "action_id",
                &action_id,
                action_id.to_string(),
            )?);
            keys.push(federation_bark_materialized_key(
                "bark_transition",
                "operation_id",
                &operation_id,
                operation_id.to_string(),
            )?);
            for input in &plan.inputs {
                keys.push(federation_bark_materialized_key(
                    "bark_vtxo",
                    "vtxo_id",
                    &input.vtxo_id,
                    input.vtxo_id.to_string(),
                )?);
                keys.push(federation_bark_materialized_key(
                    "bark_reservation",
                    "vtxo_id",
                    &input.vtxo_id,
                    input.vtxo_id.to_string(),
                )?);
            }
            for output in &plan.outputs {
                keys.push(federation_bark_materialized_key(
                    "bark_derivation_allocation",
                    "locator",
                    &output.locator,
                    output.locator.to_string(),
                )?);
            }
            if let Some(locator) = plan.revocation_locator {
                keys.push(federation_bark_materialized_key(
                    "bark_derivation_allocation",
                    "locator",
                    &locator,
                    locator.to_string(),
                )?);
            }
        }
        FederationBarkOperation::ActionTransition(transition) => {
            keys.push(federation_bark_materialized_key(
                "bark_action",
                "action_id",
                &action_id,
                action_id.to_string(),
            )?);
            keys.push(federation_bark_materialized_key(
                "bark_transition",
                "operation_id",
                &operation_id,
                operation_id.to_string(),
            )?);
            for vtxo in &transition.vtxo_transitions {
                keys.push(federation_bark_materialized_key(
                    "bark_vtxo",
                    "vtxo_id",
                    &vtxo.vtxo_id,
                    vtxo.vtxo_id.to_string(),
                )?);
                keys.push(federation_bark_materialized_key(
                    "bark_derivation_allocation",
                    "locator",
                    &vtxo.locator,
                    vtxo.locator.to_string(),
                )?);
            }
        }
        FederationBarkOperation::MemberObservation(_)
        | FederationBarkOperation::EvidenceCertificate(_) => {
            keys.push(federation_bark_materialized_key(
                "bark_observation",
                "operation_id",
                &operation_id,
                operation_id.to_string(),
            )?);
        }
    }
    Ok(keys)
}

fn federation_bark_materialized_key<T>(
    kind: &str,
    field: &str,
    value: &T,
    row_key: String,
) -> Result<FederationExternalApplicationMaterializedStateKey>
where
    T: Serialize + ?Sized,
{
    let mut object = serde_json::Map::new();
    object.insert("kind".to_string(), kind.into());
    object.insert(field.to_string(), serde_json::to_value(value)?);
    FederationExternalApplicationMaterializedStateKey::new(
        kind,
        row_key,
        serde_json::Value::Object(object),
    )
}

fn decode_federation_bark_materialized_state_key(
    value: &serde_json::Value,
) -> Result<Option<FederationExternalApplicationMaterializedStateKey>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let Some(kind) = object.get("kind").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let field = match kind {
        "bark_action" => "action_id",
        "bark_vtxo" | "bark_reservation" => "vtxo_id",
        "bark_derivation_allocation" => "locator",
        "bark_transition" | "bark_observation" => "operation_id",
        _ => return Ok(None),
    };
    if object.len() != 2 {
        return Err(invalid_bark(
            "Bark materialized-state key contains unexpected fields",
        ));
    }
    let row_key = match kind {
        "bark_action" => serde_json::from_value::<FederationBarkActionId>(
            object
                .get(field)
                .cloned()
                .ok_or_else(|| invalid_bark("Bark materialized-state action ID is missing"))?,
        )?
        .to_string(),
        "bark_vtxo" | "bark_reservation" => serde_json::from_value::<FederationBarkVtxoId>(
            object
                .get(field)
                .cloned()
                .ok_or_else(|| invalid_bark("Bark materialized-state VTXO ID is missing"))?,
        )?
        .to_string(),
        "bark_derivation_allocation" => serde_json::from_value::<FederationBarkVtxoLocator>(
            object
                .get(field)
                .cloned()
                .ok_or_else(|| invalid_bark("Bark materialized-state locator is missing"))?,
        )?
        .to_string(),
        "bark_transition" | "bark_observation" => serde_json::from_value::<FederationOperationId>(
            object
                .get(field)
                .cloned()
                .ok_or_else(|| invalid_bark("Bark materialized-state operation ID is missing"))?,
        )?
        .to_string(),
        _ => return Ok(None),
    };
    FederationExternalApplicationMaterializedStateKey::new(kind, row_key, value.clone()).map(Some)
}

/// Wrap one typed Bark operation in the application-neutral consensus envelope.
pub fn federation_bark_application_operation(
    operation: FederationBarkOperation,
) -> Result<FederationOperation> {
    register_federation_bark_wire_codec()?;
    let decoded = federation_bark_external_decoded(&operation)?;
    let application =
        FederationExternalApplicationOperation::new(FEDERATION_BARK_WIRE_TAG, &operation, decoded)?;
    Ok(FederationOperation::ExternalApplication(application))
}

/// Decode a Bark application payload and verify every generic metadata field.
pub fn validate_federation_bark_application_operation(
    application: &FederationApplicationOperation,
) -> Result<FederationBarkOperation> {
    application.validate()?;
    if application.application_id != federation_bark_application_id()? {
        return Err(invalid_bark(
            "application operation is not registered to Bark",
        ));
    }
    let operation: FederationBarkOperation = application.decode_payload()?;
    let rebuilt = federation_bark_application_metadata(&operation)?;
    if &rebuilt != application {
        return Err(invalid_bark(
            "Bark application metadata differs from its typed payload",
        ));
    }
    Ok(operation)
}

/// Decode and validate Bark from either generic or established application wire.
pub fn validate_federation_bark_operation(
    operation: &FederationOperation,
) -> Result<FederationBarkOperation> {
    match operation {
        FederationOperation::Application(application) => {
            validate_federation_bark_application_operation(application)
        }
        FederationOperation::ExternalApplication(application)
            if application.wire_tag() == FEDERATION_BARK_WIRE_TAG =>
        {
            let operation: FederationBarkOperation = application.decode_payload()?;
            operation.validate()?;
            if application.application() != &federation_bark_application_metadata(&operation)? {
                return Err(invalid_bark(
                    "Bark established-wire metadata differs from its typed payload",
                ));
            }
            Ok(operation)
        }
        FederationOperation::ExternalApplication(_) => {
            Err(invalid_bark("external application operation is not Bark"))
        }
        _ => Err(invalid_bark(
            "operation is not a federation application operation",
        )),
    }
}

fn bark_operation_kind_label(kind: FederationBarkOperationKind) -> &'static str {
    match kind {
        FederationBarkOperationKind::ReceiveIntent => "receive_intent",
        FederationBarkOperationKind::ReceivePrepared => "receive_prepared",
        FederationBarkOperationKind::ActionPlan => "action_plan",
        FederationBarkOperationKind::ActionTransition => "action_transition",
        FederationBarkOperationKind::MemberObservation => "member_observation",
        FederationBarkOperationKind::EvidenceCertificate => "evidence_certificate",
    }
}

fn bark_state_keys(
    operation: &FederationBarkOperation,
) -> Result<Vec<FederationApplicationStateKey>> {
    let mut keys = Vec::new();
    let mut push = |collection: &str, key: String| -> Result<()> {
        keys.push(FederationApplicationStateKey::new(collection, key)?);
        Ok(())
    };
    match operation {
        FederationBarkOperation::ReceiveIntent(intent) => {
            push("bark_action", operation.action_id().to_string())?;
            push("bark_derivation_allocation", intent.destination.to_string())?;
        }
        FederationBarkOperation::ReceivePrepared(_) => {
            push("bark_action", operation.action_id().to_string())?;
            push(
                "bark_transition",
                cdk_common::federation::FEDERATION_APPLICATION_OPERATION_ID_STATE_KEY.to_string(),
            )?;
        }
        FederationBarkOperation::ActionPlan(plan) => {
            push("bark_action", operation.action_id().to_string())?;
            push(
                "bark_transition",
                cdk_common::federation::FEDERATION_APPLICATION_OPERATION_ID_STATE_KEY.to_string(),
            )?;
            for input in &plan.inputs {
                push("bark_vtxo", input.vtxo_id.to_string())?;
                push("bark_reservation", input.vtxo_id.to_string())?;
            }
            for output in &plan.outputs {
                push("bark_derivation_allocation", output.locator.to_string())?;
            }
            if let Some(locator) = plan.revocation_locator {
                push("bark_derivation_allocation", locator.to_string())?;
            }
        }
        FederationBarkOperation::ActionTransition(transition) => {
            push("bark_action", operation.action_id().to_string())?;
            push(
                "bark_transition",
                cdk_common::federation::FEDERATION_APPLICATION_OPERATION_ID_STATE_KEY.to_string(),
            )?;
            for vtxo in &transition.vtxo_transitions {
                push("bark_vtxo", vtxo.vtxo_id.to_string())?;
                push("bark_derivation_allocation", vtxo.locator.to_string())?;
            }
        }
        FederationBarkOperation::MemberObservation(_)
        | FederationBarkOperation::EvidenceCertificate(_) => push(
            "bark_observation",
            cdk_common::federation::FEDERATION_APPLICATION_OPERATION_ID_STATE_KEY.to_string(),
        )?,
    }
    keys.sort_by(|left, right| (&left.collection, &left.key).cmp(&(&right.collection, &right.key)));
    keys.dedup();
    Ok(keys)
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&cdk_common::util::hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        cdk_common::util::hex::decode(&value).map_err(serde::de::Error::custom)
    }
}

mod compact_bytes {
    use core::fmt;

    use serde::de::Visitor;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ByteVisitor;

        impl<'de> Visitor<'de> for ByteVisitor {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded byte string")
            }

            fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(value.to_vec())
            }

            fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(value)
            }
        }

        deserializer.deserialize_byte_buf(ByteVisitor)
    }
}

mod hex_array_33 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8; 33], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&cdk_common::util::hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 33], D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let bytes = cdk_common::util::hex::decode(&value).map_err(serde::de::Error::custom)?;
        bytes.try_into().map_err(|bytes: Vec<u8>| {
            serde::de::Error::custom(format!(
                "compressed public key must contain 33 bytes, found {}",
                bytes.len()
            ))
        })
    }
}

#[cfg(test)]
mod tests;

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

use super::journal::{FederationJournalEntry, FederationOperationStatus};
use super::operation::{canonical_json_bytes, FederationOperationSignature};
use super::{
    FederationError, FederationFrostApplicationId, FederationId, FederationOperation,
    FederationOperationId, MemberId, Result,
};
use crate::Amount;

/// Current nested Bark operation format.
pub const FEDERATION_BARK_OPERATION_VERSION: u16 = 1;
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
/// Maximum canonical BOLT11 invoice bytes retained in an operation.
pub const FEDERATION_MAX_BARK_INVOICE_BYTES: usize = 64 * 1024;
/// Maximum UTF-8 bytes in a direct federated Bark invoice description.
pub const FEDERATION_MAX_BARK_DESCRIPTION_BYTES: usize = 4 * 1024;

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
const BARK_RECEIVE_EXTERNAL_ID_DOMAIN: &[u8] = b"cdk/federation/bark-receive-external-id/v1";
const BARK_SEND_REQUEST_DOMAIN: &str = "cdk/federation/bark-send-request/v1";
const BARK_SEND_EXTERNAL_ID_DOMAIN: &[u8] = b"cdk/federation/bark-send-external-id/v1";

/// Commit to the exact bounded transcript presented to a Bark custody method.
pub fn federation_bark_custody_transcript_hash(bytes: &[u8]) -> Result<FederationBarkDigest> {
    if bytes.is_empty() || bytes.len() > FEDERATION_MAX_BARK_AUTHORIZATION_BYTES {
        return Err(invalid_bark(
            "Bark custody transcript is empty or oversized",
        ));
    }
    domain_hash(BARK_CUSTODY_TRANSCRIPT_DOMAIN, bytes)
}

fn invalid_bark(reason: impl Into<String>) -> FederationError {
    FederationError::BarkOperationInvalid {
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
                let bytes = crate::util::hex::decode(value)?;
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
                crate::util::hex::encode(self.0)
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
        let bytes = crate::util::hex::decode(value)?;
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
        crate::util::hex::encode(self.0)
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
    "Stable idempotency identifier for one external Bark or Ark request."
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
    /// Derive the frozen idempotency key for one outgoing Lightning action.
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
    /// Stable Ark request idempotency identifier.
    pub external_id: FederationBarkExternalId,
    /// Initial deterministic signer set, sorted by member.
    pub signer_set: Vec<MemberId>,
    /// Initial accepted custody attempt.
    pub attempt: u32,
    /// Consensus-visible action expiry or chain-height bound.
    pub expires_at: u64,
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
    /// Exact outgoing payment was initiated.
    PaymentInitiated,
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

impl FederationBarkSemanticPhase {
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
            Self::PaymentInitiated => 13,
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
    /// One predecessor may admit only one next transition.
    Predecessor {
        /// Exact predecessor operation.
        operation_id: FederationOperationId,
    },
    /// One external idempotency identifier may bind only one request/result.
    ExternalId {
        /// Stable external identifier.
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
}

impl FederationBarkOperation {
    /// Return the family-level kind.
    #[must_use]
    pub const fn kind(&self) -> FederationBarkOperationKind {
        match self {
            Self::ReceiveIntent(_) => FederationBarkOperationKind::ReceiveIntent,
            Self::ReceivePrepared(_) => FederationBarkOperationKind::ReceivePrepared,
            Self::ActionPlan(_) => FederationBarkOperationKind::ActionPlan,
            Self::ActionTransition(_) => FederationBarkOperationKind::ActionTransition,
            Self::MemberObservation(_) => FederationBarkOperationKind::MemberObservation,
        }
    }

    /// Return the member whose envelope authorization must match this payload.
    #[must_use]
    pub const fn claimed_author(&self) -> MemberId {
        match self {
            Self::ReceiveIntent(operation) => operation.actor,
            Self::ReceivePrepared(operation) => operation.actor,
            Self::ActionPlan(operation) => operation.actor,
            Self::ActionTransition(operation) => operation.actor,
            Self::MemberObservation(operation) => operation.observer,
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
        }
    }

    /// Return the canonical result commitment, if this record carries a result.
    #[must_use]
    pub const fn result_hash(&self) -> Option<FederationBarkDigest> {
        match self {
            Self::ReceivePrepared(operation) => Some(operation.result_hash),
            Self::ActionTransition(operation) => Some(operation.result_hash),
            Self::MemberObservation(operation) => Some(operation.result_hash),
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
                keys.extend(operation.inputs.iter().map(|input| {
                    FederationBarkConflictKey::VtxoReservation {
                        vtxo_id: input.vtxo_id,
                    }
                }));
                keys.extend(operation.outputs.iter().map(|output| {
                    FederationBarkConflictKey::Locator {
                        locator: output.locator,
                    }
                }));
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
                keys
            }
            Self::MemberObservation(operation) => {
                vec![FederationBarkConflictKey::Observation {
                    action_id: operation.action_id,
                    observer: operation.observer,
                    observation_epoch: operation.observation_epoch,
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

    fn validate(&self) -> Result<()> {
        ensure_version(self.version)?;
        ensure_optional_quote(self.quote.as_deref())?;
        ensure_count("Bark inputs", self.inputs.len(), false)?;
        ensure_count("Bark outputs", self.outputs.len(), false)?;
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
        } else if self.external_amount != Amount::ZERO || self.revocation_locator.is_some() {
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

impl FederationBarkActionTransition {
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
        (None, Observed | Spendable)
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
        if self.fee > self.amount {
            return Err(invalid_bark("Bark observation fee exceeds observed amount"));
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
        config: &super::FederationConfig,
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
        let application = config
            .frost_wallet
            .as_ref()
            .and_then(|wallet| wallet.bark_application.as_ref())
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
        let FederationOperation::Bark(operation) = &entry.envelope.operation else {
            return Err(invalid_bark(
                "Bark authorization references a non-Bark operation",
            ));
        };
        operation.validate()?;
        if self.action_id != operation.action_id()
            || self.operation_kind != operation.kind()
            || self.phase != operation.phase()
            || self.plan_or_result_hash != operation.commitment()?
        {
            return Err(invalid_bark(
                "Bark authorization differs from the accepted operation",
            ));
        }
        let (accepted_signers, accepted_attempt) = match operation {
            FederationBarkOperation::ActionPlan(plan) => (&plan.signer_set, plan.attempt),
            FederationBarkOperation::ActionTransition(transition)
                if !transition.signer_set.is_empty() =>
            {
                (&transition.signer_set, transition.attempt)
            }
            FederationBarkOperation::ReceiveIntent(_)
            | FederationBarkOperation::ReceivePrepared(_)
            | FederationBarkOperation::ActionTransition(_)
            | FederationBarkOperation::MemberObservation(_) => {
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

    /// Decode and require sorted-key canonical JSON.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > FEDERATION_MAX_BARK_AUTHORIZATION_BYTES {
            return Err(invalid_bark("Bark FROST authorization exceeds its bound"));
        }
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|error| invalid_bark(format!("invalid Bark FROST authorization: {error}")))?;
        value.validate()?;
        if value.canonical_bytes()? != bytes {
            return Err(invalid_bark(
                "Bark FROST authorization encoding is not canonical",
            ));
        }
        Ok(value)
    }

    /// Return sorted-key canonical JSON bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(self)
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
        let FederationOperation::Bark(operation) = &entry.envelope.operation else {
            return None;
        };
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

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&crate::util::hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        crate::util::hex::decode(&value).map_err(serde::de::Error::custom)
    }
}

mod hex_array_33 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8; 33], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&crate::util::hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 33], D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let bytes = crate::util::hex::decode(&value).map_err(serde::de::Error::custom)?;
        bytes.try_into().map_err(|bytes: Vec<u8>| {
            serde::de::Error::custom(format!(
                "compressed public key must contain 33 bytes, found {}",
                bytes.len()
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};

    use super::*;
    use crate::federation::{
        FederationConsensusItem, FederationJournalAppendOutcome, FederationOperationEnvelope,
        InMemoryFederationJournal,
    };

    fn digest(seed: u8) -> FederationBarkDigest {
        FederationBarkDigest::from_bytes([seed; 32]).expect("non-zero digest")
    }

    fn action(seed: u8) -> FederationBarkActionId {
        FederationBarkActionId::from_bytes([seed; 32]).expect("non-zero action")
    }

    fn vtxo(seed: u8) -> FederationBarkVtxoId {
        FederationBarkVtxoId::from_bytes([seed; 36]).expect("non-zero VTXO")
    }

    fn external(seed: u8) -> FederationBarkExternalId {
        FederationBarkExternalId::from_bytes([seed; 32]).expect("non-zero external ID")
    }

    fn member(value: u16) -> MemberId {
        MemberId::new(value).expect("member")
    }

    fn operation_id(seed: u8) -> FederationOperationId {
        FederationOperationId::from_bytes([seed; 32])
    }

    fn ark_server_key() -> [u8; 33] {
        [
            0x02, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce,
            0x87, 0x0b, 0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81,
            0x5b, 0x16, 0xf8, 0x17, 0x98,
        ]
    }

    fn federation_id() -> FederationId {
        FederationId::from_bytes([42; 32])
    }

    fn acknowledgement(member_id: u16) -> FederationBarkReceiveSecretAcknowledgement {
        FederationBarkReceiveSecretAcknowledgement {
            member: member(member_id),
            record_hash: digest(12),
            signature: FederationOperationSignature::from_slice(&[member_id as u8; 64])
                .expect("signature"),
        }
    }

    fn receive_intent() -> FederationBarkOperation {
        FederationBarkOperation::ReceiveIntent(FederationBarkReceiveIntent {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: action(1),
            quote: "mint-quote-1".to_string(),
            actor: member(1),
            cashu_amount: Amount::from(100),
            gross_amount: Amount::from(100),
            receive_fee: Amount::ZERO,
            minimum_net_amount: Amount::from(100),
            unit: FederationBarkUnit::Sat,
            description_hash: federation_bark_description_hash(None).expect("description hash"),
            description: None,
            payment_hash: digest(3),
            preimage_commitment: digest(4),
            secret_acknowledgements: vec![acknowledgement(1), acknowledgement(2)],
            destination: FederationBarkVtxoLocator::vtxo_custody(7),
            mailbox_scope: federation_bark_mailbox_scope(&ark_server_key()).expect("mailbox scope"),
            mailbox_id: ark_server_key(),
            ark_server: digest(6),
            network: Network::Regtest,
            minimum_cltv_delta: 18,
            fee_policy_version: 1,
            anti_dos_policy_version: 1,
            receive_proof: FederationBarkReceiveProof::NoProof,
            expires_at: 1_900_000_000,
        })
    }

    fn receive_prepared() -> FederationBarkOperation {
        FederationBarkOperation::ReceivePrepared(FederationBarkReceivePrepared {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: action(1),
            quote: "mint-quote-1".to_string(),
            actor: member(2),
            intent_operation_id: operation_id(1),
            invoice: "lnbcrt1canonicalinvoice".to_string(),
            payment_hash: digest(3),
            gross_amount: Amount::from(100),
            expected_net_amount: Amount::from(100),
            network: Network::Regtest,
            expires_at: 1_900_000_000,
            final_cltv_delta: 18,
            receive_fee: Amount::ZERO,
            request_hash: digest(15),
            result_hash: digest(16),
            external_id: external(17),
        })
    }

    fn signed_receive_invoice(
        currency: Currency,
        payment_hash: [u8; 32],
        amount_msat: Option<u64>,
        description: &str,
        timestamp: u64,
        expiry_seconds: u64,
        final_cltv_delta: u64,
    ) -> Bolt11Invoice {
        let mut builder = InvoiceBuilder::new(currency)
            .description(description.to_string())
            .payment_hash(Sha256Hash::from_byte_array(payment_hash))
            .payment_secret(PaymentSecret([43; 32]))
            .timestamp(UNIX_EPOCH + Duration::from_secs(timestamp))
            .expiry_time(Duration::from_secs(expiry_seconds))
            .min_final_cltv_expiry_delta(final_cltv_delta);
        if let Some(amount_msat) = amount_msat {
            builder = builder.amount_milli_satoshis(amount_msat);
        }
        let signing_key = SecretKey::from_slice(&[42; 32]).expect("invoice signing key");
        builder
            .build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &signing_key))
            .expect("signed receive invoice")
    }

    fn exact_receive_invoice() -> Bolt11Invoice {
        signed_receive_invoice(
            Currency::Regtest,
            [3; 32],
            Some(100_000),
            "",
            1_800_000_000,
            3_600,
            18,
        )
    }

    fn send_plan() -> FederationBarkOperation {
        let action_id = action(20);
        let predecessor_operation_id = operation_id(19);
        let mut plan = FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id,
            quote: Some("melt-quote-1".to_string()),
            kind: FederationBarkActionKind::LightningSend,
            actor: member(1),
            predecessor_operation_id,
            payment_hash: Some(digest(21)),
            inputs: vec![FederationBarkVtxoInput {
                vtxo_id: vtxo(22),
                locator: FederationBarkVtxoLocator::vtxo_custody(7),
                amount: Amount::from(110),
            }],
            outputs: vec![FederationBarkVtxoOutput {
                locator: FederationBarkVtxoLocator::vtxo_custody(8),
                amount: Amount::from(5),
                artifact_hash: digest(23),
            }],
            revocation_locator: Some(FederationBarkVtxoLocator::vtxo_custody(9)),
            external_amount: Amount::from(100),
            fee_amount: Amount::from(5),
            fee_bound: Amount::from(8),
            artifacts: Vec::new(),
            external_id: FederationBarkExternalId::derive_lightning_send(
                action_id,
                predecessor_operation_id,
                1,
            )
            .expect("send external id"),
            signer_set: vec![member(1), member(2)],
            attempt: 1,
            expires_at: 850_000,
        };
        let invoice = signed_receive_invoice(
            Currency::Regtest,
            [21; 32],
            Some(100_000),
            "send",
            1_800_000_000,
            3_600,
            18,
        );
        plan.bind_lightning_send_artifacts(&invoice)
            .expect("bind send artifacts");
        FederationBarkOperation::ActionPlan(plan)
    }

    fn envelope(operation: FederationBarkOperation) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(federation_id(), FederationOperation::Bark(operation))
    }

    #[test]
    fn nested_operation_is_canonical_bounded_and_roundtrips() {
        let operation = receive_intent();
        operation.validate().expect("valid receive intent");
        let canonical = operation.canonical_bytes().expect("canonical operation");
        assert!(canonical.len() < FEDERATION_MAX_BARK_OPERATION_BYTES);
        let decoded: FederationBarkOperation =
            serde_json::from_slice(&canonical).expect("decode canonical operation");
        assert_eq!(decoded, operation);
        assert_eq!(
            decoded.commitment().expect("commitment"),
            operation.commitment().expect("commitment")
        );
    }

    #[test]
    fn prepared_receive_requires_the_exact_signed_invoice_and_intent_commitments() {
        let FederationBarkOperation::ReceiveIntent(intent) = receive_intent() else {
            panic!("receive intent")
        };
        let intent_operation_id = operation_id(1);
        let prepared = FederationBarkReceivePrepared::from_invoice(
            member(2),
            intent_operation_id,
            &intent,
            exact_receive_invoice(),
        )
        .expect("exact prepared receive");
        prepared
            .validate_against_intent(&intent)
            .expect("exact prepared receive validation");
        assert_eq!(
            prepared.request_hash,
            federation_bark_receive_request_hash(intent_operation_id, &intent)
                .expect("request commitment")
        );

        for invoice in [
            signed_receive_invoice(
                Currency::Regtest,
                [3; 32],
                Some(99_000),
                "",
                1_800_000_000,
                3_600,
                18,
            ),
            signed_receive_invoice(
                Currency::Regtest,
                [44; 32],
                Some(100_000),
                "",
                1_800_000_000,
                3_600,
                18,
            ),
            signed_receive_invoice(
                Currency::Bitcoin,
                [3; 32],
                Some(100_000),
                "",
                1_800_000_000,
                3_600,
                18,
            ),
            signed_receive_invoice(
                Currency::Regtest,
                [3; 32],
                Some(100_000),
                "changed",
                1_800_000_000,
                3_600,
                18,
            ),
            signed_receive_invoice(
                Currency::Regtest,
                [3; 32],
                Some(100_000),
                "",
                1_800_000_000,
                3_600,
                17,
            ),
            signed_receive_invoice(
                Currency::Regtest,
                [3; 32],
                Some(100_000),
                "",
                1_899_999_000,
                3_600,
                18,
            ),
            signed_receive_invoice(
                Currency::Regtest,
                [3; 32],
                None,
                "",
                1_800_000_000,
                3_600,
                18,
            ),
        ] {
            assert!(FederationBarkReceivePrepared::from_invoice(
                member(2),
                intent_operation_id,
                &intent,
                invoice,
            )
            .is_err());
        }

        let mut changed = prepared.clone();
        changed.actor = member(3);
        assert!(changed.validate_against_intent(&intent).is_err());
        let mut changed = prepared.clone();
        changed.request_hash = digest(70);
        assert!(changed.validate_against_intent(&intent).is_err());
        let mut changed = prepared.clone();
        changed.result_hash = digest(71);
        assert!(changed.validate_against_intent(&intent).is_err());
        let mut changed = prepared;
        changed.external_id = external(72);
        assert!(changed.validate_against_intent(&intent).is_err());
    }

    #[test]
    fn journal_replay_is_idempotent_and_rebuildable() {
        let envelope = envelope(receive_intent());
        let consensus_item = FederationConsensusItem::operation(envelope.clone());
        let consensus_wire =
            serde_json::to_vec(&consensus_item).expect("consensus transport bytes");
        let decoded_item: FederationConsensusItem =
            serde_json::from_slice(&consensus_wire).expect("consensus transport decode");
        assert_eq!(
            decoded_item.operation_id().expect("decoded operation ID"),
            consensus_item.operation_id().expect("operation ID")
        );
        let mut journal = InMemoryFederationJournal::new();
        assert!(matches!(
            journal.append(envelope.clone()).expect("first append"),
            FederationJournalAppendOutcome::Appended(_)
        ));
        assert!(matches!(
            journal.append(envelope).expect("duplicate append"),
            FederationJournalAppendOutcome::Duplicate(_)
        ));
        let encoded = serde_json::to_vec(journal.entries()).expect("journal transport bytes");
        let decoded = serde_json::from_slice(&encoded).expect("journal transport decode");
        let rebuilt = InMemoryFederationJournal::from_entries(decoded).expect("rebuild journal");
        assert_eq!(rebuilt.latest_order_digest(), journal.latest_order_digest());
        assert_eq!(rebuilt.len(), 1);
    }

    #[test]
    fn prepared_result_lookup_is_stable_across_journal_roundtrip() {
        let mut journal = InMemoryFederationJournal::new();
        journal
            .append(envelope(receive_intent()))
            .expect("append intent");
        journal
            .append(envelope(receive_prepared()))
            .expect("append prepared result");
        let record = FederationBarkOperationRecord::latest_for_action_phase(
            journal.entries(),
            action(1),
            FederationBarkSemanticPhase::ReceivePrepared,
        )
        .expect("prepared result");
        assert_eq!(record.operation_index, 1);
        assert_eq!(record.status, FederationOperationStatus::Accepted);
        assert_eq!(record.result_hash, Some(digest(16)));

        let rebuilt = InMemoryFederationJournal::from_entries(journal.entries().to_vec())
            .expect("rebuild result journal");
        assert_eq!(
            FederationBarkOperationRecord::latest_for_action_phase(
                rebuilt.entries(),
                action(1),
                FederationBarkSemanticPhase::ReceivePrepared,
            ),
            Some(record)
        );
    }

    #[test]
    fn typed_conflicts_cover_quote_hash_vtxo_locator_phase_and_attempt() {
        let mut journal = InMemoryFederationJournal::new();
        journal
            .append(envelope(receive_intent()))
            .expect("append receive intent");
        let FederationBarkOperation::ReceiveIntent(base) = receive_intent() else {
            panic!("receive intent")
        };
        let cases = [
            {
                let mut changed = base.clone();
                changed.action_id = action(30);
                changed.payment_hash = digest(31);
                changed.destination = FederationBarkVtxoLocator::vtxo_custody(30);
                FederationBarkOperation::ReceiveIntent(changed)
            },
            {
                let mut changed = base.clone();
                changed.action_id = action(31);
                changed.quote = "other-quote".to_string();
                changed.destination = FederationBarkVtxoLocator::vtxo_custody(31);
                FederationBarkOperation::ReceiveIntent(changed)
            },
            {
                let mut changed = base.clone();
                changed.action_id = action(32);
                changed.quote = "other-quote-2".to_string();
                changed.payment_hash = digest(32);
                FederationBarkOperation::ReceiveIntent(changed)
            },
            {
                let mut changed = base;
                changed.quote = "other-quote-3".to_string();
                changed.payment_hash = digest(33);
                changed.destination = FederationBarkVtxoLocator::vtxo_custody(33);
                FederationBarkOperation::ReceiveIntent(changed)
            },
        ];
        for operation in cases {
            assert!(journal.append(envelope(operation)).is_err());
        }

        let plan = send_plan();
        let plan_keys = plan.conflict_keys();
        assert!(plan_keys
            .iter()
            .any(|key| matches!(key, FederationBarkConflictKey::VtxoReservation { .. })));
        assert!(plan_keys
            .iter()
            .any(|key| matches!(key, FederationBarkConflictKey::Locator { .. })));
        assert!(plan_keys.iter().any(|key| matches!(
            key,
            FederationBarkConflictKey::ActionPhase {
                attempt: Some(1),
                ..
            }
        )));
        assert!(plan_keys.iter().any(|key| matches!(
            key,
            FederationBarkConflictKey::SigningAttempt { attempt: 1, .. }
        )));
        let mut plan_journal = InMemoryFederationJournal::new();
        plan_journal
            .append(envelope(plan.clone()))
            .expect("append plan");
        let FederationBarkOperation::ActionPlan(base) = plan else {
            panic!("action plan")
        };
        let mut vtxo_conflict = base.clone();
        vtxo_conflict.action_id = action(40);
        vtxo_conflict.quote = Some("melt-quote-2".to_string());
        vtxo_conflict.payment_hash = Some(digest(40));
        vtxo_conflict.outputs[0].locator = FederationBarkVtxoLocator::vtxo_custody(40);
        vtxo_conflict.predecessor_operation_id = operation_id(40);
        vtxo_conflict.external_id = external(40);
        assert!(plan_journal
            .append(envelope(FederationBarkOperation::ActionPlan(vtxo_conflict)))
            .is_err());

        let mut attempt_conflict = base;
        attempt_conflict.fee_bound = Amount::from(9);
        assert!(plan_journal
            .append(envelope(FederationBarkOperation::ActionPlan(
                attempt_conflict
            )))
            .is_err());
    }

    #[test]
    fn author_and_balance_mutations_fail_closed() {
        let FederationBarkOperation::ReceiveIntent(mut intent) = receive_intent() else {
            panic!("receive intent")
        };
        intent.actor = member(3);
        let operation = FederationBarkOperation::ReceiveIntent(intent);
        assert_eq!(operation.claimed_author(), member(3));
        assert!(operation.validate().is_err());

        let FederationBarkOperation::ActionPlan(mut plan) = send_plan() else {
            panic!("action plan")
        };
        plan.fee_amount = Amount::from(6);
        assert!(FederationBarkOperation::ActionPlan(plan)
            .validate()
            .is_err());
    }

    #[test]
    fn lightning_send_binds_inputs_keys_fee_expiry_and_invoice() {
        let FederationBarkOperation::ActionPlan(base) = send_plan() else {
            panic!("action plan")
        };
        let mut mutations = Vec::new();
        let mut changed = base.clone();
        changed.inputs[0].locator = FederationBarkVtxoLocator::vtxo_custody(40);
        mutations.push(changed);
        let mut changed = base.clone();
        changed.outputs[0].locator = FederationBarkVtxoLocator::vtxo_custody(41);
        mutations.push(changed);
        let mut changed = base.clone();
        changed.revocation_locator = Some(FederationBarkVtxoLocator::vtxo_custody(42));
        mutations.push(changed);
        let mut changed = base.clone();
        changed.fee_bound = Amount::from(9);
        mutations.push(changed);
        let mut changed = base.clone();
        changed.expires_at += 1;
        mutations.push(changed);
        let mut changed = base;
        let invoice = changed
            .artifacts
            .iter_mut()
            .find(|artifact| artifact.kind == FederationBarkArtifactKind::Invoice)
            .expect("invoice artifact");
        invoice.bytes.push(b' ');
        invoice.hash =
            FederationBarkDigest::from_bytes(Sha256Hash::hash(&invoice.bytes).to_byte_array())
                .expect("changed invoice hash");
        changed.artifacts.sort();
        mutations.push(changed);

        for plan in mutations {
            assert!(FederationBarkOperation::ActionPlan(plan)
                .validate()
                .is_err());
        }
    }

    #[test]
    fn outgoing_fee_bound_rounds_up_conservatively() {
        let policy = crate::federation::FederationBarkPolicyConfig::new(
            0,
            0,
            10,
            5_000,
            18,
            18,
            144,
            18,
            30,
            crate::federation::FederationBarkReceiveAntiDosPolicy::NoProof,
        )
        .expect("policy");
        assert_eq!(
            policy.maximum_outgoing_fee(Amount::from(1)).expect("fee"),
            Amount::from(11),
        );
        assert_eq!(
            policy
                .maximum_outgoing_fee(Amount::from(1_000))
                .expect("fee"),
            Amount::from(15),
        );
    }

    #[test]
    fn collection_artifact_and_invoice_bounds_fail_closed() {
        let FederationBarkOperation::ActionPlan(mut plan) = send_plan() else {
            panic!("action plan")
        };
        plan.inputs = (1..=65)
            .map(|seed| FederationBarkVtxoInput {
                vtxo_id: vtxo(seed),
                locator: FederationBarkVtxoLocator::vtxo_custody(u32::from(seed)),
                amount: Amount::from(1),
            })
            .collect();
        assert!(FederationBarkOperation::ActionPlan(plan)
            .validate()
            .is_err());

        let FederationBarkOperation::ActionPlan(mut plan) = send_plan() else {
            panic!("action plan")
        };
        let bytes = vec![1; FEDERATION_MAX_BARK_ARTIFACT_BYTES + 1];
        plan.artifacts = vec![FederationBarkArtifact {
            kind: FederationBarkArtifactKind::Vtxo,
            hash: FederationBarkDigest::from_bytes(Sha256Hash::hash(&bytes).to_byte_array())
                .expect("artifact hash"),
            bytes,
        }];
        assert!(FederationBarkOperation::ActionPlan(plan)
            .validate()
            .is_err());

        let FederationBarkOperation::ReceivePrepared(mut prepared) = receive_prepared() else {
            panic!("receive prepared")
        };
        prepared.invoice = "x".repeat(FEDERATION_MAX_BARK_INVOICE_BYTES + 1);
        assert!(FederationBarkOperation::ReceivePrepared(prepared)
            .validate()
            .is_err());
    }

    #[test]
    fn authorization_slot_excludes_attempt_but_session_does_not() {
        let application_id =
            FederationFrostApplicationId::from_bytes([7; 32]).expect("application ID");
        let reference = FederationBarkAuthorizationReference {
            version: FEDERATION_BARK_AUTHORIZATION_VERSION,
            federation_id: federation_id(),
            application_id,
            root_key_epoch: 1,
            application_key_epoch: 1,
            application_derivation_version: 1,
            accepted_operation_id: operation_id(8),
            accepted_operation_index: 9,
            action_id: action(10),
            phase: FederationBarkSemanticPhase::SendHtlcsCosigned,
            object_ordinal: 0,
            operation_kind: FederationBarkOperationKind::ActionTransition,
            plan_or_result_hash: digest(11),
            locator: FederationBarkVtxoLocator::vtxo_custody(12),
            protocol: FederationBarkCustodyProtocol::Musig2,
            transcript_hash: digest(13),
            signer_set: vec![member(1), member(2)],
            attempt: 1,
            network: Network::Regtest,
            ark_server_public_key: ark_server_key(),
        };
        reference.validate().expect("valid authorization");
        let slot = reference.signing_slot().expect("slot");
        let session = reference.session_id().expect("session");
        let mut retry = reference.clone();
        retry.signer_set = vec![member(2), member(3)];
        retry.attempt = 2;
        assert_eq!(retry.signing_slot().expect("retry slot"), slot);
        assert_ne!(retry.session_id().expect("retry session"), session);
        let mut mutated = reference;
        mutated.transcript_hash = digest(14);
        assert_ne!(mutated.signing_slot().expect("mutated slot"), slot);
        let mut incompatible = mutated;
        incompatible.operation_kind = FederationBarkOperationKind::ReceiveIntent;
        assert!(incompatible.validate().is_err());
    }

    #[test]
    fn frost_authorization_binds_exact_canonical_transcript() {
        let transcript = b"exact accepted Bark custody request".to_vec();
        let application_id =
            FederationFrostApplicationId::from_bytes([7; 32]).expect("application ID");
        let reference = FederationBarkAuthorizationReference {
            version: FEDERATION_BARK_AUTHORIZATION_VERSION,
            federation_id: federation_id(),
            application_id,
            root_key_epoch: 1,
            application_key_epoch: 1,
            application_derivation_version: 1,
            accepted_operation_id: operation_id(8),
            accepted_operation_index: 9,
            action_id: action(10),
            phase: FederationBarkSemanticPhase::SendHtlcsCosigned,
            object_ordinal: 0,
            operation_kind: FederationBarkOperationKind::ActionTransition,
            plan_or_result_hash: digest(11),
            locator: FederationBarkVtxoLocator::vtxo_custody(12),
            protocol: FederationBarkCustodyProtocol::Musig2,
            transcript_hash: federation_bark_custody_transcript_hash(&transcript)
                .expect("transcript hash"),
            signer_set: vec![member(1), member(2)],
            attempt: 1,
            network: Network::Regtest,
            ark_server_public_key: ark_server_key(),
        };
        let authorization = FederationBarkFrostAuthorization::new(
            reference,
            [42; 32],
            &cdk_frost::SigningDerivation::application_key(),
            transcript,
        )
        .expect("valid exact authorization");
        let canonical = authorization.canonical_bytes().expect("canonical bytes");
        assert_eq!(
            FederationBarkFrostAuthorization::from_bytes(&canonical).expect("canonical round trip"),
            authorization
        );

        let mut changed_transcript = authorization.clone();
        changed_transcript.request_transcript.push(0);
        assert!(changed_transcript.validate().is_err());

        let mut empty_message = authorization.clone();
        empty_message.message = [0; 32];
        assert!(empty_message.validate().is_err());

        let mut noncanonical = b" ".to_vec();
        noncanonical.extend_from_slice(&canonical);
        assert!(FederationBarkFrostAuthorization::from_bytes(&noncanonical).is_err());
    }

    #[test]
    fn action_id_derivation_has_a_fixed_vector() {
        let action_id = FederationBarkActionId::derive(
            federation_id(),
            operation_id(8),
            FederationBarkSemanticPhase::SendHtlcsCosigned,
            3,
        )
        .expect("action ID");
        assert_eq!(
            action_id.to_hex(),
            "2007f3f7f11ea3e128c6470494df3e6b3dee0791d946e79ff396b148ecf6123d"
        );

        let changed = FederationBarkActionId::derive(
            federation_id(),
            operation_id(8),
            FederationBarkSemanticPhase::SendHtlcsCosigned,
            4,
        )
        .expect("changed action ID");
        assert_ne!(changed, action_id);
    }
}

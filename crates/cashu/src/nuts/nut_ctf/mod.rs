//! NUT-CTF: Conditional Token Framework
//!
//! <https://github.com/cashubtc/nuts/blob/main/CTF.md>

use std::collections::{HashMap, HashSet};

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use serde::{Deserialize, Serialize};

use super::nut00::{BlindSignature, BlindedMessage, Proofs};
use super::nut02::Id;
use crate::dhke;

pub mod dlc;
pub(crate) mod serde_oracle_witness;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_helpers;

/// NUT-CTF Error
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid condition ID
    #[error("Invalid condition ID")]
    InvalidConditionId,
    /// Condition not found
    #[error("Condition not found")]
    ConditionNotFound,
    /// Overlapping outcome collections in partition
    #[error("Overlapping outcome collections in partition")]
    OverlappingOutcomeCollections,
    /// Incomplete partition
    #[error("Incomplete partition: not all outcomes are covered")]
    IncompletePartition,
    /// Invalid oracle signature
    #[error("Invalid oracle signature")]
    InvalidOracleSignature,
    /// Oracle announcement verification failed
    #[error("Oracle announcement verification failed: {0}")]
    OracleAnnouncementVerificationFailed(String),
    /// Conditional keyset requires oracle witness
    #[error("Conditional keyset requires oracle witness")]
    ConditionalKeysetRequiresWitness,
    /// Oracle has not attested to this outcome collection
    #[error("Oracle has not attested to this outcome collection")]
    OracleNotAttestedOutcome,
    /// Inputs must use the same conditional keyset
    #[error("Inputs must use the same conditional keyset")]
    InputsMustUseSameConditionalKeyset,
    /// Outputs must use a regular keyset
    #[error("Outputs must use a regular keyset")]
    OutputsMustUseRegularKeyset,
    /// Oracle threshold not met
    #[error("Oracle threshold not met")]
    OracleThresholdNotMet,
    /// Condition already exists with different configuration
    #[error("Condition already exists with different configuration")]
    ConditionAlreadyExists,
    /// DLC messages error
    #[error("DLC error: {0}")]
    Dlc(String),
    /// Hash to curve failed
    #[error("Hash to curve failed: {0}")]
    HashToCurve(String),
    /// EC point operation failed
    #[error("EC point operation failed")]
    EcPointOperationFailed,
    /// Input size limit exceeded
    #[error("Input size limit exceeded: {0}")]
    InputSizeLimitExceeded(String),
    /// Empty outcome string
    #[error("Empty outcome string is not allowed")]
    EmptyOutcomeString,
    /// Conflicting oracle attestations
    #[error("Oracle signatures attest to different outcomes")]
    ConflictingOracleAttestations,
    /// Invalid numeric range (NUT-CTF-numeric)
    #[error("Invalid numeric range: {0}")]
    InvalidNumericRange(String),
    /// Digit signature verification failed (NUT-CTF-numeric)
    #[error("Digit signature verification failed: {0}")]
    DigitSignatureVerificationFailed(String),
    /// Attested value outside representable range (NUT-CTF-numeric)
    #[error("Attested value outside range: {0}")]
    AttestedValueOutsideRange(String),
    /// Payout calculation overflow (NUT-CTF-numeric)
    #[error("Payout calculation overflow")]
    PayoutCalculationOverflow,
}

/// Zero collection ID (32 zero bytes)
pub const ZERO_COLLECTION_ID: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// Maximum number of outcomes allowed per condition (must fit in u8 for condition_id hashing)
pub const MAX_OUTCOMES: usize = 255;

/// Maximum number of partition keys per partition registration
pub const MAX_PARTITION_KEYS: usize = 256;

/// Maximum number of oracle announcements per condition
pub const MAX_ANNOUNCEMENTS: usize = 32;

/// Maximum length of a description string
pub const MAX_DESCRIPTION_LENGTH: usize = 1024;

/// Maximum length of a single announcement hex string (64 KB)
pub const MAX_ANNOUNCEMENT_HEX_LENGTH: usize = 131_072;

// --- Request/Response Types ---

/// POST /v1/conditions request body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterConditionRequest {
    /// Minimum oracles required for attestation (default: 1)
    #[serde(default = "default_threshold")]
    pub threshold: u32,
    /// Human-readable condition description
    pub description: String,
    /// Array of hex-encoded oracle announcement TLV bytes
    pub announcements: Vec<String>,
    /// Condition type: "enum" (default) or "numeric"
    #[serde(default = "default_condition_type")]
    #[serde(skip_serializing_if = "is_default_condition_type")]
    pub condition_type: String,
    /// Lower bound for numeric conditions (inclusive)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lo_bound: Option<i64>,
    /// Upper bound for numeric conditions (inclusive)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hi_bound: Option<i64>,
    /// Precision for numeric conditions (number of decimal places)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<i32>,
}

fn default_condition_type() -> String {
    "enum".to_string()
}

fn is_default_condition_type(s: &str) -> bool {
    s == "enum"
}

fn default_threshold() -> u32 {
    1
}

/// POST /v1/conditions response body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterConditionResponse {
    /// Computed condition identifier (64 hex characters)
    pub condition_id: String,
}

/// POST /v1/conditions/{condition_id}/partitions request body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterPartitionRequest {
    /// For root conditions: a NUT-00 unit string. For nested: an outcome_collection_id hex string.
    pub collateral: String,
    /// Partition keys (optional, defaults to individual outcomes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<Vec<String>>,
    /// Parent collection ID (optional, defaults to 32 zero bytes for root)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_collection_id: Option<String>,
}

/// POST /v1/conditions/{condition_id}/partitions response body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterPartitionResponse {
    /// Mapping of outcome_collection -> keyset_id
    pub keysets: HashMap<String, Id>,
}

/// Partition info entry for ConditionInfo
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionInfoEntry {
    /// Partition keys
    pub partition: Vec<String>,
    /// Collateral unit or outcome_collection_id
    pub collateral: String,
    /// Parent collection ID
    pub parent_collection_id: String,
    /// Mapping of outcome_collection -> keyset_id
    pub keysets: HashMap<String, Id>,
    /// Unix timestamp when this partition was registered
    #[serde(default)]
    pub registered_at: u64,
}

/// GET /v1/conditions query parameters
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetConditionsRequest {
    /// Unix timestamp; only return conditions registered at or after this time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    /// Maximum number of conditions to return
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// Repeatable status filter (e.g. ?status=pending&status=attested)
    #[serde(default)]
    pub status: Vec<String>,
}

/// GET /v1/conditions response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetConditionsResponse {
    /// Array of available conditions
    pub conditions: Vec<ConditionInfo>,
}

/// Full condition detail
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionInfo {
    /// Condition identifier
    pub condition_id: String,
    /// Oracle threshold
    pub threshold: u32,
    /// Description
    pub description: String,
    /// Hex-encoded oracle announcement TLV bytes
    pub announcements: Vec<String>,
    /// Registered partitions with their keysets
    pub partitions: Vec<PartitionInfoEntry>,
    /// Attestation state (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attestation: Option<AttestationState>,
    /// Condition type: "enum" or "numeric" (NUT-CTF-numeric)
    #[serde(default = "default_condition_type")]
    #[serde(skip_serializing_if = "is_default_condition_type")]
    pub condition_type: String,
    /// Lower bound for numeric conditions (NUT-CTF-numeric)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lo_bound: Option<i64>,
    /// Upper bound for numeric conditions (NUT-CTF-numeric)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hi_bound: Option<i64>,
    /// Precision for numeric conditions (NUT-CTF-numeric)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<i32>,
    /// Unix timestamp when this condition was registered
    #[serde(default)]
    pub registered_at: u64,
}

/// Attestation state for a condition
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestationState {
    /// Status: pending, attested, expired, violation
    pub status: AttestationStatus,
    /// The attested winning outcome string (null if pending)
    pub winning_outcome: Option<String>,
    /// Unix timestamp of attestation (null if pending)
    pub attested_at: Option<u64>,
}

/// Attestation status enum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttestationStatus {
    /// No attestation yet
    Pending,
    /// Oracle has attested, redemption active
    Attested,
    /// Vesting period ended
    Expired,
    /// Multiple attestations detected (DLC violation)
    Violation,
}

// --- NUT-CTF-split-merge Split/Merge Types ---

/// POST /v1/ctf/split request body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CtfSplitRequest {
    /// Condition identifier
    pub condition_id: String,
    /// Input proofs (regular keyset for root, parent collection keyset for nested)
    pub inputs: Proofs,
    /// Output blinded messages per outcome collection key
    pub outputs: HashMap<String, Vec<BlindedMessage>>,
}

/// POST /v1/ctf/split response body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CtfSplitResponse {
    /// Blind signatures per outcome collection key
    pub signatures: HashMap<String, Vec<BlindSignature>>,
}

/// POST /v1/ctf/merge request body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CtfMergeRequest {
    /// Condition identifier
    pub condition_id: String,
    /// Input proofs per outcome collection key (from conditional keysets)
    pub inputs: HashMap<String, Proofs>,
    /// Output blinded messages (regular keyset for root, parent collection keyset for nested)
    pub outputs: Vec<BlindedMessage>,
}

/// POST /v1/ctf/merge response body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CtfMergeResponse {
    /// Blind signatures for the outputs
    pub signatures: Vec<BlindSignature>,
}

/// NUT-06 mint info extension for NUT-CTF-split-merge (split/merge)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct NutCtfSplitMergeSettings {
    /// Whether NUT-CTF-split-merge (CTF split/merge) is supported
    pub supported: bool,
}

impl Default for NutCtfSplitMergeSettings {
    fn default() -> Self {
        Self { supported: true }
    }
}

/// NUT-06 mint info extension for NUT-CTF-numeric (numeric conditions)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct NutCtfNumericSettings {
    /// Whether NUT-CTF-numeric (numeric conditional tokens) is supported
    pub supported: bool,
}

impl Default for NutCtfNumericSettings {
    fn default() -> Self {
        Self { supported: true }
    }
}

/// POST /v1/redeem_outcome request body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedeemOutcomeRequest {
    /// Input proofs from a single conditional keyset, with oracle witness
    pub inputs: Proofs,
    /// Output blinded messages using a regular keyset
    pub outputs: Vec<BlindedMessage>,
}

/// POST /v1/redeem_outcome response body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedeemOutcomeResponse {
    /// Blind signatures for the outputs
    pub signatures: Vec<BlindSignature>,
}

/// Single oracle signature entry in witness
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct OracleSig {
    /// Oracle's 32-byte x-only public key (64-char hex)
    pub oracle_pubkey: String,
    /// Oracle's 64-byte Schnorr signature (128-char hex) on the attested outcome (enum mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oracle_sig: Option<String>,
    /// The outcome string this oracle attested to
    pub outcome: String,
    /// Per-digit Schnorr signatures (128-char hex each) for digit decomposition (NUT-CTF-numeric)
    /// Mutually exclusive with oracle_sig
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digit_sigs: Option<Vec<String>>,
}

/// Oracle witness for conditional token redemption
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct OracleWitness {
    /// Array of oracle attestation entries
    pub oracle_sigs: Vec<OracleSig>,
}

/// Conditional keyset info (extends KeySetInfo for conditional keyset discovery)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalKeySetInfo {
    /// Keyset ID
    pub id: Id,
    /// Currency unit
    pub unit: String,
    /// Active flag
    pub active: bool,
    /// Input fee ppk
    pub input_fee_ppk: Option<u64>,
    /// Final expiry timestamp
    pub final_expiry: Option<u64>,
    /// Condition identifier
    pub condition_id: String,
    /// Outcome collection string
    pub outcome_collection: String,
    /// Outcome collection identifier
    pub outcome_collection_id: String,
    /// Unix timestamp when this keyset was registered
    #[serde(default)]
    pub registered_at: u64,
}

/// GET /v1/conditional_keysets query parameters
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetConditionalKeysetsRequest {
    /// Unix timestamp; only return keysets registered at or after this time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    /// Maximum number of keysets to return
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// Filter by active status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
}

/// GET /v1/conditional_keysets response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalKeysetsResponse {
    /// Array of conditional keyset info
    pub keysets: Vec<ConditionalKeySetInfo>,
}

/// NUT-06 mint info extension for NUT-CTF
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct NutCtfSettings {
    /// Whether NUT-CTF is supported
    pub supported: bool,
    /// DLC protocol version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dlc_version: Option<String>,
    /// Vesting period in seconds after event maturity
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vesting_period: Option<u64>,
}

impl Default for NutCtfSettings {
    fn default() -> Self {
        Self {
            supported: true,
            dlc_version: Some("0".to_string()),
            vesting_period: Some(2592000), // 30 days
        }
    }
}

// --- Computation Functions ---

/// Compute a BIP-340 tagged hash: SHA256(SHA256(tag) || SHA256(tag) || msg)
pub fn tagged_hash(tag: &str, msg: &[u8]) -> [u8; 32] {
    let tag_hash = Sha256Hash::hash(tag.as_bytes()).to_byte_array();
    let mut engine = Sha256Hash::engine();
    bitcoin::hashes::HashEngine::input(&mut engine, &tag_hash);
    bitcoin::hashes::HashEngine::input(&mut engine, &tag_hash);
    bitcoin::hashes::HashEngine::input(&mut engine, msg);
    Sha256Hash::from_engine(engine).to_byte_array()
}

/// Compute condition_id from oracle parameters.
///
/// ```text
/// condition_id = tagged_hash("Cashu_condition_id",
///   sorted_oracle_pubkeys || event_id || outcome_count)
/// ```
pub fn compute_condition_id(
    oracle_pubkeys: &[Vec<u8>],
    event_id: &str,
    outcome_count: u8,
) -> [u8; 32] {
    // Sort oracle pubkeys lexicographically
    let mut sorted_pubkeys = oracle_pubkeys.to_vec();
    sorted_pubkeys.sort();

    // Build message
    let mut msg = Vec::new();

    // Concatenate sorted oracle pubkeys
    for pk in &sorted_pubkeys {
        msg.extend_from_slice(pk);
    }

    // Event ID as UTF-8
    msg.extend_from_slice(event_id.as_bytes());

    // Outcome count as 1 byte
    msg.push(outcome_count);

    tagged_hash("Cashu_condition_id", &msg)
}

/// Compute condition_id for numeric conditions (NUT-CTF-numeric).
///
/// Extends the enum formula with a `0x01` type byte and bound/precision parameters:
/// ```text
/// condition_id = tagged_hash("Cashu_condition_id",
///   sorted_oracle_pubkeys || event_id || outcome_count || 0x01 || lo_bound_i64be || hi_bound_i64be || precision_i32be)
/// ```
pub fn compute_condition_id_numeric(
    oracle_pubkeys: &[Vec<u8>],
    event_id: &str,
    outcome_count: u8,
    lo_bound: i64,
    hi_bound: i64,
    precision: i32,
) -> [u8; 32] {
    let mut sorted_pubkeys = oracle_pubkeys.to_vec();
    sorted_pubkeys.sort();

    let mut msg = Vec::new();
    for pk in &sorted_pubkeys {
        msg.extend_from_slice(pk);
    }
    msg.extend_from_slice(event_id.as_bytes());
    msg.push(outcome_count);

    // Numeric type marker
    msg.push(0x01);
    msg.extend_from_slice(&lo_bound.to_be_bytes());
    msg.extend_from_slice(&hi_bound.to_be_bytes());
    msg.extend_from_slice(&precision.to_be_bytes());

    tagged_hash("Cashu_condition_id", &msg)
}

/// Compute proportional payout for numeric conditions (NUT-CTF-numeric).
///
/// Given a face amount, attested value, and bounds [lo, hi]:
/// - HI payout = floor(amount * (V - lo) / (hi - lo))
/// - LO payout = amount - HI payout (conservation)
///
/// At boundaries: V <= lo → HI=0, LO=amount; V >= hi → HI=amount, LO=0.
///
/// Returns (hi_payout, lo_payout).
pub fn compute_numeric_payout(
    face_amount: u64,
    attested_value: i64,
    lo_bound: i64,
    hi_bound: i64,
) -> Result<(u64, u64), Error> {
    if lo_bound >= hi_bound {
        return Err(Error::InvalidNumericRange(format!(
            "lo_bound ({}) must be less than hi_bound ({})",
            lo_bound, hi_bound
        )));
    }

    if attested_value <= lo_bound {
        return Ok((0, face_amount));
    }
    if attested_value >= hi_bound {
        return Ok((face_amount, 0));
    }

    // Use u128 intermediate arithmetic for overflow safety
    let numerator = (attested_value - lo_bound) as u128;
    let denominator = (hi_bound - lo_bound) as u128;
    let amount_128 = face_amount as u128;

    let hi_payout = (amount_128 * numerator / denominator) as u64;
    let lo_payout = face_amount - hi_payout; // conservation

    Ok((hi_payout, lo_payout))
}

/// Compute outcome_collection_id using EC point operations.
///
/// ```text
/// h = tagged_hash("Cashu_outcome_collection_id", condition_id || outcome_collection_string)
/// P = hash_to_curve(h)
/// if parent_collection_id is identity (all zeros): return x_only(P)
/// else: Q = lift_x(parent_collection_id); return x_only(Q + P)
/// ```
pub fn compute_outcome_collection_id(
    parent_collection_id: &[u8; 32],
    condition_id: &[u8; 32],
    outcome_collection_string: &str,
) -> Result<[u8; 32], Error> {
    use bitcoin::secp256k1::{PublicKey as SecpPublicKey, XOnlyPublicKey, Parity};

    // 1. Tagged hash: condition_id || outcome_collection_string
    let mut msg = Vec::new();
    msg.extend_from_slice(condition_id);
    msg.extend_from_slice(outcome_collection_string.as_bytes());
    let h = tagged_hash("Cashu_outcome_collection_id", &msg);

    // 2. hash_to_curve(h) -> secp256k1 point P
    let p_cashu = dhke::hash_to_curve(&h).map_err(|e| Error::HashToCurve(e.to_string()))?;
    let p = SecpPublicKey::from_slice(&p_cashu.to_bytes())
        .map_err(|_| Error::EcPointOperationFailed)?;

    // 3. Check if parent is identity (all zeros)
    let is_identity = parent_collection_id.iter().all(|&b| b == 0);

    if is_identity {
        // Return x_only(P)
        let (xonly, _parity) = p.x_only_public_key();
        Ok(xonly.serialize())
    } else {
        // lift_x(parent) -> Q, then Q + P
        let parent_xonly = XOnlyPublicKey::from_slice(parent_collection_id)
            .map_err(|_| Error::EcPointOperationFailed)?;
        let q = SecpPublicKey::from_x_only_public_key(parent_xonly, Parity::Even);
        let result = q.combine(&p).map_err(|_| Error::EcPointOperationFailed)?;
        let (result_xonly, _parity) = result.x_only_public_key();
        Ok(result_xonly.serialize())
    }
}

/// Parse outcome collection string into individual outcomes.
///
/// Handles escaping: `\|` is a literal pipe, `|` is a separator.
pub fn parse_outcome_collection(oc: &str) -> Vec<String> {
    let mut outcomes = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = oc.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() && chars[i + 1] == '|' {
            current.push('|');
            i += 2;
        } else if chars[i] == '|' {
            outcomes.push(std::mem::take(&mut current));
            i += 1;
        } else {
            current.push(chars[i]);
            i += 1;
        }
    }
    outcomes.push(current);
    outcomes
}

/// Validate that partition keys form a valid partition of all outcomes.
///
/// Returns Ok(()) if:
/// 1. No empty outcome strings
/// 2. Disjoint: no outcome appears in multiple collections
/// 3. Complete: every outcome appears in exactly one collection
pub fn validate_partition(outcomes: &[String], partition: &[String]) -> Result<(), Error> {
    let all_outcomes: HashSet<&str> = outcomes.iter().map(String::as_str).collect();
    let mut covered = HashSet::new();

    for key in partition {
        let oc_outcomes = parse_outcome_collection(key);
        for outcome in &oc_outcomes {
            if outcome.is_empty() {
                return Err(Error::EmptyOutcomeString);
            }
            if !all_outcomes.contains(outcome.as_str()) {
                return Err(Error::IncompletePartition);
            }
            if !covered.insert(outcome.clone()) {
                return Err(Error::OverlappingOutcomeCollections);
            }
        }
    }

    if covered.len() != all_outcomes.len() {
        return Err(Error::IncompletePartition);
    }

    Ok(())
}

/// Convert bytes to hex string
pub fn to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>()
}

/// Convert hex string to bytes
pub fn from_hex(hex: &str) -> Result<Vec<u8>, Error> {
    if hex.len() % 2 != 0 {
        return Err(Error::InvalidConditionId);
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|_| Error::InvalidConditionId)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tagged_hash() {
        // Basic sanity: same inputs produce same output
        let h1 = tagged_hash("test_tag", b"hello");
        let h2 = tagged_hash("test_tag", b"hello");
        assert_eq!(h1, h2);

        // Different tags produce different outputs
        let h3 = tagged_hash("other_tag", b"hello");
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_compute_condition_id_deterministic() {
        let pubkeys = vec![vec![0x02; 32]];
        let event_id = "test_event";

        let id1 = compute_condition_id(&pubkeys, event_id, 2);
        let id2 = compute_condition_id(&pubkeys, event_id, 2);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_compute_outcome_collection_id() {
        let condition_id = [0xab; 32];
        let parent = [0u8; 32]; // identity
        let oc_id1 = compute_outcome_collection_id(&parent, &condition_id, "YES").unwrap();
        let oc_id2 = compute_outcome_collection_id(&parent, &condition_id, "YES").unwrap();
        assert_eq!(oc_id1, oc_id2);

        let oc_id3 = compute_outcome_collection_id(&parent, &condition_id, "NO").unwrap();
        assert_ne!(oc_id1, oc_id3);
    }

    #[test]
    fn test_parse_outcome_collection() {
        assert_eq!(parse_outcome_collection("YES"), vec!["YES"]);
        assert_eq!(
            parse_outcome_collection("ALICE|BOB"),
            vec!["ALICE", "BOB"]
        );
        assert_eq!(
            parse_outcome_collection("ALICE|BOB|CAROL"),
            vec!["ALICE", "BOB", "CAROL"]
        );
        // Escaped pipe
        assert_eq!(
            parse_outcome_collection("A\\|B|C"),
            vec!["A|B", "C"]
        );
    }

    #[test]
    fn test_validate_partition_valid() {
        let outcomes = vec!["A".into(), "B".into(), "C".into()];

        // Individual outcomes
        assert!(validate_partition(
            &outcomes,
            &["A".into(), "B".into(), "C".into()]
        )
        .is_ok());

        // Outcome collections
        assert!(validate_partition(&outcomes, &["A|B".into(), "C".into()]).is_ok());

        // Single collection covering all
        assert!(validate_partition(&outcomes, &["A|B|C".into()]).is_ok());
    }

    #[test]
    fn test_validate_partition_overlapping() {
        let outcomes = vec!["A".into(), "B".into(), "C".into()];
        let result = validate_partition(&outcomes, &["A|B".into(), "B|C".into()]);
        assert!(matches!(result, Err(Error::OverlappingOutcomeCollections)));
    }

    #[test]
    fn test_validate_partition_incomplete() {
        let outcomes = vec!["A".into(), "B".into(), "C".into()];
        let result = validate_partition(&outcomes, &["A|B".into()]);
        assert!(matches!(result, Err(Error::IncompletePartition)));
    }

    #[test]
    fn test_hex_roundtrip() {
        let bytes = vec![0xde, 0xad, 0xbe, 0xef];
        let hex_str = to_hex(&bytes);
        assert_eq!(hex_str, "deadbeef");
        let decoded = from_hex(&hex_str).expect("valid hex");
        assert_eq!(decoded, bytes);
    }

    #[test]
    fn test_from_hex_invalid() {
        // Odd-length hex
        assert!(from_hex("abc").is_err());
        // Invalid hex chars
        assert!(from_hex("zzzz").is_err());
        // Empty hex should work
        assert_eq!(from_hex("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn test_compute_condition_id_different_pubkeys() {
        let event_id = "event1";

        let pubkeys_a = vec![vec![0x02; 32]];
        let pubkeys_b = vec![vec![0x03; 32]];

        let id_a = compute_condition_id(&pubkeys_a, event_id, 2);
        let id_b = compute_condition_id(&pubkeys_b, event_id, 2);
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn test_compute_condition_id_different_event_ids() {
        let pubkeys = vec![vec![0x02; 32]];

        let id_a = compute_condition_id(&pubkeys, "event_A", 2);
        let id_b = compute_condition_id(&pubkeys, "event_B", 2);
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn test_compute_condition_id_different_outcome_count() {
        let pubkeys = vec![vec![0x02; 32]];

        let id_a = compute_condition_id(&pubkeys, "event1", 2);
        let id_b = compute_condition_id(&pubkeys, "event1", 3);
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn test_compute_condition_id_pubkey_order_invariant() {
        let event_id = "event1";

        let pubkeys_ab = vec![vec![0x02; 32], vec![0x03; 32]];
        let pubkeys_ba = vec![vec![0x03; 32], vec![0x02; 32]];

        let id_ab = compute_condition_id(&pubkeys_ab, event_id, 2);
        let id_ba = compute_condition_id(&pubkeys_ba, event_id, 2);
        assert_eq!(id_ab, id_ba, "pubkey order should not affect condition_id");
    }

    #[test]
    fn test_compute_outcome_collection_id_deterministic_and_unique() {
        let cid = [0xab; 32];
        let parent = [0u8; 32]; // identity

        let oc1_a = compute_outcome_collection_id(&parent, &cid, "YES").unwrap();
        let oc1_b = compute_outcome_collection_id(&parent, &cid, "YES").unwrap();
        assert_eq!(oc1_a, oc1_b, "same inputs must produce same output");

        let oc2 = compute_outcome_collection_id(&parent, &cid, "NO").unwrap();
        assert_ne!(oc1_a, oc2, "different outcomes must produce different IDs");

        let cid2 = [0xcd; 32];
        let oc3 = compute_outcome_collection_id(&parent, &cid2, "YES").unwrap();
        assert_ne!(oc1_a, oc3, "different condition IDs must produce different IDs");
    }

    #[test]
    fn test_compute_outcome_collection_id_with_parent() {
        let cid = [0xab; 32];
        let parent_zero = [0u8; 32];

        let oc_root = compute_outcome_collection_id(&parent_zero, &cid, "YES").unwrap();

        // With a non-zero parent, result should differ
        let parent_nonzero = oc_root; // use the root result as parent
        let oc_nested = compute_outcome_collection_id(&parent_nonzero, &cid, "YES").unwrap();
        assert_ne!(oc_root, oc_nested, "different parents must produce different IDs");
    }

    #[test]
    fn test_validate_partition_extra_outcome() {
        let outcomes = vec!["A".into(), "B".into()];
        // "C" is not in outcomes
        let result = validate_partition(&outcomes, &["A".into(), "B".into(), "C".into()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_partition_duplicate_in_single_key() {
        let outcomes = vec!["A".into(), "B".into()];
        // "A|A" duplicates A
        let result = validate_partition(&outcomes, &["A|A".into(), "B".into()]);
        assert!(matches!(result, Err(Error::OverlappingOutcomeCollections)));
    }

    #[test]
    fn test_validate_partition_empty_outcome_string() {
        let outcomes = vec!["A".into(), "B".into(), "C".into()];
        // "A||B" parses as ["A", "", "B"] — empty string should be rejected
        let result = validate_partition(&outcomes, &["A||B".into(), "C".into()]);
        assert!(matches!(result, Err(Error::EmptyOutcomeString)));
    }

    #[test]
    fn test_validate_partition_empty() {
        let outcomes = vec!["A".into(), "B".into()];
        // Empty partition = incomplete
        let result = validate_partition(&outcomes, &[]);
        assert!(matches!(result, Err(Error::IncompletePartition)));
    }

    #[test]
    fn test_oracle_witness_serde_roundtrip() {
        let witness = OracleWitness {
            oracle_sigs: vec![OracleSig {
                oracle_pubkey: "a".repeat(64),
                oracle_sig: Some("b".repeat(128)),
                outcome: "YES".to_string(),
                digit_sigs: None,
            }],
        };

        let json = serde_json::to_string(&witness).expect("serialize");
        let deser: OracleWitness = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(witness, deser);
    }

    #[test]
    fn test_oracle_witness_serde_no_sig() {
        let witness = OracleWitness {
            oracle_sigs: vec![OracleSig {
                oracle_pubkey: "a".repeat(64),
                oracle_sig: None,
                outcome: "YES".to_string(),
                digit_sigs: None,
            }],
        };

        let json = serde_json::to_string(&witness).expect("serialize");
        // oracle_sig: None should be skipped in serialization
        // But "oracle_sigs" (the array field) will be present, so check for the value key
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let sig_entry = &value["oracle_sigs"][0];
        assert!(sig_entry.get("oracle_sig").is_none(), "oracle_sig should be skipped when None");
        let deser: OracleWitness = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(witness, deser);
    }

    #[test]
    fn test_ctf_settings_default() {
        let settings = NutCtfSettings::default();
        assert!(settings.supported);
        assert_eq!(settings.dlc_version, Some("0".to_string()));
        assert!(settings.vesting_period.is_some());
    }

    #[test]
    fn test_ctf_settings_serde() {
        let settings = NutCtfSettings {
            supported: true,
            dlc_version: Some("0".to_string()),
            vesting_period: None,
        };
        let json = serde_json::to_string(&settings).expect("serialize");
        assert!(!json.contains("vesting_period"));
        let deser: NutCtfSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(settings, deser);
    }

    #[test]
    fn test_condition_id_is_32_bytes() {
        let pubkeys = vec![vec![0x02; 32]];
        let cid = compute_condition_id(&pubkeys, "event", 2);
        assert_eq!(cid.len(), 32);
    }

    #[test]
    fn test_outcome_collection_id_is_32_bytes() {
        let cid = [0x00; 32];
        let parent = [0u8; 32];
        let ocid = compute_outcome_collection_id(&parent, &cid, "YES").unwrap();
        assert_eq!(ocid.len(), 32);
    }

    #[test]
    fn test_zero_collection_id_format() {
        assert_eq!(ZERO_COLLECTION_ID.len(), 64);
        assert!(ZERO_COLLECTION_ID.chars().all(|c| c == '0'));
    }

    #[test]
    fn test_compute_condition_id_numeric_differs_from_enum() {
        let pubkeys = vec![vec![0x02; 32]];
        let event_id = "test_event";

        let enum_id = compute_condition_id(&pubkeys, event_id, 2);
        let numeric_id = compute_condition_id_numeric(&pubkeys, event_id, 2, 0, 100000, 0);
        assert_ne!(enum_id, numeric_id, "numeric and enum condition IDs should differ");
    }

    #[test]
    fn test_compute_condition_id_numeric_deterministic() {
        let pubkeys = vec![vec![0x02; 32]];
        let id1 = compute_condition_id_numeric(&pubkeys, "evt", 2, 0, 100000, 0);
        let id2 = compute_condition_id_numeric(&pubkeys, "evt", 2, 0, 100000, 0);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_compute_condition_id_numeric_different_bounds() {
        let pubkeys = vec![vec![0x02; 32]];
        let id1 = compute_condition_id_numeric(&pubkeys, "evt", 2, 0, 100000, 0);
        let id2 = compute_condition_id_numeric(&pubkeys, "evt", 2, 0, 200000, 0);
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_numeric_payout_midpoint() {
        // Range [0, 100000], V=50000, amount=100 → HI=50, LO=50
        let (hi, lo) = compute_numeric_payout(100, 50000, 0, 100000).unwrap();
        assert_eq!(hi, 50);
        assert_eq!(lo, 50);
        assert_eq!(hi + lo, 100); // conservation
    }

    #[test]
    fn test_numeric_payout_at_lo_boundary() {
        // V <= lo_bound → HI=0, LO=amount
        let (hi, lo) = compute_numeric_payout(100, 0, 0, 100000).unwrap();
        assert_eq!(hi, 0);
        assert_eq!(lo, 100);

        // V below lo_bound
        let (hi, lo) = compute_numeric_payout(100, -5, 0, 100000).unwrap();
        assert_eq!(hi, 0);
        assert_eq!(lo, 100);
    }

    #[test]
    fn test_numeric_payout_at_hi_boundary() {
        // V >= hi_bound → HI=amount, LO=0
        let (hi, lo) = compute_numeric_payout(100, 100000, 0, 100000).unwrap();
        assert_eq!(hi, 100);
        assert_eq!(lo, 0);

        // V above hi_bound
        let (hi, lo) = compute_numeric_payout(100, 150000, 0, 100000).unwrap();
        assert_eq!(hi, 100);
        assert_eq!(lo, 0);
    }

    #[test]
    fn test_numeric_payout_conservation() {
        // For any value, hi + lo = face_amount
        for v in [0, 20000, 50000, 80000, 100000] {
            let (hi, lo) = compute_numeric_payout(100, v, 0, 100000).unwrap();
            assert_eq!(hi + lo, 100, "conservation violated at V={}", v);
        }
    }

    #[test]
    fn test_numeric_payout_partial() {
        // Range [0, 100000], V=20000, amount=100 → HI=20, LO=80
        let (hi, lo) = compute_numeric_payout(100, 20000, 0, 100000).unwrap();
        assert_eq!(hi, 20);
        assert_eq!(lo, 80);
    }

    #[test]
    fn test_numeric_payout_invalid_range() {
        // lo >= hi should fail
        assert!(compute_numeric_payout(100, 50, 100, 100).is_err());
        assert!(compute_numeric_payout(100, 50, 200, 100).is_err());
    }

    #[test]
    fn test_numeric_payout_precision() {
        // Range [0, 3], V=1, amount=3: HI = floor(3 * 1/3) = 1, LO = 2
        let (hi, lo) = compute_numeric_payout(3, 1, 0, 3).unwrap();
        assert_eq!(hi, 1);
        assert_eq!(lo, 2);
        assert_eq!(hi + lo, 3);

        // Range [0, 3], V=2, amount=3: HI = floor(3 * 2/3) = 2, LO = 1
        let (hi, lo) = compute_numeric_payout(3, 2, 0, 3).unwrap();
        assert_eq!(hi, 2);
        assert_eq!(lo, 1);
        assert_eq!(hi + lo, 3);

        // Range [0, 10], V=3, amount=10: HI = floor(10 * 3/10) = 3, LO = 7
        let (hi, lo) = compute_numeric_payout(10, 3, 0, 10).unwrap();
        assert_eq!(hi, 3);
        assert_eq!(lo, 7);
        assert_eq!(hi + lo, 10);
    }

    #[test]
    fn test_numeric_payout_single_unit() {
        // amount=1: result is always 0 or 1, never a fraction
        // At midpoint: HI = floor(1 * 50/100) = 0, LO = 1
        let (hi, lo) = compute_numeric_payout(1, 50, 0, 100).unwrap();
        assert_eq!(hi, 0);
        assert_eq!(lo, 1);
        assert_eq!(hi + lo, 1);

        // Near hi boundary: HI = floor(1 * 99/100) = 0, LO = 1
        let (hi, lo) = compute_numeric_payout(1, 99, 0, 100).unwrap();
        assert_eq!(hi, 0);
        assert_eq!(lo, 1);
        assert_eq!(hi + lo, 1);

        // At hi boundary: HI = 1, LO = 0
        let (hi, lo) = compute_numeric_payout(1, 100, 0, 100).unwrap();
        assert_eq!(hi, 1);
        assert_eq!(lo, 0);
        assert_eq!(hi + lo, 1);
    }

    #[test]
    fn test_oracle_witness_serde_digit_sigs() {
        let witness = OracleWitness {
            oracle_sigs: vec![OracleSig {
                oracle_pubkey: "a".repeat(64),
                oracle_sig: None,
                outcome: "HI".to_string(),
                digit_sigs: Some(vec!["c".repeat(128), "d".repeat(128)]),
            }],
        };

        let json = serde_json::to_string(&witness).expect("serialize");
        let deser: OracleWitness = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(witness, deser);
        assert_eq!(deser.oracle_sigs[0].digit_sigs.as_ref().unwrap().len(), 2);
    }
}

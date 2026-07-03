//! Private REST transport payloads for federation members.
//!
//! These types are the stable JSON surface for member-to-member operation
//! submission and catch-up. The first router checkpoint can use them before a
//! real consensus engine is wired behind the endpoints.

use core::fmt;
use core::str::FromStr;
use std::collections::BTreeMap;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    FederationConfig, FederationError, FederationId, FederationJournalAppendOutcome,
    FederationJournalDigest, FederationJournalEntry, FederationOperationEnvelope,
    FederationOperationId, InMemoryFederationJournal, MemberConfigPrivate, MemberId, Result,
};

/// Version of the private federation REST payload format.
pub const FEDERATION_REST_API_VERSION: u16 = 1;

const FEDERATION_REST_BODY_HASH_VERSION: &[u8] = b"cdk-federation-rest-body-v1";
const FEDERATION_REST_SIGNATURE_VERSION: &[u8] = b"cdk-federation-rest-signature-v1";

/// Private federation REST message kind covered by member signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationRestMessageKind {
    /// Peer status request.
    Status,
    /// Submit one operation envelope to a peer.
    SubmitOperation,
    /// Request missing journal entries from a peer.
    CatchUp,
}

impl FederationRestMessageKind {
    fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Status => b"status",
            Self::SubmitOperation => b"submit_operation",
            Self::CatchUp => b"catch_up",
        }
    }
}

/// Canonical SHA-256 hash of a signed federation REST request payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationRestBodyHash([u8; 32]);

impl FederationRestBodyHash {
    /// Create a body hash from raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse a body hash from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidRestBodyHashSize {
                    expected: 32,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a body hash from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this body hash as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return this body hash as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    /// Compute the canonical payload hash.
    pub fn for_payload<T>(payload: &T) -> Result<Self>
    where
        T: Serialize + ?Sized,
    {
        let canonical = canonical_json_bytes(payload)?;
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, FEDERATION_REST_BODY_HASH_VERSION);
        append_hash_bytes(&mut transcript, &canonical);
        Ok(Self(Sha256Hash::hash(&transcript).to_byte_array()))
    }
}

impl fmt::Display for FederationRestBodyHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationRestBodyHash {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationRestBodyHash {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationRestBodyHash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Schnorr signature over a federation REST request transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FederationRestSignature([u8; 64]);

impl FederationRestSignature {
    /// Create a transport signature from raw Schnorr bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 64] =
            bytes
                .try_into()
                .map_err(|_| FederationError::InvalidRestSignatureSize {
                    expected: 64,
                    found: bytes.len(),
                })?;
        Ok(Self(bytes))
    }

    /// Parse a transport signature from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return this signature as bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 64] {
        self.0
    }

    /// Return this signature as hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }

    fn sign(transcript: &[u8], private_config: &MemberConfigPrivate) -> Result<Self> {
        let signature = private_config.identity_secret_key.sign(transcript)?;
        Ok(Self(signature.serialize()))
    }

    fn as_schnorr(self) -> Result<Signature> {
        Signature::from_slice(&self.0).map_err(|_| FederationError::InvalidRestSignature)
    }
}

impl fmt::Display for FederationRestSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for FederationRestSignature {
    type Err = FederationError;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_hex(value)
    }
}

impl Serialize for FederationRestSignature {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationRestSignature {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(value).map_err(serde::de::Error::custom)
    }
}

/// Signed private REST request between two federation members.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationSignedRequest<T> {
    /// Private REST payload version.
    pub version: u16,
    /// Federation addressed by this request.
    pub federation_id: FederationId,
    /// Member that signed the request.
    pub sender: MemberId,
    /// Member expected to receive the request.
    pub receiver: MemberId,
    /// Monotonic sender-local sequence used to reject simple replays.
    pub sequence: u64,
    /// Request payload kind.
    pub kind: FederationRestMessageKind,
    /// Canonical hash of `payload`.
    pub body_hash: FederationRestBodyHash,
    /// JSON payload covered by `body_hash`.
    pub payload: T,
    /// Schnorr signature over the request transcript.
    pub signature: FederationRestSignature,
}

impl<T> FederationSignedRequest<T>
where
    T: Serialize,
{
    /// Sign a private REST request from the local member to `receiver`.
    pub fn new_signed(
        federation_id: FederationId,
        receiver: MemberId,
        sequence: u64,
        kind: FederationRestMessageKind,
        payload: T,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        let sender = private_config.member_id;
        let body_hash = FederationRestBodyHash::for_payload(&payload)?;
        let transcript =
            signing_transcript(federation_id, sender, receiver, sequence, kind, body_hash);
        let signature = FederationRestSignature::sign(&transcript, private_config)?;

        Ok(Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            sender,
            receiver,
            sequence,
            kind,
            body_hash,
            payload,
            signature,
        })
    }

    /// Verify the request envelope, payload hash, sender roster membership, and signature.
    pub fn verify(
        &self,
        public_config: &FederationConfig,
        expected_receiver: MemberId,
    ) -> Result<()> {
        if self.version != FEDERATION_REST_API_VERSION {
            return Err(FederationError::RestVersionMismatch {
                expected: FEDERATION_REST_API_VERSION,
                found: self.version,
            });
        }

        if self.federation_id != public_config.federation_id {
            return Err(FederationError::RestFederationMismatch {
                expected: public_config.federation_id,
                found: self.federation_id,
            });
        }

        if self.receiver != expected_receiver {
            return Err(FederationError::RestReceiverMismatch {
                expected: expected_receiver,
                found: self.receiver,
            });
        }

        let member = public_config
            .member(self.sender)
            .ok_or(FederationError::UnknownMember {
                member_id: self.sender,
            })?;

        let found_body_hash = FederationRestBodyHash::for_payload(&self.payload)?;
        if self.body_hash != found_body_hash {
            return Err(FederationError::RestBodyHashMismatch {
                expected: self.body_hash,
                found: found_body_hash,
            });
        }

        let transcript = signing_transcript(
            self.federation_id,
            self.sender,
            self.receiver,
            self.sequence,
            self.kind,
            self.body_hash,
        );
        let signature = self.signature.as_schnorr()?;
        member
            .identity_public_key
            .verify(&transcript, &signature)
            .map_err(|_| FederationError::RestSignatureVerificationFailed {
                member_id: self.sender,
            })?;

        Ok(())
    }
}

/// In-memory monotonic sequence tracker for authenticated federation REST requests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FederationRestSequenceTracker {
    highest_by_sender: BTreeMap<MemberId, u64>,
}

impl FederationRestSequenceTracker {
    /// Create an empty sequence tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild a sequence tracker from persisted sender watermarks.
    #[must_use]
    pub fn from_sequences(highest_by_sender: BTreeMap<MemberId, u64>) -> Self {
        Self { highest_by_sender }
    }

    /// Return persisted sender watermarks known to this tracker.
    #[must_use]
    pub fn sequences(&self) -> &BTreeMap<MemberId, u64> {
        &self.highest_by_sender
    }

    /// Verify a request and record its sequence if it advances the sender stream.
    pub fn verify_and_record<T>(
        &mut self,
        request: &FederationSignedRequest<T>,
        public_config: &FederationConfig,
        expected_receiver: MemberId,
    ) -> Result<()>
    where
        T: Serialize,
    {
        self.verify_next(request, public_config, expected_receiver)?;
        self.record_sequence(request.sender, request.sequence);

        Ok(())
    }

    /// Verify a request without recording its sequence.
    pub fn verify_next<T>(
        &self,
        request: &FederationSignedRequest<T>,
        public_config: &FederationConfig,
        expected_receiver: MemberId,
    ) -> Result<()>
    where
        T: Serialize,
    {
        request.verify(public_config, expected_receiver)?;

        self.ensure_sequence_advances(request.sender, request.sequence)
    }

    /// Record an already-verified sender sequence.
    pub fn record_sequence(&mut self, sender: MemberId, sequence: u64) {
        match self.highest_by_sender.get(&sender) {
            Some(last) if *last >= sequence => {}
            _ => {
                self.highest_by_sender.insert(sender, sequence);
            }
        }
    }

    fn ensure_sequence_advances(&self, sender: MemberId, sequence: u64) -> Result<()> {
        if let Some(last) = self.highest_by_sender.get(&sender) {
            if sequence <= *last {
                return Err(FederationError::RestStaleSequence {
                    member_id: sender,
                    last: *last,
                    found: sequence,
                });
            }
        }

        Ok(())
    }
}

/// Empty signed payload used to request a peer status snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationPeerStatusRequest {}

/// Peer status response for federation health and catch-up checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationPeerStatus {
    /// Private REST payload version.
    pub version: u16,
    /// Federation served by this peer.
    pub federation_id: FederationId,
    /// Member ID of this peer.
    pub member_id: MemberId,
    /// Number of accepted operations in the local journal.
    pub operation_count: u64,
    /// Highest accepted operation index, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_index: Option<u64>,
    /// Latest accepted-order digest.
    pub latest_order_digest: FederationJournalDigest,
}

impl FederationPeerStatus {
    /// Create a peer status snapshot.
    #[must_use]
    pub fn new(
        federation_id: FederationId,
        member_id: MemberId,
        operation_count: u64,
        latest_index: Option<u64>,
        latest_order_digest: FederationJournalDigest,
    ) -> Self {
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            member_id,
            operation_count,
            latest_index,
            latest_order_digest,
        }
    }

    /// Create a peer status snapshot from the in-memory journal scaffold.
    #[must_use]
    pub fn from_journal(
        federation_id: FederationId,
        member_id: MemberId,
        journal: &InMemoryFederationJournal,
    ) -> Self {
        let latest_index = journal.entries().last().map(|entry| entry.index);
        Self::new(
            federation_id,
            member_id,
            journal.len() as u64,
            latest_index,
            journal.latest_order_digest(),
        )
    }
}

/// Request to submit an operation envelope to a federation peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationSubmitOperationRequest {
    /// Operation envelope proposed for consensus.
    pub envelope: FederationOperationEnvelope,
}

impl FederationSubmitOperationRequest {
    /// Create an operation submit request.
    #[must_use]
    pub fn new(envelope: FederationOperationEnvelope) -> Self {
        Self { envelope }
    }

    /// Compute the submitted operation ID.
    pub fn operation_id(&self) -> super::Result<FederationOperationId> {
        self.envelope.operation_id()
    }
}

/// Response to an operation submit request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationSubmitOperationResponse {
    /// Submitted operation ID.
    pub operation_id: FederationOperationId,
    /// Journal append result.
    pub outcome: FederationJournalAppendOutcome,
}

impl FederationSubmitOperationResponse {
    /// Create a submit response from a journal append outcome.
    #[must_use]
    pub fn from_outcome(outcome: FederationJournalAppendOutcome) -> Self {
        Self {
            operation_id: outcome.entry().operation_id,
            outcome,
        }
    }
}

/// Request journal entries starting at the next missing index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationJournalCatchUpRequest {
    /// First missing zero-based journal index.
    pub next_index: u64,
    /// Optional maximum entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
    /// Optional digest the requester has after `next_index - 1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_order_digest: Option<FederationJournalDigest>,
}

impl FederationJournalCatchUpRequest {
    /// Create a catch-up request from a next missing index.
    #[must_use]
    pub fn new(next_index: u64) -> Self {
        Self {
            next_index,
            limit: None,
            known_order_digest: None,
        }
    }

    /// Set an optional entry limit.
    #[must_use]
    pub fn with_limit(mut self, limit: u16) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the requester's known order digest.
    #[must_use]
    pub fn with_known_order_digest(mut self, digest: FederationJournalDigest) -> Self {
        self.known_order_digest = Some(digest);
        self
    }

    /// Return the bounded limit to apply to a response.
    #[must_use]
    pub fn effective_limit(&self, default_limit: u16, max_limit: u16) -> usize {
        let requested = self.limit.unwrap_or(default_limit).min(max_limit);
        usize::from(requested)
    }
}

/// Catch-up response containing accepted journal entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationJournalCatchUpResponse {
    /// Private REST payload version.
    pub version: u16,
    /// Federation served by this peer.
    pub federation_id: FederationId,
    /// First requested index.
    pub from_index: u64,
    /// Journal entries returned in ascending order.
    pub entries: Vec<FederationJournalEntry>,
    /// Highest local operation index, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_index: Option<u64>,
    /// Latest local accepted-order digest.
    pub latest_order_digest: FederationJournalDigest,
    /// Whether the peer returned all entries currently available from `from_index`.
    pub complete: bool,
}

impl FederationJournalCatchUpResponse {
    /// Create a catch-up response from an in-memory journal.
    #[must_use]
    pub fn from_journal(
        federation_id: FederationId,
        journal: &InMemoryFederationJournal,
        request: &FederationJournalCatchUpRequest,
        default_limit: u16,
        max_limit: u16,
    ) -> Self {
        let mut entries = journal.entries_from(request.next_index);
        let limit = request.effective_limit(default_limit, max_limit);
        let complete = entries.len() <= limit;
        entries.truncate(limit);
        Self {
            version: FEDERATION_REST_API_VERSION,
            federation_id,
            from_index: request.next_index,
            entries,
            latest_index: journal.entries().last().map(|entry| entry.index),
            latest_order_digest: journal.latest_order_digest(),
            complete,
        }
    }
}

fn signing_transcript(
    federation_id: FederationId,
    sender: MemberId,
    receiver: MemberId,
    sequence: u64,
    kind: FederationRestMessageKind,
    body_hash: FederationRestBodyHash,
) -> Vec<u8> {
    let mut transcript = Vec::new();
    append_hash_bytes(&mut transcript, FEDERATION_REST_SIGNATURE_VERSION);
    transcript.extend_from_slice(&FEDERATION_REST_API_VERSION.to_be_bytes());
    append_hash_bytes(&mut transcript, &federation_id.to_bytes());
    transcript.extend_from_slice(&sender.get().to_be_bytes());
    transcript.extend_from_slice(&receiver.get().to_be_bytes());
    transcript.extend_from_slice(&sequence.to_be_bytes());
    append_hash_bytes(&mut transcript, kind.as_bytes());
    append_hash_bytes(&mut transcript, &body_hash.to_bytes());
    transcript
}

fn canonical_json_bytes<T>(value: &T) -> Result<Vec<u8>>
where
    T: Serialize + ?Sized,
{
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(serde_json::to_vec(&value)?)
}

fn append_hash_bytes(transcript: &mut Vec<u8>, value: &[u8]) {
    transcript.extend_from_slice(&(value.len() as u32).to_be_bytes());
    transcript.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use super::super::{
        FederationConfig, FederationError, MemberConfigPrivate, MemberConfigPublic, ThresholdParams,
    };
    use super::*;
    use crate::{
        mint_url::MintUrl, Amount, BlindedMessage, CurrencyUnit, Id, MintQuoteBolt11Request,
        MintQuoteBolt11Response, MintQuoteRequest, MintQuoteResponse, MintRequest, PublicKey,
        SecretKey,
    };

    fn federation_id() -> FederationId {
        FederationId::from_bytes([11; 32])
    }

    fn member_id() -> MemberId {
        MemberId::new(1).expect("member id")
    }

    fn roster_member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    fn identity_secret(id: u8) -> SecretKey {
        SecretKey::from_slice(&[id; 32]).expect("identity secret")
    }

    fn member_config(id: u16, identity_secret: &SecretKey) -> MemberConfigPublic {
        MemberConfigPublic {
            member_id: roster_member_id(id),
            public_mint_url: MintUrl::from_str(&format!("https://mint{}.example.com", id))
                .expect("public mint url"),
            federation_api_url: MintUrl::from_str(&format!("https://fed{}.example.com", id))
                .expect("federation api url"),
            identity_public_key: identity_secret.public_key(),
        }
    }

    fn private_member(id: u16, identity_secret: SecretKey) -> MemberConfigPrivate {
        MemberConfigPrivate {
            member_id: roster_member_id(id),
            identity_secret_key: identity_secret,
            keyset_secret_shares: BTreeMap::new(),
        }
    }

    fn transport_config() -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
        let first_secret = identity_secret(1);
        let second_secret = identity_secret(2);
        let members = BTreeMap::from([
            (roster_member_id(1), member_config(1, &first_secret)),
            (roster_member_id(2), member_config(2, &second_secret)),
        ]);
        let threshold_params = ThresholdParams::new(2, 2, 2).expect("threshold params");
        let config = FederationConfig::new(threshold_params, members, Vec::new())
            .expect("federation config");
        let first_private = private_member(1, first_secret);
        let second_private = private_member(2, second_secret);
        (config, first_private, second_private)
    }

    fn keyset_id() -> Id {
        Id::from_str("00bfa73302d12ffd").expect("keyset id")
    }

    fn output(hex: &str) -> BlindedMessage {
        BlindedMessage::new(
            Amount::from(1),
            keyset_id(),
            PublicKey::from_hex(hex).expect("public key"),
        )
    }

    fn mint_envelope(quote: &str, hex: &str) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id(),
            super::super::FederationOperation::Mint(super::super::FederationMintOperation {
                request: super::super::FederationMintRequest::Single(MintRequest {
                    quote: quote.to_string(),
                    outputs: vec![output(hex)],
                    signature: None,
                }),
            }),
        )
    }

    fn mint_quote_envelope() -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id(),
            super::super::FederationOperation::MintQuote(
                super::super::FederationMintQuoteOperation {
                    quote: "quote-1".to_string(),
                    request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                        amount: Amount::from(1),
                        unit: CurrencyUnit::Sat,
                        description: None,
                        pubkey: None,
                    }),
                    response: MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                        quote: "quote-1".to_string(),
                        request: "invoice-quote-1".to_string(),
                        amount: Some(Amount::from(1)),
                        unit: Some(CurrencyUnit::Sat),
                        state: crate::MintQuoteState::Unpaid,
                        expiry: None,
                        pubkey: None,
                    }),
                },
            ),
        )
    }

    #[test]
    fn peer_status_serializes_with_journal_digest() {
        let mut journal = InMemoryFederationJournal::new();
        let entry = journal
            .append(mint_quote_envelope())
            .expect("append")
            .entry()
            .clone();
        let status = FederationPeerStatus::from_journal(federation_id(), member_id(), &journal);
        let json = serde_json::to_string(&status).expect("status json");
        let decoded: FederationPeerStatus = serde_json::from_str(&json).expect("status");

        assert_eq!(decoded.version, FEDERATION_REST_API_VERSION);
        assert_eq!(decoded.latest_index, Some(entry.index));
        assert_eq!(decoded.operation_count, 1);
        assert_eq!(decoded.latest_order_digest, entry.order_digest);
    }

    #[test]
    fn submit_response_uses_append_outcome_operation_id() {
        let mut journal = InMemoryFederationJournal::new();
        let outcome = journal.append(mint_quote_envelope()).expect("append");
        let response = FederationSubmitOperationResponse::from_outcome(outcome.clone());

        assert_eq!(response.operation_id, outcome.entry().operation_id);
        serde_json::to_string(&response).expect("submit response json");
    }

    #[test]
    fn catch_up_response_applies_limits_and_reports_completeness() {
        let mut journal = InMemoryFederationJournal::new();
        journal.append(mint_quote_envelope()).expect("append quote");
        journal
            .append(mint_envelope(
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("append mint");
        journal
            .append(mint_envelope(
                "quote-2",
                "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
            ))
            .expect("append second mint");

        let request = FederationJournalCatchUpRequest::new(1)
            .with_limit(1)
            .with_known_order_digest(journal.entries()[0].order_digest);
        let response = FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &request,
            50,
            100,
        );

        assert_eq!(request.effective_limit(50, 100), 1);
        assert_eq!(response.from_index, 1);
        assert_eq!(response.entries.len(), 1);
        assert!(!response.complete);
        assert_eq!(response.latest_index, Some(2));
        assert_eq!(response.latest_order_digest, journal.latest_order_digest());
    }

    #[test]
    fn signed_request_verifies_sender_and_payload_hash() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let payload = FederationPeerStatusRequest::default();
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            1,
            FederationRestMessageKind::Status,
            payload,
            &first_private,
        )
        .expect("signed request");

        request.verify(&config, receiver).expect("verified request");
        let json = serde_json::to_string(&request).expect("signed request json");
        let decoded: FederationSignedRequest<FederationPeerStatusRequest> =
            serde_json::from_str(&json).expect("decoded request");
        decoded.verify(&config, receiver).expect("decoded verifies");
    }

    #[test]
    fn signed_request_rejects_tampered_payload() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let payload = FederationJournalCatchUpRequest::new(1);
        let mut request = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            1,
            FederationRestMessageKind::CatchUp,
            payload,
            &first_private,
        )
        .expect("signed request");

        request.payload.next_index = 2;
        assert!(matches!(
            request.verify(&config, receiver),
            Err(FederationError::RestBodyHashMismatch { .. })
        ));
    }

    #[test]
    fn signed_request_rejects_wrong_federation() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let request = FederationSignedRequest::new_signed(
            FederationId::from_bytes([99; 32]),
            receiver,
            1,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &first_private,
        )
        .expect("signed request");

        assert!(matches!(
            request.verify(&config, receiver),
            Err(FederationError::RestFederationMismatch { .. })
        ));
    }

    #[test]
    fn signed_request_rejects_wrong_receiver() {
        let (config, first_private, _) = transport_config();
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            roster_member_id(2),
            1,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &first_private,
        )
        .expect("signed request");

        assert!(matches!(
            request.verify(&config, roster_member_id(1)),
            Err(FederationError::RestReceiverMismatch { .. })
        ));
    }

    #[test]
    fn signed_request_rejects_unknown_sender() {
        let (config, _, _) = transport_config();
        let unknown_private = private_member(3, identity_secret(3));
        let request = FederationSignedRequest::new_signed(
            config.federation_id,
            roster_member_id(2),
            1,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &unknown_private,
        )
        .expect("signed request");

        assert!(matches!(
            request.verify(&config, roster_member_id(2)),
            Err(FederationError::UnknownMember { member_id }) if member_id == roster_member_id(3)
        ));
    }

    #[test]
    fn signed_request_rejects_signature_for_different_sender() {
        let (config, _, second_private) = transport_config();
        let mut request = FederationSignedRequest::new_signed(
            config.federation_id,
            roster_member_id(1),
            1,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &second_private,
        )
        .expect("signed request");
        request.sender = roster_member_id(1);

        assert!(matches!(
            request.verify(&config, roster_member_id(1)),
            Err(FederationError::RestSignatureVerificationFailed { member_id })
                if member_id == roster_member_id(1)
        ));
    }

    #[test]
    fn sequence_tracker_rejects_replayed_or_stale_request() {
        let (config, first_private, _) = transport_config();
        let receiver = roster_member_id(2);
        let first = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            5,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &first_private,
        )
        .expect("first request");
        let replay = first.clone();
        let second = FederationSignedRequest::new_signed(
            config.federation_id,
            receiver,
            6,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            &first_private,
        )
        .expect("second request");

        let mut tracker = FederationRestSequenceTracker::new();
        tracker
            .verify_and_record(&first, &config, receiver)
            .expect("first accepted");
        assert!(matches!(
            tracker.verify_and_record(&replay, &config, receiver),
            Err(FederationError::RestStaleSequence {
                member_id,
                last: 5,
                found: 5,
            }) if member_id == roster_member_id(1)
        ));
        tracker
            .verify_and_record(&second, &config, receiver)
            .expect("higher sequence accepted");
    }
}

//! Private REST transport payloads for federation members.
//!
//! These types are the stable JSON surface for member-to-member operation
//! submission and catch-up. The first router checkpoint can use them before a
//! real consensus engine is wired behind the endpoints.

use serde::{Deserialize, Serialize};

use super::{
    FederationId, FederationJournalAppendOutcome, FederationJournalDigest, FederationJournalEntry,
    FederationOperationEnvelope, FederationOperationId, InMemoryFederationJournal, MemberId,
};

/// Version of the private federation REST payload format.
pub const FEDERATION_REST_API_VERSION: u16 = 1;

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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::{
        Amount, BlindedMessage, CurrencyUnit, Id, MintQuoteBolt11Request, MintQuoteBolt11Response,
        MintQuoteRequest, MintQuoteResponse, MintRequest, PublicKey,
    };

    fn federation_id() -> FederationId {
        FederationId::from_bytes([11; 32])
    }

    fn member_id() -> MemberId {
        MemberId::new(1).expect("member id")
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
}

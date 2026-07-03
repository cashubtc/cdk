//! Deterministic local consensus scaffold.
//!
//! This is not a quorum protocol. It is the first consensus-facing boundary:
//! validate federation identity, append operations to the journal, and expose
//! the same status/catch-up/submit payloads used by private REST. Real peer
//! networking can replace the decision source while keeping this API shape.

use super::{
    FederationError, FederationId, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationJournalEntry, FederationOperationEnvelope,
    FederationOperationId, FederationOperationStatus, FederationPeerStatus,
    FederationSubmitOperationRequest, FederationSubmitOperationResponse, InMemoryFederationJournal,
    MemberId, Result,
};

const DEFAULT_CATCH_UP_LIMIT: u16 = 100;
const MAX_CATCH_UP_LIMIT: u16 = 1000;

/// Deterministic local consensus driver used before real quorum networking.
#[derive(Debug, Clone)]
pub struct InMemoryFederationConsensus {
    federation_id: FederationId,
    member_id: MemberId,
    journal: InMemoryFederationJournal,
}

impl InMemoryFederationConsensus {
    /// Create an empty local consensus driver for one federation member.
    #[must_use]
    pub fn new(federation_id: FederationId, member_id: MemberId) -> Self {
        Self {
            federation_id,
            member_id,
            journal: InMemoryFederationJournal::new(),
        }
    }

    /// Return this member's federation ID.
    #[must_use]
    pub fn federation_id(&self) -> FederationId {
        self.federation_id
    }

    /// Return this member's ID.
    #[must_use]
    pub fn member_id(&self) -> MemberId {
        self.member_id
    }

    /// Return the underlying journal.
    #[must_use]
    pub fn journal(&self) -> &InMemoryFederationJournal {
        &self.journal
    }

    /// Return a mutable reference to the underlying journal.
    pub fn journal_mut(&mut self) -> &mut InMemoryFederationJournal {
        &mut self.journal
    }

    /// Submit an already-built operation envelope.
    pub fn submit_envelope(
        &mut self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse> {
        self.ensure_federation(envelope.federation_id)?;
        let outcome = self.journal.append(envelope)?;
        Ok(FederationSubmitOperationResponse::from_outcome(outcome))
    }

    /// Submit an operation request from the private REST transport.
    pub fn submit_operation(
        &mut self,
        request: FederationSubmitOperationRequest,
    ) -> Result<FederationSubmitOperationResponse> {
        self.submit_envelope(request.envelope)
    }

    /// Return this member's current peer status.
    #[must_use]
    pub fn peer_status(&self) -> FederationPeerStatus {
        FederationPeerStatus::from_journal(self.federation_id, self.member_id, &self.journal)
    }

    /// Return journal entries for a catch-up request.
    #[must_use]
    pub fn catch_up(
        &self,
        request: &FederationJournalCatchUpRequest,
    ) -> FederationJournalCatchUpResponse {
        FederationJournalCatchUpResponse::from_journal(
            self.federation_id,
            &self.journal,
            request,
            DEFAULT_CATCH_UP_LIMIT,
            MAX_CATCH_UP_LIMIT,
        )
    }

    /// Update local application status for an accepted operation.
    pub fn update_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<FederationJournalEntry> {
        self.journal.update_status(operation_id, status)
    }

    fn ensure_federation(&self, found: FederationId) -> Result<()> {
        if found != self.federation_id {
            return Err(FederationError::OperationFederationMismatch {
                expected: self.federation_id,
                found,
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::{
        Amount, BlindedMessage, CurrencyUnit, Id, MintQuoteBolt11Request, MintQuoteRequest,
        MintRequest, PublicKey,
    };

    fn federation_id(byte: u8) -> FederationId {
        FederationId::from_bytes([byte; 32])
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

    fn mint_envelope(
        federation_id: FederationId,
        quote: &str,
        hex: &str,
    ) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id,
            super::super::FederationOperation::Mint(super::super::FederationMintOperation {
                request: super::super::FederationMintRequest::Single(MintRequest {
                    quote: quote.to_string(),
                    outputs: vec![output(hex)],
                    signature: None,
                }),
            }),
        )
    }

    fn mint_quote_envelope(federation_id: FederationId) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id,
            super::super::FederationOperation::MintQuote(
                super::super::FederationMintQuoteOperation {
                    quote: "quote-1".to_string(),
                    request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                        amount: Amount::from(1),
                        unit: CurrencyUnit::Sat,
                        description: None,
                        pubkey: None,
                    }),
                },
            ),
        )
    }

    #[test]
    fn dummy_consensus_accepts_duplicates_and_rejects_conflicts() {
        let federation_id = federation_id(13);
        let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
        let first = mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let duplicate = first.clone();
        let conflict = mint_envelope(
            federation_id,
            "quote-1",
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );

        let first_response = consensus.submit_envelope(first).expect("submit");
        let duplicate_response = consensus.submit_envelope(duplicate).expect("duplicate");

        assert_eq!(first_response.operation_id, duplicate_response.operation_id);
        assert_eq!(consensus.peer_status().operation_count, 1);
        assert!(matches!(
            consensus.submit_envelope(conflict),
            Err(FederationError::ConflictingOperation { .. })
        ));
    }

    #[test]
    fn dummy_consensus_rejects_wrong_federation() {
        let mut consensus = InMemoryFederationConsensus::new(federation_id(13), member_id());
        let envelope = mint_quote_envelope(federation_id(14));

        assert!(matches!(
            consensus.submit_operation(FederationSubmitOperationRequest::new(envelope)),
            Err(FederationError::OperationFederationMismatch { .. })
        ));
    }

    #[test]
    fn dummy_consensus_serves_peer_status_and_catch_up() {
        let federation_id = federation_id(13);
        let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
        consensus
            .submit_envelope(mint_quote_envelope(federation_id))
            .expect("submit quote");
        consensus
            .submit_envelope(mint_envelope(
                federation_id,
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .expect("submit mint");

        let status = consensus.peer_status();
        let request = FederationJournalCatchUpRequest::new(1);
        let response = consensus.catch_up(&request);

        assert_eq!(status.federation_id, federation_id);
        assert_eq!(status.member_id, member_id());
        assert_eq!(status.operation_count, 2);
        assert_eq!(response.from_index, 1);
        assert_eq!(response.entries.len(), 1);
        assert!(response.complete);
        assert_eq!(response.latest_order_digest, status.latest_order_digest);
    }

    #[test]
    fn dummy_consensus_updates_application_status() {
        let federation_id = federation_id(13);
        let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
        let response = consensus
            .submit_envelope(mint_quote_envelope(federation_id))
            .expect("submit");
        let updated = consensus
            .update_status(response.operation_id, FederationOperationStatus::Applied)
            .expect("update status");

        assert_eq!(updated.status, FederationOperationStatus::Applied);
    }
}

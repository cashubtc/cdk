//! Federation journal storage tests.

use std::collections::BTreeMap;

use crate::database::mint::{Database, FederationJournalDatabase};
use crate::federation::{
    FederationId, FederationJournalAppendOutcome, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationJournalEntry, FederationMintQuoteOperation,
    FederationMintQuotePaymentObservation, FederationOperation, FederationOperationEnvelope,
    FederationOperationStatus, InMemoryFederationJournal, MemberConfigPrivate, MemberId,
};
use crate::nuts::nut00::KnownMethod;
use crate::{
    Amount, CurrencyUnit, MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteRequest,
    MintQuoteResponse, MintQuoteState, PaymentMethod, SecretKey,
};

fn federation_id() -> FederationId {
    FederationId::from_bytes([77; 32])
}

fn mint_quote_envelope(quote: &str, amount: u64) -> FederationOperationEnvelope {
    FederationOperationEnvelope::new(
        federation_id(),
        FederationOperation::MintQuote(FederationMintQuoteOperation {
            quote: quote.to_string(),
            request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                amount: Amount::from(amount),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }),
            response: MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                quote: quote.to_string(),
                request: format!("invoice-{quote}"),
                amount: Some(Amount::from(amount)),
                unit: Some(CurrencyUnit::Sat),
                state: MintQuoteState::Unpaid,
                expiry: None,
                pubkey: None,
            }),
        }),
    )
}

fn payment_observation_envelope(quote: &str) -> FederationOperationEnvelope {
    FederationOperationEnvelope::new(
        federation_id(),
        FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
            quote: quote.to_string(),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(21),
            amount_issued: Amount::ZERO,
            payment_proof: Some("fake-wallet-paid".to_string()),
        }),
    )
}

fn appended(entry: FederationJournalAppendOutcome) -> FederationJournalEntry {
    entry.entry().clone()
}

fn private_member(id: u16) -> MemberConfigPrivate {
    MemberConfigPrivate {
        member_id: MemberId::new(id).expect("member id"),
        identity_secret_key: SecretKey::from_slice(&[id as u8; 32]).expect("identity secret"),
        keyset_secret_shares: BTreeMap::new(),
    }
}

/// Assert federation journal entries, operation statuses, and member sequences are persisted.
pub async fn federation_journal_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let mut journal = InMemoryFederationJournal::new();
    let first = appended(
        journal
            .append(mint_quote_envelope("quote-1", 21))
            .expect("append first"),
    );
    let second = appended(
        journal
            .append(payment_observation_envelope("quote-1"))
            .expect("append second"),
    );

    let member_one = MemberId::new(1).expect("member one");
    let member_two = MemberId::new(2).expect("member two");
    let second_private = private_member(2);
    let first_session_outcome = FederationJournalCatchUpResponse::from_journal(
        federation_id(),
        &journal,
        &FederationJournalCatchUpRequest::new(0),
        50,
        100,
    )
    .with_signed_session_outcome(&second_private)
    .expect("first session outcome")
    .session_outcome
    .expect("first session outcome");
    let second_session_outcome = FederationJournalCatchUpResponse::from_journal(
        federation_id(),
        &journal,
        &FederationJournalCatchUpRequest::new(1),
        50,
        100,
    )
    .with_signed_session_outcome(&second_private)
    .expect("second session outcome")
    .session_outcome
    .expect("second session outcome");

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_journal_entry(&first)
        .await
        .expect("persist first");
    tx.add_federation_journal_entry(&second)
        .await
        .expect("persist second");
    tx.update_federation_operation_status(first.operation_id, FederationOperationStatus::Applied)
        .await
        .expect("update status");
    tx.set_federation_member_sequence(member_one, 7)
        .await
        .expect("sequence one");
    tx.set_federation_member_sequence(member_two, 11)
        .await
        .expect("sequence two");
    tx.set_federation_member_sequence(member_one, 9)
        .await
        .expect("sequence one update");
    tx.set_federation_member_sequence(member_one, 5)
        .await
        .expect("sequence one stale update");
    tx.add_federation_session_outcome(&first_session_outcome)
        .await
        .expect("persist first session outcome");
    tx.add_federation_session_outcome(&second_session_outcome)
        .await
        .expect("persist second session outcome");
    tx.commit().await.expect("commit");

    let entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].index, 0);
    assert_eq!(entries[0].operation_id, first.operation_id);
    assert_eq!(entries[0].status, FederationOperationStatus::Applied);
    assert_eq!(entries[0].conflict_keys, first.conflict_keys);
    assert_eq!(entries[1].index, 1);
    assert_eq!(entries[1].operation_id, second.operation_id);
    assert_eq!(entries[1].status, FederationOperationStatus::Accepted);
    assert_eq!(entries[1].previous_order_digest, first.order_digest);

    let catch_up_entries = db
        .get_federation_journal_entries_from(1)
        .await
        .expect("catch-up entries");
    assert_eq!(catch_up_entries.len(), 1);
    assert_eq!(catch_up_entries[0].operation_id, second.operation_id);

    let sequences = db
        .get_federation_member_sequences()
        .await
        .expect("sequences");
    assert_eq!(sequences.get(&member_one), Some(&9));
    assert_eq!(sequences.get(&member_two), Some(&11));

    let session_outcomes = db
        .get_federation_session_outcomes()
        .await
        .expect("session outcomes");
    assert_eq!(session_outcomes.len(), 2);
    assert_eq!(session_outcomes[0].from_index, 0);
    assert_eq!(session_outcomes[0].signer, member_two);
    assert_eq!(
        session_outcomes[0].operation_ids,
        first_session_outcome.operation_ids
    );
    assert_eq!(session_outcomes[1].from_index, 1);
    assert_eq!(
        session_outcomes[1].signature,
        second_session_outcome.signature
    );

    let session_outcomes_from = db
        .get_federation_session_outcomes_from(1)
        .await
        .expect("session outcomes from");
    assert_eq!(session_outcomes_from.len(), 1);
    assert_eq!(session_outcomes_from[0].from_index, 1);
}

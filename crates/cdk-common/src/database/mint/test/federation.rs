//! Federation journal storage tests.

use std::collections::BTreeMap;
use std::sync::Arc;

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
            observer: MemberId::new(1).expect("member id"),
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

/// Assert concurrent federation journal writes do not deadlock or reorder reads.
pub async fn federation_journal_concurrent_writes<DB>(db: DB)
where
    DB: Database<crate::database::Error>
        + FederationJournalDatabase<Err = crate::database::Error>
        + Send
        + Sync
        + 'static,
{
    const ENTRY_COUNT: u64 = 16;

    let mut journal = InMemoryFederationJournal::new();
    let entries = (0..ENTRY_COUNT)
        .map(|index| {
            appended(
                journal
                    .append(mint_quote_envelope(
                        &format!("concurrent-quote-{index}"),
                        index + 1,
                    ))
                    .expect("append concurrent entry"),
            )
        })
        .collect::<Vec<_>>();

    let db = Arc::new(db);
    let mut tasks = Vec::new();
    for entry in entries.clone() {
        let db = Arc::clone(&db);
        tasks.push(tokio::spawn(async move {
            let mut tx = Database::begin_transaction(db.as_ref())
                .await
                .expect("begin concurrent transaction");
            tx.add_federation_journal_entry(&entry)
                .await
                .expect("persist concurrent entry");
            tx.update_federation_operation_status(
                entry.operation_id,
                FederationOperationStatus::Applied,
            )
            .await
            .expect("update concurrent status");
            tx.commit().await.expect("commit concurrent transaction");
        }));
    }

    for task in tasks {
        task.await.expect("concurrent writer task");
    }

    let persisted = db
        .as_ref()
        .get_federation_journal_entries()
        .await
        .expect("concurrent journal entries");
    assert_eq!(persisted.len(), entries.len());

    for (index, (persisted, expected)) in persisted.iter().zip(entries.iter()).enumerate() {
        assert_eq!(persisted.index, index as u64);
        assert_eq!(persisted.operation_id, expected.operation_id);
        assert_eq!(
            persisted.previous_order_digest,
            expected.previous_order_digest
        );
        assert_eq!(persisted.order_digest, expected.order_digest);
        assert_eq!(persisted.status, FederationOperationStatus::Applied);
    }

    let catch_up_from = ENTRY_COUNT / 2;
    let catch_up_entries = db
        .as_ref()
        .get_federation_journal_entries_from(catch_up_from)
        .await
        .expect("concurrent catch-up entries");
    assert_eq!(
        catch_up_entries.len(),
        (ENTRY_COUNT - catch_up_from) as usize
    );
    assert_eq!(catch_up_entries[0].index, catch_up_from);
}

/// Assert concurrent federation metadata updates preserve monotonic watermarks and outcomes.
pub async fn federation_journal_concurrent_metadata_updates<DB>(db: DB)
where
    DB: Database<crate::database::Error>
        + FederationJournalDatabase<Err = crate::database::Error>
        + Send
        + Sync
        + 'static,
{
    let mut journal = InMemoryFederationJournal::new();
    let entries = (0..4)
        .map(|index| {
            appended(
                journal
                    .append(mint_quote_envelope(
                        &format!("metadata-quote-{index}"),
                        index + 1,
                    ))
                    .expect("append metadata entry"),
            )
        })
        .collect::<Vec<_>>();

    {
        let mut tx = Database::begin_transaction(&db)
            .await
            .expect("begin metadata seed transaction");
        for entry in &entries {
            tx.add_federation_journal_entry(entry)
                .await
                .expect("persist metadata seed entry");
        }
        tx.commit().await.expect("commit metadata seed transaction");
    }

    let member_one = MemberId::new(1).expect("member one");
    let member_two = MemberId::new(2).expect("member two");
    let member_three = MemberId::new(3).expect("member three");
    let db = Arc::new(db);

    let sequence_updates = [
        (member_one, 1),
        (member_one, 5),
        (member_one, 3),
        (member_two, 2),
        (member_two, 7),
        (member_two, 6),
        (member_three, 4),
    ];
    let mut sequence_tasks = Vec::new();
    for (member, sequence) in sequence_updates {
        let db = Arc::clone(&db);
        sequence_tasks.push(tokio::spawn(async move {
            let mut tx = Database::begin_transaction(db.as_ref())
                .await
                .expect("begin sequence transaction");
            tx.set_federation_member_sequence(member, sequence)
                .await
                .expect("persist sequence update");
            tx.commit().await.expect("commit sequence transaction");
        }));
    }

    for task in sequence_tasks {
        task.await.expect("sequence writer task");
    }

    let member_two_private = private_member(2);
    let member_three_private = private_member(3);
    let session_outcomes = [
        FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &FederationJournalCatchUpRequest::new(0),
            50,
            100,
        )
        .with_signed_session_outcome(&member_two_private)
        .expect("member two outcome from zero")
        .session_outcome
        .expect("member two outcome from zero"),
        FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &FederationJournalCatchUpRequest::new(0),
            50,
            100,
        )
        .with_signed_session_outcome(&member_three_private)
        .expect("member three outcome from zero")
        .session_outcome
        .expect("member three outcome from zero"),
        FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &FederationJournalCatchUpRequest::new(1),
            50,
            100,
        )
        .with_signed_session_outcome(&member_two_private)
        .expect("member two outcome from one")
        .session_outcome
        .expect("member two outcome from one"),
        FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            &FederationJournalCatchUpRequest::new(2),
            50,
            100,
        )
        .with_signed_session_outcome(&member_three_private)
        .expect("member three outcome from two")
        .session_outcome
        .expect("member three outcome from two"),
    ];

    let mut outcome_tasks = Vec::new();
    for outcome in session_outcomes.clone() {
        let db = Arc::clone(&db);
        outcome_tasks.push(tokio::spawn(async move {
            let mut tx = Database::begin_transaction(db.as_ref())
                .await
                .expect("begin outcome transaction");
            tx.add_federation_session_outcome(&outcome)
                .await
                .expect("persist session outcome");
            tx.commit().await.expect("commit outcome transaction");
        }));
    }

    for task in outcome_tasks {
        task.await.expect("outcome writer task");
    }

    let sequences = db
        .as_ref()
        .get_federation_member_sequences()
        .await
        .expect("metadata sequences");
    assert_eq!(sequences.get(&member_one), Some(&5));
    assert_eq!(sequences.get(&member_two), Some(&7));
    assert_eq!(sequences.get(&member_three), Some(&4));

    let stored_outcomes = db
        .as_ref()
        .get_federation_session_outcomes()
        .await
        .expect("metadata session outcomes");
    assert_eq!(stored_outcomes.len(), session_outcomes.len());
    assert_eq!(stored_outcomes[0].from_index, 0);
    assert_eq!(stored_outcomes[0].signer, member_two);
    assert_eq!(stored_outcomes[1].from_index, 0);
    assert_eq!(stored_outcomes[1].signer, member_three);
    assert_eq!(stored_outcomes[2].from_index, 1);
    assert_eq!(stored_outcomes[2].signer, member_two);
    assert_eq!(stored_outcomes[3].from_index, 2);
    assert_eq!(stored_outcomes[3].signer, member_three);

    let ranged_outcomes = db
        .as_ref()
        .get_federation_session_outcomes_from(1)
        .await
        .expect("metadata ranged outcomes");
    assert_eq!(ranged_outcomes.len(), 2);
    assert_eq!(ranged_outcomes[0].from_index, 1);
    assert_eq!(ranged_outcomes[1].from_index, 2);
}

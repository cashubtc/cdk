//! Federation journal storage tests.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use crate::database::mint::{Database, FederationJournalDatabase};
use crate::federation::{
    FederatedKeyset, FederationConfig, FederationDkgMemberKeysetShare,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgSetupState, FederationId, FederationJournalAppendOutcome,
    FederationJournalCatchUpRequest, FederationJournalCatchUpResponse, FederationJournalEntry,
    FederationMintQuoteOperation, FederationMintQuotePaymentObservation, FederationOperation,
    FederationOperationEnvelope, FederationOperationStatus, FederationPaymentObservation,
    FederationPaymentObservationContext, FederationPaymentObservationRecord,
    FederationPaymentObservationReport, FederationPaymentObservationReportStatus,
    InMemoryFederationJournal, MemberConfigPrivate, MemberConfigPublic, MemberId, ThresholdParams,
};
use crate::mint_url::MintUrl;
use crate::nuts::nut00::KnownMethod;
use crate::nuts::nut01::{trusted_dealer_keygen, BlsSecretKey};
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
    payment_observation_envelope_for_member(quote, 1, 21, "fake-wallet-paid")
}

fn payment_observation_envelope_for_member(
    quote: &str,
    member_id: u16,
    amount_paid: u64,
    payment_proof: &str,
) -> FederationOperationEnvelope {
    FederationOperationEnvelope::new(
        federation_id(),
        FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
            quote: quote.to_string(),
            observer: MemberId::new(member_id).expect("member id"),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(amount_paid),
            amount_issued: Amount::ZERO,
            payment_proof: Some(payment_proof.to_string()),
        }),
    )
}

fn appended(entry: FederationJournalAppendOutcome) -> FederationJournalEntry {
    entry.entry().clone()
}

fn private_member(id: u16) -> MemberConfigPrivate {
    MemberConfigPrivate {
        federation_id: None,
        member_id: MemberId::new(id).expect("member id"),
        identity_secret_key: SecretKey::from_slice(&[id as u8; 32]).expect("identity secret"),
        keyset_secret_shares: BTreeMap::new(),
        setup_transcript_hash: None,
    }
}

fn member_public(id: u16) -> MemberConfigPublic {
    let identity_secret = SecretKey::from_slice(&[id as u8; 32]).expect("identity secret");
    MemberConfigPublic {
        member_id: MemberId::new(id).expect("member id"),
        public_mint_url: MintUrl::from_str(&format!("https://mint{id}.example.com"))
            .expect("public mint url"),
        federation_api_url: MintUrl::from_str(&format!("https://fed{id}.example.com"))
            .expect("federation api url"),
        identity_public_key: identity_secret.public_key(),
    }
}

fn dkg_public_config() -> FederationConfig {
    let members = BTreeMap::from([
        (member_id(1), member_public(1)),
        (member_id(2), member_public(2)),
    ]);
    FederationConfig::new(
        ThresholdParams::new(2, 2, 2).expect("threshold params"),
        members,
        Vec::new(),
    )
    .expect("dkg federation config")
}

fn member_id(id: u16) -> MemberId {
    MemberId::new(id).expect("member id")
}

fn bls_key(byte: u8) -> BlsSecretKey {
    BlsSecretKey::from_reduced_bytes(&[byte; 32])
}

fn dkg_reveal(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> FederationDkgParticipantCommitmentReveal {
    FederationDkgParticipantCommitmentReveal {
        federation_id: public_config.federation_id,
        member_id,
        coefficient_commitments: BTreeMap::from([(
            Amount::from(1),
            vec![
                bls_key(seed).public_key_g2(),
                bls_key(seed + 10).public_key_g2(),
            ],
        )]),
    }
}

fn dkg_setup_state_from_db_records(
    public_config: &FederationConfig,
    commitments: Vec<FederationDkgParticipantCommitment>,
    reveals: Vec<FederationDkgParticipantCommitmentReveal>,
) -> FederationDkgSetupState {
    let mut state = FederationDkgSetupState::new(public_config);
    for commitment in commitments {
        state
            .insert_commitment(public_config, commitment)
            .expect("stored commitment validates");
    }
    for reveal in reveals {
        state
            .insert_reveal(public_config, reveal)
            .expect("stored reveal validates");
    }
    state
}

fn dkg_member_keyset_share(local_member_id: MemberId) -> FederationDkgMemberKeysetShare {
    let public_config = dkg_public_config();
    let signer_ids = public_config
        .members
        .keys()
        .map(|member_id| member_id.to_bls_signer_id())
        .collect::<Vec<_>>();
    let amount = Amount::from(1);
    let threshold_keyset =
        trusted_dealer_keygen(&[bls_key(7), bls_key(17)], &signer_ids).expect("threshold keyset");
    let mut aggregate_keys = BTreeMap::new();
    aggregate_keys.insert(amount, threshold_keyset.aggregate_public_key());
    let mut public_shares = BTreeMap::new();
    for share in threshold_keyset.public_shares() {
        public_shares
            .entry(MemberId::new(share.signer_id().get()).expect("member id"))
            .or_insert_with(BTreeMap::new)
            .insert(amount, *share);
    }
    let mut secret_shares = BTreeMap::new();
    for share in threshold_keyset.secret_shares() {
        if share.signer_id().get() == local_member_id.get() {
            secret_shares.insert(amount, share.clone());
        }
    }
    let keyset = FederatedKeyset::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        aggregate_keys,
        public_shares,
    );
    FederationDkgMemberKeysetShare {
        federation_id: public_config.federation_id,
        member_id: local_member_id,
        keyset,
        secret_shares,
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

/// Assert payment observations persist as durable per-member quorum evidence.
pub async fn federation_payment_observation_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let mut journal = InMemoryFederationJournal::new();
    let first = appended(
        journal
            .append(payment_observation_envelope_for_member(
                "quote-1", 1, 21, "settled",
            ))
            .expect("append first observation"),
    );
    let matching_second = appended(
        journal
            .append(payment_observation_envelope_for_member(
                "quote-1", 2, 21, "settled",
            ))
            .expect("append matching second observation"),
    );
    let conflicting_third = appended(
        journal
            .append(payment_observation_envelope_for_member(
                "quote-1",
                3,
                22,
                "different-settlement",
            ))
            .expect("append conflicting third observation"),
    );
    let unrelated = appended(
        journal
            .append(payment_observation_envelope_for_member(
                "quote-2",
                1,
                7,
                "other-settlement",
            ))
            .expect("append unrelated observation"),
    );

    let entries = [
        first.clone(),
        matching_second.clone(),
        conflicting_third.clone(),
        unrelated.clone(),
    ];
    let records = entries
        .iter()
        .map(|entry| {
            FederationPaymentObservationRecord::from_journal_entry(entry)
                .expect("payment observation record")
        })
        .collect::<Vec<_>>();

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    for entry in &entries {
        tx.add_federation_journal_entry(entry)
            .await
            .expect("persist journal entry");
    }
    for record in &records {
        tx.add_federation_payment_observation(record)
            .await
            .expect("persist payment observation");
    }
    tx.add_federation_payment_observation(&records[0])
        .await
        .expect("idempotent payment observation persistence");
    tx.commit().await.expect("commit");

    let persisted = db
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MintQuote,
            "quote-1",
        )
        .await
        .expect("payment observations");
    assert_eq!(persisted.len(), 3);
    assert_eq!(persisted[0].operation_id, first.operation_id);
    assert_eq!(persisted[0].operation_index, first.index);
    assert_eq!(persisted[1].operation_id, matching_second.operation_id);
    assert_eq!(persisted[2].operation_id, conflicting_third.operation_id);

    let mint_observations = persisted
        .iter()
        .map(|record| match &record.observation {
            FederationPaymentObservation::MintQuote(observation) => observation,
            FederationPaymentObservation::MeltQuote(_) => panic!("expected mint observation"),
        })
        .collect::<Vec<_>>();
    let quorum = mint_observations[0].quorum(mint_observations.iter().skip(1).copied(), 2);
    assert!(quorum.is_reached());
    assert!(quorum.has_conflicts());
    assert_eq!(
        quorum.matching_observers,
        vec![
            MemberId::new(1).expect("member one"),
            MemberId::new(2).expect("member two")
        ]
    );
    assert_eq!(
        quorum.conflicting_observers,
        vec![MemberId::new(3).expect("member three")]
    );

    let report = FederationPaymentObservationReport::from_records(
        FederationPaymentObservationContext::MintQuote,
        "quote-1",
        2,
        &persisted,
    );
    assert_eq!(
        report.status,
        FederationPaymentObservationReportStatus::Conflicting
    );
    assert_eq!(report.total_observations, 3);
    assert_eq!(report.groups.len(), 2);
    assert_eq!(
        report.groups[0].observers,
        vec![
            MemberId::new(1).expect("member one"),
            MemberId::new(2).expect("member two")
        ]
    );
    assert!(report.groups[0].quorum_reached);
    assert_eq!(
        report.groups[0].conflicting_observers,
        vec![MemberId::new(3).expect("member three")]
    );
    assert!(report.has_conflicts());

    let unrelated_persisted = db
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MintQuote,
            "quote-2",
        )
        .await
        .expect("unrelated payment observations");
    assert_eq!(unrelated_persisted.len(), 1);
    assert_eq!(unrelated_persisted[0].operation_id, unrelated.operation_id);

    let wrong_context = db
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            "quote-1",
        )
        .await
        .expect("wrong-context payment observations");
    assert!(wrong_context.is_empty());
}

/// Assert DKG setup state persists as idempotent replayable setup material.
pub async fn federation_dkg_setup_state_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let public_config = dkg_public_config();
    let first_reveal = dkg_reveal(&public_config, member_id(1), 11);
    let second_reveal = dkg_reveal(&public_config, member_id(2), 21);
    let conflicting_first_reveal = dkg_reveal(&public_config, member_id(1), 91);
    let first_commitment =
        FederationDkgParticipantCommitment::from_reveal(&first_reveal).expect("first commitment");
    let second_commitment =
        FederationDkgParticipantCommitment::from_reveal(&second_reveal).expect("second commitment");
    let conflicting_first_commitment =
        FederationDkgParticipantCommitment::from_reveal(&conflicting_first_reveal)
            .expect("conflicting first commitment");

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_dkg_commitment(&first_commitment)
        .await
        .expect("persist first commitment");
    tx.add_federation_dkg_commitment(&first_commitment)
        .await
        .expect("idempotent duplicate commitment replay");
    tx.add_federation_dkg_commitment(&second_commitment)
        .await
        .expect("persist second commitment");
    tx.add_federation_dkg_reveal(&first_reveal)
        .await
        .expect("persist first reveal");
    tx.add_federation_dkg_reveal(&first_reveal)
        .await
        .expect("idempotent duplicate reveal replay");
    tx.commit().await.expect("commit");

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting commitment");
    assert!(tx
        .add_federation_dkg_commitment(&conflicting_first_commitment)
        .await
        .is_err());
    tx.rollback()
        .await
        .expect("rollback conflicting commitment");

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting reveal");
    assert!(tx
        .add_federation_dkg_reveal(&conflicting_first_reveal)
        .await
        .is_err());
    tx.rollback().await.expect("rollback conflicting reveal");

    let commitments = db
        .get_federation_dkg_commitments()
        .await
        .expect("dkg commitments");
    let reveals = db.get_federation_dkg_reveals().await.expect("dkg reveals");
    assert_eq!(commitments, vec![first_commitment, second_commitment]);
    assert_eq!(reveals, vec![first_reveal.clone()]);

    let partial_state = dkg_setup_state_from_db_records(&public_config, commitments, reveals);
    assert!(partial_state
        .complete_round(&public_config)
        .expect("partial setup state")
        .is_none());

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin second");
    tx.add_federation_dkg_reveal(&second_reveal)
        .await
        .expect("persist second reveal");
    tx.commit().await.expect("commit second");

    let complete_state = dkg_setup_state_from_db_records(
        &public_config,
        db.get_federation_dkg_commitments()
            .await
            .expect("complete commitments"),
        db.get_federation_dkg_reveals()
            .await
            .expect("complete reveals"),
    );
    assert!(complete_state
        .complete_round(&public_config)
        .expect("complete setup state")
        .is_some());
}

/// Assert completed local DKG keyset shares persist idempotently.
pub async fn federation_dkg_member_keyset_share_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let share = dkg_member_keyset_share(member_id(1));
    let mut conflicting_share = share.clone();
    conflicting_share.federation_id = FederationId::from_bytes([99; 32]);

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_dkg_member_keyset_share(&share)
        .await
        .expect("persist member keyset share");
    tx.add_federation_dkg_member_keyset_share(&share)
        .await
        .expect("idempotent duplicate member keyset share replay");
    tx.commit().await.expect("commit");

    let shares = db
        .get_federation_dkg_member_keyset_shares()
        .await
        .expect("member keyset shares");
    assert_eq!(shares, vec![share.clone()]);

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting share");
    assert!(tx
        .add_federation_dkg_member_keyset_share(&conflicting_share)
        .await
        .is_err());
    tx.rollback().await.expect("rollback conflicting share");

    let shares = db
        .get_federation_dkg_member_keyset_shares()
        .await
        .expect("member keyset shares after conflict");
    assert_eq!(shares, vec![share]);
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

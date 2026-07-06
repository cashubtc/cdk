//! Federation journal storage tests.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use crate::database::mint::{Database, FederationJournalDatabase};
use crate::federation::{
    FederatedKeyset, FederationAlephBftConsensusAdapter, FederationAlephBftUnitPayload,
    FederationConfig, FederationConfigSnapshot, FederationConsensusItem,
    FederationDkgMemberKeysetShare, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgSecretShareRequest,
    FederationDkgSetupMethod, FederationDkgSetupState, FederationDkgTranscript,
    FederationDkgTranscriptSignature, FederationDkgTranscriptSignatureRequest, FederationId,
    FederationJournalAppendOutcome, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationJournalConsensusEvidence, FederationJournalEntry,
    FederationMemberStatusKind, FederationMemberStatusSnapshot, FederationMintQuoteOperation,
    FederationMintQuotePaymentObservation, FederationOperation, FederationOperationEnvelope,
    FederationOperationId, FederationOperationStatus, FederationPaymentObservation,
    FederationPaymentObservationContext, FederationPaymentObservationRecord,
    FederationPaymentObservationReport, FederationPaymentObservationReportStatus,
    FederationSignatureShareRecord, FederationStateDigest, FederationStateDigestCheckpoint,
    InMemoryFederationJournal, MemberConfigPrivate, MemberConfigPublic, MemberId, ThresholdParams,
};
use crate::mint_url::MintUrl;
use crate::nuts::nut00::KnownMethod;
use crate::nuts::nut01::{
    evaluate_secret_key_coefficients, trusted_dealer_keygen, BlsBlindSignatureShare,
    BlsG1PublicKey, BlsSecretKey,
};
use crate::{
    Amount, CurrencyUnit, Id, MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteRequest,
    MintQuoteResponse, MintQuoteState, PaymentMethod, PublicKey, SecretKey,
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
            payment_id: None,
            payment_proof: Some(payment_proof.to_string()),
            observed_time: None,
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

fn dkg_secret_share_request(
    public_config: &FederationConfig,
    sender: MemberId,
    receiver: MemberId,
    seed: u8,
) -> FederationDkgSecretShareRequest {
    FederationDkgSecretShareRequest::new(
        public_config.federation_id,
        sender,
        receiver,
        BTreeMap::from([(
            Amount::from(1),
            evaluate_secret_key_coefficients(
                receiver.to_bls_signer_id(),
                &[bls_key(seed), bls_key(seed + 10)],
            )
            .expect("secret share"),
        )]),
    )
}

fn signature_share_record(
    operation_id: FederationOperationId,
    output_index: u32,
    member_id: MemberId,
    blinded_message_seed: &[u8],
    signature_seed: &[u8],
) -> FederationSignatureShareRecord {
    let mut keyset_id_bytes = vec![2u8];
    keyset_id_bytes.extend([42u8; 32]);

    FederationSignatureShareRecord {
        operation_id,
        accepted_operation_index: None,
        consensus_evidence: None,
        output_index,
        member_id,
        keyset_id: Id::from_bytes(&keyset_id_bytes).expect("keyset id"),
        amount: Amount::from(21),
        blinded_secret: PublicKey::from(BlsG1PublicKey::hash_to_curve(blinded_message_seed)),
        share: BlsBlindSignatureShare::new(
            member_id.to_bls_signer_id(),
            BlsG1PublicKey::hash_to_curve(signature_seed),
        ),
    }
}

fn dkg_transcript_signature_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
) -> FederationDkgTranscriptSignatureRequest {
    let transcript_hash = FederationDkgTranscript::hash_for_config(
        FederationDkgSetupMethod::PedersenDkg,
        public_config,
    )
    .expect("transcript hash");
    let signature = FederationDkgTranscriptSignature::sign(transcript_hash, private_config)
        .expect("transcript signature");
    FederationDkgTranscriptSignatureRequest::new(
        public_config.federation_id,
        private_config.member_id,
        transcript_hash,
        signature,
    )
}

/// Assert federation journal entries, operation statuses, and member sequences are persisted.
pub async fn federation_journal_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let mut journal = InMemoryFederationJournal::new();
    let member_one = MemberId::new(1).expect("member one");
    let member_two = MemberId::new(2).expect("member two");
    let first_evidence = FederationJournalConsensusEvidence::new(0, 0, 0, member_one, 0);
    let first = appended(
        journal
            .append_with_consensus_evidence(mint_quote_envelope("quote-1", 21), first_evidence)
            .expect("append first"),
    );
    let second = appended(
        journal
            .append(payment_observation_envelope("quote-1"))
            .expect("append second"),
    );

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
    assert_eq!(
        entries[0].operation_outcome_hash,
        first.operation_outcome_hash
    );
    assert_eq!(entries[0].state_digest, first.state_digest);
    assert_eq!(entries[0].consensus_evidence, Some(first_evidence));
    assert_eq!(entries[1].index, 1);
    assert_eq!(entries[1].operation_id, second.operation_id);
    assert_eq!(entries[1].status, FederationOperationStatus::Accepted);
    assert_eq!(entries[1].previous_order_digest, first.order_digest);
    assert_eq!(
        entries[1].operation_outcome_hash,
        second.operation_outcome_hash
    );
    assert_eq!(entries[1].state_digest, second.state_digest);

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

/// Assert AlephBFT finalized items are durable and can recover adapter state.
pub async fn federation_aleph_bft_finalized_item_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let federation_id = federation_id();
    let member_one = MemberId::new(1).expect("member one");
    let member_two = MemberId::new(2).expect("member two");
    let member_three = MemberId::new(3).expect("member three");
    let envelope = mint_quote_envelope("aleph-bft-quote-1", 21);
    let operation_id = envelope.operation_id().expect("operation id");
    let first_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_two,
        0,
        0,
        vec![FederationConsensusItem::operation(envelope.clone())],
    )
    .expect("first AlephBFT unit");
    let duplicate_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_three,
        0,
        0,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("duplicate AlephBFT unit");
    let mut adapter = FederationAlephBftConsensusAdapter::for_members(
        federation_id,
        member_one,
        0,
        [member_one, member_two, member_three],
    )
    .expect("AlephBFT adapter");

    adapter.finalize_unit(first_unit).expect("finalize first");
    adapter
        .finalize_unit(duplicate_unit)
        .expect("finalize duplicate");
    let finalized_items = adapter.finalized_items().to_vec();
    let finalized_json = finalized_items
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .expect("finalized item JSON");

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_aleph_bft_finalized_items(&finalized_items)
        .await
        .expect("persist finalized items");
    tx.add_federation_aleph_bft_finalized_item(&finalized_items[0])
        .await
        .expect("idempotent finalized item replay");
    tx.commit().await.expect("commit finalized items");

    let persisted = db
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("persisted AlephBFT finalized items");
    assert_eq!(persisted.len(), finalized_items.len());
    let persisted_json = persisted
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .expect("persisted finalized item JSON");
    assert_eq!(persisted_json, finalized_json);

    let ranged = db
        .get_federation_aleph_bft_finalized_items_from(1)
        .await
        .expect("ranged AlephBFT finalized items");
    assert_eq!(ranged.len(), 1);
    assert_eq!(ranged[0].accepted_item_index, 1);

    let recovered = FederationAlephBftConsensusAdapter::recover_for_members_from_finalized_items(
        federation_id,
        member_one,
        0,
        [member_one, member_two, member_three],
        persisted,
    )
    .expect("recover adapter from persisted finalized items");
    assert_eq!(recovered.journal().len(), 1);
    assert_eq!(recovered.journal().entries()[0].operation_id, operation_id);
    assert_eq!(recovered.next_item_index(), 2);
    assert_eq!(recovered.status().next_unit_finalization_index, 2);

    let mut conflicting = finalized_items[0].clone();
    conflicting.unit_index = conflicting.unit_index.saturating_add(1);
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting finalized item");
    assert!(tx
        .add_federation_aleph_bft_finalized_item(&conflicting)
        .await
        .is_err());
    tx.rollback()
        .await
        .expect("rollback conflicting finalized item");
}

/// Assert public federation config snapshots are durable and conflict-checked.
pub async fn federation_config_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let config = dkg_public_config();
    let snapshot = FederationConfigSnapshot::new(config.clone()).expect("config snapshot");
    let missing = FederationId::from_bytes([9; 32]);

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.set_federation_config(&snapshot)
        .await
        .expect("persist federation config");
    tx.commit().await.expect("commit federation config");

    assert_eq!(
        db.get_federation_config(config.federation_id)
            .await
            .expect("load federation config"),
        Some(snapshot.clone())
    );
    assert_eq!(
        db.get_federation_config(missing)
            .await
            .expect("load missing federation config"),
        None
    );

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin idempotent config");
    tx.set_federation_config(&snapshot)
        .await
        .expect("persist idempotent federation config");
    tx.commit()
        .await
        .expect("commit idempotent federation config");

    let mut conflicting_config = config;
    conflicting_config
        .payment_observation_policy
        .max_pending_quotes += 1;
    conflicting_config
        .validate()
        .expect("conflicting config validates");
    let conflicting_snapshot =
        FederationConfigSnapshot::new(conflicting_config).expect("conflicting snapshot");
    assert_eq!(conflicting_snapshot.federation_id, snapshot.federation_id);
    assert_ne!(conflicting_snapshot.config_digest, snapshot.config_digest);

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting config");
    assert!(tx
        .set_federation_config(&conflicting_snapshot)
        .await
        .is_err());
    tx.rollback().await.expect("rollback conflicting config");
}

/// Assert local federation member sync status snapshots are durable and idempotent.
pub async fn federation_member_status_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let local_member = MemberId::new(1).expect("local member");
    let peer_member = MemberId::new(2).expect("peer member");
    let missing_member = MemberId::new(3).expect("missing member");
    let lagging = FederationMemberStatusSnapshot {
        member_id: local_member,
        status: FederationMemberStatusKind::Lagging,
        peer: Some(peer_member),
        local_operation_count: Some(3),
        peer_operation_count: Some(7),
        operation_count: None,
        message: Some("lagging behind peer".to_string()),
        updated_time: 11,
    };

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.set_federation_member_status(&lagging)
        .await
        .expect("persist lagging status");
    tx.commit().await.expect("commit lagging status");

    assert_eq!(
        db.get_federation_member_status(local_member)
            .await
            .expect("lagging status"),
        Some(lagging.clone())
    );
    assert_eq!(
        db.get_federation_member_status(missing_member)
            .await
            .expect("missing status"),
        None
    );

    let healthy = FederationMemberStatusSnapshot {
        member_id: local_member,
        status: FederationMemberStatusKind::Healthy,
        peer: None,
        local_operation_count: None,
        peer_operation_count: None,
        operation_count: None,
        message: None,
        updated_time: 12,
    };
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin update");
    tx.set_federation_member_status(&healthy)
        .await
        .expect("persist healthy status");
    tx.commit().await.expect("commit healthy status");

    assert_eq!(
        db.get_federation_member_status(local_member)
            .await
            .expect("healthy status"),
        Some(healthy)
    );
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

/// Assert materialized state digest checkpoints persist in consensus order.
pub async fn federation_state_digest_checkpoint_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let mut journal = InMemoryFederationJournal::new();
    let first = appended(
        journal
            .append(mint_quote_envelope("digest-quote-1", 21))
            .expect("append first digest entry"),
    );
    let second = appended(
        journal
            .append(payment_observation_envelope("digest-quote-1"))
            .expect("append second digest entry"),
    );
    let first_checkpoint =
        FederationStateDigestCheckpoint::new(&first, FederationStateDigest::from_bytes([11; 32]));
    let second_checkpoint =
        FederationStateDigestCheckpoint::new(&second, FederationStateDigest::from_bytes([22; 32]));
    let conflicting_first =
        FederationStateDigestCheckpoint::new(&first, FederationStateDigest::from_bytes([33; 32]));

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_journal_entry(&first)
        .await
        .expect("persist first journal entry");
    tx.add_federation_journal_entry(&second)
        .await
        .expect("persist second journal entry");
    tx.add_federation_state_digest_checkpoint(&first_checkpoint)
        .await
        .expect("persist first state digest checkpoint");
    tx.add_federation_state_digest_checkpoint(&second_checkpoint)
        .await
        .expect("persist second state digest checkpoint");
    tx.add_federation_state_digest_checkpoint(&first_checkpoint)
        .await
        .expect("idempotent checkpoint replay");
    tx.commit().await.expect("commit");

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting checkpoint");
    assert!(tx
        .add_federation_state_digest_checkpoint(&conflicting_first)
        .await
        .is_err());
    tx.rollback()
        .await
        .expect("rollback conflicting checkpoint");

    let persisted = db
        .get_federation_state_digest_checkpoints()
        .await
        .expect("state digest checkpoints");
    assert_eq!(
        persisted,
        vec![first_checkpoint, second_checkpoint],
        "state digest checkpoints should persist in operation order"
    );

    let ranged = db
        .get_federation_state_digest_checkpoints_from(1)
        .await
        .expect("ranged state digest checkpoints");
    assert_eq!(ranged, vec![second_checkpoint]);

    let third = appended(
        journal
            .append(mint_quote_envelope("digest-quote-2", 34))
            .expect("append third digest entry"),
    );
    let fourth = appended(
        journal
            .append(payment_observation_envelope("digest-quote-2"))
            .expect("append fourth digest entry"),
    );
    let third_checkpoint =
        FederationStateDigestCheckpoint::new(&third, FederationStateDigest::from_bytes([44; 32]));
    let fourth_checkpoint =
        FederationStateDigestCheckpoint::new(&fourth, FederationStateDigest::from_bytes([55; 32]));

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin retention transaction");
    tx.add_federation_journal_entry(&third)
        .await
        .expect("persist third journal entry");
    tx.add_federation_journal_entry(&fourth)
        .await
        .expect("persist fourth journal entry");
    tx.add_federation_state_digest_checkpoint(&third_checkpoint)
        .await
        .expect("persist third state digest checkpoint");
    tx.add_federation_state_digest_checkpoint(&fourth_checkpoint)
        .await
        .expect("persist fourth state digest checkpoint");
    tx.prune_federation_state_digest_checkpoints(2)
        .await
        .expect("prune state digest checkpoints");
    tx.commit().await.expect("commit retention transaction");

    let retained = db
        .get_federation_state_digest_checkpoints()
        .await
        .expect("retained state digest checkpoints");
    assert_eq!(retained, vec![third_checkpoint, fourth_checkpoint]);
}

/// Assert threshold signature shares persist independently from final blind signatures.
pub async fn federation_signature_share_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let operation_id = FederationOperationId::from_bytes([19; 32]);
    let member_one = member_id(1);
    let member_two = member_id(2);
    let first = signature_share_record(
        operation_id,
        0,
        member_one,
        b"shared blinded message",
        b"member one share",
    );
    let second = signature_share_record(
        operation_id,
        0,
        member_two,
        b"shared blinded message",
        b"member two share",
    );
    let conflicting_first = signature_share_record(
        operation_id,
        0,
        member_one,
        b"shared blinded message",
        b"conflicting member one share",
    );

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_signature_share(&first)
        .await
        .expect("persist first share");
    tx.add_federation_signature_share(&second)
        .await
        .expect("persist second share");
    tx.add_federation_signature_share(&first)
        .await
        .expect("idempotent first share replay");
    tx.commit().await.expect("commit");

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting share");
    assert!(tx
        .add_federation_signature_share(&conflicting_first)
        .await
        .is_err());
    tx.rollback().await.expect("rollback conflicting share");

    let output_shares = db
        .get_federation_signature_shares_for_output(operation_id, 0)
        .await
        .expect("shares by output");
    assert_eq!(output_shares, vec![first.clone(), second.clone()]);

    let blinded_message_shares = db
        .get_federation_signature_shares_for_blinded_message(first.blinded_secret)
        .await
        .expect("shares by blinded message");
    assert_eq!(blinded_message_shares, vec![first, second]);

    let other_output_shares = db
        .get_federation_signature_shares_for_output(operation_id, 1)
        .await
        .expect("shares for other output");
    assert!(other_output_shares.is_empty());
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

/// Assert private DKG secret-share contributions persist idempotently.
pub async fn federation_dkg_secret_share_contribution_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let public_config = dkg_public_config();
    let first_contribution =
        dkg_secret_share_request(&public_config, member_id(1), member_id(2), 7);
    let second_contribution =
        dkg_secret_share_request(&public_config, member_id(2), member_id(1), 17);
    let conflicting_contribution =
        dkg_secret_share_request(&public_config, member_id(1), member_id(2), 27);

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_dkg_secret_share_contribution(&first_contribution)
        .await
        .expect("persist first secret-share contribution");
    tx.add_federation_dkg_secret_share_contribution(&first_contribution)
        .await
        .expect("idempotent duplicate secret-share contribution replay");
    tx.add_federation_dkg_secret_share_contribution(&second_contribution)
        .await
        .expect("persist second secret-share contribution");
    tx.commit().await.expect("commit");

    let contributions = db
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("secret-share contributions");
    assert_eq!(
        contributions,
        vec![first_contribution.clone(), second_contribution.clone()]
    );

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting secret-share contribution");
    assert!(tx
        .add_federation_dkg_secret_share_contribution(&conflicting_contribution)
        .await
        .is_err());
    tx.rollback()
        .await
        .expect("rollback conflicting secret-share contribution");

    let contributions = db
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("secret-share contributions after conflict");
    assert_eq!(contributions, vec![first_contribution, second_contribution]);
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

/// Assert DKG transcript signatures persist idempotently and can rebuild a transcript.
pub async fn federation_dkg_transcript_signature_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let public_config = dkg_public_config();
    let first_private = private_member(1);
    let second_private = private_member(2);
    let first_signature = dkg_transcript_signature_request(&public_config, &first_private);
    let second_signature = dkg_transcript_signature_request(&public_config, &second_private);
    let mut conflicting_signature = first_signature;
    conflicting_signature.federation_id = FederationId::from_bytes([99; 32]);

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_dkg_transcript_signature(&first_signature)
        .await
        .expect("persist first transcript signature");
    tx.add_federation_dkg_transcript_signature(&first_signature)
        .await
        .expect("idempotent duplicate transcript signature replay");
    tx.add_federation_dkg_transcript_signature(&second_signature)
        .await
        .expect("persist second transcript signature");
    tx.commit().await.expect("commit");

    let signatures = db
        .get_federation_dkg_transcript_signatures()
        .await
        .expect("transcript signatures");
    assert_eq!(signatures, vec![first_signature, second_signature]);

    let transcript_signatures = signatures
        .iter()
        .map(|signature| (signature.signer, signature.signature))
        .collect::<BTreeMap<_, _>>();
    let transcript = FederationDkgTranscript::from_signatures(
        FederationDkgSetupMethod::PedersenDkg,
        &public_config,
        transcript_signatures,
    )
    .expect("assembled DKG transcript");
    assert_eq!(transcript.transcript_hash, first_signature.transcript_hash);
    transcript
        .validate_against_config(&public_config)
        .expect("stored signatures validate");

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting transcript signature");
    assert!(tx
        .add_federation_dkg_transcript_signature(&conflicting_signature)
        .await
        .is_err());
    tx.rollback()
        .await
        .expect("rollback conflicting transcript signature");

    let signatures = db
        .get_federation_dkg_transcript_signatures()
        .await
        .expect("transcript signatures after conflict");
    assert_eq!(signatures, vec![first_signature, second_signature]);
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

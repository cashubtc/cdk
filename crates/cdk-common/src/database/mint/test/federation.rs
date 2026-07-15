//! Federation journal storage tests.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use crate::database::mint::{Database, FederationJournalDatabase};
use crate::error::{Error, ErrorResponse};
use crate::federation::{
    FederationAlephBftPendingOperationRecord, FederationAlephBftUnitPayload,
    FederationApplicationState, FederationConfig, FederationConfigDigest, FederationConfigSnapshot,
    FederationConsensusItem, FederationConsensusRuntime, FederationDkgActivationRequest,
    FederationDkgCeremonyId, FederationDkgCeremonyRecord, FederationDkgCeremonyState,
    FederationDkgCommitmentRound, FederationDkgKeysetSpec, FederationDkgMemberKeysetShare,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgParticipantState, FederationDkgPublicCeremony, FederationDkgSecretShareRequest,
    FederationDkgSetupMethod, FederationDkgSetupState, FederationDkgTranscript,
    FederationDkgTranscriptSignature, FederationDkgTranscriptSignatureRequest,
    FederationFrostApplicationConfig, FederationFrostApplicationId,
    FederationFrostDkgConfirmationRequest, FederationFrostDkgContext, FederationFrostDkgLocalPhase,
    FederationFrostDkgLocalState, FederationFrostDkgRound1Request, FederationFrostDkgRound2Record,
    FederationFrostRootPrivateConfig, FederationFrostRootPublicConfig, FederationFrostRootRecord,
    FederationFrostSealedStorageMetadata, FederationId, FederationJournalAppendOutcome,
    FederationJournalCatchUpRequest, FederationJournalCatchUpResponse,
    FederationJournalConsensusEvidence, FederationJournalEntry, FederationMaterializedStateDigest,
    FederationMintQuoteOperation, FederationMintQuotePaymentObservation,
    FederationOnchainConfirmationPolicy, FederationOperation, FederationOperationEnvelope,
    FederationOperationId, FederationOperationStatus, FederationPaymentObservation,
    FederationPaymentObservationContext, FederationPaymentObservationRecord,
    FederationPaymentObservationReport, FederationPaymentObservationReportStatus,
    FederationSignatureShareRecord, InMemoryFederationJournal, MemberConfigPrivate,
    MemberConfigPublic, MemberId, ThresholdParams,
};
use crate::mint::{MeltPaymentRequest, MeltQuote, Operation, OperationKind};
use crate::mint_url::MintUrl;
use crate::nuts::nut00::KnownMethod;
use crate::nuts::nut01::{
    evaluate_secret_key_coefficients, BlsBlindSignatureShare, BlsG1PublicKey, BlsSecretKey,
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
            request_lookup_id: crate::payment::PaymentIdentifier::CustomId(quote.to_string()),
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
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                amount_paid: Amount::ZERO,
                amount_issued: Amount::ZERO,
                updated_at: 0,
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

fn dkg_ceremony_id() -> FederationDkgCeremonyId {
    FederationDkgCeremonyId::from_bytes([33; 32])
}

fn dkg_reveal(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> FederationDkgParticipantCommitmentReveal {
    FederationDkgParticipantCommitmentReveal {
        federation_id: public_config.federation_id,
        ceremony_id: dkg_ceremony_id(),
        member_id,
        keyset_spec: FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [Amount::from(1)].into_iter().collect(),
        )
        .expect("DKG keyset spec"),
        coefficient_commitments: BTreeMap::from([(
            Amount::from(1),
            vec![
                bls_key(seed).public_key_g2(),
                bls_key(seed + 10).public_key_g2(),
            ],
        )]),
    }
}

/// Build one valid DKG commitment for storage-backend failure tests.
pub fn federation_dkg_test_commitment() -> FederationDkgParticipantCommitment {
    let public_config = dkg_public_config();
    FederationDkgParticipantCommitment::from_reveal(&dkg_reveal(&public_config, member_id(1), 11))
        .expect("test DKG commitment")
}

fn dkg_participant_state(
    public_config: &FederationConfig,
    ceremony_id: FederationDkgCeremonyId,
) -> FederationDkgParticipantState {
    FederationDkgParticipantState::generate(
        public_config,
        ceremony_id,
        member_id(1),
        FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [Amount::from(1)].into_iter().collect(),
        )
        .expect("DKG keyset spec"),
    )
    .expect("DKG participant state")
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
    let reveals = BTreeMap::from([
        (member_id(1), dkg_reveal(&public_config, member_id(1), 11)),
        (member_id(2), dkg_reveal(&public_config, member_id(2), 21)),
    ]);
    let commitments = reveals
        .iter()
        .map(|(member_id, reveal)| {
            (
                *member_id,
                FederationDkgParticipantCommitment::from_reveal(reveal)
                    .expect("commitment from reveal"),
            )
        })
        .collect();
    let round =
        FederationDkgCommitmentRound::new(&public_config, commitments).expect("commitment round");
    let contributions = BTreeMap::from([
        (
            member_id(1),
            dkg_secret_share_request(&public_config, member_id(1), local_member_id, 11),
        ),
        (
            member_id(2),
            dkg_secret_share_request(&public_config, member_id(2), local_member_id, 21),
        ),
    ]);
    round
        .derive_member_keyset_share(&public_config, &reveals, local_member_id, &contributions)
        .expect("member keyset share")
}

fn dkg_finalized_public_config(setup_config: &FederationConfig) -> FederationConfig {
    let keyset_share = dkg_member_keyset_share(member_id(1));
    assert_eq!(keyset_share.federation_id, setup_config.federation_id);

    let mut finalized_config = setup_config.clone();
    finalized_config.keysets = vec![keyset_share.keyset];
    finalized_config
        .validate()
        .expect("finalized config public keysets validate");
    finalized_config.setup_transcript = Some(
        FederationDkgTranscript::pedersen_dkg(
            &finalized_config,
            std::slice::from_ref(&keyset_share.public_ceremony),
            &BTreeMap::from([
                (member_id(1), private_member(1)),
                (member_id(2), private_member(2)),
            ]),
        )
        .expect("pedersen DKG transcript"),
    );
    finalized_config
        .validate()
        .expect("finalized config transcript validates");
    finalized_config
}

fn dkg_secret_share_request(
    public_config: &FederationConfig,
    sender: MemberId,
    receiver: MemberId,
    seed: u8,
) -> FederationDkgSecretShareRequest {
    FederationDkgSecretShareRequest::new(
        public_config.federation_id,
        dkg_ceremony_id(),
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
    ceremonies: &[FederationDkgPublicCeremony],
    private_config: &MemberConfigPrivate,
) -> FederationDkgTranscriptSignatureRequest {
    let transcript_hash = FederationDkgTranscript::hash_for_config(
        FederationDkgSetupMethod::PedersenDkg,
        public_config,
        ceremonies,
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
    assert!(db
        .get_federation_journal_tip()
        .await
        .expect("empty journal tip")
        .is_none());

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
    let first_catch_up_certificate = FederationJournalCatchUpResponse::from_journal(
        federation_id(),
        &journal,
        FederationMaterializedStateDigest::genesis(),
        &FederationJournalCatchUpRequest::new(0),
        50,
        100,
    )
    .with_signed_catch_up_certificate(&second_private)
    .expect("first catch-up certificate")
    .catch_up_certificate
    .expect("first catch-up certificate");
    let second_catch_up_certificate = FederationJournalCatchUpResponse::from_journal(
        federation_id(),
        &journal,
        FederationMaterializedStateDigest::genesis(),
        &FederationJournalCatchUpRequest::new(1),
        50,
        100,
    )
    .with_signed_catch_up_certificate(&second_private)
    .expect("second catch-up certificate")
    .catch_up_certificate
    .expect("second catch-up certificate");

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_journal_entry(&first)
        .await
        .expect("persist first");
    tx.add_federation_journal_entry(&second)
        .await
        .expect("persist second");
    tx.set_federation_operation_application_state(
        first.operation_id,
        &FederationApplicationState::Applied {},
    )
    .await
    .expect("update status");
    let rejection = ErrorResponse::from(Error::TokenAlreadySpent);
    tx.set_federation_operation_application_state(
        second.operation_id,
        &FederationApplicationState::Rejected {
            response: rejection.clone(),
        },
    )
    .await
    .expect("reject operation");
    tx.set_federation_dkg_member_sequence(member_one, 7)
        .await
        .expect("sequence one");
    tx.set_federation_dkg_member_sequence(member_two, 11)
        .await
        .expect("sequence two");
    tx.set_federation_dkg_member_sequence(member_one, 9)
        .await
        .expect("sequence one update");
    tx.set_federation_dkg_member_sequence(member_one, 5)
        .await
        .expect("sequence one stale update");
    tx.add_federation_catch_up_certificate(&first_catch_up_certificate)
        .await
        .expect("persist first catch-up certificate");
    tx.add_federation_catch_up_certificate(&second_catch_up_certificate)
        .await
        .expect("persist second catch-up certificate");
    tx.commit().await.expect("commit");

    let entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].index, 0);
    assert_eq!(entries[0].operation_id, first.operation_id);
    assert_eq!(entries[0].status(), FederationOperationStatus::Applied);
    assert_eq!(entries[0].conflict_keys, first.conflict_keys);
    assert_eq!(entries[0].consensus_evidence, Some(first_evidence));
    assert_eq!(entries[1].index, 1);
    assert_eq!(entries[1].operation_id, second.operation_id);
    assert_eq!(entries[1].status(), FederationOperationStatus::Rejected);
    assert_eq!(entries[1].rejection(), Some(&rejection));
    assert_eq!(entries[1].previous_order_digest, first.order_digest);

    let tip = db
        .get_federation_journal_tip()
        .await
        .expect("journal tip")
        .expect("persisted journal tip");
    assert_eq!(tip.index, second.index);
    assert_eq!(tip.order_digest, second.order_digest);

    let first_by_operation_id = db
        .get_federation_journal_entry_by_operation_id(first.operation_id)
        .await
        .expect("first entry by operation id")
        .expect("first entry");
    assert_eq!(first_by_operation_id.index, first.index);
    assert_eq!(
        first_by_operation_id.status(),
        FederationOperationStatus::Applied
    );
    assert_eq!(
        first_by_operation_id.consensus_evidence,
        Some(first_evidence)
    );

    let second_by_operation_id = db
        .get_federation_journal_entry_by_operation_id(second.operation_id)
        .await
        .expect("second entry by operation id")
        .expect("second entry");
    assert_eq!(second_by_operation_id.index, second.index);
    assert_eq!(
        second_by_operation_id.status(),
        FederationOperationStatus::Rejected
    );
    assert_eq!(second_by_operation_id.rejection(), Some(&rejection));

    let missing_by_operation_id = db
        .get_federation_journal_entry_by_operation_id(FederationOperationId::from_bytes([99; 32]))
        .await
        .expect("missing entry by operation id");
    assert!(missing_by_operation_id.is_none());

    let catch_up_entries = db
        .get_federation_journal_entries_from(1)
        .await
        .expect("catch-up entries");
    assert_eq!(catch_up_entries.len(), 1);
    assert_eq!(catch_up_entries[0].operation_id, second.operation_id);
    assert_eq!(catch_up_entries[0].rejection(), Some(&rejection));

    let sequences = db
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequences");
    assert_eq!(sequences.get(&member_one), Some(&9));
    assert_eq!(sequences.get(&member_two), Some(&11));

    let catch_up_certificates = db
        .get_federation_catch_up_certificates()
        .await
        .expect("catch-up certificates");
    assert_eq!(catch_up_certificates.len(), 2);
    assert_eq!(catch_up_certificates[0].from_index, 0);
    assert_eq!(catch_up_certificates[0].signer, member_two);
    assert_eq!(
        catch_up_certificates[0].operation_ids,
        first_catch_up_certificate.operation_ids
    );
    assert_eq!(catch_up_certificates[1].from_index, 1);
    assert_eq!(
        catch_up_certificates[1].signature,
        second_catch_up_certificate.signature
    );

    let catch_up_certificates_from = db
        .get_federation_catch_up_certificates_from(1)
        .await
        .expect("catch-up certificates from");
    assert_eq!(catch_up_certificates_from.len(), 1);
    assert_eq!(catch_up_certificates_from[0].from_index, 1);
}

/// Assert every legal journal application state survives database round trips.
pub async fn federation_application_state_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let expected_states = vec![
        FederationApplicationState::Accepted {},
        FederationApplicationState::Applying {},
        FederationApplicationState::Applied {},
        FederationApplicationState::Failed {},
        FederationApplicationState::ConsensusRejected {},
        FederationApplicationState::Rejected {
            response: ErrorResponse::from(Error::TokenAlreadySpent),
        },
    ];
    let mut journal = InMemoryFederationJournal::new();
    let mut entries = Vec::with_capacity(expected_states.len());
    for index in 0..expected_states.len() {
        let entry = if index == 4 {
            appended(
                journal
                    .append_finalized_with_consensus_evidence(
                        mint_quote_envelope("application-state-0", 99),
                        FederationJournalConsensusEvidence::new(
                            0,
                            index as u64,
                            index as u64,
                            MemberId::new(1).expect("member one"),
                            index as u64,
                        ),
                    )
                    .expect("append consensus-rejected conflict"),
            )
        } else {
            appended(
                journal
                    .append(mint_quote_envelope(
                        &format!("application-state-{index}"),
                        index as u64 + 1,
                    ))
                    .expect("append application-state entry"),
            )
        };
        entries.push(entry);
    }

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    for (entry, application) in entries.iter().zip(expected_states.iter()) {
        tx.add_federation_journal_entry(entry)
            .await
            .expect("persist application-state entry");
        tx.set_federation_operation_application_state(entry.operation_id, application)
            .await
            .expect("persist application state");
    }
    tx.commit().await.expect("commit application states");

    let persisted = db
        .get_federation_journal_entries()
        .await
        .expect("load application states");
    let persisted_states = persisted
        .iter()
        .map(|entry| entry.application.clone())
        .collect::<Vec<_>>();
    assert_eq!(persisted_states, expected_states);

    let replayed = InMemoryFederationJournal::from_entries(persisted)
        .expect("replay every legal application state");
    let replayed_states = replayed
        .entries()
        .iter()
        .map(|entry| entry.application.clone())
        .collect::<Vec<_>>();
    assert_eq!(replayed_states, expected_states);
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
    let mut adapter = FederationConsensusRuntime::for_members(
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

    let recovered = FederationConsensusRuntime::recover_for_members_from_finalized_items(
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

/// Assert AlephBFT backup bytes append durably per session.
pub async fn federation_aleph_bft_backup_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    assert_eq!(
        db.get_federation_aleph_bft_backup_bytes(7)
            .await
            .expect("missing backup session"),
        None
    );

    let mut tx = Database::begin_transaction(&db).await.expect("begin empty");
    tx.append_federation_aleph_bft_backup_bytes(7, &[])
        .await
        .expect("empty backup append");
    tx.commit().await.expect("commit empty append");
    assert_eq!(
        db.get_federation_aleph_bft_backup_bytes(7)
            .await
            .expect("empty append should not create row"),
        None
    );

    let mut tx = Database::begin_transaction(&db).await.expect("begin first");
    tx.append_federation_aleph_bft_backup_bytes(7, b"first")
        .await
        .expect("append first bytes");
    tx.append_federation_aleph_bft_backup_bytes(8, b"other-session")
        .await
        .expect("append other session");
    tx.commit().await.expect("commit first bytes");

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin rollback");
    tx.append_federation_aleph_bft_backup_bytes(7, b"-rolled-back")
        .await
        .expect("append rolled back bytes");
    tx.rollback().await.expect("rollback backup append");

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin second");
    tx.append_federation_aleph_bft_backup_bytes(7, b"-second")
        .await
        .expect("append second bytes");
    tx.commit().await.expect("commit second bytes");

    assert_eq!(
        db.get_federation_aleph_bft_backup_bytes(7)
            .await
            .expect("session 7 backup bytes"),
        Some(b"first-second".to_vec())
    );
    assert_eq!(
        db.get_federation_aleph_bft_backup_bytes(8)
            .await
            .expect("session 8 backup bytes"),
        Some(b"other-session".to_vec())
    );
}

/// Assert AlephBFT active-session state is durable and monotonic.
pub async fn federation_aleph_bft_active_session_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    assert_eq!(
        db.get_federation_aleph_bft_active_session()
            .await
            .expect("missing active session"),
        None
    );

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin first session tx");
    tx.set_federation_aleph_bft_active_session(7)
        .await
        .expect("set active session");
    tx.commit().await.expect("commit active session");
    assert_eq!(
        db.get_federation_aleph_bft_active_session()
            .await
            .expect("active session after first commit"),
        Some(7)
    );

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin rollback session tx");
    tx.set_federation_aleph_bft_active_session(8)
        .await
        .expect("set rolled-back active session");
    tx.rollback().await.expect("rollback active session");
    assert_eq!(
        db.get_federation_aleph_bft_active_session()
            .await
            .expect("active session after rollback"),
        Some(7)
    );

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin stale session tx");
    tx.set_federation_aleph_bft_active_session(6)
        .await
        .expect("set stale active session");
    tx.commit().await.expect("commit stale session");
    assert_eq!(
        db.get_federation_aleph_bft_active_session()
            .await
            .expect("active session after stale write"),
        Some(7)
    );

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin newer session tx");
    tx.set_federation_aleph_bft_active_session(9)
        .await
        .expect("set newer active session");
    tx.commit().await.expect("commit newer session");
    assert_eq!(
        db.get_federation_aleph_bft_active_session()
            .await
            .expect("active session after newer write"),
        Some(9)
    );
}

/// Assert AlephBFT pending operation rows persist per session and can be removed.
pub async fn federation_aleph_bft_pending_operation_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let envelope = mint_quote_envelope("pending-a", 7);
    let operation_id = envelope.operation_id().expect("operation id");
    let pending = FederationAlephBftPendingOperationRecord {
        operation_id,
        session_id: 3,
        envelope,
    };
    let other_envelope = mint_quote_envelope("pending-b", 8);
    let other_session = FederationAlephBftPendingOperationRecord {
        operation_id: other_envelope.operation_id().expect("other operation id"),
        session_id: 4,
        envelope: other_envelope,
    };

    assert!(db
        .get_federation_aleph_bft_pending_operations(3)
        .await
        .expect("missing pending session")
        .is_empty());

    let mut tx = db.begin_transaction().await.expect("begin pending tx");
    tx.add_federation_aleph_bft_pending_operation(&pending)
        .await
        .expect("persist pending operation");
    tx.add_federation_aleph_bft_pending_operation(&other_session)
        .await
        .expect("persist other pending operation");
    tx.commit().await.expect("commit pending tx");

    let session_three = db
        .get_federation_aleph_bft_pending_operations(3)
        .await
        .expect("session 3 pending operations");
    assert_eq!(session_three.len(), 1);
    assert_eq!(session_three[0].operation_id, pending.operation_id);
    assert_eq!(session_three[0].session_id, pending.session_id);
    assert_eq!(
        session_three[0]
            .envelope
            .canonical_bytes()
            .expect("session 3 envelope bytes"),
        pending
            .envelope
            .canonical_bytes()
            .expect("expected pending envelope bytes")
    );
    let session_four = db
        .get_federation_aleph_bft_pending_operations(4)
        .await
        .expect("session 4 pending operations");
    assert_eq!(session_four.len(), 1);
    assert_eq!(session_four[0].operation_id, other_session.operation_id);
    assert_eq!(session_four[0].session_id, other_session.session_id);
    assert_eq!(
        session_four[0]
            .envelope
            .canonical_bytes()
            .expect("session 4 envelope bytes"),
        other_session
            .envelope
            .canonical_bytes()
            .expect("expected other pending envelope bytes")
    );

    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin pending rebind tx");
    tx.set_federation_aleph_bft_active_session(5)
        .await
        .expect("advance session and rebind pending operations");
    tx.commit().await.expect("commit pending rebind");

    assert!(db
        .get_federation_aleph_bft_pending_operations(3)
        .await
        .expect("rebound session 3")
        .is_empty());
    assert!(db
        .get_federation_aleph_bft_pending_operations(4)
        .await
        .expect("rebound session 4")
        .is_empty());
    let rebound = db
        .get_federation_aleph_bft_pending_operations(5)
        .await
        .expect("session 5 rebound operations");
    assert_eq!(rebound.len(), 2);
    assert!(rebound.iter().all(|operation| operation.session_id == 5));
    assert_eq!(
        rebound
            .iter()
            .map(|operation| operation.operation_id)
            .collect::<std::collections::BTreeSet<_>>(),
        [pending.operation_id, other_session.operation_id]
            .into_iter()
            .collect()
    );

    let stale_envelope = mint_quote_envelope("pending-stale", 9);
    let stale = FederationAlephBftPendingOperationRecord {
        operation_id: stale_envelope.operation_id().expect("stale operation id"),
        session_id: 4,
        envelope: stale_envelope,
    };
    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin stale pending tx");
    assert!(tx
        .add_federation_aleph_bft_pending_operation(&stale)
        .await
        .is_err());
    tx.rollback().await.expect("rollback stale pending tx");

    let future_envelope = mint_quote_envelope("pending-future", 9);
    let future = FederationAlephBftPendingOperationRecord {
        operation_id: future_envelope.operation_id().expect("future operation id"),
        session_id: 6,
        envelope: future_envelope,
    };
    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin future pending tx");
    tx.add_federation_aleph_bft_pending_operation(&future)
        .await
        .expect("persist future pending operation");
    tx.commit().await.expect("commit future pending operation");

    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin future validation tx");
    assert!(tx.set_federation_aleph_bft_active_session(5).await.is_err());
    tx.rollback().await.expect("rollback future validation tx");

    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin future session advance tx");
    tx.set_federation_aleph_bft_active_session(6)
        .await
        .expect("advance to future session");
    tx.commit().await.expect("commit future session advance");
    assert_eq!(
        db.get_federation_aleph_bft_pending_operations(6)
            .await
            .expect("session 6 pending operations")
            .len(),
        3
    );

    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin rolled-back session advance tx");
    tx.set_federation_aleph_bft_active_session(7)
        .await
        .expect("stage session 7 rebind");
    tx.rollback().await.expect("rollback session 7 rebind");
    assert_eq!(
        db.get_federation_aleph_bft_active_session()
            .await
            .expect("active session after rebind rollback"),
        Some(6)
    );
    assert_eq!(
        db.get_federation_aleph_bft_pending_operations(6)
            .await
            .expect("session 6 rows after rebind rollback")
            .len(),
        3
    );
    assert!(db
        .get_federation_aleph_bft_pending_operations(7)
        .await
        .expect("session 7 rows after rebind rollback")
        .is_empty());

    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin remove pending tx");
    tx.remove_federation_aleph_bft_pending_operation(pending.operation_id)
        .await
        .expect("remove pending operation");
    tx.commit().await.expect("commit remove pending tx");

    assert!(db
        .get_federation_aleph_bft_pending_operations(6)
        .await
        .expect("pending operations after remove")
        .iter()
        .all(|operation| operation.operation_id != pending.operation_id));
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

    let finalized_config = dkg_finalized_public_config(&snapshot.config);
    let finalized_snapshot =
        FederationConfigSnapshot::new(finalized_config).expect("finalized config snapshot");
    assert_eq!(finalized_snapshot.federation_id, snapshot.federation_id);
    assert_ne!(finalized_snapshot.config_digest, snapshot.config_digest);

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin finalized DKG config");
    tx.set_federation_config(&finalized_snapshot)
        .await
        .expect("persist finalized DKG config");
    tx.commit().await.expect("commit finalized DKG config");

    assert_eq!(
        db.get_federation_config(config.federation_id)
            .await
            .expect("load finalized DKG config"),
        Some(finalized_snapshot.clone())
    );

    let mut conflicting_config = finalized_snapshot.config.clone();
    conflicting_config.payment_observation_policy.mint_quotes =
        crate::federation::FederationPaymentVerificationPolicy::development_single_observation_for_testing();
    conflicting_config
        .validate()
        .expect("conflicting config validates");
    let conflicting_snapshot =
        FederationConfigSnapshot::new(conflicting_config).expect("conflicting snapshot");
    assert_eq!(conflicting_snapshot.federation_id, snapshot.federation_id);
    assert_ne!(conflicting_snapshot.config_digest, snapshot.config_digest);
    assert_ne!(
        conflicting_snapshot.config_digest,
        finalized_snapshot.config_digest
    );

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting config");
    assert!(tx
        .set_federation_config(&conflicting_snapshot)
        .await
        .is_err());
    tx.rollback().await.expect("rollback conflicting config");
}

/// Assert projection quote-state reads preserve input order and missing rows.
pub async fn federation_mint_quote_state_projection<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let first_id = crate::quote_id::QuoteId::new();
    let second_id = crate::quote_id::QuoteId::new();
    let missing_id = crate::quote_id::QuoteId::new();
    let quote = |id: crate::quote_id::QuoteId| {
        crate::mint::MintQuote::new(
            Some(id.clone()),
            format!("invoice-{id}"),
            CurrencyUnit::Sat,
            Some(Amount::from(5).with_unit(CurrencyUnit::Sat)),
            10_000,
            crate::payment::PaymentIdentifier::QuoteId(id),
            None,
            Amount::ZERO.with_unit(CurrencyUnit::Sat),
            Amount::ZERO.with_unit(CurrencyUnit::Sat),
            PaymentMethod::Known(KnownMethod::Bolt11),
            1,
            1,
            Vec::new(),
            Vec::new(),
            None,
        )
    };

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_mint_quote(quote(first_id.clone()))
        .await
        .expect("add first quote");
    let mut second_quote = tx
        .add_mint_quote(quote(second_id.clone()))
        .await
        .expect("add second quote");
    second_quote
        .increment_amount_paid(Amount::from(5).with_unit(CurrencyUnit::Sat))
        .expect("mark second quote paid");
    second_quote
        .add_issuance(Amount::from(2).with_unit(CurrencyUnit::Sat))
        .expect("partially issue second quote");
    tx.update_mint_quote(&mut second_quote)
        .await
        .expect("persist second quote state");
    tx.commit().await.expect("commit quotes");

    let states = db
        .get_federation_mint_quote_states(&[second_id.clone(), missing_id, first_id.clone()])
        .await
        .expect("load projection quote states");
    assert_eq!(states.len(), 3);
    let second = states[0].as_ref().expect("second quote state");
    assert_eq!(second.id, second_id);
    assert_eq!(second.amount_paid.value(), 5);
    assert_eq!(second.amount_issued.value(), 2);
    assert_eq!(second.state, MintQuoteState::Paid);
    assert!(states[1].is_none());
    let first = states[2].as_ref().expect("first quote state");
    assert_eq!(first.id, first_id);
    assert_eq!(first.state, MintQuoteState::Unpaid);
}

/// Assert projection reads stay keyed when unrelated materialized history is large.
pub async fn federation_materialized_projection_keyed_reads<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let melt_quote = |label: &str| {
        MeltQuote::new(
            None,
            MeltPaymentRequest::Custom {
                method: "projection-test".to_string(),
                request: label.to_string(),
            },
            CurrencyUnit::Sat,
            Amount::new(10, CurrencyUnit::Sat),
            Amount::new(1, CurrencyUnit::Sat),
            10_000,
            None,
            None,
            PaymentMethod::Custom("projection-test".to_string()),
            None,
            None,
        )
    };
    let selected_quote = melt_quote("selected");
    let selected_quote_id = selected_quote.id.clone();
    let selected_operation_id = uuid::Uuid::now_v7();
    let selected_operation = Operation::new(
        selected_operation_id,
        OperationKind::Melt,
        Amount::ZERO,
        Amount::from(10),
        Amount::from(1),
        Some(42),
        Some(PaymentMethod::Custom("projection-test".to_string())),
    );
    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_melt_quote(selected_quote)
        .await
        .expect("add selected melt quote");
    tx.add_completed_operation(&selected_operation, &std::collections::HashMap::new())
        .await
        .expect("add selected completed operation");
    for index in 0..128 {
        tx.add_melt_quote(melt_quote(&format!("unrelated-{index}")))
            .await
            .expect("add unrelated melt quote");
        let operation = Operation::new(
            uuid::Uuid::now_v7(),
            OperationKind::Swap,
            Amount::from(1),
            Amount::from(1),
            Amount::ZERO,
            Some(index),
            None,
        );
        tx.add_completed_operation(&operation, &std::collections::HashMap::new())
            .await
            .expect("add unrelated completed operation");
    }
    tx.commit().await.expect("commit projection history");

    let melt_quotes = db
        .get_federation_melt_quotes(&[selected_quote_id.clone(), crate::quote_id::QuoteId::new()])
        .await
        .expect("load keyed melt quotes");
    assert_eq!(melt_quotes.len(), 1);
    assert_eq!(melt_quotes[0].id, selected_quote_id);

    let completed_operations = db
        .get_federation_completed_operations(&[selected_operation_id, uuid::Uuid::now_v7()])
        .await
        .expect("load keyed completed operations");
    assert_eq!(completed_operations.len(), 1);
    assert_eq!(*completed_operations[0].id(), selected_operation_id);
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

    let all_observations = db
        .get_all_federation_payment_observations()
        .await
        .expect("all payment observations");
    assert_eq!(all_observations.len(), 4);
    assert_eq!(all_observations[0].operation_id, first.operation_id);
    assert_eq!(all_observations[3].operation_id, unrelated.operation_id);

    let wrong_context = db
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            "quote-1",
        )
        .await
        .expect("wrong-context payment observations");
    assert!(wrong_context.is_empty());
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

    let replacement_ceremony_id = FederationDkgCeremonyId::from_bytes([34; 32]);
    let mut replacement_reveal = dkg_reveal(&public_config, member_id(1), 31);
    replacement_reveal.ceremony_id = replacement_ceremony_id;
    let replacement_commitment =
        FederationDkgParticipantCommitment::from_reveal(&replacement_reveal)
            .expect("replacement commitment");
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin replacement ceremony");
    tx.add_federation_dkg_commitment(&replacement_commitment)
        .await
        .expect("persist replacement commitment at same member coordinate");
    tx.add_federation_dkg_reveal(&replacement_reveal)
        .await
        .expect("persist replacement reveal at same member coordinate");
    tx.commit().await.expect("commit replacement ceremony");

    let commitments = db
        .get_federation_dkg_commitments()
        .await
        .expect("multi-ceremony commitments");
    let reveals = db
        .get_federation_dkg_reveals()
        .await
        .expect("multi-ceremony reveals");
    assert_eq!(commitments.len(), 3);
    assert_eq!(reveals.len(), 3);
    assert_eq!(
        commitments
            .iter()
            .filter(|commitment| commitment.ceremony_id == replacement_ceremony_id)
            .count(),
        1
    );
    assert_eq!(
        reveals
            .iter()
            .filter(|reveal| reveal.ceremony_id == replacement_ceremony_id)
            .count(),
        1
    );
}

/// Assert local restartable DKG participant state persists idempotently by ceremony.
pub async fn federation_dkg_participant_state_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let public_config = dkg_public_config();
    let first = dkg_participant_state(&public_config, dkg_ceremony_id());

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_dkg_participant_state(&first)
        .await
        .expect("persist participant state");
    tx.add_federation_dkg_participant_state(&first)
        .await
        .expect("idempotent participant-state replay");
    tx.commit().await.expect("commit");
    assert_eq!(
        db.get_federation_dkg_participant_states()
            .await
            .expect("participant states"),
        vec![first.clone()]
    );

    let conflicting = dkg_participant_state(&public_config, dkg_ceremony_id());
    assert_ne!(conflicting, first);
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting state");
    assert!(tx
        .add_federation_dkg_participant_state(&conflicting)
        .await
        .is_err());
    tx.rollback().await.expect("rollback conflicting state");

    let second_ceremony_id = FederationDkgCeremonyId::from_bytes([34; 32]);
    let second = dkg_participant_state(&public_config, second_ceremony_id);
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin second ceremony");
    tx.add_federation_dkg_participant_state(&second)
        .await
        .expect("persist second ceremony");
    tx.commit().await.expect("commit second ceremony");
    assert_eq!(
        db.get_federation_dkg_participant_states()
            .await
            .expect("participant states"),
        vec![first, second]
    );
}

/// Assert DKG ceremony lifecycle transitions are durable compare-and-set updates.
pub async fn federation_dkg_ceremony_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let first_id = dkg_ceremony_id();
    let second_id = FederationDkgCeremonyId::from_bytes([34; 32]);
    let spec = dkg_reveal(&dkg_public_config(), member_id(1), 11).keyset_spec;
    let first = FederationDkgCeremonyRecord::active(
        dkg_public_config().federation_id,
        first_id,
        member_id(1),
        spec.clone(),
        None,
    );
    let second = FederationDkgCeremonyRecord::active(
        dkg_public_config().federation_id,
        second_id,
        member_id(1),
        spec,
        Some(first_id),
    );

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_dkg_ceremony(&first)
        .await
        .expect("persist active ceremony");
    tx.add_federation_dkg_ceremony(&first)
        .await
        .expect("idempotent active ceremony replay");
    tx.commit().await.expect("commit active ceremony");

    let aborted = first.aborted_by(second_id).expect("aborted state");
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin replacement");
    tx.transition_federation_dkg_ceremony(&first, &aborted)
        .await
        .expect("abort first ceremony");
    tx.add_federation_dkg_ceremony(&second)
        .await
        .expect("persist replacement ceremony");
    tx.commit().await.expect("commit replacement");

    let records = db
        .get_federation_dkg_ceremonies()
        .await
        .expect("ceremony records");
    assert_eq!(records, vec![aborted.clone(), second.clone()]);
    assert_eq!(records[0].state, FederationDkgCeremonyState::Aborted);
    assert_eq!(records[1].state, FederationDkgCeremonyState::Active);

    let completed = second.completed().expect("completed state");
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin completion");
    tx.transition_federation_dkg_ceremony(&second, &completed)
        .await
        .expect("complete replacement");
    tx.transition_federation_dkg_ceremony(&second, &completed)
        .await
        .expect("idempotent completion replay");
    tx.commit().await.expect("commit completion");

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin stale transition");
    assert!(tx
        .transition_federation_dkg_ceremony(&first, &first.completed().expect("stale completion"))
        .await
        .is_err());
    tx.rollback().await.expect("rollback stale transition");
    assert_eq!(
        db.get_federation_dkg_ceremonies()
            .await
            .expect("completed ceremonies"),
        vec![aborted, completed]
    );
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
    assert_eq!(
        contributions,
        vec![first_contribution.clone(), second_contribution]
    );

    let mut replacement_contribution = first_contribution;
    replacement_contribution.ceremony_id = FederationDkgCeremonyId::from_bytes([34; 32]);
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin replacement contribution");
    tx.add_federation_dkg_secret_share_contribution(&replacement_contribution)
        .await
        .expect("persist replacement contribution at same member coordinate");
    tx.commit().await.expect("commit replacement contribution");
    let contributions = db
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("multi-ceremony secret-share contributions");
    assert_eq!(contributions.len(), 3);
    assert!(contributions.contains(&replacement_contribution));
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
    let setup_config = dkg_public_config();
    let keyset_share = dkg_member_keyset_share(member_id(1));
    let mut public_config = setup_config;
    public_config.keysets = vec![keyset_share.keyset];
    public_config
        .validate()
        .expect("public DKG keyset validates");
    let ceremonies = vec![keyset_share.public_ceremony];
    let first_private = private_member(1);
    let second_private = private_member(2);
    let first_signature =
        dkg_transcript_signature_request(&public_config, &ceremonies, &first_private);
    let second_signature =
        dkg_transcript_signature_request(&public_config, &ceremonies, &second_private);
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
        &ceremonies,
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

/// Assert finalized-config activation confirmations persist idempotently by ceremony/member.
pub async fn federation_dkg_activation_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let setup_config = dkg_public_config();
    let final_config = dkg_finalized_public_config(&setup_config);
    let first_activation =
        FederationDkgActivationRequest::sign(dkg_ceremony_id(), &final_config, &private_member(1))
            .expect("first activation");
    let second_activation =
        FederationDkgActivationRequest::sign(dkg_ceremony_id(), &final_config, &private_member(2))
            .expect("second activation");
    let mut conflicting_activation = first_activation;
    conflicting_activation.final_config_digest = FederationConfigDigest::from_bytes([99; 32]);

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_dkg_activation(&first_activation)
        .await
        .expect("persist first activation");
    tx.add_federation_dkg_activation(&first_activation)
        .await
        .expect("idempotent first activation replay");
    tx.add_federation_dkg_activation(&second_activation)
        .await
        .expect("persist second activation");
    tx.commit().await.expect("commit activations");

    let activations = db
        .get_federation_dkg_activations()
        .await
        .expect("DKG activations");
    assert_eq!(activations, vec![first_activation, second_activation]);

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting activation");
    assert!(tx
        .add_federation_dkg_activation(&conflicting_activation)
        .await
        .is_err());
    tx.rollback()
        .await
        .expect("rollback conflicting activation");
    assert_eq!(
        db.get_federation_dkg_activations()
            .await
            .expect("activations after conflict"),
        vec![first_activation, second_activation]
    );
}

/// Assert every FROST DKG phase is durable, replay-idempotent, and equivocation-safe.
pub async fn federation_frost_dkg_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let config = dkg_public_config();
    let context =
        FederationFrostDkgContext::new(&config, dkg_ceremony_id(), 1).expect("FROST DKG context");
    let first = member_id(1);
    let second = member_id(2);
    let round1 = FederationFrostDkgRound1Request {
        context: context.clone(),
        sender: first,
        package: vec![1, 2, 3],
    };
    let mut conflicting_round1 = round1.clone();
    conflicting_round1.package.push(4);
    let round2 = FederationFrostDkgRound2Record {
        context: context.clone(),
        sender: first,
        receiver: second,
        encrypted_package: vec![5, 6, 7],
    };
    let mut conflicting_round2 = round2.clone();
    conflicting_round2.encrypted_package.push(8);
    let local_round1 = FederationFrostDkgLocalState {
        context: context.clone(),
        member_id: first,
        phase: FederationFrostDkgLocalPhase::Round1,
        encrypted_state: vec![9, 10],
    };
    let local_round2 = FederationFrostDkgLocalState {
        context: context.clone(),
        member_id: first,
        phase: FederationFrostDkgLocalPhase::Round2,
        encrypted_state: vec![11, 12],
    };
    let root = FederationFrostRootRecord {
        context: context.clone(),
        member_id: first,
        public_package: vec![13, 14],
        encrypted_root_share: vec![15, 16],
    };
    let confirmation = FederationFrostDkgConfirmationRequest {
        context,
        member_id: first,
        public_package: vec![13, 14],
    };

    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_frost_dkg_round1(&round1)
        .await
        .expect("persist round one");
    tx.add_federation_frost_dkg_round1(&round1)
        .await
        .expect("idempotent round-one replay");
    tx.add_federation_frost_dkg_round2(&round2)
        .await
        .expect("persist round two");
    tx.set_federation_frost_dkg_local_state(&local_round1)
        .await
        .expect("persist sealed round-one state");
    tx.set_federation_frost_dkg_local_state(&local_round2)
        .await
        .expect("persist sealed round-two state");
    tx.add_federation_frost_root_record(&root)
        .await
        .expect("persist sealed root");
    tx.add_federation_frost_dkg_confirmation(&confirmation)
        .await
        .expect("persist root confirmation");
    tx.commit().await.expect("commit FROST DKG records");

    assert_eq!(
        db.get_federation_frost_dkg_round1()
            .await
            .expect("round-one records"),
        vec![round1.clone()]
    );
    assert_eq!(
        db.get_federation_frost_dkg_round2()
            .await
            .expect("round-two records"),
        vec![round2.clone()]
    );
    assert_eq!(
        db.get_federation_frost_dkg_local_states()
            .await
            .expect("sealed local states"),
        vec![local_round1, local_round2]
    );
    assert_eq!(
        db.get_federation_frost_root_records()
            .await
            .expect("root records"),
        vec![root]
    );
    assert_eq!(
        db.get_federation_frost_dkg_confirmations()
            .await
            .expect("confirmations"),
        vec![confirmation]
    );

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting FROST records");
    assert!(tx
        .add_federation_frost_dkg_round1(&conflicting_round1)
        .await
        .is_err());
    tx.rollback().await.expect("rollback round-one conflict");

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin round-two conflict");
    assert!(tx
        .add_federation_frost_dkg_round2(&conflicting_round2)
        .await
        .is_err());
    tx.rollback().await.expect("rollback round-two conflict");
}

/// Assert FROST wallet config snapshots persist independently and equivocation-safely.
pub async fn federation_frost_wallet_config_persistence<DB>(db: DB)
where
    DB: Database<crate::database::Error> + FederationJournalDatabase<Err = crate::database::Error>,
{
    let public_config = dkg_public_config();
    let application_id =
        FederationFrostApplicationId::from_bytes([42; 32]).expect("application id");
    let root = FederationFrostRootPublicConfig {
        version: 1,
        federation_id: public_config.federation_id,
        ceremony_id: dkg_ceremony_id(),
        root_key_epoch: 1,
        dkg_protocol_version: 1,
        ciphersuite_version: 1,
        threshold: public_config.threshold_params.signature_threshold,
        participant_mapping: [(member_id(1), 1), (member_id(2), 2)].into_iter().collect(),
        verification_shares: [(member_id(1), [2; 33]), (member_id(2), [3; 33])]
            .into_iter()
            .collect(),
        untweaked_group_key: [4; 33],
        transcript_binding: [5; 32],
        canonical_public_package: vec![6, 7],
    };
    let private = FederationFrostRootPrivateConfig {
        version: 1,
        federation_id: public_config.federation_id,
        member_id: member_id(1),
        root_key_epoch: 1,
        public_root_digest: [8; 32],
        local_verification_share: [2; 33],
        encrypted_root_share: vec![9, 10],
        sealed_storage: FederationFrostSealedStorageMetadata::member_identity_derived(
            public_config.federation_id,
            dkg_ceremony_id(),
            member_id(1),
            1,
        ),
    };
    let application = FederationFrostApplicationConfig {
        version: 1,
        application_id,
        application_key_epoch: 1,
        root_key_epoch: 1,
        derivation_version: 1,
        authorization_policy: [11; 32],
        threshold: public_config.threshold_params.signature_threshold,
        verification_shares: [(member_id(1), [12; 33]), (member_id(2), [13; 33])]
            .into_iter()
            .collect(),
        derived_group_key: [14; 33],
        chain_code: [15; 32],
        external_descriptor: "tr(tpub-test/0/*)".to_string(),
        internal_descriptor: "tr(tpub-test/1/*)".to_string(),
        descriptor_commitment: [16; 32],
        network: bitcoin::Network::Regtest,
        confirmation_policy: FederationOnchainConfirmationPolicy {
            version: 1,
            minimum_confirmations: 2,
        },
        fee_policy_version: 1,
    };
    let mut tx = Database::begin_transaction(&db).await.expect("begin");
    tx.add_federation_frost_root_public_config(&root)
        .await
        .expect("root public");
    tx.add_federation_frost_root_public_config(&root)
        .await
        .expect("idempotent root public");
    tx.add_federation_frost_root_private_config(&private)
        .await
        .expect("root private");
    tx.add_federation_frost_application_config(&application)
        .await
        .expect("application");
    tx.commit().await.expect("commit wallet config");

    assert_eq!(
        db.get_federation_frost_root_public_configs()
            .await
            .expect("public roots"),
        vec![root.clone()]
    );
    assert_eq!(
        db.get_federation_frost_root_private_configs()
            .await
            .expect("private roots"),
        vec![private]
    );
    assert_eq!(
        db.get_federation_frost_application_configs()
            .await
            .expect("applications"),
        vec![application]
    );
    let mut conflicting = root;
    conflicting.transcript_binding[0] ^= 1;
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("begin conflicting root");
    assert!(tx
        .add_federation_frost_root_public_config(&conflicting)
        .await
        .is_err());
    tx.rollback().await.expect("rollback conflicting root");
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
            tx.set_federation_operation_application_state(
                entry.operation_id,
                &FederationApplicationState::Applied {},
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
        assert_eq!(persisted.status(), FederationOperationStatus::Applied);
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

/// Assert concurrent federation metadata updates preserve monotonic watermarks and certificates.
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
            tx.set_federation_dkg_member_sequence(member, sequence)
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
    let catch_up_certificates = [
        FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            FederationMaterializedStateDigest::genesis(),
            &FederationJournalCatchUpRequest::new(0),
            50,
            100,
        )
        .with_signed_catch_up_certificate(&member_two_private)
        .expect("member two certificate from zero")
        .catch_up_certificate
        .expect("member two certificate from zero"),
        FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            FederationMaterializedStateDigest::genesis(),
            &FederationJournalCatchUpRequest::new(0),
            50,
            100,
        )
        .with_signed_catch_up_certificate(&member_three_private)
        .expect("member three certificate from zero")
        .catch_up_certificate
        .expect("member three certificate from zero"),
        FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            FederationMaterializedStateDigest::genesis(),
            &FederationJournalCatchUpRequest::new(1),
            50,
            100,
        )
        .with_signed_catch_up_certificate(&member_two_private)
        .expect("member two certificate from one")
        .catch_up_certificate
        .expect("member two certificate from one"),
        FederationJournalCatchUpResponse::from_journal(
            federation_id(),
            &journal,
            FederationMaterializedStateDigest::genesis(),
            &FederationJournalCatchUpRequest::new(2),
            50,
            100,
        )
        .with_signed_catch_up_certificate(&member_three_private)
        .expect("member three certificate from two")
        .catch_up_certificate
        .expect("member three certificate from two"),
    ];

    let mut certificate_tasks = Vec::new();
    for certificate in catch_up_certificates.clone() {
        let db = Arc::clone(&db);
        certificate_tasks.push(tokio::spawn(async move {
            let mut tx = Database::begin_transaction(db.as_ref())
                .await
                .expect("begin certificate transaction");
            tx.add_federation_catch_up_certificate(&certificate)
                .await
                .expect("persist catch-up certificate");
            tx.commit().await.expect("commit certificate transaction");
        }));
    }

    for task in certificate_tasks {
        task.await.expect("certificate writer task");
    }

    let sequences = db
        .as_ref()
        .get_federation_dkg_member_sequences()
        .await
        .expect("metadata sequences");
    assert_eq!(sequences.get(&member_one), Some(&5));
    assert_eq!(sequences.get(&member_two), Some(&7));
    assert_eq!(sequences.get(&member_three), Some(&4));

    let stored_certificates = db
        .as_ref()
        .get_federation_catch_up_certificates()
        .await
        .expect("metadata catch-up certificates");
    assert_eq!(stored_certificates.len(), catch_up_certificates.len());
    assert_eq!(stored_certificates[0].from_index, 0);
    assert_eq!(stored_certificates[0].signer, member_two);
    assert_eq!(stored_certificates[1].from_index, 0);
    assert_eq!(stored_certificates[1].signer, member_three);
    assert_eq!(stored_certificates[2].from_index, 1);
    assert_eq!(stored_certificates[2].signer, member_two);
    assert_eq!(stored_certificates[3].from_index, 2);
    assert_eq!(stored_certificates[3].signer, member_three);

    let ranged_certificates = db
        .as_ref()
        .get_federation_catch_up_certificates_from(1)
        .await
        .expect("metadata ranged certificates");
    assert_eq!(ranged_certificates.len(), 2);
    assert_eq!(ranged_certificates[0].from_index, 1);
    assert_eq!(ranged_certificates[1].from_index, 2);
}

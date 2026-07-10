use std::collections::BTreeMap;
use std::str::FromStr;

use cdk_common::federation::{
    generate_trusted_dealer_configs, FederatedKeyset, FederationDkgCeremonyId,
    FederationDkgKeysetSpec, FederationDkgTranscript, FederationDkgTranscriptSignature, MemberId,
    ThresholdParams, TrustedDealerKeysetSpec, TrustedDealerMember,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::nut01::{evaluate_secret_key_coefficients, BlsSecretKey};
use cdk_common::{Amount, CurrencyUnit, SecretKey};

use super::*;

struct DkgCompletionFixture {
    setup_public_config: FederationConfig,
    setup_private_config: MemberConfigPrivate,
    keyset_share: FederationDkgMemberKeysetShare,
    transcript_signatures: Vec<FederationDkgTranscriptSignatureRequest>,
}

fn member_id(id: u16) -> MemberId {
    MemberId::new(id).expect("member id")
}

fn identity_secret(member_id: MemberId) -> SecretKey {
    let mut bytes = [0u8; 32];
    bytes[31] = member_id.get() as u8;
    SecretKey::from_slice(&bytes).expect("identity secret")
}

fn member(member_id: MemberId) -> TrustedDealerMember {
    TrustedDealerMember::new(
        member_id,
        MintUrl::from_str(&format!("https://mint{}.example.com", member_id.get()))
            .expect("public mint url"),
        MintUrl::from_str(&format!("https://fed{}.example.com", member_id.get()))
            .expect("federation api url"),
        identity_secret(member_id),
    )
}

fn bls_key(byte: u8) -> BlsSecretKey {
    BlsSecretKey::from_reduced_bytes(&[byte; 32])
}

fn dkg_ceremony_id() -> FederationDkgCeremonyId {
    FederationDkgCeremonyId::from_bytes([44; 32])
}

fn dkg_reveal_for_status(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> FederationDkgParticipantCommitmentReveal {
    let commitments = (0..public_config.threshold_params.signature_threshold)
        .map(|offset| bls_key(seed + offset as u8).public_key_g2())
        .collect::<Vec<_>>();
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
        coefficient_commitments: BTreeMap::from([(Amount::from(1), commitments)]),
    }
}

fn dkg_secret_share_request_for_status(
    public_config: &FederationConfig,
    sender: MemberId,
    receiver: MemberId,
    seed: u8,
) -> FederationDkgSecretShareRequest {
    let coefficients = (0..public_config.threshold_params.signature_threshold)
        .map(|offset| bls_key(seed + offset as u8))
        .collect::<Vec<_>>();
    FederationDkgSecretShareRequest::new(
        public_config.federation_id,
        dkg_ceremony_id(),
        sender,
        receiver,
        BTreeMap::from([(
            Amount::from(1),
            evaluate_secret_key_coefficients(receiver.to_bls_signer_id(), &coefficients)
                .expect("secret share contribution"),
        )]),
    )
}

fn dkg_completion_fixture(local_member: MemberId) -> DkgCompletionFixture {
    let bundle = generate_trusted_dealer_configs(
        ThresholdParams::with_bft_consensus(3, 2).expect("params"),
        vec![
            member(member_id(1)),
            member(member_id(2)),
            member(member_id(3)),
        ],
        vec![TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            BTreeMap::from([
                (Amount::from(1), vec![bls_key(11), bls_key(12)]),
                (Amount::from(2), vec![bls_key(21), bls_key(22)]),
            ]),
        )],
    )
    .expect("trusted dealer bundle");

    let mut final_public_config = bundle.public_config.clone();
    final_public_config.setup_transcript = None;
    let transcript_hash = FederationDkgTranscript::hash_for_config(
        FederationDkgSetupMethod::PedersenDkg,
        &final_public_config,
    )
    .expect("pedersen transcript hash");
    let transcript_signatures = bundle
        .private_configs
        .values()
        .map(|private_config| {
            FederationDkgTranscriptSignatureRequest::new(
                final_public_config.federation_id,
                private_config.member_id,
                transcript_hash,
                FederationDkgTranscriptSignature::sign(transcript_hash, private_config)
                    .expect("transcript signature"),
            )
        })
        .collect::<Vec<_>>();

    let final_keyset = final_public_config
        .keysets
        .first()
        .expect("federated keyset")
        .clone();
    let local_private_config = bundle
        .private_configs
        .get(&local_member)
        .expect("local private config");
    let keyset_share = keyset_share_from_private(
        final_public_config.federation_id,
        local_member,
        &final_keyset,
        local_private_config,
    );

    let mut setup_public_config = final_public_config;
    setup_public_config.keysets.clear();
    setup_public_config.validate().expect("setup public config");

    let mut setup_private_config = local_private_config.clone();
    setup_private_config.keyset_secret_shares.clear();
    setup_private_config.setup_transcript_hash = None;
    setup_private_config
        .validate_against_public(&setup_public_config)
        .expect("setup private config");

    DkgCompletionFixture {
        setup_public_config,
        setup_private_config,
        keyset_share,
        transcript_signatures,
    }
}

#[cfg(feature = "sqlcipher")]
fn test_db_password() -> Option<String> {
    Some("federation-dkg-status-test-password".to_string())
}

#[cfg(not(feature = "sqlcipher"))]
fn test_db_password() -> Option<String> {
    None
}

fn test_cli_args(work_dir: PathBuf) -> CLIArgs {
    CLIArgs {
        command: None,
        work_dir: Some(work_dir),
        #[cfg(feature = "sqlcipher")]
        password: test_db_password().expect("sqlcipher test password"),
        config: None,
        seed_file: None,
        enable_logging: false,
    }
}

fn keyset_share_from_private(
    federation_id: cdk_common::federation::FederationId,
    local_member: MemberId,
    keyset: &FederatedKeyset,
    private_config: &MemberConfigPrivate,
) -> FederationDkgMemberKeysetShare {
    FederationDkgMemberKeysetShare {
        federation_id,
        member_id: local_member,
        keyset: keyset.clone(),
        secret_shares: private_config
            .keyset_secret_shares
            .get(&keyset.id)
            .expect("local keyset secret shares")
            .clone(),
    }
}

#[test]
fn dkg_status_report_marks_finalized_records_ready_without_leaking_private_shares() {
    let fixture = dkg_completion_fixture(member_id(2));

    let report = dkg_status_report(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        std::slice::from_ref(&fixture.keyset_share),
        &fixture.transcript_signatures,
    );

    assert!(report.ready);
    assert!(report.text.contains("DKG status for member 2"));
    assert!(report.text.contains("setup_keysets=0"));
    assert!(report.text.contains("keyset_share_records=1"));
    assert!(report.text.contains("transcript_signature_records=3"));
    assert!(report.text.contains("transcript_signers=1,2,3"));
    assert!(report.text.contains("finalized_ready=true"));
    assert!(report.text.contains("final_keysets=1"));
    assert!(report.text.contains("local_keysets=1 local_amounts=2"));
    for private_fragment in [
        "identity_secret_key",
        "keyset_secret_shares",
        "BlsSecretKeyShare",
    ] {
        assert!(!report.text.contains(private_fragment));
    }
}

#[test]
fn dkg_status_report_marks_missing_transcript_signature_not_ready() {
    let mut fixture = dkg_completion_fixture(member_id(2));
    fixture
        .transcript_signatures
        .retain(|signature| signature.signer != member_id(3));

    let report = dkg_status_report(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        std::slice::from_ref(&fixture.keyset_share),
        &fixture.transcript_signatures,
    );

    assert!(!report.ready);
    assert!(report.text.contains("transcript_signature_records=2"));
    assert!(report.text.contains("finalized_ready=false"));
    assert!(report
        .text
        .contains("missing transcript signature for member 3"));
    assert!(!report.text.contains("keyset_secret_shares"));
}

#[test]
fn dkg_status_report_summarizes_persisted_setup_records_without_secrets() {
    let fixture = dkg_completion_fixture(member_id(2));
    let setup_summary = DkgSetupRecordSummary {
        commitments: 3,
        reveals: 2,
        secret_share_contributions: 4,
        commitment_members: BTreeSet::from([member_id(1), member_id(2), member_id(3)]),
        reveal_members: BTreeSet::from([member_id(1), member_id(2)]),
        secret_share_senders: BTreeSet::from([member_id(1), member_id(2), member_id(3)]),
        secret_share_receivers: BTreeSet::from([member_id(2)]),
    };

    let report = dkg_status_report_with_setup_records(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        std::slice::from_ref(&fixture.keyset_share),
        &fixture.transcript_signatures,
        &setup_summary,
    );

    assert!(report.ready);
    assert!(report
        .text
        .contains("setup_records commitments=3 reveals=2"));
    assert!(report
        .text
        .contains("secret_share_contributions=4 commitment_members=1,2,3"));
    assert!(report.text.contains("reveal_members=1,2"));
    assert!(report.text.contains("secret_share_senders=1,2,3"));
    assert!(report.text.contains("secret_share_receivers=2"));
    for private_fragment in [
        "secret_shares",
        "BlsSecretKeyShare",
        "coefficient_commitments",
        "commitment_hash",
    ] {
        assert!(!report.text.contains(private_fragment));
    }
}

#[test]
fn dkg_status_report_marks_finalized_config_ready_without_records() {
    let bundle = generate_trusted_dealer_configs(
        ThresholdParams::with_bft_consensus(3, 2).expect("params"),
        vec![
            member(member_id(1)),
            member(member_id(2)),
            member(member_id(3)),
        ],
        vec![TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            BTreeMap::from([(Amount::from(1), vec![bls_key(11), bls_key(12)])]),
        )],
    )
    .expect("trusted dealer bundle");
    let private_config = bundle
        .private_configs
        .get(&member_id(2))
        .expect("private config");

    let report = dkg_status_report(&bundle.public_config, private_config, &[], &[]);

    assert!(report.ready);
    assert!(report.text.contains("setup_transcript=trusted_dealer"));
    assert!(report.text.contains("finalized_ready=true source=config"));
    assert!(report.text.contains("final_keysets=1"));
    assert!(report.text.contains("local_keysets=1 local_amounts=1"));
    assert!(!report.text.contains("keyset_secret_shares"));
}

#[test]
fn merge_dkg_records_appends_database_records_to_file_records() {
    let fixture = dkg_completion_fixture(member_id(2));
    let mut keyset_shares = Vec::new();
    let mut transcript_signatures = fixture.transcript_signatures[..1].to_vec();

    merge_dkg_records(
        &mut keyset_shares,
        &mut transcript_signatures,
        DkgCompletionRecords {
            keyset_shares: vec![fixture.keyset_share.clone()],
            transcript_signatures: fixture.transcript_signatures[1..].to_vec(),
            ..DkgCompletionRecords::default()
        },
    );

    assert_eq!(keyset_shares, vec![fixture.keyset_share.clone()]);
    assert_eq!(transcript_signatures, fixture.transcript_signatures);
    let report = dkg_status_report(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        &keyset_shares,
        &transcript_signatures,
    );
    assert!(report.ready);
    assert!(report.text.contains("finalized_ready=true"));
}

#[test]
fn merge_dkg_records_deduplicates_database_records_already_supplied_from_files() {
    let fixture = dkg_completion_fixture(member_id(2));
    let mut keyset_shares = vec![fixture.keyset_share.clone()];
    let mut transcript_signatures = fixture.transcript_signatures.clone();

    merge_dkg_records(
        &mut keyset_shares,
        &mut transcript_signatures,
        DkgCompletionRecords {
            keyset_shares: vec![fixture.keyset_share.clone()],
            transcript_signatures: fixture.transcript_signatures.clone(),
            ..DkgCompletionRecords::default()
        },
    );

    assert_eq!(keyset_shares, vec![fixture.keyset_share.clone()]);
    assert_eq!(transcript_signatures, fixture.transcript_signatures);
    let report = dkg_status_report(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        &keyset_shares,
        &transcript_signatures,
    );
    assert!(report.ready);
    assert!(report.text.contains("keyset_share_records=1"));
    assert!(report.text.contains("transcript_signature_records=3"));
}

#[test]
fn dkg_status_report_shows_partial_database_setup_progress_without_marking_ready() {
    let fixture = dkg_completion_fixture(member_id(2));
    let setup_summary = DkgSetupRecordSummary {
        commitments: 2,
        reveals: 1,
        secret_share_contributions: 1,
        commitment_members: BTreeSet::from([member_id(1), member_id(2)]),
        reveal_members: BTreeSet::from([member_id(1)]),
        secret_share_senders: BTreeSet::from([member_id(1)]),
        secret_share_receivers: BTreeSet::from([member_id(2)]),
    };

    let report = dkg_status_report_with_setup_records(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        &[],
        &[],
        &setup_summary,
    );

    assert!(!report.ready);
    assert!(report
        .text
        .contains("setup_records commitments=2 reveals=1"));
    assert!(report.text.contains("secret_share_contributions=1"));
    assert!(report.text.contains("commitment_members=1,2"));
    assert!(report.text.contains("reveal_members=1"));
    assert!(report.text.contains("secret_share_senders=1"));
    assert!(report.text.contains("secret_share_receivers=2"));
    assert!(report.text.contains("finalized_ready=false"));
    assert!(report.text.contains("missing DKG keyset shares"));
    for private_fragment in [
        "secret_shares",
        "BlsSecretKeyShare",
        "coefficient_commitments",
        "commitment_hash",
    ] {
        assert!(!report.text.contains(private_fragment));
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn read_database_records_loads_persisted_dkg_completion_records() {
    let fixture = dkg_completion_fixture(member_id(2));
    let reveal = dkg_reveal_for_status(&fixture.setup_public_config, member_id(1), 41);
    let commitment = FederationDkgParticipantCommitment::from_reveal(&reveal).expect("commitment");
    let secret_share_contribution = dkg_secret_share_request_for_status(
        &fixture.setup_public_config,
        member_id(1),
        member_id(2),
        51,
    );
    let work_dir =
        std::env::temp_dir().join(format!("cdk_mintd_dkg_status_db_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work_dir);
    std::fs::create_dir_all(&work_dir).expect("create work dir");

    let settings = crate::config::Settings::default();
    let (db, _, _) = crate::setup_database(&settings, &work_dir, test_db_password())
        .await
        .expect("open test database");
    let mut tx = db.begin_transaction().await.expect("begin transaction");
    tx.add_federation_dkg_commitment(&commitment)
        .await
        .expect("persist setup commitment");
    tx.add_federation_dkg_reveal(&reveal)
        .await
        .expect("persist setup reveal");
    tx.add_federation_dkg_secret_share_contribution(&secret_share_contribution)
        .await
        .expect("persist setup secret-share contribution");
    tx.add_federation_dkg_member_keyset_share(&fixture.keyset_share)
        .await
        .expect("persist keyset share");
    for signature in &fixture.transcript_signatures {
        tx.add_federation_dkg_transcript_signature(signature)
            .await
            .expect("persist transcript signature");
    }
    tx.commit().await.expect("commit DKG records");
    drop(db);

    let records = read_database_records(&test_cli_args(work_dir.clone()))
        .await
        .expect("read DKG records from database");

    assert_eq!(records.commitments, vec![commitment]);
    assert_eq!(records.reveals, vec![reveal]);
    assert_eq!(
        records.secret_share_contributions,
        vec![secret_share_contribution]
    );
    assert_eq!(records.keyset_shares, vec![fixture.keyset_share]);
    assert_eq!(records.transcript_signatures, fixture.transcript_signatures);
    let setup_summary = DkgSetupRecordSummary::from_records(&records);
    let report = dkg_status_report_with_setup_records(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        &records.keyset_shares,
        &records.transcript_signatures,
        &setup_summary,
    );
    assert!(report
        .text
        .contains("setup_records commitments=1 reveals=1"));
    assert!(report.text.contains("secret_share_contributions=1"));
    let _ = std::fs::remove_dir_all(&work_dir);
}

use std::collections::BTreeMap;
use std::str::FromStr;

use cdk_common::federation::{
    generate_trusted_dealer_configs, FederationDkgCeremonyId, FederationDkgCommitmentRound,
    FederationDkgKeysetSpec, FederationDkgTranscript, FederationDkgTranscriptSignature,
    FederationFrostDkgContext, FederationFrostRootRecord, MemberId, ThresholdParams,
    TrustedDealerKeysetSpec, TrustedDealerMember,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::nut01::{evaluate_secret_key_coefficients, BlsSecretKey};
use cdk_common::{Amount, CurrencyUnit, SecretKey};
use cdk_frost::{advance_dkg, finish_dkg, start_dkg, SecretEncryptionKey};

use super::*;

#[test]
fn dkg_driver_phase_labels_cover_bls_and_frost_ceremonies() {
    let labels = [
        (FederationDkgDriverPhase::Commitment, "commitment"),
        (FederationDkgDriverPhase::Reveal, "reveal"),
        (FederationDkgDriverPhase::SecretShares, "share_delivery"),
        (
            FederationDkgDriverPhase::LocalKeysetShare,
            "local_keyset_share",
        ),
        (
            FederationDkgDriverPhase::TranscriptSignature,
            "transcript_signature",
        ),
        (FederationDkgDriverPhase::Activation, "activation"),
        (FederationDkgDriverPhase::FrostRound1, "frost_round_1"),
        (FederationDkgDriverPhase::FrostRound2, "frost_round_2"),
        (
            FederationDkgDriverPhase::FrostConfirmation,
            "frost_confirmation",
        ),
        (FederationDkgDriverPhase::Complete, "complete"),
    ];
    for (phase, expected) in labels {
        assert_eq!(dkg_driver_phase_label(phase), expected);
    }
}

struct DkgCompletionFixture {
    setup_public_config: FederationConfig,
    setup_private_config: MemberConfigPrivate,
    keyset_share: FederationDkgMemberKeysetShare,
    transcript_signatures: Vec<FederationDkgTranscriptSignatureRequest>,
    activations: Vec<FederationDkgActivationRequest>,
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

fn dkg_participant_state_for_status(
    public_config: &FederationConfig,
    participant: MemberId,
) -> FederationDkgParticipantState {
    FederationDkgParticipantState::generate(
        public_config,
        dkg_ceremony_id(),
        participant,
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

    let mut setup_public_config = bundle.public_config.clone();
    setup_public_config.keysets.clear();
    setup_public_config.setup_transcript = None;
    setup_public_config.validate().expect("setup public config");
    let reveals = BTreeMap::from([
        (
            member_id(1),
            dkg_reveal_for_status(&setup_public_config, member_id(1), 11),
        ),
        (
            member_id(2),
            dkg_reveal_for_status(&setup_public_config, member_id(2), 21),
        ),
        (
            member_id(3),
            dkg_reveal_for_status(&setup_public_config, member_id(3), 31),
        ),
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
    let round = FederationDkgCommitmentRound::new(&setup_public_config, commitments)
        .expect("commitment round");
    let contributions = [(member_id(1), 11), (member_id(2), 21), (member_id(3), 31)]
        .into_iter()
        .map(|(sender, seed)| {
            (
                sender,
                dkg_secret_share_request_for_status(
                    &setup_public_config,
                    sender,
                    local_member,
                    seed,
                ),
            )
        })
        .collect();
    let keyset_share = round
        .derive_member_keyset_share(&setup_public_config, &reveals, local_member, &contributions)
        .expect("local member keyset share");
    let mut final_public_config = FederationConfig::new(
        setup_public_config.threshold_params,
        setup_public_config.members.clone(),
        vec![keyset_share.keyset.clone()],
    )
    .expect("final public config");
    final_public_config.payment_observation_policy =
        setup_public_config.payment_observation_policy.clone();
    let transcript_hash = FederationDkgTranscript::hash_for_config(
        FederationDkgSetupMethod::PedersenDkg,
        &final_public_config,
        std::slice::from_ref(&keyset_share.public_ceremony),
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

    let local_private_config = bundle
        .private_configs
        .get(&local_member)
        .expect("local private config");

    let mut setup_private_config = local_private_config.clone();
    setup_private_config.keyset_secret_shares.clear();
    setup_private_config.setup_transcript_hash = None;
    setup_private_config
        .validate_against_public(&setup_public_config)
        .expect("setup private config");
    let finalized = FederationDkgFinalizedMemberConfig::from_records(
        &setup_public_config,
        &setup_private_config,
        std::slice::from_ref(&keyset_share),
        &transcript_signatures,
    )
    .expect("finalized member config");
    let activations = bundle
        .private_configs
        .values()
        .map(|private_config| {
            FederationDkgActivationRequest::sign(
                dkg_ceremony_id(),
                &finalized.public_config,
                private_config,
            )
            .expect("activation confirmation")
        })
        .collect();

    DkgCompletionFixture {
        setup_public_config,
        setup_private_config,
        keyset_share,
        transcript_signatures,
        activations,
    }
}

fn completed_frost_root(config: &FederationConfig) -> FederationFrostRootPublicConfig {
    let context =
        FederationFrostDkgContext::new(config, dkg_ceremony_id(), 1).expect("FROST DKG context");
    let frost_context = context.to_frost_context().expect("crypto DKG context");
    let mut round1 = Vec::new();
    for id in 1..=3 {
        round1.push(
            start_dkg(
                frost_context.clone(),
                cdk_frost::MemberId::new(id).expect("FROST member"),
                cdk_frost::rand_core::OsRng,
            )
            .expect("start FROST DKG"),
        );
    }
    let public_round1 = round1
        .iter()
        .map(|(_, package)| package.clone())
        .collect::<Vec<_>>();
    let mut round2 = round1
        .into_iter()
        .map(|(secret, own)| {
            let received = public_round1
                .iter()
                .filter(|package| package.sender() != own.sender())
                .cloned();
            advance_dkg(secret, received).expect("advance FROST DKG")
        })
        .collect::<Vec<_>>();
    let local = cdk_frost::MemberId::new(1).expect("local FROST member");
    let incoming = round2
        .iter()
        .skip(1)
        .map(|(_, outgoing)| outgoing.get(&local).expect("round-two package").clone())
        .collect::<Vec<_>>();
    let root = finish_dkg(
        round2.remove(0).0,
        public_round1
            .iter()
            .filter(|package| package.sender() != local)
            .cloned(),
        incoming,
    )
    .expect("finish FROST DKG");
    let storage_key = SecretEncryptionKey::new([91; 32]).expect("storage key");
    let mut rng = cdk_frost::rand_core::OsRng;
    let encrypted = root.seal(&storage_key, &mut rng).expect("seal root share");
    let record = FederationFrostRootRecord {
        context,
        member_id: member_id(1),
        public_package: root.public().encode(),
        encrypted_root_share: encrypted.to_bytes(),
    };
    FederationFrostRootPublicConfig::from_dkg_record(config, &record).expect("public FROST root")
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

#[test]
fn dkg_status_report_marks_finalized_records_ready_without_leaking_private_shares() {
    let fixture = dkg_completion_fixture(member_id(2));

    let report = dkg_status_report_with_activations(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        std::slice::from_ref(&fixture.keyset_share),
        &fixture.transcript_signatures,
        &fixture.activations,
    );

    assert!(report.ready);
    assert!(report.text.contains("DKG status for member 2"));
    assert!(report.text.contains("setup_keysets=0"));
    assert!(report.text.contains("keyset_share_records=1"));
    assert!(report.text.contains("transcript_signature_records=3"));
    assert!(report.text.contains("transcript_signers=1,2,3"));
    assert!(report.text.contains("finalized_ready=true"));
    assert!(report.text.contains("final_keysets=1"));
    assert!(report.text.contains("local_keysets=1 local_amounts=1"));
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
fn dkg_status_report_marks_missing_activation_not_ready() {
    let mut fixture = dkg_completion_fixture(member_id(2));
    fixture
        .activations
        .retain(|activation| activation.signer != member_id(3));

    let report = dkg_status_report_with_activations(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        std::slice::from_ref(&fixture.keyset_share),
        &fixture.transcript_signatures,
        &fixture.activations,
    );

    assert!(!report.ready);
    assert!(report.text.contains("activation_records=2"));
    assert!(report.text.contains("finalized_ready=false"));
    assert!(report.text.contains("missing DKG activation for member 3"));
}

#[test]
fn dkg_status_report_summarizes_persisted_setup_records_without_secrets() {
    let fixture = dkg_completion_fixture(member_id(2));
    let setup_summary = DkgSetupRecordSummary {
        ceremony_lifecycle: Vec::new(),
        participant_states: 1,
        commitments: 3,
        reveals: 2,
        secret_share_contributions: 4,
        keyset_shares: 1,
        transcript_signatures: 3,
        activations: 3,
        frost_root_public_configs: 1,
        participant_state_members: BTreeSet::from([member_id(2)]),
        participant_state_ceremonies: BTreeSet::from([dkg_ceremony_id().to_string()]),
        commitment_members: BTreeSet::from([member_id(1), member_id(2), member_id(3)]),
        reveal_members: BTreeSet::from([member_id(1), member_id(2)]),
        secret_share_senders: BTreeSet::from([member_id(1), member_id(2), member_id(3)]),
        secret_share_receivers: BTreeSet::from([member_id(2)]),
        driver_phase: Some(FederationDkgDriverPhase::SecretShares),
        driver_error: None,
    };

    let report = dkg_status_report_with_setup_records(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        std::slice::from_ref(&fixture.keyset_share),
        &fixture.transcript_signatures,
        &fixture.activations,
        &setup_summary,
    );

    assert!(report.ready);
    assert!(report
        .text
        .contains("setup_records participant_states=1 commitments=3 reveals=2"));
    assert!(report
        .text
        .contains("secret_share_contributions=4 keyset_shares=1 transcript_signatures=3"));
    assert!(report.text.contains("frost_root_public_configs=1"));
    assert!(report.text.contains("participant_state_members=2"));
    assert!(report.text.contains(&format!(
        "participant_state_ceremonies={}",
        dkg_ceremony_id()
    )));
    assert!(report.text.contains("commitment_members=1,2,3"));
    assert!(report.text.contains("reveal_members=1,2"));
    assert!(report.text.contains("secret_share_senders=1,2,3"));
    assert!(report.text.contains("secret_share_receivers=2"));
    assert!(report.text.contains("driver_phase=share_delivery"));
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
    let mut activations = fixture.activations[..1].to_vec();

    merge_dkg_records(
        &mut keyset_shares,
        &mut transcript_signatures,
        &mut activations,
        DkgCompletionRecords {
            keyset_shares: vec![fixture.keyset_share.clone()],
            transcript_signatures: fixture.transcript_signatures[1..].to_vec(),
            activations: fixture.activations[1..].to_vec(),
            ..DkgCompletionRecords::default()
        },
    );

    assert_eq!(keyset_shares, vec![fixture.keyset_share.clone()]);
    assert_eq!(transcript_signatures, fixture.transcript_signatures);
    assert_eq!(activations, fixture.activations);
    let report = dkg_status_report_with_activations(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        &keyset_shares,
        &transcript_signatures,
        &activations,
    );
    assert!(report.ready);
    assert!(report.text.contains("finalized_ready=true"));
}

#[test]
fn merge_dkg_records_deduplicates_database_records_already_supplied_from_files() {
    let fixture = dkg_completion_fixture(member_id(2));
    let mut keyset_shares = vec![fixture.keyset_share.clone()];
    let mut transcript_signatures = fixture.transcript_signatures.clone();
    let mut activations = fixture.activations.clone();

    merge_dkg_records(
        &mut keyset_shares,
        &mut transcript_signatures,
        &mut activations,
        DkgCompletionRecords {
            keyset_shares: vec![fixture.keyset_share.clone()],
            transcript_signatures: fixture.transcript_signatures.clone(),
            activations: fixture.activations.clone(),
            ..DkgCompletionRecords::default()
        },
    );

    assert_eq!(keyset_shares, vec![fixture.keyset_share.clone()]);
    assert_eq!(transcript_signatures, fixture.transcript_signatures);
    assert_eq!(activations, fixture.activations);
    let report = dkg_status_report_with_activations(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        &keyset_shares,
        &transcript_signatures,
        &activations,
    );
    assert!(report.ready);
    assert!(report.text.contains("keyset_share_records=1"));
    assert!(report.text.contains("transcript_signature_records=3"));
}

#[test]
fn finalized_dkg_configs_are_exported_with_overwrite_protection() {
    let fixture = dkg_completion_fixture(member_id(2));
    let finalized = FederationDkgFinalizedMemberConfig::from_records(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        std::slice::from_ref(&fixture.keyset_share),
        &fixture.transcript_signatures,
    )
    .expect("finalized member config");
    let output_dir =
        std::env::temp_dir().join(format!("cdk_mintd_dkg_final_config_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&output_dir);

    let (public_path, private_path) =
        write_finalized_member_config(&output_dir, &finalized, false).expect("write final configs");
    let public_config: FederationConfig =
        crate::read_federation_json(&public_path, "exported public config")
            .expect("read public config");
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(&private_path, "exported private config")
            .expect("read private config");
    assert_eq!(public_config, finalized.public_config);
    assert_eq!(private_config, finalized.private_config);
    private_config
        .validate_against_public(&public_config)
        .expect("exported configs match");
    assert!(write_finalized_member_config(&output_dir, &finalized, false).is_err());
    write_finalized_member_config(&output_dir, &finalized, true)
        .expect("explicit overwrite succeeds");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = std::fs::metadata(&private_path)
            .expect("private config metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn completed_dual_dkg_bootstraps_one_cdk_onchain_application() {
    let fixture = dkg_completion_fixture(member_id(2));
    let mut finalized = FederationDkgFinalizedMemberConfig::from_records(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        std::slice::from_ref(&fixture.keyset_share),
        &fixture.transcript_signatures,
    )
    .expect("finalized member config");
    let root = completed_frost_root(&finalized.public_config);
    let root_package_before = root.canonical_public_package.clone();

    attach_initial_frost_cdk_wallet(
        &mut finalized,
        std::slice::from_ref(&root),
        FrostCdkBootstrap {
            network: bitcoin::Network::Regtest,
            minimum_confirmations: 1,
            fee_policy_version: 1,
            application_key_epoch: 1,
        },
    )
    .expect("bootstrap initial FROST CDK application");

    let wallet = finalized
        .public_config
        .frost_wallet
        .as_ref()
        .expect("FROST wallet");
    let application = &wallet.cdk_onchain_application;
    assert_eq!(application.network, bitcoin::Network::Regtest);
    assert_eq!(application.confirmation_policy.minimum_confirmations, 1);
    assert_eq!(application.fee_policy_version, 1);
    assert_eq!(application.threshold, 2);
    assert_eq!(wallet.root.canonical_public_package, root_package_before);
    finalized
        .private_config
        .validate_against_public(&finalized.public_config)
        .expect("bootstrapped public config matches private BLS config");

    let err = attach_initial_frost_cdk_wallet(
        &mut finalized,
        std::slice::from_ref(&root),
        FrostCdkBootstrap {
            network: bitcoin::Network::Regtest,
            minimum_confirmations: 1,
            fee_policy_version: 1,
            application_key_epoch: 1,
        },
    )
    .expect_err("bootstrap cannot overwrite an active wallet");
    assert!(err.to_string().contains("existing wallet"));
}

#[test]
fn completed_dual_dkg_registers_bark_under_the_same_sealed_root() {
    let fixture = dkg_completion_fixture(member_id(2));
    let mut finalized = FederationDkgFinalizedMemberConfig::from_records(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        std::slice::from_ref(&fixture.keyset_share),
        &fixture.transcript_signatures,
    )
    .expect("finalized member config");
    let root = completed_frost_root(&finalized.public_config);
    attach_initial_frost_cdk_wallet(
        &mut finalized,
        std::slice::from_ref(&root),
        FrostCdkBootstrap {
            network: bitcoin::Network::Regtest,
            minimum_confirmations: 1,
            fee_policy_version: 1,
            application_key_epoch: 1,
        },
    )
    .expect("bootstrap recovery wallet");
    let server_secret =
        bitcoin::secp256k1::SecretKey::from_slice(&[73; 32]).expect("Ark server secret");
    let server = server_secret.public_key(&bitcoin::secp256k1::Secp256k1::new());
    let policy = FederationBarkPolicyConfig::new(
        0,
        0,
        10,
        5_000,
        18,
        18,
        144,
        18,
        30,
        cdk_common::federation::FederationBarkReceiveAntiDosPolicy::ProtectedToken,
    )
    .expect("Bark policy");
    let mut wrong_network = finalized.clone();
    let wrong_network_error = attach_initial_frost_bark_application(
        &mut wrong_network,
        FrostBarkBootstrap {
            network: bitcoin::Network::Signet,
            ark_server_public_key: server,
            application_key_epoch: 1,
            policy: policy.clone(),
        },
    )
    .expect_err("Bark recovery network mismatch fails closed");
    assert!(wrong_network_error
        .to_string()
        .contains("networks must match"));
    assert!(wrong_network
        .public_config
        .frost_wallet
        .as_ref()
        .expect("wallet")
        .bark_application
        .is_none());
    attach_initial_frost_bark_application(
        &mut finalized,
        FrostBarkBootstrap {
            network: bitcoin::Network::Regtest,
            ark_server_public_key: server,
            application_key_epoch: 1,
            policy: policy.clone(),
        },
    )
    .expect("register Bark application");

    let wallet = finalized
        .public_config
        .frost_wallet
        .as_ref()
        .expect("FROST wallet");
    let bark = wallet.bark_application.as_ref().expect("Bark application");
    assert_eq!(wallet.version, 2);
    assert_eq!(bark.network, bitcoin::Network::Regtest);
    assert_eq!(bark.ark_server_public_key, server.serialize());
    assert_eq!(bark.policy, policy);
    assert_eq!(bark.root_key_epoch, wallet.root.root_key_epoch);
    assert_ne!(
        bark.derived_group_key,
        wallet.cdk_onchain_application.derived_group_key
    );
    finalized
        .private_config
        .validate_against_public(&finalized.public_config)
        .expect("private member config remains bound to public snapshot");

    let error = attach_initial_frost_bark_application(
        &mut finalized,
        FrostBarkBootstrap {
            network: bitcoin::Network::Regtest,
            ark_server_public_key: server,
            application_key_epoch: 1,
            policy,
        },
    )
    .expect_err("existing or wrong-network Bark registration fails closed");
    assert!(error.to_string().contains("existing Bark"));
}

#[test]
fn frost_cdk_bootstrap_fails_closed_without_exactly_one_dual_dkg_root() {
    let fixture = dkg_completion_fixture(member_id(2));
    let mut finalized = FederationDkgFinalizedMemberConfig::from_records(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        std::slice::from_ref(&fixture.keyset_share),
        &fixture.transcript_signatures,
    )
    .expect("finalized member config");
    let err = attach_initial_frost_cdk_wallet(
        &mut finalized,
        &[],
        FrostCdkBootstrap {
            network: bitcoin::Network::Regtest,
            minimum_confirmations: 1,
            fee_policy_version: 1,
            application_key_epoch: 1,
        },
    )
    .expect_err("missing FROST root fails closed");
    assert!(err.to_string().contains("exactly one root"));
    assert!(finalized.public_config.frost_wallet.is_none());
}

#[test]
fn finalized_dkg_config_export_requires_complete_records() {
    let fixture = dkg_completion_fixture(member_id(2));
    let err = finalized_member_config(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        &[],
        &[],
        &[],
    )
    .expect_err("incomplete DKG records fail");

    assert!(err.to_string().contains("missing DKG keyset shares"));
}

#[test]
fn dkg_status_report_shows_partial_database_setup_progress_without_marking_ready() {
    let fixture = dkg_completion_fixture(member_id(2));
    let setup_summary = DkgSetupRecordSummary {
        ceremony_lifecycle: Vec::new(),
        participant_states: 1,
        commitments: 2,
        reveals: 1,
        secret_share_contributions: 1,
        keyset_shares: 0,
        transcript_signatures: 0,
        activations: 0,
        frost_root_public_configs: 0,
        participant_state_members: BTreeSet::from([member_id(2)]),
        participant_state_ceremonies: BTreeSet::from([dkg_ceremony_id().to_string()]),
        commitment_members: BTreeSet::from([member_id(1), member_id(2)]),
        reveal_members: BTreeSet::from([member_id(1)]),
        secret_share_senders: BTreeSet::from([member_id(1)]),
        secret_share_receivers: BTreeSet::from([member_id(2)]),
        driver_phase: Some(FederationDkgDriverPhase::Reveal),
        driver_error: None,
    };

    let report = dkg_status_report_with_setup_records(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        &[],
        &[],
        &[],
        &setup_summary,
    );

    assert!(!report.ready);
    assert!(report
        .text
        .contains("setup_records participant_states=1 commitments=2 reveals=1"));
    assert!(report.text.contains("participant_state_members=2"));
    assert!(report.text.contains(&format!(
        "participant_state_ceremonies={}",
        dkg_ceremony_id()
    )));
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

#[test]
fn dkg_progress_report_uses_record_counts_without_finalization() {
    let fixture = dkg_completion_fixture(member_id(2));
    let records =
        DkgCompletionRecords {
            participant_states: vec![dkg_participant_state_for_status(
                &fixture.setup_public_config,
                member_id(2),
            )],
            commitments: vec![FederationDkgParticipantCommitment::from_reveal(
                &dkg_reveal_for_status(&fixture.setup_public_config, member_id(1), 41),
            )
            .expect("commitment")],
            keyset_shares: vec![fixture.keyset_share],
            transcript_signatures: fixture.transcript_signatures,
            activations: fixture.activations,
            ..DkgCompletionRecords::default()
        };
    let summary = DkgSetupRecordSummary::from_record_counts(&records);
    let report = dkg_progress_report(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        &summary,
    );

    assert!(report.contains("DKG progress for member 2"));
    assert!(report.contains("participant_states=1 commitments=1"));
    assert!(report.contains("keyset_shares=1 transcript_signatures=3"));
    assert!(report.contains("driver_phase=unchecked"));
    assert!(!report.contains("finalized_ready"));
    assert!(!report.contains("secret_shares"));
}

#[test]
fn dkg_progress_report_exposes_aborted_and_replacement_lifecycle() {
    let fixture = dkg_completion_fixture(member_id(2));
    let old_id = dkg_ceremony_id();
    let replacement_id = FederationDkgCeremonyId::from_bytes([43; 32]);
    let keyset_spec = fixture
        .keyset_share
        .public_ceremony
        .reveals
        .values()
        .next()
        .expect("ceremony reveal")
        .keyset_spec
        .clone();
    let old = FederationDkgCeremonyRecord::active(
        fixture.setup_public_config.federation_id,
        old_id,
        member_id(2),
        keyset_spec.clone(),
        None,
    );
    let aborted = old.aborted_by(replacement_id).expect("aborted ceremony");
    let replacement = FederationDkgCeremonyRecord::active(
        fixture.setup_public_config.federation_id,
        replacement_id,
        member_id(2),
        keyset_spec,
        Some(old_id),
    );
    let records = DkgCompletionRecords {
        ceremonies: vec![aborted, replacement],
        ..DkgCompletionRecords::default()
    };
    let summary = DkgSetupRecordSummary::from_record_counts(&records);
    let report = dkg_progress_report(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        &summary,
    );

    assert!(report.contains(&format!("{old_id}:aborted:replacement={replacement_id}")));
    assert!(report.contains(&format!("{replacement_id}:active:replacement=none")));
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
    let participant_state =
        dkg_participant_state_for_status(&fixture.setup_public_config, member_id(2));
    let ceremony = FederationDkgCeremonyRecord::active(
        fixture.setup_public_config.federation_id,
        dkg_ceremony_id(),
        member_id(2),
        reveal.keyset_spec.clone(),
        None,
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
    tx.add_federation_dkg_ceremony(&ceremony)
        .await
        .expect("persist ceremony lifecycle");
    tx.add_federation_dkg_participant_state(&participant_state)
        .await
        .expect("persist local participant state");
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
    for activation in &fixture.activations {
        tx.add_federation_dkg_activation(activation)
            .await
            .expect("persist activation");
    }
    tx.commit().await.expect("commit DKG records");
    drop(db);

    let records = read_database_records(&test_cli_args(work_dir.clone()))
        .await
        .expect("read DKG records from database");

    assert_eq!(records.participant_states, vec![participant_state]);
    assert_eq!(records.ceremonies, vec![ceremony]);
    assert_eq!(records.commitments, vec![commitment]);
    assert_eq!(records.reveals, vec![reveal]);
    assert_eq!(
        records.secret_share_contributions,
        vec![secret_share_contribution]
    );
    assert_eq!(records.keyset_shares, vec![fixture.keyset_share]);
    assert_eq!(records.transcript_signatures, fixture.transcript_signatures);
    assert_eq!(records.activations, fixture.activations);
    let setup_summary = DkgSetupRecordSummary::from_records(
        &records,
        &fixture.setup_public_config,
        &fixture.setup_private_config,
    );
    let report = dkg_status_report_with_setup_records(
        &fixture.setup_public_config,
        &fixture.setup_private_config,
        &records.keyset_shares,
        &records.transcript_signatures,
        &records.activations,
        &setup_summary,
    );
    assert!(report
        .text
        .contains("setup_records participant_states=1 commitments=1 reveals=1"));
    assert!(report.text.contains("secret_share_contributions=1"));
    let _ = std::fs::remove_dir_all(&work_dir);
}

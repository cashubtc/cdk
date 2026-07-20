use std::collections::BTreeMap;
use std::str::FromStr;

use bark::custody::{
    CustodySessionId, KeySourceDescriptor, SchnorrSigningRequest, VtxoCustody, VtxoKeyLocator,
    VtxoSigningOperation,
};
use bark::{Config as BarkConfig, WalletProperties, WalletSeed};
use bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
use bitcoin::Network;
use cashu::MintUrl;
use cdk_common::federation::FederationApplicationDigest;
use cdk_common::federation::{
    ark_server_commitment, payment_hash_for_preimage, preimage_commitment, FederationBarkActionId,
    FederationBarkFrostApplicationConfig, FederationBarkPolicyConfig,
    FederationBarkPrepareReceiveSecretRequest, FederationBarkReceiveAntiDosPolicy,
    FederationBarkReceiveIntent, FederationBarkReceiveProof, FederationBarkReceiveSecretBinding,
    FederationBarkUnit, FederationBarkVtxoLocator, FederationCheckpointKind,
    FederationCheckpointQuorum, FederationCheckpointSignature, FederationConfig,
    FederationDkgCeremonyId, FederationFinalizedItemDigest, FederationFrostApplicationConfig,
    FederationFrostDkgContext, FederationFrostRootPublicConfig, FederationFrostRootRecord,
    FederationFrostWalletConfig, FederationId, FederationJournalDigest,
    FederationMaterializedStateDigest, FederationOnchainConfirmationPolicy, FederationOperationId,
    MemberConfigPrivate, MemberConfigPublic, MemberId, ThresholdParams,
    VerifiedFederationCheckpointQuorum, FEDERATION_CHECKPOINT_VERSION,
};
use cdk_common::SecretKey as CdkSecretKey;
use cdk_common::{Amount, CurrencyUnit};
use cdk_frost::rand_core::SeedableRng;
use cdk_frost::{advance_dkg, finish_dkg, start_dkg, SecretEncryptionKey};
use rand_chacha::ChaCha20Rng;

use crate::runtime::validate_wallet_properties;
use crate::{
    CdkFrostVtxoCustody, Error, FederatedBarkLimits, FederatedBarkLocalBindings,
    FederatedBarkLocalConfig, FederatedBarkReceiveSecretAcceptedRef,
    FederatedBarkReceiveSecretEncryptionKey, FederatedBarkReceiveSecretStore,
    FederatedBarkReceiveSecretStoreConfig, FederatedBarkStoreAvailability,
    ValidatedFederatedBarkConfig, FEDERATED_BARK_RECEIVE_SECRET_ORPHAN_RETENTION_SECONDS,
};

fn member(id: u16) -> MemberId {
    MemberId::new(id).expect("member")
}

fn public_key(seed: u8) -> PublicKey {
    SecretKey::from_slice(&[seed; 32])
        .expect("secret")
        .public_key(&Secp256k1::new())
}

fn base_config() -> FederationConfig {
    let members = (1..=3)
        .map(|id| {
            let secret = CdkSecretKey::from_slice(&[id as u8; 32]).expect("identity secret");
            (
                member(id),
                MemberConfigPublic {
                    member_id: member(id),
                    public_mint_url: MintUrl::from_str(&format!("https://mint{id}.example.com"))
                        .expect("mint URL"),
                    federation_api_url: MintUrl::from_str(&format!(
                        "https://federation{id}.example.com"
                    ))
                    .expect("federation URL"),
                    identity_public_key: secret.public_key(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    FederationConfig::new(
        ThresholdParams::new(3, 2, 2).expect("threshold"),
        members,
        Vec::new(),
    )
    .expect("config")
}

fn root_record(config: &FederationConfig) -> FederationFrostRootRecord {
    let context =
        FederationFrostDkgContext::new(config, FederationDkgCeremonyId::from_bytes([41; 32]), 1)
            .expect("context");
    let frost_context = context.to_frost_context().expect("FROST context");
    let mut rng = ChaCha20Rng::from_seed([42; 32]);
    let mut round1_secrets = BTreeMap::new();
    let mut round1_packages = BTreeMap::new();
    for id in 1..=3 {
        let frost_member = cdk_frost::MemberId::new(id).expect("FROST member");
        let (secret, package) =
            start_dkg(frost_context.clone(), frost_member, &mut rng).expect("round one");
        round1_secrets.insert(id, secret);
        round1_packages.insert(id, package);
    }
    let mut round2_secrets = BTreeMap::new();
    let mut round2_packages = BTreeMap::new();
    for id in 1..=3 {
        let received = round1_packages
            .iter()
            .filter(|(peer, _)| **peer != id)
            .map(|(_, package)| package.clone());
        let (secret, outgoing) = advance_dkg(
            round1_secrets.remove(&id).expect("round-one secret"),
            received,
        )
        .expect("round two");
        round2_secrets.insert(id, secret);
        round2_packages.insert(id, outgoing);
    }
    let id = 1;
    let frost_member = cdk_frost::MemberId::new(id).expect("FROST member");
    let received_round1 = round1_packages
        .iter()
        .filter(|(peer, _)| **peer != id)
        .map(|(_, package)| package.clone());
    let received_round2 =
        round2_packages
            .iter()
            .filter(|(peer, _)| **peer != id)
            .map(|(_, outgoing)| {
                outgoing
                    .get(&frost_member)
                    .expect("round-two contribution")
                    .clone()
            });
    let root = finish_dkg(
        round2_secrets.remove(&id).expect("round-two secret"),
        received_round1,
        received_round2,
    )
    .expect("finished root");
    let encryption = SecretEncryptionKey::new([91; 32]).expect("encryption key");
    let encrypted = root.seal(&encryption, &mut rng).expect("sealed root");
    FederationFrostRootRecord {
        context,
        member_id: member(id),
        public_package: root.public().encode(),
        encrypted_root_share: encrypted.to_bytes(),
    }
}

fn policy() -> FederationBarkPolicyConfig {
    FederationBarkPolicyConfig::new(
        0,
        0,
        10,
        5_000,
        18,
        18,
        144,
        18,
        30,
        FederationBarkReceiveAntiDosPolicy::NoProof,
    )
    .expect("policy")
}

fn federated_config() -> FederationConfig {
    let mut config = base_config();
    let root = FederationFrostRootPublicConfig::from_dkg_record(&config, &root_record(&config))
        .expect("public root");
    let onchain = FederationFrostApplicationConfig::new_cdk_onchain(
        &config,
        &root,
        1,
        Network::Regtest,
        FederationOnchainConfirmationPolicy::new(2).expect("confirmations"),
        1,
    )
    .expect("onchain application");
    let bark = FederationBarkFrostApplicationConfig::new(
        &config,
        &root,
        1,
        Network::Regtest,
        public_key(72),
        policy(),
    )
    .expect("Bark application");
    config.frost_wallet = Some(
        FederationFrostWalletConfig::new_with_bark(&config, root, onchain, bark)
            .expect("wallet config"),
    );
    config.validate().expect("federated config");
    config
}

fn local_config(config: &FederationConfig) -> FederatedBarkLocalConfig {
    let application = config
        .frost_wallet
        .as_ref()
        .expect("wallet")
        .bark_application
        .as_ref()
        .expect("Bark application");
    let mut bark = BarkConfig::network_default(Network::Regtest);
    bark.server_address = "https://operator:credential@ark.example.invalid".to_string();
    bark.esplora_address = Some("https://chain-secret.example.invalid".to_string());
    bark.vtxo_refresh_expiry_threshold = application.policy.refresh_before_expiry_blocks;
    bark.vtxo_exit_margin = u16::try_from(application.policy.emergency_exit_before_expiry_blocks)
        .expect("test exit margin");
    bark.htlc_recv_claim_delta =
        u16::try_from(application.policy.minimum_incoming_cltv_delta).expect("test CLTV");
    FederatedBarkLocalConfig {
        enabled: true,
        bindings: FederatedBarkLocalBindings {
            federation_id: config.federation_id,
            network: application.network,
            ark_server_public_key: PublicKey::from_slice(&application.ark_server_public_key)
                .expect("server key"),
            root_key_epoch: application.root_key_epoch,
            application_id: application.application_id,
            application_key_epoch: application.application_key_epoch,
            threshold: application.threshold,
            members: config.members.keys().copied().collect(),
            policy: application.policy.clone(),
            unit: CurrencyUnit::Sat,
        },
        bark,
        data_directory: "/private/operator/secret-wallet-path".into(),
        operational_seed: WalletSeed::new_from_seed(Network::Regtest, &[83; 64]),
        create_if_missing: true,
        stores: FederatedBarkStoreAvailability {
            frost_protocol_store: true,
            receive_secret_store: true,
        },
        receive_secret_store: FederatedBarkReceiveSecretStoreConfig {
            database_path: "/private/operator/receive-secrets.redb".into(),
            encryption_key: std::sync::Arc::new(
                FederatedBarkReceiveSecretEncryptionKey::new([85; 32]).expect("receive-secret key"),
            ),
        },
        limits: FederatedBarkLimits::new(1, 1_000_000).expect("limits"),
    }
}

fn validates(config: &FederationConfig, local: &FederatedBarkLocalConfig) -> Result<(), Error> {
    ValidatedFederatedBarkConfig::validate(true, config, local).map(|_| ())
}

#[tokio::test]
async fn accepted_scope_derives_bark_keys_and_custody_stays_fail_closed() {
    let config = federated_config();
    let local = local_config(&config);
    let validated =
        ValidatedFederatedBarkConfig::validate(true, &config, &local).expect("validated");
    let custody = CdkFrostVtxoCustody::new(validated.clone());
    validated
        .validate_descriptor(&custody.descriptor())
        .expect("descriptor");
    let derived = custody
        .derive_public_key(VtxoKeyLocator { index: 19 })
        .expect("derived");
    let expected = validated
        .public_adapter()
        .derive(cdk_frost_bark::BarkKeyLocator::vtxo(19))
        .expect("expected");
    assert_eq!(derived.public_key, expected.public_key());
    assert!(custody
        .sign_schnorr(SchnorrSigningRequest {
            session_id: CustodySessionId::from_bytes([7; 32]).expect("session"),
            locator: VtxoKeyLocator { index: 19 },
            message: [8; 32],
            operation: VtxoSigningOperation::LightningSend,
            authorization: vec![9; 32],
        })
        .await
        .is_err());
    let debug = format!("{local:?} {custody:?} {validated:?}");
    for secret in [
        "operator:credential",
        "chain-secret",
        "secret-wallet-path",
        "83838383",
    ] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn startup_rejects_every_member_local_binding_mismatch_before_io() {
    let config = federated_config();
    let baseline = local_config(&config);
    validates(&config, &baseline).expect("baseline");

    let mut cases: Vec<(&'static str, FederatedBarkLocalConfig)> = Vec::new();
    let mut local = baseline.clone();
    local.bindings.federation_id = FederationId::from_bytes([99; 32]);
    cases.push(("federation_id", local));
    let mut local = baseline.clone();
    local.bindings.network = Network::Bitcoin;
    cases.push(("network", local));
    let mut local = baseline.clone();
    local.bindings.ark_server_public_key = public_key(73);
    cases.push(("ark_server_public_key", local));
    let mut local = baseline.clone();
    local.bindings.root_key_epoch += 1;
    cases.push(("root_key_epoch", local));
    let mut local = baseline.clone();
    local.bindings.application_id =
        cdk_common::federation::FederationFrostApplicationId::from_bytes([74; 32])
            .expect("application id");
    cases.push(("application_id", local));
    let mut local = baseline.clone();
    local.bindings.application_key_epoch += 1;
    cases.push(("application_key_epoch", local));
    let mut local = baseline.clone();
    local.bindings.threshold += 1;
    cases.push(("threshold", local));
    let mut local = baseline.clone();
    local.bindings.members.remove(&member(3));
    cases.push(("member_roster", local));
    let mut local = baseline.clone();
    local.bindings.policy.maximum_outgoing_base_fee_sat += 1;
    cases.push(("policy", local));
    let mut local = baseline.clone();
    local.bark.vtxo_refresh_expiry_threshold += 1;
    cases.push(("refresh_before_expiry_blocks", local));
    let mut local = baseline.clone();
    local.bark.vtxo_exit_margin += 1;
    cases.push(("emergency_exit_before_expiry_blocks", local));
    let mut local = baseline.clone();
    local.bark.htlc_recv_claim_delta -= 1;
    cases.push(("minimum_incoming_cltv_delta", local));

    for (field, local) in cases {
        assert!(matches!(
            validates(&config, &local),
            Err(Error::ConfigMismatch { field: actual }) if actual == field
        ));
    }

    let mut local = baseline.clone();
    local.enabled = false;
    assert!(matches!(validates(&config, &local), Err(Error::Disabled)));
    assert!(matches!(
        ValidatedFederatedBarkConfig::validate(false, &config, &baseline),
        Err(Error::FederationRequired)
    ));
    let mut local = baseline.clone();
    local.bindings.unit = CurrencyUnit::Msat;
    assert!(matches!(
        validates(&config, &local),
        Err(Error::UnsupportedUnit)
    ));
    let mut local = baseline.clone();
    local.stores.frost_protocol_store = false;
    assert!(matches!(
        validates(&config, &local),
        Err(Error::MissingFrostProtocolStore)
    ));
    let mut local = baseline.clone();
    local.stores.receive_secret_store = false;
    assert!(matches!(
        validates(&config, &local),
        Err(Error::MissingReceiveSecretStore)
    ));
    let mut local = baseline.clone();
    local.bark.esplora_address = None;
    assert!(matches!(
        validates(&config, &local),
        Err(Error::InvalidChainSource)
    ));
    let mut local = baseline.clone();
    local.bark.bitcoind_address = Some("http://bitcoind.invalid".to_string());
    assert!(matches!(
        validates(&config, &local),
        Err(Error::InvalidChainSource)
    ));
}

#[test]
fn production_v1_rejects_receive_fees_and_anti_dos_modes_it_does_not_implement() {
    let baseline = federated_config();

    let mut config = baseline.clone();
    let application = config
        .frost_wallet
        .as_mut()
        .expect("wallet")
        .bark_application
        .as_mut()
        .expect("Bark application");
    application.policy.receive_base_fee_sat = 1;
    let mut local = local_config(&baseline);
    local.bindings.policy = application.policy.clone();
    assert!(matches!(
        validates(&config, &local),
        Err(Error::UnsupportedReceivePolicy)
    ));

    let mut config = baseline.clone();
    let application = config
        .frost_wallet
        .as_mut()
        .expect("wallet")
        .bark_application
        .as_mut()
        .expect("Bark application");
    application.policy.receive_anti_dos = FederationBarkReceiveAntiDosPolicy::ProtectedToken;
    let mut local = local_config(&baseline);
    local.bindings.policy = application.policy.clone();
    assert!(matches!(
        validates(&config, &local),
        Err(Error::UnsupportedReceivePolicy)
    ));
}

#[test]
fn persisted_wallet_properties_require_exact_frost_descriptor_and_operational_seed() {
    let config = federated_config();
    let local = local_config(&config);
    let validated =
        ValidatedFederatedBarkConfig::validate(true, &config, &local).expect("validated");
    let descriptor = CdkFrostVtxoCustody::new(validated.clone()).descriptor();
    let mailbox = public_key(75);
    let properties = WalletProperties {
        network: Network::Regtest,
        fingerprint: local.operational_seed.fingerprint(),
        key_source: Some(descriptor.clone()),
        server_pubkey: Some(validated.ark_server_public_key()),
        server_mailbox_pubkey: Some(mailbox),
    };
    validate_wallet_properties(
        &properties,
        Network::Regtest,
        local.operational_seed.fingerprint(),
        &descriptor,
        validated.ark_server_public_key(),
    )
    .expect("matching properties");

    let mut wrong = properties.clone();
    wrong.key_source = Some(KeySourceDescriptor::local(&local.operational_seed));
    assert!(matches!(
        validate_wallet_properties(
            &wrong,
            Network::Regtest,
            local.operational_seed.fingerprint(),
            &descriptor,
            validated.ark_server_public_key(),
        ),
        Err(Error::StandaloneCustodyForbidden)
    ));
    assert!(matches!(
        validated.validate_descriptor(wrong.key_source.as_ref().expect("descriptor")),
        Err(Error::StandaloneCustodyForbidden)
    ));

    let mut wrong = properties.clone();
    wrong.server_pubkey = Some(public_key(76));
    assert!(matches!(
        validate_wallet_properties(
            &wrong,
            Network::Regtest,
            local.operational_seed.fingerprint(),
            &descriptor,
            validated.ark_server_public_key(),
        ),
        Err(Error::ConfigMismatch {
            field: "ark_server_public_key"
        })
    ));

    let mut wrong = properties;
    wrong.fingerprint = WalletSeed::new_from_seed(Network::Regtest, &[84; 64]).fingerprint();
    assert!(matches!(
        validate_wallet_properties(
            &wrong,
            Network::Regtest,
            local.operational_seed.fingerprint(),
            &descriptor,
            validated.ark_server_public_key(),
        ),
        Err(Error::ConfigMismatch {
            field: "operational_seed"
        })
    ));
}

#[test]
fn missing_bark_application_and_invalid_limits_fail_closed() {
    let mut config = federated_config();
    let local = local_config(&config);
    config
        .frost_wallet
        .as_mut()
        .expect("wallet")
        .bark_application = None;
    assert!(matches!(
        validates(&config, &local),
        Err(Error::InvalidPublicConfig | Error::MissingApplication)
    ));
    assert!(matches!(
        FederatedBarkLimits::new(0, 10),
        Err(Error::InvalidLimits)
    ));
    assert!(matches!(
        FederatedBarkLimits::new(11, 10),
        Err(Error::InvalidLimits)
    ));
}

fn private_member(config: &FederationConfig, id: u16) -> MemberConfigPrivate {
    MemberConfigPrivate {
        federation_id: Some(config.federation_id),
        member_id: member(id),
        identity_secret_key: CdkSecretKey::from_slice(&[id as u8; 32]).expect("identity secret"),
        keyset_secret_shares: BTreeMap::new(),
        setup_transcript_hash: config
            .setup_transcript
            .as_ref()
            .map(|transcript| transcript.transcript_hash),
    }
}

fn receive_binding(
    config: &FederationConfig,
    action_seed: u8,
    preimage: &[u8; 32],
) -> FederationBarkReceiveSecretBinding {
    let application = config
        .frost_wallet
        .as_ref()
        .expect("wallet")
        .bark_application
        .as_ref()
        .expect("Bark application");
    FederationBarkReceiveSecretBinding {
        version: cdk_common::federation::FEDERATION_BARK_RECEIVE_SECRET_VERSION,
        federation_id: config.federation_id,
        action_id: FederationBarkActionId::from_bytes([action_seed; 32]).expect("action"),
        quote: format!("receive-secret-quote-{action_seed}"),
        actor: member(1),
        cashu_amount: Amount::from(100),
        gross_amount: Amount::from(100),
        receive_fee: Amount::ZERO,
        minimum_net_amount: Amount::from(100),
        unit: FederationBarkUnit::Sat,
        description_hash: cdk_common::federation::federation_bark_description_hash(None)
            .expect("description commitment"),
        payment_hash: payment_hash_for_preimage(preimage).expect("payment hash"),
        preimage_commitment: preimage_commitment(preimage).expect("preimage commitment"),
        destination: FederationBarkVtxoLocator::vtxo_custody(u32::from(action_seed)),
        mailbox_scope: cdk_common::federation::federation_bark_mailbox_scope(
            &application.ark_server_public_key,
        )
        .expect("mailbox commitment"),
        mailbox_id: application.ark_server_public_key,
        ark_server: ark_server_commitment(&application.ark_server_public_key)
            .expect("Ark commitment"),
        network: application.network,
        minimum_cltv_delta: application.policy.minimum_incoming_cltv_delta,
        fee_policy_version: 1,
        anti_dos_policy_version: 1,
        receive_proof: FederationBarkReceiveProof::NoProof,
        expires_at: 1_900_000_000,
    }
}

fn terminal_checkpoint(
    config: &FederationConfig,
    operation_index: u64,
) -> VerifiedFederationCheckpointQuorum {
    let checkpoint = cdk_common::federation::FederationCheckpoint {
        version: FEDERATION_CHECKPOINT_VERSION,
        federation_id: config.federation_id,
        federation_config_digest: config.config_digest().expect("config digest"),
        kind: FederationCheckpointKind::Scheduled,
        scheduled_checkpoint_sequence: 1,
        scheduled_anchor_id: None,
        journal_next_index: operation_index + 1,
        journal_order_digest: FederationJournalDigest::from_bytes([101; 32]),
        finalized_next_item_index: operation_index + 1,
        finalized_item_digest: FederationFinalizedItemDigest::from_bytes([102; 32]),
        application_digest: FederationApplicationDigest::from_bytes([103; 32]),
        aleph_bft_session: 1,
        next_unit_finalization_index: 1,
        next_unit_index_by_member: config
            .members
            .keys()
            .copied()
            .map(|member| (member, 1))
            .collect(),
        materialized_state_digest: FederationMaterializedStateDigest::from_bytes([104; 32]),
    };
    let signatures = [1, 2]
        .into_iter()
        .map(|id| {
            FederationCheckpointSignature::new_signed(&checkpoint, &private_member(config, id))
                .expect("checkpoint signature")
        })
        .collect();
    FederationCheckpointQuorum::new(checkpoint, signatures)
        .verify(config)
        .expect("verified checkpoint")
}

#[test]
fn protected_receive_secret_is_persisted_acknowledged_recoverable_and_retained() {
    let config = federated_config();
    let directory = tempfile::tempdir().expect("temporary secret stores");
    let path = directory.path().join("member-two-receive-secrets.redb");
    let key = std::sync::Arc::new(
        FederatedBarkReceiveSecretEncryptionKey::new([91; 32]).expect("storage key"),
    );
    let store = FederatedBarkReceiveSecretStore::open(&path, std::sync::Arc::clone(&key))
        .expect("open secret store");
    let preimage = [42; 32];
    let binding = receive_binding(&config, 51, &preimage);
    let request =
        FederationBarkPrepareReceiveSecretRequest::new(binding.clone(), member(2), preimage);
    let private = private_member(&config, 2);

    let acknowledgement = store
        .prepare_receive_secret(&request, member(1), &config, &private, 1_000)
        .expect("persist before acknowledgement");
    acknowledgement
        .verify(&config, &binding)
        .expect("durable acknowledgement");
    let identical = store
        .prepare_receive_secret(&request, member(1), &config, &private, 2_000)
        .expect("exact retry");
    assert_eq!(identical, acknowledgement);
    assert_eq!(
        store
            .load_secret(&binding)
            .expect("decrypt receive secret")
            .as_bytes(),
        &preimage
    );
    let status = store
        .status(binding.action_id)
        .expect("secret status")
        .expect("stored secret");
    assert!(status.acknowledged);
    assert_eq!(status.received_at, 1_000);
    assert!(status.accepted.is_none());
    let resumable = store
        .find_by_quote(config.federation_id, &binding.quote)
        .expect("quote recovery lookup")
        .expect("resumable quote secret");
    assert_eq!(resumable.binding, binding);
    assert_eq!(resumable.acknowledgement, Some(acknowledgement));
    assert!(resumable.accepted.is_none());
    assert_eq!(resumable.secret.as_bytes(), &preimage);
    assert!(store
        .find_by_quote(config.federation_id, "unknown-quote")
        .expect("unknown quote lookup")
        .is_none());

    let debug = format!("{request:?} {store:?} {key:?}");
    assert!(!debug.contains(&cdk_common::util::hex::encode(preimage)));
    let backup = store.export_backup().expect("encrypted backup");
    assert!(!String::from_utf8_lossy(&backup).contains(&cdk_common::util::hex::encode(preimage)));
    let mut tampered_backup: serde_json::Value =
        serde_json::from_slice(&backup).expect("backup JSON");
    tampered_backup["records"][0]["received_at"] = serde_json::json!(999);
    let tampered_path = directory.path().join("tampered-receive-secrets.redb");
    let tampered_store =
        FederatedBarkReceiveSecretStore::open(&tampered_path, std::sync::Arc::clone(&key))
            .expect("open tampered restore store");
    assert!(tampered_store
        .import_backup(
            &serde_json::to_vec(&tampered_backup).expect("tampered backup bytes"),
            &config,
        )
        .is_err());

    let mut conflicting_binding = binding.clone();
    conflicting_binding.quote = "another-quote".to_string();
    let conflicting =
        FederationBarkPrepareReceiveSecretRequest::new(conflicting_binding, member(2), preimage);
    assert!(matches!(
        store.prepare_receive_secret(&conflicting, member(1), &config, &private, 2_000),
        Err(Error::ReceiveSecretConflict)
    ));

    let accepted = FederatedBarkReceiveSecretAcceptedRef {
        operation_id: FederationOperationId::from_bytes([61; 32]),
        operation_index: 7,
    };
    store
        .mark_intent_accepted(binding.action_id, accepted)
        .expect("retain accepted intent");
    assert_eq!(
        store
            .find_by_quote(config.federation_id, &binding.quote)
            .expect("accepted quote recovery")
            .expect("accepted resumable quote")
            .accepted,
        Some(accepted)
    );
    assert_eq!(
        store.cleanup(u64::MAX / 2, 30).expect("accepted cleanup"),
        0
    );
    let terminal_operation_id = FederationOperationId::from_bytes([62; 32]);
    let terminal_operation_index = 12;
    let checkpointed_at = 10_000;
    let checkpoint = terminal_checkpoint(&config, terminal_operation_index);
    store
        .mark_terminal_checkpointed(
            binding.action_id,
            terminal_operation_id,
            terminal_operation_index,
            &checkpoint,
            checkpointed_at,
        )
        .expect("terminal checkpoint");
    assert_eq!(store.cleanup(20_000, 30).expect("retained cleanup"), 0);

    let restore_path = directory.path().join("restored-receive-secrets.redb");
    let restored =
        FederatedBarkReceiveSecretStore::open(&restore_path, std::sync::Arc::clone(&key))
            .expect("open restored store");
    restored
        .import_backup(&backup, &config)
        .expect("restore encrypted backup");
    assert_eq!(
        restored
            .load_secret(&binding)
            .expect("restored secret")
            .as_bytes(),
        &preimage
    );
    restored
        .import_backup(&backup, &config)
        .expect("idempotent backup restore");

    let retention_seconds = 30 * 24 * 60 * 60;
    assert_eq!(
        store
            .cleanup(checkpointed_at + retention_seconds, 30)
            .expect("terminal cleanup"),
        1
    );
    assert!(store
        .status(binding.action_id)
        .expect("deleted status")
        .is_none());
}

#[test]
fn receive_secret_survives_crash_after_persistence_before_acknowledgement() {
    let config = federated_config();
    let directory = tempfile::tempdir().expect("temporary secret store");
    let path = directory.path().join("crash-boundary-receive-secrets.redb");
    let key = std::sync::Arc::new(
        FederatedBarkReceiveSecretEncryptionKey::new([92; 32]).expect("storage key"),
    );
    let store = FederatedBarkReceiveSecretStore::open(&path, std::sync::Arc::clone(&key))
        .expect("open secret store");
    let preimage = [43; 32];
    let binding = receive_binding(&config, 52, &preimage);
    let request =
        FederationBarkPrepareReceiveSecretRequest::new(binding.clone(), member(2), preimage);
    let mut rng = ChaCha20Rng::from_seed([93; 32]);

    assert!(store
        .persist_before_ack_for_test(&request, 3_000, &mut rng)
        .expect("commit encrypted record")
        .is_none());
    assert!(
        !store
            .status(binding.action_id)
            .expect("secret status")
            .expect("stored secret")
            .acknowledged
    );
    drop(store);

    let reopened = FederatedBarkReceiveSecretStore::open(&path, key).expect("reopen secret store");
    assert_eq!(
        reopened
            .load_secret(&binding)
            .expect("recover persisted secret")
            .as_bytes(),
        &preimage
    );
    let acknowledgement = reopened
        .prepare_receive_secret(
            &request,
            member(1),
            &config,
            &private_member(&config, 2),
            4_000,
        )
        .expect("complete acknowledgement after retry");
    acknowledgement
        .verify(&config, &binding)
        .expect("verify recovered acknowledgement");
    let status = reopened
        .status(binding.action_id)
        .expect("secret status")
        .expect("stored secret");
    assert!(status.acknowledged);
    assert_eq!(status.received_at, 3_000);
}

#[test]
fn orphan_cleanup_threshold_acknowledgements_and_replay_bindings_fail_closed() {
    let config = federated_config();
    let directory = tempfile::tempdir().expect("temporary secret stores");
    let preimage = [71; 32];
    let binding = receive_binding(&config, 72, &preimage);
    let mut acknowledgements = Vec::new();
    let mut stores = Vec::new();

    for id in 1..=2 {
        let store = FederatedBarkReceiveSecretStore::open(
            directory.path().join(format!("member-{id}.redb")),
            std::sync::Arc::new(
                FederatedBarkReceiveSecretEncryptionKey::new([id as u8 + 80; 32])
                    .expect("storage key"),
            ),
        )
        .expect("open member store");
        let request =
            FederationBarkPrepareReceiveSecretRequest::new(binding.clone(), member(id), preimage);
        acknowledgements.push(
            store
                .prepare_receive_secret(
                    &request,
                    member(1),
                    &config,
                    &private_member(&config, id),
                    100,
                )
                .expect("prepare member secret"),
        );
        stores.push(store);
    }
    acknowledgements.sort_by_key(|acknowledgement| acknowledgement.member);
    let intent = FederationBarkReceiveIntent {
        version: cdk_common::federation::FEDERATION_BARK_OPERATION_VERSION,
        action_id: binding.action_id,
        quote: binding.quote.clone(),
        actor: binding.actor,
        cashu_amount: binding.cashu_amount,
        gross_amount: binding.gross_amount,
        receive_fee: binding.receive_fee,
        minimum_net_amount: binding.minimum_net_amount,
        unit: binding.unit,
        description_hash: binding.description_hash,
        description: None,
        payment_hash: binding.payment_hash,
        preimage_commitment: binding.preimage_commitment,
        secret_acknowledgements: acknowledgements.clone(),
        destination: binding.destination,
        mailbox_scope: binding.mailbox_scope,
        mailbox_id: binding.mailbox_id,
        ark_server: binding.ark_server,
        network: binding.network,
        minimum_cltv_delta: binding.minimum_cltv_delta,
        fee_policy_version: binding.fee_policy_version,
        anti_dos_policy_version: binding.anti_dos_policy_version,
        receive_proof: binding.receive_proof,
        expires_at: binding.expires_at,
    };
    intent
        .validate_secret_acknowledgements(&config)
        .expect("signature threshold");
    let accepted = FederatedBarkReceiveSecretAcceptedRef {
        operation_id: FederationOperationId::from_bytes([74; 32]),
        operation_index: 9,
    };
    for store in &stores {
        store
            .mark_intent_accepted(binding.action_id, accepted)
            .expect("retain threshold secret");
    }
    let surviving_member = stores.pop().expect("surviving member store");
    drop(stores);
    assert_eq!(
        surviving_member
            .load_secret(&binding)
            .expect("recover after invoice actor loss")
            .as_bytes(),
        &preimage
    );
    assert_eq!(
        surviving_member
            .cleanup(u64::MAX / 2, 30)
            .expect("accepted secret retention"),
        0
    );

    let orphan_preimage = [75; 32];
    let orphan_binding = receive_binding(&config, 76, &orphan_preimage);
    let orphan_store = FederatedBarkReceiveSecretStore::open(
        directory.path().join("orphan-member-three.redb"),
        std::sync::Arc::new(
            FederatedBarkReceiveSecretEncryptionKey::new([83; 32]).expect("storage key"),
        ),
    )
    .expect("open orphan store");
    orphan_store
        .prepare_receive_secret(
            &FederationBarkPrepareReceiveSecretRequest::new(
                orphan_binding.clone(),
                member(3),
                orphan_preimage,
            ),
            member(1),
            &config,
            &private_member(&config, 3),
            100,
        )
        .expect("prepare orphan secret");
    assert_eq!(
        orphan_store
            .cleanup(
                100 + FEDERATED_BARK_RECEIVE_SECRET_ORPHAN_RETENTION_SECONDS - 1,
                30,
            )
            .expect("pre-expiry cleanup"),
        0
    );
    assert_eq!(
        orphan_store
            .cleanup(
                100 + FEDERATED_BARK_RECEIVE_SECRET_ORPHAN_RETENTION_SECONDS,
                30,
            )
            .expect("orphan cleanup"),
        1
    );

    let mut insufficient = intent.clone();
    insufficient.secret_acknowledgements.pop();
    assert!(insufficient
        .validate_secret_acknowledgements(&config)
        .is_err());
    let mut forged = intent.clone();
    forged.secret_acknowledgements[0].member = member(3);
    forged
        .secret_acknowledgements
        .sort_by_key(|acknowledgement| acknowledgement.member);
    assert!(forged.validate_secret_acknowledgements(&config).is_err());
    let mut duplicated = intent.clone();
    duplicated.secret_acknowledgements[1] = duplicated.secret_acknowledgements[0];
    assert!(duplicated
        .validate_secret_acknowledgements(&config)
        .is_err());
    let mut replayed = intent;
    replayed.destination = FederationBarkVtxoLocator::vtxo_custody(99);
    assert!(replayed.validate_secret_acknowledgements(&config).is_err());
}

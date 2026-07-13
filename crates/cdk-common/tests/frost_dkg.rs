#![cfg(feature = "federation")]
//! Integration coverage for federation-bound FROST DKG transport records.

use std::collections::BTreeMap;
use std::str::FromStr;

use cashu::MintUrl;
use cdk_common::federation::{
    FederationConfig, FederationDkgCeremonyId, FederationFrostActiveEpochs,
    FederationFrostApplicationConfig, FederationFrostConsumerRegistration,
    FederationFrostDkgContext, FederationFrostDkgRound1Request, FederationFrostDkgRound2Request,
    FederationFrostEpochState, FederationFrostRootPrivateConfig, FederationFrostRootPublicConfig,
    FederationFrostRootRecord, FederationFrostWalletConfig, FederationOnchainConfirmationPolicy,
    MemberConfigPublic, MemberId, ThresholdParams,
};
use cdk_common::SecretKey;
use cdk_frost::rand_core::{OsRng, SeedableRng};
use cdk_frost::{advance_dkg, finish_dkg, start_dkg, SecretEncryptionKey};
use cdk_frost_bip32::{FrostBip32, WalletBranch, WalletIndex};
use rand_chacha::ChaCha20Rng;

fn member(id: u16) -> MemberId {
    MemberId::new(id).expect("member")
}

fn config() -> FederationConfig {
    let members = (1..=3)
        .map(|id| {
            let secret = SecretKey::from_slice(&[id as u8; 32]).expect("identity secret");
            (
                member(id),
                MemberConfigPublic {
                    member_id: member(id),
                    public_mint_url: MintUrl::from_str(&format!("https://mint{id}.example.com"))
                        .expect("mint URL"),
                    federation_api_url: MintUrl::from_str(&format!("https://fed{id}.example.com"))
                        .expect("federation URL"),
                    identity_public_key: secret.public_key(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    FederationConfig::new(
        ThresholdParams::new(3, 2, 2).expect("thresholds"),
        members,
        Vec::new(),
    )
    .expect("federation config")
}

fn completed_roots() -> (
    FederationConfig,
    Vec<FederationFrostRootRecord>,
    Vec<SecretEncryptionKey>,
) {
    let config = config();
    let context =
        FederationFrostDkgContext::new(&config, FederationDkgCeremonyId::from_bytes([51; 32]), 1)
            .expect("FROST context");
    let frost_context = context.to_frost_context().expect("crypto context");
    let mut rng = ChaCha20Rng::from_seed([52; 32]);
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
    let mut records = Vec::new();
    let mut storage_keys = Vec::new();
    for id in 1..=3 {
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
        let storage_key = SecretEncryptionKey::new([id as u8; 32]).expect("storage key");
        let encrypted = root.seal(&storage_key, &mut rng).expect("sealed root");
        records.push(FederationFrostRootRecord {
            context: context.clone(),
            member_id: member(id),
            public_package: root.public().encode(),
            encrypted_root_share: encrypted.to_bytes(),
        });
        storage_keys.push(storage_key);
    }
    (config, records, storage_keys)
}

#[test]
fn frost_context_is_exactly_bound_to_bls_roster_threshold_and_suite() {
    let config = config();
    let ceremony = FederationDkgCeremonyId::from_bytes([31; 32]);
    let context = FederationFrostDkgContext::new(&config, ceremony, 7).expect("FROST context");
    assert_eq!(
        context.threshold,
        config.threshold_params.signature_threshold
    );
    assert_eq!(
        context.participants,
        config.members.keys().copied().collect()
    );
    context.validate_against(&config).expect("valid context");

    let mut wrong_suite = context.clone();
    wrong_suite.ciphersuite = "bls12_381".to_string();
    assert!(wrong_suite.validate_against(&config).is_err());
    let mut wrong_protocol = context.clone();
    wrong_protocol.protocol_version += 1;
    assert!(wrong_protocol.validate_against(&config).is_err());
    let mut wrong_threshold = context.clone();
    wrong_threshold.threshold = 3;
    assert!(wrong_threshold.validate_against(&config).is_err());
    let mut wrong_roster = context;
    wrong_roster.participants.remove(&member(3));
    assert!(wrong_roster.validate_against(&config).is_err());
}

#[test]
fn frost_rest_payloads_reject_spoofed_senders_and_wrong_recipients() {
    let config = config();
    let context =
        FederationFrostDkgContext::new(&config, FederationDkgCeremonyId::from_bytes([41; 32]), 8)
            .expect("FROST context");
    let frost_context = context.to_frost_context().expect("crypto context");
    let mut secrets = BTreeMap::new();
    let mut round1 = BTreeMap::new();
    for id in 1..=3 {
        let frost_member = cdk_frost::MemberId::new(id).expect("FROST member");
        let (secret, package) =
            start_dkg(frost_context.clone(), frost_member, OsRng).expect("round one");
        secrets.insert(id, secret);
        round1.insert(id, package);
    }

    let request = FederationFrostDkgRound1Request {
        context: context.clone(),
        sender: member(1),
        package: round1.get(&1).expect("round-one package").encode(),
    };
    request
        .decode_package(&config, member(1))
        .expect("valid signed round-one request");
    assert!(request.decode_package(&config, member(2)).is_err());
    let mut spoofed = request;
    spoofed.sender = member(2);
    assert!(spoofed.decode_package(&config, member(2)).is_err());

    let received = [2, 3]
        .into_iter()
        .map(|id| round1.get(&id).expect("peer package").clone());
    let (_, outgoing) =
        advance_dkg(secrets.remove(&1).expect("round-one secret"), received).expect("round two");
    let package = outgoing
        .get(&cdk_frost::MemberId::new(2).expect("receiver"))
        .expect("round-two package");
    let request = FederationFrostDkgRound2Request {
        context,
        sender: member(1),
        receiver: member(2),
        package: package.encode(),
    };
    request
        .decode_package(&config, member(1), member(2))
        .expect("valid signed round-two request");
    assert!(request
        .decode_package(&config, member(1), member(3))
        .is_err());
    assert!(request
        .decode_package(&config, member(2), member(2))
        .is_err());
    let debug = format!("{request:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains(&format!("{:?}", request.package)));
}

#[test]
fn frost_wallet_configs_round_trip_derive_addresses_and_reject_wrong_bindings() {
    let (mut config, records, storage_keys) = completed_roots();
    let root = FederationFrostRootPublicConfig::from_dkg_record(
        &config,
        &records[0],
        FederationFrostEpochState::Active,
    )
    .expect("public root config");
    let private = FederationFrostRootPrivateConfig::from_dkg_record(&root, &records[0])
        .expect("private root config");
    private
        .open(&config, &root, member(1), &storage_keys[0])
        .expect("local private root");
    assert!(private
        .open(&config, &root, member(2), &storage_keys[1])
        .is_err());

    let application = FederationFrostApplicationConfig::new_cdk_onchain(
        &config,
        &root,
        1,
        FederationFrostEpochState::Active,
        bitcoin::Network::Regtest,
        FederationOnchainConfirmationPolicy::new(2).expect("confirmation policy"),
        1,
    )
    .expect("application config");
    let registration = FederationFrostConsumerRegistration::new(
        root.root_key_epoch,
        application.application_id,
        application.application_key_epoch,
    )
    .expect("consumer registration");
    let active_epochs = FederationFrostActiveEpochs {
        root_key_epoch: root.root_key_epoch,
        application_key_epochs: [(application.application_id, 1)].into_iter().collect(),
    };
    let wallet = FederationFrostWalletConfig::new(
        &config,
        vec![root.clone()],
        vec![application.clone()],
        vec![registration.clone()],
        active_epochs,
    )
    .expect("wallet config");
    assert_eq!(
        root.digest().expect("root digest vector"),
        [
            163, 120, 199, 199, 114, 188, 77, 172, 189, 65, 171, 138, 224, 143, 181, 208, 103, 87,
            201, 171, 79, 152, 195, 105, 100, 68, 247, 1, 205, 95, 255, 119,
        ]
    );
    assert_eq!(
        wallet.digest().expect("wallet digest vector"),
        [
            156, 124, 24, 13, 193, 23, 94, 231, 7, 105, 243, 93, 222, 198, 124, 56, 0, 122, 31,
            173, 75, 124, 80, 25, 40, 240, 87, 253, 60, 87, 167, 154,
        ]
    );
    let base_digest = config.config_digest().expect("base config digest");
    config.frost_wallet = Some(wallet.clone());
    config.validate().expect("federated wallet config");
    assert_ne!(base_digest, config.config_digest().expect("wallet digest"));

    let json = serde_json::to_string(&config).expect("public JSON");
    let decoded: FederationConfig = serde_json::from_str(&json).expect("public round trip");
    assert_eq!(decoded, config);
    assert_eq!(
        wallet.digest().expect("wallet digest"),
        decoded
            .frost_wallet
            .as_ref()
            .expect("decoded wallet")
            .digest()
            .expect("decoded digest")
    );
    assert!(!json.contains("encrypted_root_share"));

    let derived = application
        .derived_public_package(&config, &root)
        .expect("derived public package");
    let adapter = FrostBip32::new(derived, bitcoin::Network::Regtest).expect("public BIP32");
    let address = adapter
        .derive(WalletBranch::External, WalletIndex::new(0).expect("index"))
        .expect("public child")
        .address(bitcoin::Network::Regtest)
        .expect("address");
    assert!(address.to_string().starts_with("bcrt1p"));

    let mut wrong_descriptor = application.clone();
    wrong_descriptor.external_descriptor.push('x');
    assert!(wrong_descriptor.validate(&config, &root).is_err());
    let mut wrong_network = application.clone();
    wrong_network.network = bitcoin::Network::Bitcoin;
    assert!(wrong_network.validate(&config, &root).is_err());
    let mut wrong_group = application.clone();
    wrong_group.derived_group_key[1] ^= 1;
    assert!(wrong_group.validate(&config, &root).is_err());
    let mut wrong_application_id = application.clone();
    wrong_application_id.application_id =
        cdk_common::federation::FederationFrostApplicationId::from_bytes([98; 32])
            .expect("wrong application");
    assert!(wrong_application_id.validate(&config, &root).is_err());
    let mut wrong_root_reference = application.clone();
    wrong_root_reference.root_key_epoch += 1;
    assert!(wrong_root_reference.validate(&config, &root).is_err());
    let mut wrong_derivation = application.clone();
    wrong_derivation.derivation_version += 1;
    assert!(wrong_derivation.validate(&config, &root).is_err());
    let mut wrong_transcript = root.clone();
    wrong_transcript.transcript_binding[0] ^= 1;
    assert!(wrong_transcript.validate(&config).is_err());
    let mut wrong_threshold = root.clone();
    wrong_threshold.threshold += 1;
    assert!(wrong_threshold.validate(&config).is_err());
    let mut missing_participant = root.clone();
    missing_participant.verification_shares.remove(&member(3));
    assert!(missing_participant.validate(&config).is_err());
    let mut extra_participant = root.clone();
    extra_participant
        .verification_shares
        .insert(member(4), [2; 33]);
    assert!(extra_participant.validate(&config).is_err());
    let mut wrong_private_epoch = private.clone();
    wrong_private_epoch.root_key_epoch += 1;
    assert!(wrong_private_epoch
        .validate_metadata(&root, member(1))
        .is_err());

    let unregistered = FederationFrostWalletConfig::new(
        &config,
        vec![root.clone()],
        vec![application.clone()],
        Vec::new(),
        wallet.active_epochs.clone(),
    );
    assert!(unregistered.is_err());
    let mut consumers = wallet.registered_consumers.clone();
    consumers.push(
        FederationFrostConsumerRegistration::new(
            root.root_key_epoch,
            cdk_common::federation::FederationFrostApplicationId::from_bytes([99; 32])
                .expect("test application"),
            1,
        )
        .expect("test consumer"),
    );
    let with_test_consumer = FederationFrostWalletConfig::new(
        &config,
        vec![root.clone()],
        vec![application.clone()],
        consumers,
        wallet.active_epochs.clone(),
    )
    .expect("additional consumer");
    assert_eq!(with_test_consumer.roots[0], root);
    assert_eq!(with_test_consumer.cdk_onchain_applications[0], application);

    let private_json = serde_json::to_string(&private).expect("private JSON");
    assert!(private_json.contains("encrypted_root_share"));
    let private_debug = format!("{private:?}");
    assert!(private_debug.contains("<redacted>"));
    assert!(!private_debug.contains(&format!("{:?}", private.encrypted_root_share)));
}

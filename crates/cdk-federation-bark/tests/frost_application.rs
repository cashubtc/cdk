//! Bark-owned coverage for the reusable-root FROST application boundary.

use std::collections::BTreeMap;
use std::str::FromStr;

use cdk_common::federation::{
    FederationCdkOnchainFrostAuthorizationAdapter, FederationConfig, FederationDkgCeremonyId,
    FederationFrostApplicationConfig, FederationFrostDkgContext, FederationFrostRootPrivateConfig,
    FederationFrostRootPublicConfig, FederationFrostRootRecord,
    FederationOnchainConfirmationPolicy, FederationSetupAuthorizationHash, MemberConfigPublic,
    MemberId, ThresholdParams,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::SecretKey;
use cdk_federation_bark::{
    federation_bark_application, federation_frost_wallet_with_bark,
    FederationBarkFrostApplicationConfig, FederationBarkFrostRegistrationAdapter,
    FederationBarkPolicyConfig, FederationBarkReceiveAntiDosPolicy,
};
use cdk_frost::rand_core::SeedableRng;
use cdk_frost::{advance_dkg, finish_dkg, start_dkg, ParticipantKeyManager, SecretEncryptionKey};
use rand_chacha::ChaCha20Rng;

fn member(id: u16) -> MemberId {
    MemberId::new(id).expect("member")
}

fn federation_config() -> FederationConfig {
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
        FederationSetupAuthorizationHash::from_bytes([1; 32]),
        ThresholdParams::new(3, 2, 2).expect("thresholds"),
        members,
        Vec::new(),
    )
    .expect("federation config")
}

fn completed_root() -> (
    FederationConfig,
    Vec<FederationFrostRootRecord>,
    Vec<SecretEncryptionKey>,
) {
    let config = federation_config();
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
        records.push(FederationFrostRootRecord {
            context: context.clone(),
            member_id: member(id),
            public_package: root.public().encode(),
            encrypted_root_share: root
                .seal(&storage_key, &mut rng)
                .expect("sealed root")
                .to_bytes(),
        });
        storage_keys.push(storage_key);
    }
    (config, records, storage_keys)
}

fn bark_policy() -> FederationBarkPolicyConfig {
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
        FederationBarkReceiveAntiDosPolicy::ProtectedToken,
    )
    .expect("Bark policy")
}

#[test]
fn bark_registration_is_distinct_persistent_and_derived_by_each_member() {
    let (mut config, records, storage_keys) = completed_root();
    let root = FederationFrostRootPublicConfig::from_dkg_record(&config, &records[0])
        .expect("public root");
    let cdk_application = FederationFrostApplicationConfig::new_cdk_onchain(
        &config,
        &root,
        1,
        bitcoin::Network::Regtest,
        FederationOnchainConfirmationPolicy::new(2).expect("confirmation policy"),
        1,
    )
    .expect("CDK application");
    let server_secret =
        bitcoin::secp256k1::SecretKey::from_slice(&[91; 32]).expect("Ark server secret");
    let server = server_secret.public_key(&bitcoin::secp256k1::Secp256k1::new());
    let bark_application = FederationBarkFrostApplicationConfig::new(
        &config,
        &root,
        1,
        bitcoin::Network::Regtest,
        server,
        bark_policy(),
    )
    .expect("Bark application");
    assert_ne!(
        cdk_application.derived_group_key,
        bark_application.derived_group_key
    );

    let expected_public = bark_application
        .derived_public_package(&config, &root)
        .expect("public package");
    for (offset, record) in records.iter().enumerate() {
        let member_id = member(u16::try_from(offset + 1).expect("member id"));
        let private = FederationFrostRootPrivateConfig::from_dkg_record(&root, record)
            .expect("private root config");
        let root_package = private
            .open(&config, &root, member_id, &storage_keys[offset])
            .expect("sealed root opens");
        let nonce_key =
            SecretEncryptionKey::new([150 + offset as u8; 32]).expect("nonce encryption key");
        let mut manager = ParticipantKeyManager::new_in_memory(root_package, nonce_key)
            .expect("participant manager");
        manager
            .register_application(
                FederationCdkOnchainFrostAuthorizationAdapter::new(&cdk_application)
                    .expect("CDK adapter"),
            )
            .expect("register CDK");
        let member_bark = manager
            .register_application(
                FederationBarkFrostRegistrationAdapter::new(&bark_application)
                    .expect("Bark adapter"),
            )
            .expect("register Bark");
        assert_eq!(member_bark, expected_public);
    }

    config.frost_wallet = Some(
        federation_frost_wallet_with_bark(
            &config,
            root.clone(),
            cdk_application,
            bark_application.clone(),
        )
        .expect("Bark-capable wallet"),
    );
    config.validate().expect("Bark federation config");
    let restored: FederationConfig =
        serde_json::from_slice(&serde_json::to_vec(&config).expect("public config JSON"))
            .expect("public config round trip");
    assert_eq!(restored, config);
    assert_eq!(
        federation_bark_application(&restored)
            .expect("Bark application lookup")
            .expect("Bark application"),
        bark_application
    );

    let mut legacy_value = serde_json::to_value(&config).expect("public config value");
    let wallet = legacy_value
        .get_mut("frost_wallet")
        .and_then(serde_json::Value::as_object_mut)
        .expect("FROST wallet object");
    wallet
        .shift_remove("extension_applications")
        .expect("generic extension field");
    wallet.insert(
        "bark_application".to_string(),
        serde_json::to_value(&bark_application).expect("legacy Bark application"),
    );
    let legacy_json = serde_json::to_string(&legacy_value).expect("legacy public config JSON");
    let legacy_restored: FederationConfig =
        serde_json::from_str(&legacy_json).expect("legacy public config decode");
    legacy_restored.validate().expect("legacy public config");
    assert_eq!(
        federation_bark_application(&legacy_restored)
            .expect("legacy Bark application lookup")
            .expect("legacy Bark application"),
        bark_application
    );
    assert_eq!(
        serde_json::to_string(&legacy_restored).expect("legacy public config re-encode"),
        legacy_json,
        "established public-config field order and digest bytes must remain stable",
    );
    assert!(bark_application
        .open_public_adapter(&config, &root, bitcoin::Network::Signet, server, 1, 1,)
        .is_err());
}

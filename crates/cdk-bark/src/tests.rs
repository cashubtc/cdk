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
use cdk_common::federation::{
    FederationBarkFrostApplicationConfig, FederationBarkPolicyConfig,
    FederationBarkReceiveAntiDosPolicy, FederationConfig, FederationDkgCeremonyId,
    FederationFrostApplicationConfig, FederationFrostDkgContext, FederationFrostRootPublicConfig,
    FederationFrostRootRecord, FederationFrostWalletConfig, FederationId,
    FederationOnchainConfirmationPolicy, MemberConfigPublic, MemberId, ThresholdParams,
};
use cdk_common::CurrencyUnit;
use cdk_common::SecretKey as CdkSecretKey;
use cdk_frost::rand_core::SeedableRng;
use cdk_frost::{advance_dkg, finish_dkg, start_dkg, SecretEncryptionKey};
use rand_chacha::ChaCha20Rng;

use crate::runtime::validate_wallet_properties;
use crate::{
    CdkFrostVtxoCustody, Error, FederatedBarkLimits, FederatedBarkLocalBindings,
    FederatedBarkLocalConfig, FederatedBarkStoreAvailability, ValidatedFederatedBarkConfig,
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
        FederationBarkReceiveAntiDosPolicy::ProtectedToken,
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

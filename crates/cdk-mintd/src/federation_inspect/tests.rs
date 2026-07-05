use std::collections::BTreeMap;
use std::str::FromStr;

use cdk_common::federation::{
    generate_trusted_dealer_configs, MemberId, ThresholdParams, TrustedDealerConfigBundle,
    TrustedDealerKeysetSpec, TrustedDealerMember,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::nut01::BlsSecretKey;
use cdk_common::{Amount, CurrencyUnit, SecretKey};

use super::*;

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
            .expect("mint url"),
        MintUrl::from_str(&format!("https://fed{}.example.com", member_id.get())).expect("fed url"),
        identity_secret(member_id),
    )
}

fn inspect_fixture() -> TrustedDealerConfigBundle {
    generate_trusted_dealer_configs(
        ThresholdParams::with_bft_consensus(3, 2).expect("params"),
        vec![
            member(member_id(1)),
            member(member_id(2)),
            member(member_id(3)),
        ],
        vec![TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            7,
            Some(123_456),
            BTreeMap::from([
                (
                    Amount::from(1),
                    vec![
                        BlsSecretKey::from_reduced_bytes(&[2; 32]),
                        BlsSecretKey::from_reduced_bytes(&[3; 32]),
                    ],
                ),
                (
                    Amount::from(2),
                    vec![
                        BlsSecretKey::from_reduced_bytes(&[4; 32]),
                        BlsSecretKey::from_reduced_bytes(&[5; 32]),
                    ],
                ),
            ]),
        )],
    )
    .expect("trusted dealer bundle")
}

#[test]
fn inspect_report_summarizes_public_config() {
    let bundle = inspect_fixture();
    let report = inspect_report(&bundle.public_config, None).expect("inspect report");
    let keyset = &bundle.public_config.keysets[0];

    assert!(report.contains(&format!(
        "Federation {}",
        bundle.public_config.federation_id
    )));
    assert!(report.contains("members=3 signature_threshold=2 consensus_threshold=3"));
    assert!(report.contains("setup_transcript=trusted_dealer"));
    assert!(report.contains(&format!(
        "hash={}",
        bundle
            .public_config
            .setup_transcript
            .as_ref()
            .expect("setup transcript")
            .transcript_hash
    )));
    assert!(report.contains("signatures=3"));
    assert!(report.contains("member 1 public_mint_url=https://mint1.example.com"));
    assert!(report.contains("federation_api_url=https://fed1.example.com"));
    assert!(report.contains(&format!("keyset {}", keyset.id)));
    assert!(report.contains("unit=sat active=true version=v3"));
    assert!(report.contains("input_fee_ppk=7 final_expiry=123456 amounts=[1,2]"));
    assert!(report.contains("Local member config\n- not provided"));
}

#[test]
fn inspect_report_validates_private_config_without_leaking_fields() {
    let bundle = inspect_fixture();
    let private_config = bundle
        .private_configs
        .get(&member_id(1))
        .expect("private config");
    let report =
        inspect_report(&bundle.public_config, Some(private_config)).expect("inspect report");

    assert!(report.contains("Local member config\n- valid=true member=1"));
    assert!(report.contains(&format!(
        "federation_id={}",
        bundle.public_config.federation_id
    )));
    assert!(report.contains(&format!(
        "setup_transcript_hash={}",
        bundle
            .public_config
            .setup_transcript
            .as_ref()
            .expect("setup transcript")
            .transcript_hash
    )));
    assert!(report.contains("keysets=1 total_amounts=2"));
    assert!(!report.contains("identity_secret_key"));
    assert!(!report.contains("identity secret"));
    assert!(!report.contains("keyset_secret_shares"));
    assert!(!report.contains("BlsSecretKeyShare"));
}

#[test]
fn inspect_report_rejects_mismatched_private_config() {
    let bundle = inspect_fixture();
    let mut private_config = bundle
        .private_configs
        .get(&member_id(1))
        .expect("private config")
        .clone();
    private_config.member_id = member_id(42);

    assert!(inspect_report(&bundle.public_config, Some(&private_config)).is_err());
}

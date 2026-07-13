#![cfg(feature = "federation")]
//! Integration coverage for federation-bound FROST DKG transport records.

use std::collections::BTreeMap;
use std::str::FromStr;

use cashu::MintUrl;
use cdk_common::federation::{
    FederationConfig, FederationDkgCeremonyId, FederationFrostDkgContext,
    FederationFrostDkgRound1Request, FederationFrostDkgRound2Request, MemberConfigPublic, MemberId,
    ThresholdParams,
};
use cdk_common::SecretKey;
use cdk_frost::rand_core::OsRng;
use cdk_frost::{advance_dkg, start_dkg};

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

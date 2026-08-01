//! Frozen multi-operator federation setup transcript vectors.

#![cfg(feature = "federation")]

use cdk_common::bitcoin::hashes::sha256::Hash as Sha256Hash;
use cdk_common::bitcoin::hashes::Hash;
use cdk_common::bitcoin::secp256k1::schnorr::Signature;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use cdk_common::federation::{
    FederationCheckpointPolicy, FederationDkgKeysetSpec, FederationPaymentObservationPolicy,
    FederationSetupApproval, FederationSetupBitcoinNetwork, FederationSetupCoordinatorPublicKey,
    FederationSetupEnrollment, FederationSetupFrostApplication, FederationSetupFrostPolicy,
    FederationSetupId, FederationSetupInvitationCapability, FederationSetupInvitationId,
    FederationSetupManifest, FederationSetupProposal, FederationSetupProposalBody,
    FederationSetupProtocolVersions, FederationSetupUrlPolicy, MemberId, ThresholdParams,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::nut01::SecretKey;
use cdk_common::{Amount, CurrencyUnit};
use serde::{Deserialize, Serialize};

const SETUP_PROTOCOL_VERSION: u16 = 1;
const DKG_PROTOCOL_VERSION: u16 = 2;
const FEDERATION_REST_API_VERSION: u16 = 2;
const WALLET_PROTOCOL_VERSION: u16 = 1;
const BLS_KEYSET_VERSION: u16 = 2;
const FROST_DKG_PROTOCOL_VERSION: u16 = 2;
const FROST_CIPHERSUITE_VERSION: u16 = 1;

const INVITATION_CAPABILITY_DOMAIN: &[u8] = b"cdk/federation/setup/invitation-capability/v1";
const INVITATION_CHECKSUM_DOMAIN: &[u8] = b"cdk/federation/setup/invitation-checksum/v1";
const ENROLLMENT_SIGNATURE_DOMAIN: &[u8] = b"cdk/federation/setup/enrollment-signature/v1";
const PROPOSAL_DOMAIN: &[u8] = b"cdk/federation/setup/proposal/v1";
const PROPOSAL_PUBLICATION_DOMAIN: &[u8] = b"cdk/federation/setup/proposal-publication/v1";
const APPROVAL_DOMAIN: &[u8] = b"cdk/federation/setup/approval/v1";
const MANIFEST_DOMAIN: &[u8] = b"cdk/federation/setup/manifest/v1";
const MANIFEST_PUBLICATION_DOMAIN: &[u8] = b"cdk/federation/setup/manifest-publication/v1";
const CEREMONY_ID_DOMAIN: &[u8] = b"cdk/federation/setup/ceremony-id/v1";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VectorFile {
    version: u16,
    vectors: Vec<ExpectedVector>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExpectedVector {
    name: String,
    member_count: u16,
    signature_threshold: u16,
    consensus_threshold: u16,
    proposal_canonical_bytes: usize,
    coordinator_public_key: String,
    invitation_capability_hash: String,
    invitation_checksum: String,
    first_enrollment_signature: String,
    proposal_hash: String,
    setup_authorization_hash: String,
    ceremony_id: String,
    coordinator_proposal_signature: String,
    first_approval_signature: String,
    manifest_certification_hash: String,
    coordinator_manifest_signature: String,
}

struct EnrollmentRecord {
    canonical: Vec<u8>,
    public_key: cdk_common::nuts::nut01::PublicKey,
    signature: Signature,
    signature_transcript: Vec<u8>,
}

struct ApprovalRecord {
    member_id: u16,
    public_key: cdk_common::nuts::nut01::PublicKey,
    signature: Signature,
    signature_transcript: Vec<u8>,
}

#[test]
fn frozen_setup_transcript_vectors_are_stable_and_signatures_verify() {
    let expected: VectorFile =
        serde_json::from_str(include_str!("data/federation_setup_transcript_v1.json"))
            .expect("fixed setup transcript vectors must be valid JSON");
    assert_eq!(expected.version, SETUP_PROTOCOL_VERSION);

    let actual = VectorFile {
        version: SETUP_PROTOCOL_VERSION,
        vectors: expected
            .vectors
            .iter()
            .map(|vector| {
                build_vector(
                    &vector.name,
                    vector.member_count,
                    vector.signature_threshold,
                    vector.consensus_threshold,
                )
            })
            .collect(),
    };

    if expected.vectors.iter().any(vector_is_unfilled) {
        panic!(
            "replace placeholder setup vectors with:\n{}",
            serde_json::to_string_pretty(&actual).expect("actual vectors serialize")
        );
    }
    assert_eq!(actual, expected);

    let production = VectorFile {
        version: SETUP_PROTOCOL_VERSION,
        vectors: expected
            .vectors
            .iter()
            .map(build_production_vector)
            .collect(),
    };
    assert_eq!(production, expected);
}

fn vector_is_unfilled(vector: &ExpectedVector) -> bool {
    vector.proposal_hash.is_empty()
}

fn build_vector(
    name: &str,
    member_count: u16,
    signature_threshold: u16,
    consensus_threshold: u16,
) -> ExpectedVector {
    let setup_id = labeled_hash(b"setup-id", name.as_bytes());
    let coordinator_secret = secret_key(65_000);
    let coordinator_public_key = coordinator_secret.public_key();
    let coordinator_url = format!("https://setup.{}.example", name.replace("-of-", "of"));

    let mut enrollments = Vec::with_capacity(usize::from(member_count));
    for member_id in 1..=member_count {
        enrollments.push(enrollment(name, setup_id, member_id));
    }

    let first_invitation_id = invitation_id(setup_id, 1);
    let first_capability = invitation_capability(setup_id, 1);
    let invitation_capability_hash =
        capability_hash(setup_id, first_invitation_id, 1, first_capability);
    let invitation_public = invitation_public_body(
        setup_id,
        first_invitation_id,
        1,
        &coordinator_url,
        &coordinator_public_key.to_bytes(),
    );
    let invitation_checksum = hash_transcript(
        INVITATION_CHECKSUM_DOMAIN,
        &[&length_prefixed(&invitation_public), &first_capability],
    );

    let proposal = proposal_body(
        setup_id,
        &coordinator_public_key.to_bytes(),
        &enrollments,
        member_count,
        signature_threshold,
        consensus_threshold,
    );
    let proposal_hash = hash_transcript(PROPOSAL_DOMAIN, &[&length_prefixed(&proposal)]);
    let ceremony_id = hash_transcript(CEREMONY_ID_DOMAIN, &[&setup_id, &proposal_hash]);

    let coordinator_proposal_transcript = proposal_publication_transcript(setup_id, proposal_hash);
    let coordinator_proposal_signature = coordinator_secret
        .sign_no_aux_rand(&coordinator_proposal_transcript)
        .expect("coordinator proposal fixture signs");
    coordinator_public_key
        .verify(
            &coordinator_proposal_transcript,
            &coordinator_proposal_signature,
        )
        .expect("coordinator proposal fixture verifies");

    let approvals = (1..=member_count)
        .map(|member_id| approval(setup_id, proposal_hash, member_id))
        .collect::<Vec<_>>();
    let manifest_certification_hash = manifest_hash(setup_id, proposal_hash, &approvals);
    let coordinator_manifest_transcript =
        manifest_publication_transcript(setup_id, proposal_hash, manifest_certification_hash);
    let coordinator_manifest_signature = coordinator_secret
        .sign_no_aux_rand(&coordinator_manifest_transcript)
        .expect("coordinator manifest fixture signs");
    coordinator_public_key
        .verify(
            &coordinator_manifest_transcript,
            &coordinator_manifest_signature,
        )
        .expect("coordinator manifest fixture verifies");

    for enrollment in &enrollments {
        enrollment
            .public_key
            .verify(&enrollment.signature_transcript, &enrollment.signature)
            .expect("enrollment fixture signature verifies");
    }
    for approval in &approvals {
        approval
            .public_key
            .verify(&approval.signature_transcript, &approval.signature)
            .expect("approval fixture signature verifies");
    }

    ExpectedVector {
        name: name.to_string(),
        member_count,
        signature_threshold,
        consensus_threshold,
        proposal_canonical_bytes: proposal.len(),
        coordinator_public_key: coordinator_public_key.to_hex(),
        invitation_capability_hash: hex(invitation_capability_hash),
        invitation_checksum: hex(&invitation_checksum[..6]),
        first_enrollment_signature: enrollments[0].signature.to_string(),
        proposal_hash: hex(proposal_hash),
        setup_authorization_hash: hex(proposal_hash),
        ceremony_id: hex(ceremony_id),
        coordinator_proposal_signature: coordinator_proposal_signature.to_string(),
        first_approval_signature: approvals[0].signature.to_string(),
        manifest_certification_hash: hex(manifest_certification_hash),
        coordinator_manifest_signature: coordinator_manifest_signature.to_string(),
    }
}

fn build_production_vector(expected: &ExpectedVector) -> ExpectedVector {
    let setup_id_bytes = labeled_hash(b"setup-id", expected.name.as_bytes());
    let setup_id = FederationSetupId::from_bytes(setup_id_bytes);
    let coordinator_secret = secret_key(65_000);
    let coordinator_identity =
        FederationSetupCoordinatorPublicKey::new(coordinator_secret.public_key())
            .expect("coordinator fixture identity");
    let url_policy = FederationSetupUrlPolicy::RequireHttps;
    let slug = expected.name.replace("-of-", "of");
    let coordinator_url =
        MintUrl::from_str(&format!("https://setup.{slug}.example")).expect("coordinator URL");

    let mut enrollments = BTreeMap::new();
    for member in 1..=expected.member_count {
        let member_id = MemberId::new(member).expect("fixture member id");
        let invitation_id =
            FederationSetupInvitationId::from_bytes(invitation_id(setup_id_bytes, member));
        let enrollment = FederationSetupEnrollment::sign_no_aux_rand_for_test_vector(
            setup_id,
            invitation_id,
            member_id,
            MintUrl::from_str(&format!("https://mint-{member}.{slug}.example")).expect("mint URL"),
            MintUrl::from_str(&format!("https://federation-{member}.{slug}.example"))
                .expect("federation URL"),
            member_nonce(setup_id_bytes, member),
            FederationSetupProtocolVersions::default(),
            &secret_key(member),
            url_policy,
        )
        .expect("production enrollment fixture");
        assert!(enrollments.insert(member_id, enrollment).is_none());
    }

    let keyset = FederationDkgKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        [Amount::from(1), Amount::from(2), Amount::from(4)]
            .into_iter()
            .collect::<BTreeSet<_>>(),
    )
    .expect("production keyset fixture");
    let network = FederationSetupBitcoinNetwork::Signet;
    let frost = FederationSetupFrostPolicy::new(
        1,
        vec![FederationSetupFrostApplication::CdkOnchain {
            application_key_epoch: 1,
            confirmation_policy_version: 1,
            minimum_confirmations: 1,
            fee_policy_version: 1,
        }],
        network,
    )
    .expect("production FROST fixture");
    let proposal_body = FederationSetupProposalBody::new(
        setup_id,
        coordinator_identity,
        enrollments,
        ThresholdParams::new(
            expected.member_count,
            expected.signature_threshold,
            expected.consensus_threshold,
        )
        .expect("production threshold fixture"),
        FederationSetupProtocolVersions::default(),
        network,
        FederationPaymentObservationPolicy::member_quorum(expected.consensus_threshold),
        FederationCheckpointPolicy::default(),
        vec![keyset],
        frost,
        url_policy,
    )
    .expect("production proposal body fixture");
    let proposal_canonical_bytes = proposal_body
        .canonical_bytes()
        .expect("production proposal bytes")
        .len();
    let proposal = FederationSetupProposal::sign_no_aux_rand_for_test_vector(
        proposal_body,
        &coordinator_secret,
        url_policy,
    )
    .expect("production proposal fixture");

    let mut approvals = BTreeMap::new();
    for member in 1..=expected.member_count {
        let member_id = MemberId::new(member).expect("fixture member id");
        let approval = FederationSetupApproval::sign_no_aux_rand_for_test_vector(
            &proposal,
            member_id,
            &secret_key(member),
            url_policy,
        )
        .expect("production approval fixture");
        assert!(approvals.insert(member_id, approval).is_none());
    }
    let first_enrollment_signature = proposal
        .body
        .enrollments
        .get(&MemberId::new(1).expect("first member"))
        .expect("first enrollment")
        .signature
        .to_string();
    let first_approval_signature = approvals
        .get(&MemberId::new(1).expect("first member"))
        .expect("first approval")
        .signature
        .to_string();
    let manifest = FederationSetupManifest::certify_no_aux_rand_for_test_vector(
        proposal,
        approvals,
        &coordinator_secret,
        url_policy,
    )
    .expect("production manifest fixture");
    let setup_config = manifest
        .setup_public_config()
        .expect("manifest derives setup public config");
    assert_eq!(
        setup_config.setup_authorization,
        manifest
            .setup_authorization_hash()
            .expect("setup authorization")
    );

    let first_invitation_id =
        FederationSetupInvitationId::from_bytes(invitation_id(setup_id_bytes, 1));
    let first_capability =
        FederationSetupInvitationCapability::from_bytes(invitation_capability(setup_id_bytes, 1))
            .expect("fixture invitation capability");
    let invitation_capability_hash = first_capability.capability_hash(
        setup_id,
        first_invitation_id,
        MemberId::new(1).expect("first member"),
    );
    let invitation_public = invitation_public_body(
        setup_id_bytes,
        first_invitation_id.to_bytes(),
        1,
        &coordinator_url.to_string(),
        &coordinator_identity.public_key().to_bytes(),
    );
    let first_capability_bytes = first_capability.expose_secret();
    let invitation_checksum = hash_transcript(
        INVITATION_CHECKSUM_DOMAIN,
        &[
            &length_prefixed(&invitation_public),
            &first_capability_bytes,
        ],
    );

    ExpectedVector {
        name: expected.name.clone(),
        member_count: expected.member_count,
        signature_threshold: expected.signature_threshold,
        consensus_threshold: expected.consensus_threshold,
        proposal_canonical_bytes,
        coordinator_public_key: coordinator_identity.to_string(),
        invitation_capability_hash: hex(invitation_capability_hash.to_bytes()),
        invitation_checksum: hex(&invitation_checksum[..6]),
        first_enrollment_signature,
        proposal_hash: manifest.proposal.proposal_hash.to_string(),
        setup_authorization_hash: manifest
            .setup_authorization_hash()
            .expect("setup authorization")
            .to_string(),
        ceremony_id: manifest.ceremony_id().expect("ceremony id").to_string(),
        coordinator_proposal_signature: manifest.proposal.coordinator_signature.to_string(),
        first_approval_signature,
        manifest_certification_hash: manifest.manifest_hash.to_string(),
        coordinator_manifest_signature: manifest.coordinator_signature.to_string(),
    }
}

fn enrollment(name: &str, setup_id: [u8; 32], member_id: u16) -> EnrollmentRecord {
    let identity_secret = secret_key(member_id);
    let public_key = identity_secret.public_key();
    let invitation_id = invitation_id(setup_id, member_id);
    let nonce = member_nonce(setup_id, member_id);
    let slug = name.replace("-of-", "of");
    let mut body = Vec::new();
    body.extend_from_slice(&SETUP_PROTOCOL_VERSION.to_be_bytes());
    body.extend_from_slice(&setup_id);
    body.extend_from_slice(&invitation_id);
    body.extend_from_slice(&member_id.to_be_bytes());
    append_bytes(
        &mut body,
        format!("https://mint-{member_id}.{slug}.example").as_bytes(),
    );
    append_bytes(
        &mut body,
        format!("https://federation-{member_id}.{slug}.example").as_bytes(),
    );
    body.extend_from_slice(&public_key.to_bytes());
    body.extend_from_slice(&nonce);
    body.extend_from_slice(&DKG_PROTOCOL_VERSION.to_be_bytes());
    body.extend_from_slice(&FEDERATION_REST_API_VERSION.to_be_bytes());
    body.extend_from_slice(&WALLET_PROTOCOL_VERSION.to_be_bytes());
    body.extend_from_slice(&BLS_KEYSET_VERSION.to_be_bytes());
    body.extend_from_slice(&FROST_DKG_PROTOCOL_VERSION.to_be_bytes());
    body.extend_from_slice(&FROST_CIPHERSUITE_VERSION.to_be_bytes());

    let signature_transcript = transcript(ENROLLMENT_SIGNATURE_DOMAIN, &[&length_prefixed(&body)]);
    let signature = identity_secret
        .sign_no_aux_rand(&signature_transcript)
        .expect("enrollment fixture signs");
    let mut canonical = body;
    canonical.extend_from_slice(signature.as_ref());
    EnrollmentRecord {
        canonical,
        public_key,
        signature,
        signature_transcript,
    }
}

fn proposal_body(
    setup_id: [u8; 32],
    coordinator_public_key: &[u8],
    enrollments: &[EnrollmentRecord],
    member_count: u16,
    signature_threshold: u16,
    consensus_threshold: u16,
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&SETUP_PROTOCOL_VERSION.to_be_bytes());
    body.extend_from_slice(&setup_id);
    body.extend_from_slice(coordinator_public_key);
    body.extend_from_slice(&member_count.to_be_bytes());
    for enrollment in enrollments {
        append_bytes(&mut body, &enrollment.canonical);
    }
    body.extend_from_slice(&member_count.to_be_bytes());
    body.extend_from_slice(&signature_threshold.to_be_bytes());
    body.extend_from_slice(&consensus_threshold.to_be_bytes());
    for version in [
        SETUP_PROTOCOL_VERSION,
        DKG_PROTOCOL_VERSION,
        FEDERATION_REST_API_VERSION,
        WALLET_PROTOCOL_VERSION,
        BLS_KEYSET_VERSION,
        FROST_DKG_PROTOCOL_VERSION,
        FROST_CIPHERSUITE_VERSION,
    ] {
        body.extend_from_slice(&version.to_be_bytes());
    }
    body.push(3); // signet
    append_member_quorum_policy(&mut body, consensus_threshold);
    append_member_quorum_policy(&mut body, consensus_threshold);
    body.extend_from_slice(&10_000_u64.to_be_bytes());
    body.extend_from_slice(&10_000_u64.to_be_bytes());
    body.extend_from_slice(&(32_u64 * 1024 * 1024).to_be_bytes());
    body.extend_from_slice(&1_u16.to_be_bytes());
    append_bytes(&mut body, b"sat");
    body.push(1);
    body.extend_from_slice(&0_u64.to_be_bytes());
    body.push(0);
    body.extend_from_slice(&3_u16.to_be_bytes());
    for amount in [1_u64, 2, 4] {
        body.extend_from_slice(&amount.to_be_bytes());
    }
    body.extend_from_slice(&1_u64.to_be_bytes());
    body.extend_from_slice(&1_u16.to_be_bytes());
    body.push(1); // CDK on-chain application
    body.extend_from_slice(&1_u64.to_be_bytes());
    body.extend_from_slice(&1_u16.to_be_bytes());
    body.extend_from_slice(&1_u32.to_be_bytes());
    body.extend_from_slice(&1_u16.to_be_bytes());
    body
}

fn append_member_quorum_policy(output: &mut Vec<u8>, required: u16) {
    output.push(1);
    output.extend_from_slice(&required.to_be_bytes());
}

fn approval(setup_id: [u8; 32], proposal_hash: [u8; 32], member_id: u16) -> ApprovalRecord {
    let identity_secret = secret_key(member_id);
    let signature_transcript = transcript(
        APPROVAL_DOMAIN,
        &[
            &setup_id,
            &proposal_hash,
            &member_id.to_be_bytes(),
            &SETUP_PROTOCOL_VERSION.to_be_bytes(),
        ],
    );
    let signature = identity_secret
        .sign_no_aux_rand(&signature_transcript)
        .expect("approval fixture signs");
    ApprovalRecord {
        member_id,
        public_key: identity_secret.public_key(),
        signature,
        signature_transcript,
    }
}

fn proposal_publication_transcript(setup_id: [u8; 32], proposal_hash: [u8; 32]) -> Vec<u8> {
    transcript(
        PROPOSAL_PUBLICATION_DOMAIN,
        &[
            &setup_id,
            &proposal_hash,
            &SETUP_PROTOCOL_VERSION.to_be_bytes(),
        ],
    )
}

fn manifest_hash(
    setup_id: [u8; 32],
    proposal_hash: [u8; 32],
    approvals: &[ApprovalRecord],
) -> [u8; 32] {
    let mut body = Vec::new();
    body.extend_from_slice(&setup_id);
    body.extend_from_slice(&proposal_hash);
    body.extend_from_slice(&(approvals.len() as u16).to_be_bytes());
    for approval in approvals {
        body.extend_from_slice(&approval.member_id.to_be_bytes());
        body.extend_from_slice(approval.signature.as_ref());
    }
    hash_transcript(MANIFEST_DOMAIN, &[&body])
}

fn manifest_publication_transcript(
    setup_id: [u8; 32],
    proposal_hash: [u8; 32],
    manifest_hash: [u8; 32],
) -> Vec<u8> {
    transcript(
        MANIFEST_PUBLICATION_DOMAIN,
        &[
            &setup_id,
            &proposal_hash,
            &manifest_hash,
            &SETUP_PROTOCOL_VERSION.to_be_bytes(),
        ],
    )
}

fn invitation_public_body(
    setup_id: [u8; 32],
    invitation_id: [u8; 32],
    member_id: u16,
    coordinator_url: &str,
    coordinator_public_key: &[u8],
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&SETUP_PROTOCOL_VERSION.to_be_bytes());
    body.extend_from_slice(&setup_id);
    body.extend_from_slice(&invitation_id);
    body.extend_from_slice(&member_id.to_be_bytes());
    append_bytes(&mut body, coordinator_url.as_bytes());
    body.extend_from_slice(coordinator_public_key);
    body.extend_from_slice(&2_000_000_000_u64.to_be_bytes());
    body
}

fn capability_hash(
    setup_id: [u8; 32],
    invitation_id: [u8; 32],
    member_id: u16,
    capability: [u8; 32],
) -> [u8; 32] {
    hash_transcript(
        INVITATION_CAPABILITY_DOMAIN,
        &[
            &setup_id,
            &invitation_id,
            &member_id.to_be_bytes(),
            &capability,
        ],
    )
}

fn invitation_id(setup_id: [u8; 32], member_id: u16) -> [u8; 32] {
    labeled_hash(
        b"invitation-id",
        &[setup_id.as_slice(), &member_id.to_be_bytes()].concat(),
    )
}

fn invitation_capability(setup_id: [u8; 32], member_id: u16) -> [u8; 32] {
    labeled_hash(
        b"invitation-capability",
        &[setup_id.as_slice(), &member_id.to_be_bytes()].concat(),
    )
}

fn member_nonce(setup_id: [u8; 32], member_id: u16) -> [u8; 32] {
    labeled_hash(
        b"member-nonce",
        &[setup_id.as_slice(), &member_id.to_be_bytes()].concat(),
    )
}

fn secret_key(value: u16) -> SecretKey {
    let mut bytes = [0_u8; 32];
    bytes[30..].copy_from_slice(&value.to_be_bytes());
    SecretKey::from_slice(&bytes).expect("fixture scalar is a valid non-zero secret key")
}

fn labeled_hash(label: &[u8], value: &[u8]) -> [u8; 32] {
    let mut bytes = Vec::new();
    append_bytes(&mut bytes, b"cdk/federation/setup/test-vector/v1");
    append_bytes(&mut bytes, label);
    append_bytes(&mut bytes, value);
    Sha256Hash::hash(&bytes).to_byte_array()
}

fn hash_transcript(domain: &[u8], fields: &[&[u8]]) -> [u8; 32] {
    Sha256Hash::hash(&transcript(domain, fields)).to_byte_array()
}

fn transcript(domain: &[u8], fields: &[&[u8]]) -> Vec<u8> {
    let mut output = Vec::new();
    append_bytes(&mut output, domain);
    for field in fields {
        output.extend_from_slice(field);
    }
    output
}

fn length_prefixed(value: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    append_bytes(&mut output, value);
    output
}

fn append_bytes(output: &mut Vec<u8>, value: &[u8]) {
    let length = u32::try_from(value.len()).expect("fixture field length fits u32");
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

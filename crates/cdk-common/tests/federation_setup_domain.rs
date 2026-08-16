//! Multi-operator federation setup domain validation tests.

#![cfg(feature = "federation")]

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use bitcoin::base64::Engine as _;
use bitcoin::hashes::{sha256, Hash};
use cdk_common::federation::{
    FederationCheckpointPolicy, FederationConfig, FederationDkgCeremonyId, FederationDkgKeysetSpec,
    FederationError, FederationFrostApplicationId, FederationFrostDkgContext,
    FederationPaymentObservationPolicy, FederationSetupApproval, FederationSetupAuthorizationHash,
    FederationSetupBitcoinNetwork, FederationSetupCoordinatorPublicKey, FederationSetupEnrollment,
    FederationSetupFrostApplication, FederationSetupFrostPolicy, FederationSetupId,
    FederationSetupInvitation, FederationSetupInvitationCapability, FederationSetupInvitationId,
    FederationSetupInvitationRecord, FederationSetupLocalRole, FederationSetupManifest,
    FederationSetupManifestHash, FederationSetupProposal, FederationSetupProposalBody,
    FederationSetupProposalReview, FederationSetupProtocolVersions, FederationSetupSession,
    FederationSetupUrlPolicy, MemberId, ThresholdParams, FEDERATION_MAX_SETUP_INVITATION_BYTES,
    FEDERATION_MAX_SETUP_URL_BYTES, FEDERATION_SETUP_INVITATION_PREFIX,
    FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::nut01::SecretKey;
use cdk_common::{Amount, CurrencyUnit};

const SETUP_ID_BYTES: [u8; 32] = [7; 32];
const COORDINATOR_SECRET: u16 = 60_000;
const NOW_UNIX: u64 = 1_900_000_000;
const URL_POLICY: FederationSetupUrlPolicy = FederationSetupUrlPolicy::RequireSecureTransport;

#[test]
fn frost_policy_does_not_require_the_cdk_onchain_application() {
    let empty =
        FederationSetupFrostPolicy::new(1, Vec::new(), FederationSetupBitcoinNetwork::Regtest)
            .expect("wallet-free FROST root policy");
    assert!(empty.applications.is_empty());

    let extension = FederationSetupFrostApplication::Extension {
        application_id: FederationFrostApplicationId::from_bytes([42; 32])
            .expect("extension application ID"),
        application_key_epoch: 1,
        derivation_version: 1,
        authorization_policy: [43; 32],
        configuration: serde_json::json!({"backend": "independent"}),
    };
    let extension_only = FederationSetupFrostPolicy::new(
        1,
        vec![extension.clone()],
        FederationSetupBitcoinNetwork::Regtest,
    )
    .expect("extension-only FROST policy");
    assert_eq!(extension_only.applications, vec![extension]);
}

#[test]
fn constructors_canonicalize_keysets_and_manifest_round_trips() {
    let enrollments = enrollments();
    let mut reversed_keysets = keysets();
    reversed_keysets.reverse();

    let canonical = proposal_body(enrollments.clone(), thresholds(), versions(), keysets())
        .expect("canonical proposal body");
    let reordered = proposal_body(enrollments, thresholds(), versions(), reversed_keysets)
        .expect("reordered proposal body");
    assert_eq!(canonical, reordered);
    assert_eq!(
        canonical.canonical_bytes().expect("canonical bytes"),
        reordered.canonical_bytes().expect("reordered bytes")
    );

    let manifest = manifest_from_body(canonical);
    let encoded = serde_json::to_vec(&manifest).expect("manifest serializes");
    let decoded: FederationSetupManifest =
        serde_json::from_slice(&encoded).expect("manifest deserializes");
    assert_eq!(decoded, manifest);
    decoded.validate(URL_POLICY).expect("round trip validates");
}

#[test]
fn proposal_and_manifest_tampering_fail_closed() {
    let fixture = fixture();

    let mut tampered_proposal = fixture.proposal.clone();
    tampered_proposal
        .body
        .checkpoint_policy
        .checkpoint_interval_journal_operations += 1;
    assert!(matches!(
        tampered_proposal.validate(URL_POLICY),
        Err(FederationError::SetupProposalHashMismatch { .. })
    ));

    let mut tampered_approval = fixture
        .approvals
        .get(&member_id(1))
        .expect("member one approval")
        .clone();
    tampered_approval.signature = fixture
        .approvals
        .get(&member_id(2))
        .expect("member two approval")
        .signature;
    assert!(matches!(
        tampered_approval.validate(&fixture.proposal),
        Err(FederationError::SetupSignatureVerificationFailed {
            context: "approval"
        })
    ));

    let mut tampered_manifest = fixture.manifest;
    tampered_manifest.manifest_hash = FederationSetupManifestHash::from_bytes([9; 32]);
    assert!(matches!(
        tampered_manifest.validate(URL_POLICY),
        Err(FederationError::SetupManifestHashMismatch { .. })
    ));
}

#[test]
fn wrong_signer_and_incomplete_or_extra_approvals_are_rejected() {
    let fixture = fixture();
    let wrong_signer = FederationSetupApproval::sign_no_aux_rand_for_test_vector(
        &fixture.proposal,
        member_id(1),
        &secret_key(2),
        URL_POLICY,
    );
    assert_invalid_setup(
        wrong_signer.expect_err("wrong signer must fail"),
        "approval secret",
    );

    let mut missing = fixture.approvals.clone();
    missing.remove(&member_id(3));
    let missing_result = FederationSetupManifest::certify_no_aux_rand_for_test_vector(
        fixture.proposal.clone(),
        missing,
        &secret_key(COORDINATOR_SECRET),
        URL_POLICY,
    );
    assert_invalid_setup(
        missing_result.expect_err("missing approval must fail"),
        "exactly one approval",
    );

    let mut extra = fixture.approvals;
    let extra_member = member_id(4);
    let mut extra_approval = extra
        .get(&member_id(1))
        .expect("member one approval")
        .clone();
    extra_approval.member_id = extra_member;
    assert!(extra.insert(extra_member, extra_approval).is_none());
    let extra_result = FederationSetupManifest::certify_no_aux_rand_for_test_vector(
        fixture.proposal,
        extra,
        &secret_key(COORDINATOR_SECRET),
        URL_POLICY,
    );
    assert_invalid_setup(
        extra_result.expect_err("extra approval must fail"),
        "exactly one approval",
    );
}

#[test]
fn duplicate_identity_url_member_binding_and_invitation_are_rejected() {
    let base = enrollments();

    let mut duplicate_identity = base.clone();
    duplicate_identity.insert(
        member_id(2),
        signed_enrollment(2, 1, 2, "https://mint-2.example", "https://peer-2.example"),
    );
    assert_invalid_setup(
        proposal_body(duplicate_identity, thresholds(), versions(), keysets())
            .expect_err("duplicate identity must fail"),
        "duplicate member identity",
    );

    let mut duplicate_url = base.clone();
    duplicate_url.insert(
        member_id(2),
        signed_enrollment(2, 2, 2, "https://mint-1.example", "https://peer-2.example"),
    );
    assert_invalid_setup(
        proposal_body(duplicate_url, thresholds(), versions(), keysets())
            .expect_err("duplicate URL must fail"),
        "duplicate member URL",
    );

    let mut duplicate_member_binding = base.clone();
    duplicate_member_binding.insert(
        member_id(2),
        signed_enrollment(
            1,
            4,
            4,
            "https://mint-other.example",
            "https://peer-other.example",
        ),
    );
    assert_invalid_setup(
        proposal_body(
            duplicate_member_binding,
            thresholds(),
            versions(),
            keysets(),
        )
        .expect_err("duplicate member binding must fail"),
        "enrollment binding mismatch",
    );

    let mut duplicate_invitation = base;
    duplicate_invitation.insert(
        member_id(2),
        signed_enrollment(2, 2, 1, "https://mint-2.example", "https://peer-2.example"),
    );
    assert_invalid_setup(
        proposal_body(duplicate_invitation, thresholds(), versions(), keysets())
            .expect_err("duplicate invitation must fail"),
        "duplicate invitation ID",
    );
}

#[test]
fn secure_setup_policy_accepts_iroh_and_allows_one_shared_member_endpoint() {
    let shared = mint_url("iroh://member-one-endpoint");
    FederationSetupEnrollment::sign_no_aux_rand_for_test_vector(
        setup_id(),
        FederationSetupInvitationId::from_bytes([1; 32]),
        member_id(1),
        shared.clone(),
        shared,
        [11; 32],
        versions(),
        &secret_key(1),
        URL_POLICY,
    )
    .expect("one Iroh endpoint may serve public and federation routes");

    let mut shared_endpoint_enrollments = enrollments();
    shared_endpoint_enrollments.insert(
        member_id(1),
        signed_enrollment(
            1,
            1,
            1,
            "iroh://member-one-endpoint",
            "iroh://member-one-endpoint",
        ),
    );
    proposal_body(
        shared_endpoint_enrollments,
        thresholds(),
        versions(),
        keysets(),
    )
    .expect("proposal permits one member to share its Iroh endpoint");

    let shared_https = mint_url("https://member-one.example");
    let duplicate_https = FederationSetupEnrollment::sign_no_aux_rand_for_test_vector(
        setup_id(),
        FederationSetupInvitationId::from_bytes([1; 32]),
        member_id(1),
        shared_https.clone(),
        shared_https,
        [11; 32],
        versions(),
        &secret_key(1),
        URL_POLICY,
    )
    .expect_err("one HTTPS URL cannot collapse public and private route roles");
    assert_invalid_setup(duplicate_https, "must be distinct");

    let iroh_with_port = FederationSetupEnrollment::sign_no_aux_rand_for_test_vector(
        setup_id(),
        FederationSetupInvitationId::from_bytes([1; 32]),
        member_id(1),
        mint_url("iroh://member-one-endpoint:443"),
        mint_url("iroh://member-one-federation"),
        [11; 32],
        versions(),
        &secret_key(1),
        URL_POLICY,
    )
    .expect_err("Iroh endpoint URLs do not carry TCP ports");
    assert_invalid_setup(iroh_with_port, "HTTPS or Iroh");
}

#[test]
fn unsafe_threshold_incompatible_version_and_oversized_url_are_rejected() {
    let unsafe_threshold = ThresholdParams::new(3, 1, 2).expect("structurally valid threshold");
    assert!(matches!(
        proposal_body(enrollments(), unsafe_threshold, versions(), keysets()),
        Err(FederationError::UnsafeThresholdProfile { .. })
    ));

    let mut incompatible = versions();
    incompatible.bls_dkg += 1;
    assert!(matches!(
        incompatible.validate(),
        Err(FederationError::SetupVersionMismatch {
            component: "bls_dkg",
            ..
        })
    ));

    let oversized_url = MintUrl::from_str(&format!(
        "https://coordinator.example/{}",
        "a".repeat(FEDERATION_MAX_SETUP_URL_BYTES)
    ))
    .expect("oversized URL remains syntactically valid");
    let invitation = FederationSetupInvitation::new(
        setup_id(),
        invitation_id(1),
        member_id(1),
        oversized_url,
        coordinator_identity(),
        NOW_UNIX + FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
        invitation_capability(),
        NOW_UNIX,
        URL_POLICY,
    );
    assert!(matches!(
        invitation,
        Err(FederationError::SetupSizeLimitExceeded {
            artifact: "coordinator",
            ..
        })
    ));
}

#[test]
fn invitation_validation_is_expiry_aware_and_diagnostics_are_redacted() {
    let capability = invitation_capability();
    let invitation = FederationSetupInvitation::new(
        setup_id(),
        invitation_id(1),
        member_id(1),
        mint_url("https://coordinator.example"),
        coordinator_identity(),
        NOW_UNIX + FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
        capability,
        NOW_UNIX,
        URL_POLICY,
    )
    .expect("valid invitation");

    let capability_debug = format!("{capability:?}");
    let invitation_debug = format!("{invitation:?}");
    let secret_hex = "ab".repeat(32);
    assert!(capability_debug.contains("[REDACTED]"));
    assert!(invitation_debug.contains("[REDACTED]"));
    assert!(!capability_debug.contains(&secret_hex));
    assert!(!invitation_debug.contains(&secret_hex));

    invitation
        .validate(
            NOW_UNIX + FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS - 1,
            URL_POLICY,
        )
        .expect("an invitation remains valid until its expiry");
    assert!(invitation
        .validate(
            NOW_UNIX + FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
            URL_POLICY,
        )
        .is_err());
}

#[test]
fn invitation_code_round_trips_and_rejects_malformed_or_tampered_input() {
    let invitation = FederationSetupInvitation::new(
        setup_id(),
        invitation_id(2),
        member_id(2),
        mint_url("https://coordinator.example/setup"),
        coordinator_identity(),
        NOW_UNIX + FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
        invitation_capability(),
        NOW_UNIX,
        URL_POLICY,
    )
    .expect("valid invitation");
    let code = invitation.to_string();
    let decoded = FederationSetupInvitation::from_str(&code).expect("invitation parses");

    assert_eq!(decoded, invitation);
    assert_eq!(decoded.to_string(), code);
    decoded
        .validate(NOW_UNIX, URL_POLICY)
        .expect("parsed invitation validates at receipt time");

    let wrong_prefix = code.replacen(FEDERATION_SETUP_INVITATION_PREFIX, "cashufedsetupB", 1);
    assert_invalid_setup(
        FederationSetupInvitation::from_str(&wrong_prefix)
            .expect_err("unknown invitation prefix must fail"),
        "prefix",
    );

    let payload = code
        .strip_prefix(FEDERATION_SETUP_INVITATION_PREFIX)
        .expect("fixture prefix");
    let mut bytes = bitcoin::base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("fixture payload decodes");
    let final_byte = bytes.last_mut().expect("checksum byte");
    *final_byte ^= 1;
    let tampered = format!(
        "{}{}",
        FEDERATION_SETUP_INVITATION_PREFIX,
        bitcoin::base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    );
    assert_invalid_setup(
        FederationSetupInvitation::from_str(&tampered).expect_err("tampered invitation must fail"),
        "checksum",
    );

    assert!(FederationSetupInvitation::from_str(
        &"x".repeat(FEDERATION_MAX_SETUP_INVITATION_BYTES + 1)
    )
    .is_err());
    assert!(decoded
        .validate(
            NOW_UNIX + FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
            URL_POLICY,
        )
        .is_err());
}

#[test]
fn durable_setup_timestamps_fit_both_sql_backends() {
    let session = FederationSetupSession::new(
        setup_id(),
        FederationSetupLocalRole::Member {
            member_id: member_id(1),
        },
        u64::MAX,
    );
    assert_invalid_setup(
        session.expect_err("oversized session timestamp must fail"),
        "portable setup persistence range",
    );

    let invitation = FederationSetupInvitation::new(
        setup_id(),
        invitation_id(1),
        member_id(1),
        mint_url("https://coordinator.example"),
        coordinator_identity(),
        NOW_UNIX + FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
        invitation_capability(),
        NOW_UNIX,
        URL_POLICY,
    )
    .expect("valid invitation");
    let mut record = FederationSetupInvitationRecord::from_invitation(&invitation, NOW_UNIX)
        .expect("valid durable invitation record");
    record.expires_at_unix = u64::MAX;
    assert_invalid_setup(
        record
            .validate()
            .expect_err("oversized invitation timestamp must fail"),
        "portable setup persistence range",
    );
}

#[test]
fn proposal_review_is_stable_bounded_and_secret_free() {
    let fixture = fixture();
    let review = FederationSetupProposalReview::from_proposal(&fixture.proposal, URL_POLICY)
        .expect("proposal review");
    let human = review.to_string();
    let json = review.to_pretty_json().expect("review JSON");
    let decoded: FederationSetupProposalReview =
        serde_json::from_str(&json).expect("review JSON round trip");

    assert_eq!(decoded, review);
    assert!(human.contains(&format!(
        "proposal_hash: {}",
        fixture.proposal.proposal_hash
    )));
    assert!(human.contains("thresholds: members=3 signatures=2 consensus=3"));
    assert!(human.contains("member=1 mint=https://mint-1.example"));
    assert!(!human.contains(&fixture.proposal.coordinator_signature.to_string()));
    assert!(!json.contains(&fixture.proposal.coordinator_signature.to_string()));
    assert_eq!(
        sha256::Hash::hash(human.as_bytes()).to_string(),
        "4e2ab10f68efbfffd5b285bc9671f84ec78bc9bb4c9749d6950363e9406a83a9"
    );
}

#[test]
fn manifest_derives_exact_config_and_authorization_is_mandatory() {
    let fixture = fixture();
    let setup_config = fixture
        .manifest
        .setup_public_config()
        .expect("manifest derives setup config");
    let authorization = fixture
        .manifest
        .setup_authorization_hash()
        .expect("manifest authorization");
    let mut expected = FederationConfig::new(
        authorization,
        fixture.proposal.body.threshold_params,
        fixture.proposal.body.member_configurations(),
        Vec::new(),
    )
    .expect("expected setup config");
    expected.payment_observation_policy = fixture.proposal.body.payment_observation_policy.clone();
    expected.checkpoint_policy = fixture.proposal.body.checkpoint_policy;
    assert_eq!(setup_config, expected);
    assert_eq!(setup_config.setup_authorization, authorization);
    assert_eq!(
        fixture.manifest.keyset_specs(),
        fixture.proposal.body.keysets
    );
    assert_eq!(
        fixture.manifest.frost_policy(),
        &fixture.proposal.body.frost
    );
    assert_ne!(
        fixture
            .manifest
            .ceremony_id()
            .expect("ceremony ID")
            .to_bytes(),
        authorization.to_bytes()
    );

    let mut different_policy = fixture.proposal.body.clone();
    different_policy
        .checkpoint_policy
        .checkpoint_interval_journal_operations += 1;
    assert_ne!(
        different_policy
            .setup_authorization_hash()
            .expect("alternate policy authorization"),
        authorization,
        "the same roster with a different policy must authorize a different DKG"
    );

    let mut different_setup = fixture.proposal.body.clone();
    different_setup.setup_id = FederationSetupId::from_bytes([8; 32]);
    assert_ne!(
        different_setup
            .setup_authorization_hash()
            .expect("alternate setup authorization"),
        authorization,
        "an otherwise identical proposal from another setup must not replay"
    );

    let alternate = FederationConfig::new(
        FederationSetupAuthorizationHash::from_bytes([99; 32]),
        setup_config.threshold_params,
        setup_config.members.clone(),
        Vec::new(),
    )
    .expect("alternate explicit authorization");
    assert_ne!(alternate.federation_id, setup_config.federation_id);

    let mut frost_context = FederationFrostDkgContext::new(
        &setup_config,
        FederationDkgCeremonyId::from_bytes([42; 32]),
        fixture.proposal.body.frost.root_key_epoch,
    )
    .expect("manifest-bound FROST context");
    frost_context.setup_authorization = FederationSetupAuthorizationHash::from_bytes([99; 32]);
    assert!(matches!(
        frost_context.validate_against(&setup_config),
        Err(FederationError::DkgTranscriptMismatch { .. })
    ));

    let mut encoded = serde_json::to_value(&setup_config).expect("config serializes");
    encoded
        .as_object_mut()
        .expect("config JSON object")
        .remove("setup_authorization");
    assert!(serde_json::from_value::<FederationConfig>(encoded).is_err());
}

#[test]
fn zero_setup_and_invitation_identifiers_are_rejected() {
    let zero_setup = FederationSetupInvitation::new(
        FederationSetupId::from_bytes([0; 32]),
        invitation_id(1),
        member_id(1),
        mint_url("https://coordinator.example"),
        coordinator_identity(),
        NOW_UNIX + FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
        invitation_capability(),
        NOW_UNIX,
        URL_POLICY,
    );
    assert_invalid_setup(zero_setup.expect_err("zero setup ID must fail"), "setup ID");

    let zero_invitation = FederationSetupInvitation::new(
        setup_id(),
        FederationSetupInvitationId::from_bytes([0; 32]),
        member_id(1),
        mint_url("https://coordinator.example"),
        coordinator_identity(),
        NOW_UNIX + FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
        invitation_capability(),
        NOW_UNIX,
        URL_POLICY,
    );
    assert_invalid_setup(
        zero_invitation.expect_err("zero invitation ID must fail"),
        "invitation ID",
    );
}

struct Fixture {
    proposal: FederationSetupProposal,
    approvals: BTreeMap<MemberId, FederationSetupApproval>,
    manifest: FederationSetupManifest,
}

fn fixture() -> Fixture {
    let body = proposal_body(enrollments(), thresholds(), versions(), keysets())
        .expect("valid proposal body");
    let proposal = FederationSetupProposal::sign_no_aux_rand_for_test_vector(
        body,
        &secret_key(COORDINATOR_SECRET),
        URL_POLICY,
    )
    .expect("valid proposal");
    let approvals = approvals(&proposal);
    let manifest = FederationSetupManifest::certify_no_aux_rand_for_test_vector(
        proposal.clone(),
        approvals.clone(),
        &secret_key(COORDINATOR_SECRET),
        URL_POLICY,
    )
    .expect("valid manifest");
    Fixture {
        proposal,
        approvals,
        manifest,
    }
}

fn manifest_from_body(body: FederationSetupProposalBody) -> FederationSetupManifest {
    let proposal = FederationSetupProposal::sign_no_aux_rand_for_test_vector(
        body,
        &secret_key(COORDINATOR_SECRET),
        URL_POLICY,
    )
    .expect("valid proposal");
    FederationSetupManifest::certify_no_aux_rand_for_test_vector(
        proposal.clone(),
        approvals(&proposal),
        &secret_key(COORDINATOR_SECRET),
        URL_POLICY,
    )
    .expect("valid manifest")
}

fn approvals(proposal: &FederationSetupProposal) -> BTreeMap<MemberId, FederationSetupApproval> {
    (1..=3)
        .map(|member| {
            let member_id = member_id(member);
            let approval = FederationSetupApproval::sign_no_aux_rand_for_test_vector(
                proposal,
                member_id,
                &secret_key(member),
                URL_POLICY,
            )
            .expect("valid approval");
            (member_id, approval)
        })
        .collect()
}

fn proposal_body(
    enrollments: BTreeMap<MemberId, FederationSetupEnrollment>,
    threshold_params: ThresholdParams,
    required_versions: FederationSetupProtocolVersions,
    keysets: Vec<FederationDkgKeysetSpec>,
) -> Result<FederationSetupProposalBody, FederationError> {
    FederationSetupProposalBody::new(
        setup_id(),
        coordinator_identity(),
        enrollments,
        threshold_params,
        required_versions,
        FederationSetupBitcoinNetwork::Signet,
        FederationPaymentObservationPolicy::member_quorum(threshold_params.consensus_threshold),
        FederationCheckpointPolicy::default(),
        keysets,
        FederationSetupFrostPolicy::new(
            1,
            vec![FederationSetupFrostApplication::CdkOnchain {
                application_key_epoch: 1,
                confirmation_policy_version: 1,
                minimum_confirmations: 1,
                fee_policy_version: 1,
            }],
            FederationSetupBitcoinNetwork::Signet,
        )
        .expect("valid FROST policy"),
        URL_POLICY,
    )
}

fn enrollments() -> BTreeMap<MemberId, FederationSetupEnrollment> {
    (1..=3)
        .map(|member| {
            (
                member_id(member),
                signed_enrollment(
                    member,
                    member,
                    u8::try_from(member).expect("small member ID"),
                    &format!("https://mint-{member}.example"),
                    &format!("https://peer-{member}.example"),
                ),
            )
        })
        .collect()
}

fn signed_enrollment(
    member: u16,
    secret_scalar: u16,
    invitation_byte: u8,
    public_url: &str,
    federation_url: &str,
) -> FederationSetupEnrollment {
    FederationSetupEnrollment::sign_no_aux_rand_for_test_vector(
        setup_id(),
        FederationSetupInvitationId::from_bytes([invitation_byte; 32]),
        member_id(member),
        mint_url(public_url),
        mint_url(federation_url),
        [u8::try_from(member).expect("small member ID") + 10; 32],
        versions(),
        &secret_key(secret_scalar),
        URL_POLICY,
    )
    .expect("valid enrollment")
}

fn keysets() -> Vec<FederationDkgKeysetSpec> {
    vec![
        FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [Amount::from(1), Amount::from(2)]
                .into_iter()
                .collect::<BTreeSet<_>>(),
        )
        .expect("sat keyset"),
        FederationDkgKeysetSpec::new(
            CurrencyUnit::Usd,
            true,
            1,
            None,
            [Amount::from(1), Amount::from(5)]
                .into_iter()
                .collect::<BTreeSet<_>>(),
        )
        .expect("USD keyset"),
    ]
}

fn thresholds() -> ThresholdParams {
    ThresholdParams::new(3, 2, 3).expect("safe 2-of-3 threshold")
}

fn versions() -> FederationSetupProtocolVersions {
    FederationSetupProtocolVersions::default()
}

fn setup_id() -> FederationSetupId {
    FederationSetupId::from_bytes(SETUP_ID_BYTES)
}

fn invitation_id(member: u8) -> FederationSetupInvitationId {
    FederationSetupInvitationId::from_bytes([member; 32])
}

fn invitation_capability() -> FederationSetupInvitationCapability {
    FederationSetupInvitationCapability::from_bytes([0xab; 32])
        .expect("non-zero invitation capability")
}

fn coordinator_identity() -> FederationSetupCoordinatorPublicKey {
    FederationSetupCoordinatorPublicKey::new(secret_key(COORDINATOR_SECRET).public_key())
        .expect("valid coordinator identity")
}

fn member_id(value: u16) -> MemberId {
    MemberId::new(value).expect("non-zero member ID")
}

fn mint_url(value: &str) -> MintUrl {
    MintUrl::from_str(value).expect("valid URL fixture")
}

fn secret_key(value: u16) -> SecretKey {
    let mut bytes = [0_u8; 32];
    bytes[30..].copy_from_slice(&value.to_be_bytes());
    SecretKey::from_slice(&bytes).expect("fixture scalar is a valid non-zero secret key")
}

fn assert_invalid_setup(error: FederationError, expected_reason: &str) {
    match error {
        FederationError::InvalidSetup { reason } => assert!(
            reason.contains(expected_reason),
            "expected `{expected_reason}` in `{reason}`"
        ),
        other => panic!("expected invalid setup error, got {other}"),
    }
}

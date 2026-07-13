use std::collections::BTreeMap;

use bitcoin::secp256k1::{schnorr, Message, PublicKey, Secp256k1};
use frost_secp256k1_tr as frost;
use frost_secp256k1_tr::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use sha2::{Digest, Sha256};

use crate::canonical::CanonicalRootKeyPackage;
use crate::{
    ApplicationAuthorization, ApplicationDerivationVersion, ApplicationId, ApplicationRegistration,
    AuthorizationRequest, Error, MemberId, NonceCommitmentSet, ParticipantKeyManager, RootKeyEpoch,
    RootPublicPackage, SignatureShare, SigningSessionId,
};

#[derive(Debug, Clone, Copy)]
struct WalletTestAdapter {
    registration: ApplicationRegistration,
}

impl ApplicationAuthorization for WalletTestAdapter {
    fn registration(&self) -> ApplicationRegistration {
        self.registration
    }

    fn authorize(&self, request: &AuthorizationRequest) -> Result<[u8; 32], Error> {
        authorize_payload(self.registration, request, b"wallet-test-policy/v1")
    }
}

#[derive(Debug, Clone, Copy)]
struct CompatibilityTestAdapter {
    registration: ApplicationRegistration,
}

impl ApplicationAuthorization for CompatibilityTestAdapter {
    fn registration(&self) -> ApplicationRegistration {
        self.registration
    }

    fn authorize(&self, request: &AuthorizationRequest) -> Result<[u8; 32], Error> {
        authorize_payload(self.registration, request, b"compatibility-policy/v1")
    }
}

fn authorize_payload(
    registration: ApplicationRegistration,
    request: &AuthorizationRequest,
    domain: &[u8],
) -> Result<[u8; 32], Error> {
    if request.application_id() != registration.id()
        || request.derivation_version() != registration.derivation_version()
        || request.payload().len() != 64
    {
        return Err(Error::Application(
            "test authorization metadata is invalid".to_string(),
        ));
    }
    let message: [u8; 32] = request.payload()[..32]
        .try_into()
        .map_err(|_| Error::Application("test message is malformed".to_string()))?;
    let proof: [u8; 32] = request.payload()[32..]
        .try_into()
        .map_err(|_| Error::Application("test proof is malformed".to_string()))?;
    let expected = authorization_proof(domain, registration, request.session_id(), &message);
    if proof != expected {
        return Err(Error::Application(
            "test authorization proof is invalid".to_string(),
        ));
    }
    Ok(message)
}

fn authorization_proof(
    domain: &[u8],
    registration: ApplicationRegistration,
    session: SigningSessionId,
    message: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"cdk/frost/test-authorization/v1");
    hasher.update((domain.len() as u16).to_be_bytes());
    hasher.update(domain);
    hasher.update(registration.id().to_bytes());
    hasher.update(registration.derivation_version().get().to_be_bytes());
    hasher.update(registration.authorization_policy());
    hasher.update(session.to_bytes());
    hasher.update(message);
    hasher.finalize().into()
}

fn wallet_registration() -> ApplicationRegistration {
    ApplicationRegistration::new(
        ApplicationId::from_label("org.cashubtc.cdk.test-wallet").expect("application ID"),
        ApplicationDerivationVersion::new(1).expect("version"),
        Sha256::digest(b"wallet-test-policy/v1").into(),
    )
    .expect("registration")
}

fn compatibility_registration() -> ApplicationRegistration {
    ApplicationRegistration::new(
        ApplicationId::from_label("org.cashubtc.cdk.compatibility-test").expect("application ID"),
        ApplicationDerivationVersion::new(7).expect("version"),
        Sha256::digest(b"compatibility-policy/v1").into(),
    )
    .expect("registration")
}

fn session(byte: u8) -> SigningSessionId {
    SigningSessionId::from_bytes([byte; 32]).expect("session")
}

fn authorized_request(
    registration: ApplicationRegistration,
    session: SigningSessionId,
    message: [u8; 32],
    domain: &[u8],
) -> AuthorizationRequest {
    let mut payload = message.to_vec();
    payload.extend_from_slice(&authorization_proof(
        domain,
        registration,
        session,
        &message,
    ));
    AuthorizationRequest::new(
        registration.id(),
        registration.derivation_version(),
        session,
        payload,
    )
    .expect("authorization request")
}

fn managers(max_signers: u16, threshold: u16, seed: u8) -> Vec<ParticipantKeyManager> {
    let mut rng = ChaCha20Rng::from_seed([seed; 32]);
    let (shares, public) = frost::keys::generate_with_dealer(
        max_signers,
        threshold,
        frost::keys::IdentifierList::Default,
        &mut rng,
    )
    .expect("trusted dealer test harness");
    let roster = (1..=max_signers)
        .map(|value| {
            let member = MemberId::new(value).expect("member");
            let identifier = frost::Identifier::try_from(value).expect("FROST identifier");
            (member, identifier)
        })
        .collect::<BTreeMap<_, _>>();
    let transcript = Sha256::digest(
        [
            b"cdk/frost/test-root-transcript/v1".as_slice(),
            &[seed],
            &max_signers.to_be_bytes(),
            &threshold.to_be_bytes(),
        ]
        .concat(),
    )
    .into();

    shares
        .into_iter()
        .map(|(identifier, secret)| {
            let key = frost::keys::KeyPackage::try_from(secret).expect("validated dealer share");
            let member = roster
                .iter()
                .find_map(|(member, candidate)| (*candidate == identifier).then_some(*member))
                .expect("member mapping");
            let root = CanonicalRootKeyPackage::from_library(
                member,
                &roster,
                &key,
                &public,
                RootKeyEpoch::new(1).expect("epoch"),
                transcript,
            )
            .expect("canonical root package");
            let mut manager = ParticipantKeyManager::new(root);
            manager
                .register_application(WalletTestAdapter {
                    registration: wallet_registration(),
                })
                .expect("wallet adapter");
            manager
                .register_application(CompatibilityTestAdapter {
                    registration: compatibility_registration(),
                })
                .expect("compatibility adapter");
            manager
        })
        .collect()
}

fn sign_with_subset(
    managers: &mut [ParticipantKeyManager],
    signer_indexes: &[usize],
    registration: ApplicationRegistration,
    session_id: SigningSessionId,
    message: [u8; 32],
    domain: &[u8],
    seed: u8,
) -> (
    crate::DerivedPublicPackage,
    NonceCommitmentSet,
    Vec<SignatureShare>,
    crate::AggregateSignature,
) {
    let public = managers[0]
        .public_package(registration.id())
        .expect("public package")
        .clone();
    let mut rng = ChaCha20Rng::from_seed([seed; 32]);
    let commitments = signer_indexes
        .iter()
        .map(|index| {
            managers[*index]
                .commit_nonce(registration.id(), session_id, &mut rng)
                .expect("nonce commitment")
        })
        .collect::<Vec<_>>();
    let set = NonceCommitmentSet::new(commitments.clone()).expect("commitment set");
    let request = authorized_request(registration, session_id, message, domain);
    let shares = signer_indexes
        .iter()
        .zip(commitments)
        .map(|(index, commitment)| {
            managers[*index]
                .sign_share(commitment.nonce_id(), &request, &set)
                .expect("signature share")
        })
        .collect::<Vec<_>>();
    let signature =
        ParticipantKeyManager::aggregate(&public, session_id, &message, &set, shares.clone())
            .expect("aggregate signature");
    (public, set, shares, signature)
}

fn verify_with_rust_bitcoin(
    public: &crate::DerivedPublicPackage,
    message: [u8; 32],
    signature: crate::AggregateSignature,
) {
    let secp = Secp256k1::verification_only();
    let full_key = PublicKey::from_slice(&public.group_key()).expect("compressed group key");
    let (x_only, _) = full_key.x_only_public_key();
    let signature =
        schnorr::Signature::from_slice(&signature.to_bytes()).expect("BIP340 signature");
    secp.verify_schnorr(&signature, &Message::from_digest(message), &x_only)
        .expect("rust-bitcoin BIP340 verification");
}

#[test]
fn threshold_matrix_and_bip340_interoperability() {
    for (profile, max_signers, threshold) in [(1, 3, 2), (2, 5, 3), (3, 11, 8), (4, 20, 14)] {
        let mut managers = managers(max_signers, threshold, profile);
        let signers = (0..usize::from(threshold)).collect::<Vec<_>>();
        let message = [profile; 32];
        let (public, _, _, signature) = sign_with_subset(
            &mut managers,
            &signers,
            wallet_registration(),
            session(profile),
            message,
            b"wallet-test-policy/v1",
            profile.saturating_add(20),
        );
        verify_with_rust_bitcoin(&public, message, signature);
    }
}

#[test]
fn different_threshold_subsets_sign_without_root_reconstruction() {
    let mut managers = managers(5, 3, 10);
    for (index, signers) in [[0, 1, 2], [0, 2, 4], [1, 3, 4]].into_iter().enumerate() {
        let message = [index as u8 + 1; 32];
        let (public, _, _, signature) = sign_with_subset(
            &mut managers,
            &signers,
            wallet_registration(),
            session(index as u8 + 1),
            message,
            b"wallet-test-policy/v1",
            index as u8 + 40,
        );
        verify_with_rust_bitcoin(&public, message, signature);
    }
}

#[test]
fn threshold_minus_one_and_wrong_application_are_rejected() {
    let mut managers = managers(3, 2, 11);
    let registration = wallet_registration();
    let mut rng = ChaCha20Rng::from_seed([55; 32]);
    let commitment = managers[0]
        .commit_nonce(registration.id(), session(12), &mut rng)
        .expect("commitment");
    let set = NonceCommitmentSet::new([commitment.clone()]).expect("set");
    let request = authorized_request(registration, session(12), [9; 32], b"wallet-test-policy/v1");
    assert!(matches!(
        managers[0].sign_share(commitment.nonce_id(), &request, &set),
        Err(Error::InsufficientThreshold { .. })
    ));

    let second = managers[1]
        .commit_nonce(registration.id(), session(12), &mut rng)
        .expect("second commitment");
    let full_set = NonceCommitmentSet::new([commitment.clone(), second]).expect("threshold set");
    let wrong = authorized_request(
        compatibility_registration(),
        session(12),
        [9; 32],
        b"compatibility-policy/v1",
    );
    assert!(matches!(
        managers[0].sign_share(commitment.nonce_id(), &wrong, &full_set),
        Err(Error::WrongSession)
    ));
    let wrong_version = AuthorizationRequest::new(
        registration.id(),
        ApplicationDerivationVersion::new(2).expect("version"),
        session(12),
        vec![],
    )
    .expect("request");
    assert!(matches!(
        managers[0].sign_share(commitment.nonce_id(), &wrong_version, &full_set),
        Err(Error::Application(_))
    ));

    let wrong_policy = ApplicationRegistration::new(
        registration.id(),
        registration.derivation_version(),
        [99; 32],
    )
    .expect("wrong policy registration");
    let policy_commitments = full_set
        .commitments()
        .values()
        .cloned()
        .map(|mut value| {
            value.registration = wrong_policy;
            value.nonce_id = crate::types::derive_nonce_id(
                value.root_epoch,
                value.registration,
                value.session_id,
                value.participant,
                &value.commitments,
            );
            value
        })
        .collect::<Vec<_>>();
    let wrong_policy_set = NonceCommitmentSet::new(policy_commitments).expect("wrong policy set");
    assert!(matches!(
        managers[0].sign_share(commitment.nonce_id(), &request, &wrong_policy_set),
        Err(Error::WrongSession)
    ));
}

#[test]
fn applications_are_domain_separated_and_root_stays_stable() {
    let managers = managers(3, 2, 12);
    let root_before = managers[0].root_public_package().encode();
    let wallet = managers[0]
        .public_package(wallet_registration().id())
        .expect("wallet package");
    let compatibility = managers[0]
        .public_package(compatibility_registration().id())
        .expect("compatibility package");
    assert_ne!(wallet.group_key(), compatibility.group_key());
    assert_ne!(wallet.verifying_shares(), compatibility.verifying_shares());
    assert_eq!(managers[0].root_public_package().encode(), root_before);
    assert_eq!(
        hex_bytes(&Sha256::digest(&root_before)),
        "72c53e7c947c5f04de0657f98a63c124333092003076eb9ab95511f9b7beaec8"
    );
    assert_eq!(
        hex_bytes(&wallet.group_key()),
        "02763302cbe1af57737681b7a6a391a02631563929921a3a9170d4551f1d42e8eb"
    );
    assert_eq!(
        hex_bytes(&compatibility.group_key()),
        "02df7685111dafe9828180f02dbf2591a4d554e251a8f6dc6b48129407290b41df"
    );
}

fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("write to string");
    }
    encoded
}

#[test]
fn invalid_authorization_does_not_expose_a_digest_signing_api() {
    let mut managers = managers(3, 2, 13);
    let registration = wallet_registration();
    let mut rng = ChaCha20Rng::from_seed([56; 32]);
    let commitments = [0, 1]
        .into_iter()
        .map(|index| {
            managers[index]
                .commit_nonce(registration.id(), session(13), &mut rng)
                .expect("commitment")
        })
        .collect::<Vec<_>>();
    let set = NonceCommitmentSet::new(commitments.clone()).expect("set");
    let mut invalid_payload = [13; 32].to_vec();
    invalid_payload.extend_from_slice(&[0; 32]);
    let invalid = AuthorizationRequest::new(
        registration.id(),
        registration.derivation_version(),
        session(13),
        invalid_payload,
    )
    .expect("invalid authorization request");
    assert!(matches!(
        managers[0].sign_share(commitments[0].nonce_id(), &invalid, &set),
        Err(Error::Application(_))
    ));
    let valid = authorized_request(
        registration,
        session(13),
        [13; 32],
        b"wallet-test-policy/v1",
    );
    managers[0]
        .sign_share(commitments[0].nonce_id(), &valid, &set)
        .expect("unused nonce remains valid after failed authorization");
}

#[test]
fn invalid_share_message_group_and_signer_set_are_rejected() {
    let mut primary_managers = managers(5, 3, 14);
    let message = [14; 32];
    let (public, set, shares, _) = sign_with_subset(
        &mut primary_managers,
        &[0, 1, 2],
        wallet_registration(),
        session(14),
        message,
        b"wallet-test-policy/v1",
        60,
    );

    assert!(ParticipantKeyManager::aggregate(
        &public,
        session(14),
        &[15; 32],
        &set,
        shares.clone(),
    )
    .is_err());

    let mut invalid_share = shares.clone();
    invalid_share[0].share[0] ^= 1;
    assert!(
        ParticipantKeyManager::aggregate(&public, session(14), &message, &set, invalid_share,)
            .is_err()
    );

    let mut other_managers = managers(5, 3, 15);
    let other_public = other_managers[0]
        .public_package(wallet_registration().id())
        .expect("other public")
        .clone();
    let mut wrong_group = public.clone();
    wrong_group.group_key = other_public.group_key;
    assert!(ParticipantKeyManager::aggregate(
        &wrong_group,
        session(14),
        &message,
        &set,
        shares.clone(),
    )
    .is_err());

    let extra = other_managers[3]
        .commit_nonce(
            wallet_registration().id(),
            session(16),
            &mut ChaCha20Rng::from_seed([61; 32]),
        )
        .expect("extra commitment");
    let mut mismatched_commitments = set.commitments().values().cloned().collect::<Vec<_>>();
    mismatched_commitments.push(extra);
    let mismatched_set = NonceCommitmentSet::new(mismatched_commitments).expect("set");
    assert!(ParticipantKeyManager::aggregate(
        &public,
        session(14),
        &message,
        &mismatched_set,
        shares,
    )
    .is_err());

    let mut signer_set_managers = managers(5, 3, 20);
    let registration = wallet_registration();
    let signer_session = session(20);
    let mut rng = ChaCha20Rng::from_seed([63; 32]);
    let signer_commitments = [0, 1, 2, 3]
        .into_iter()
        .map(|index| {
            signer_set_managers[index]
                .commit_nonce(registration.id(), signer_session, &mut rng)
                .expect("signer-set commitment")
        })
        .collect::<Vec<_>>();
    let signer_set = NonceCommitmentSet::new(signer_commitments.clone()).expect("four-signer set");
    let signer_request = authorized_request(
        registration,
        signer_session,
        [20; 32],
        b"wallet-test-policy/v1",
    );
    let only_three_shares = [0, 1, 2]
        .into_iter()
        .map(|index| {
            signer_set_managers[index]
                .sign_share(
                    signer_commitments[index].nonce_id(),
                    &signer_request,
                    &signer_set,
                )
                .expect("share")
        })
        .collect::<Vec<_>>();
    let signer_public = signer_set_managers[0]
        .public_package(registration.id())
        .expect("public")
        .clone();
    assert!(matches!(
        ParticipantKeyManager::aggregate(
            &signer_public,
            signer_session,
            &[20; 32],
            &signer_set,
            only_three_shares,
        ),
        Err(Error::InvalidShare(_))
    ));
}

#[test]
fn canonical_wire_round_trips_and_rejects_mutations() {
    let mut managers = managers(3, 2, 16);
    let root = managers[0].root_public_package().clone();
    let encoded_root = root.encode();
    assert_eq!(
        RootPublicPackage::decode(&encoded_root).expect("root decode"),
        root
    );

    let mut trailing = encoded_root.clone();
    trailing.push(0);
    assert!(RootPublicPackage::decode(&trailing).is_err());

    let mut wrong_ciphersuite = encoded_root.clone();
    wrong_ciphersuite[18..20].copy_from_slice(&2u16.to_be_bytes());
    assert!(RootPublicPackage::decode(&wrong_ciphersuite).is_err());

    let mut duplicate = encoded_root.clone();
    let first_member = [duplicate[24], duplicate[25]];
    duplicate[59..61].copy_from_slice(&first_member);
    assert!(RootPublicPackage::decode(&duplicate).is_err());

    let mut rng = ChaCha20Rng::from_seed([62; 32]);
    let commitments = [0, 1]
        .into_iter()
        .map(|index| {
            managers[index]
                .commit_nonce(wallet_registration().id(), session(17), &mut rng)
                .expect("commitment")
        })
        .collect::<Vec<_>>();
    let commitment_bytes = commitments[0].encode();
    assert_eq!(
        crate::NonceCommitment::decode(&commitment_bytes).expect("commitment decode"),
        commitments[0]
    );
    let mut invalid_commitment = commitments[0].clone();
    invalid_commitment.commitments = vec![0; 66];
    let invalid_set = NonceCommitmentSet::new([invalid_commitment, commitments[1].clone()])
        .expect("structural set");
    let invalid_request = authorized_request(
        wallet_registration(),
        session(17),
        [17; 32],
        b"wallet-test-policy/v1",
    );
    assert!(matches!(
        managers[0].sign_share(commitments[0].nonce_id(), &invalid_request, &invalid_set),
        Err(Error::InvalidCommitment(_))
    ));

    assert!(matches!(
        NonceCommitmentSet::new([commitments[0].clone(), commitments[0].clone()]),
        Err(Error::InvalidParticipant { .. })
    ));
    let set = NonceCommitmentSet::new(commitments.clone()).expect("set");
    let request = authorized_request(
        wallet_registration(),
        session(17),
        [17; 32],
        b"wallet-test-policy/v1",
    );
    let share = managers[0]
        .sign_share(commitments[0].nonce_id(), &request, &set)
        .expect("share");
    assert_eq!(
        SignatureShare::decode(&share.encode()).expect("share decode"),
        share
    );

    let oversized = vec![0; crate::types::MAX_COMMITMENT_BYTES + 161];
    assert!(matches!(
        crate::NonceCommitment::decode(&oversized),
        Err(Error::OversizedInput { .. })
    ));
}

#[test]
fn participant_duplicates_versions_and_debug_secrets_are_rejected_or_redacted() {
    assert!(MemberId::new(0).is_err());
    let managers = managers(3, 2, 18);
    let debug = format!("{:?}", managers[0]);
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("SigningShare"));

    let registration = wallet_registration();
    let wrong_version = AuthorizationRequest::new(
        registration.id(),
        ApplicationDerivationVersion::new(2).expect("version"),
        session(18),
        vec![],
    )
    .expect("request");
    assert_ne!(
        wrong_version.derivation_version(),
        registration.derivation_version()
    );

    let mut manager = managers.into_iter().next().expect("manager");
    assert!(manager
        .register_application(WalletTestAdapter { registration })
        .is_err());
}

#[test]
fn public_wire_decoders_reject_truncation_and_trailing_bytes_property() {
    let mut managers = managers(3, 2, 19);
    let root = managers[0].root_public_package().encode();
    let mut rng = ChaCha20Rng::from_seed([64; 32]);
    let commitments = [0, 1]
        .into_iter()
        .map(|index| {
            managers[index]
                .commit_nonce(wallet_registration().id(), session(21), &mut rng)
                .expect("commitment")
        })
        .collect::<Vec<_>>();
    let commitment_set = NonceCommitmentSet::new(commitments.clone()).expect("set");
    let request = authorized_request(
        wallet_registration(),
        session(21),
        [21; 32],
        b"wallet-test-policy/v1",
    );
    let share = managers[0]
        .sign_share(commitments[0].nonce_id(), &request, &commitment_set)
        .expect("share")
        .encode();
    let commitment = commitments[0].encode();

    assert_truncation_and_trailing_rejected(&root, |bytes| {
        RootPublicPackage::decode(bytes).map(|_| ())
    });
    assert_truncation_and_trailing_rejected(&commitment, |bytes| {
        crate::NonceCommitment::decode(bytes).map(|_| ())
    });
    assert_truncation_and_trailing_rejected(&share, |bytes| {
        SignatureShare::decode(bytes).map(|_| ())
    });
}

fn assert_truncation_and_trailing_rejected(
    bytes: &[u8],
    decode: impl Fn(&[u8]) -> Result<(), Error>,
) {
    for length in 0..bytes.len() {
        assert!(decode(&bytes[..length]).is_err());
    }
    for suffix in [vec![0], vec![1, 2], vec![255; 8]] {
        let mut candidate = bytes.to_vec();
        candidate.extend_from_slice(&suffix);
        assert!(decode(&candidate).is_err());
    }
}

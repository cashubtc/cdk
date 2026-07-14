use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread;

use bitcoin::secp256k1::{schnorr, Message, PublicKey, Secp256k1};
use frost_secp256k1_tr as frost;
use frost_secp256k1_tr::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use sha2::{Digest, Sha256};

use crate::canonical::CanonicalRootKeyPackage;
use crate::{
    ApplicationAuthorization, ApplicationDerivationVersion, ApplicationId, ApplicationKeyEpoch,
    ApplicationRegistration, AuthorizationRequest, AuthorizedSigningRequest, DurableNonceStore,
    Error, MemberId, MemoryNonceStore, NonceBurnReason, NonceCommitmentSet, NonceId,
    NonceLifecycle, NonceRecord, NonceReservation, ParticipantKeyManager, RedbNonceStore,
    RootKeyEpoch, RootPublicPackage, SecretEncryptionKey, SignatureMode, SignatureShare,
    SigningAuthorizationScope, SigningDerivation, SigningSessionContext, SigningSessionId,
};

#[derive(Debug, Clone, Copy)]
struct WalletTestAdapter {
    registration: ApplicationRegistration,
}

impl ApplicationAuthorization for WalletTestAdapter {
    fn registration(&self) -> ApplicationRegistration {
        self.registration
    }

    fn authorize(&self, request: &AuthorizationRequest) -> Result<AuthorizedSigningRequest, Error> {
        Ok(AuthorizedSigningRequest::new(
            authorize_payload(self.registration, request, b"wallet-test-policy/v1")?,
            SigningDerivation::application_key(),
        ))
    }
}

#[derive(Debug, Clone, Copy)]
struct CompatibilityTestAdapter {
    registration: ApplicationRegistration,
}

#[derive(Debug, Clone)]
struct DerivedTestAdapter {
    registration: ApplicationRegistration,
    derivation: SigningDerivation,
}

impl ApplicationAuthorization for DerivedTestAdapter {
    fn registration(&self) -> ApplicationRegistration {
        self.registration
    }

    fn authorize(&self, request: &AuthorizationRequest) -> Result<AuthorizedSigningRequest, Error> {
        Ok(AuthorizedSigningRequest::new(
            authorize_payload(self.registration, request, b"derived-test-policy/v1")?,
            self.derivation.clone(),
        ))
    }
}

impl ApplicationAuthorization for CompatibilityTestAdapter {
    fn registration(&self) -> ApplicationRegistration {
        self.registration
    }

    fn authorize(&self, request: &AuthorizationRequest) -> Result<AuthorizedSigningRequest, Error> {
        Ok(AuthorizedSigningRequest::new(
            authorize_payload(self.registration, request, b"compatibility-policy/v1")?,
            SigningDerivation::application_key(),
        ))
    }
}

fn authorize_payload(
    registration: ApplicationRegistration,
    request: &AuthorizationRequest,
    domain: &[u8],
) -> Result<[u8; 32], Error> {
    if request.application_id() != registration.id()
        || request.application_key_epoch() != registration.key_epoch()
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
    hasher.update(registration.key_epoch().get().to_be_bytes());
    hasher.update(registration.derivation_version().get().to_be_bytes());
    hasher.update(registration.authorization_policy());
    hasher.update(session.to_bytes());
    hasher.update(message);
    hasher.finalize().into()
}

fn wallet_registration() -> ApplicationRegistration {
    ApplicationRegistration::new(
        ApplicationId::from_label("org.cashubtc.cdk.test-wallet").expect("application ID"),
        ApplicationKeyEpoch::new(1).expect("key epoch"),
        ApplicationDerivationVersion::new(1).expect("version"),
        Sha256::digest(b"wallet-test-policy/v1").into(),
    )
    .expect("registration")
}

fn derived_registration() -> ApplicationRegistration {
    ApplicationRegistration::new(
        ApplicationId::from_label("org.cashubtc.cdk.test-derived-wallet").expect("application ID"),
        ApplicationKeyEpoch::new(1).expect("key epoch"),
        ApplicationDerivationVersion::new(1).expect("version"),
        Sha256::digest(b"derived-test-policy/v1").into(),
    )
    .expect("registration")
}

fn derived_signing_derivation() -> SigningDerivation {
    SigningDerivation::new(
        Sha256::digest(b"cdk/frost/test-bip32-target/v1").into(),
        vec![[1; 32], [2; 32]],
        SignatureMode::TaprootKeyPath { merkle_root: None },
    )
    .expect("derived signing target")
}

fn compatibility_registration() -> ApplicationRegistration {
    ApplicationRegistration::new(
        ApplicationId::from_label("org.cashubtc.cdk.compatibility-test").expect("application ID"),
        ApplicationKeyEpoch::new(1).expect("key epoch"),
        ApplicationDerivationVersion::new(7).expect("version"),
        Sha256::digest(b"compatibility-policy/v1").into(),
    )
    .expect("registration")
}

fn session(byte: u8) -> SigningSessionId {
    SigningSessionId::from_bytes([byte; 32]).expect("session")
}

fn signing_context(
    registration: ApplicationRegistration,
    derivation: &SigningDerivation,
    legacy_session: SigningSessionId,
    message: [u8; 32],
    signer_indexes: &[usize],
) -> SigningSessionContext {
    let scope = SigningAuthorizationScope::new(
        [0xf0; 32],
        RootKeyEpoch::new(1).expect("root epoch"),
        registration,
        derivation.binding(),
        legacy_session.to_bytes(),
        message,
    )
    .expect("signing authorization scope");
    SigningSessionContext::new(
        scope,
        0,
        signer_indexes
            .iter()
            .map(|index| MemberId::new((*index + 1) as u16).expect("signer member")),
    )
    .expect("signing context")
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
        registration.key_epoch(),
        registration.derivation_version(),
        session,
        payload,
    )
    .expect("authorization request")
}

fn managers(max_signers: u16, threshold: u16, seed: u8) -> Vec<ParticipantKeyManager> {
    managers_with_store(max_signers, threshold, seed, |_| {
        MemoryNonceStore::default()
    })
}

fn managers_with_store<S, F>(
    max_signers: u16,
    threshold: u16,
    seed: u8,
    mut store_for_member: F,
) -> Vec<ParticipantKeyManager>
where
    S: DurableNonceStore,
    F: FnMut(MemberId) -> S,
{
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
            let nonce_key = SecretEncryptionKey::new(
                [seed.wrapping_add(member.get() as u8).wrapping_add(1); 32],
            )
            .expect("nonce storage key");
            let mut manager =
                ParticipantKeyManager::with_nonce_store(root, nonce_key, store_for_member(member))
                    .expect("participant key manager");
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
                .register_application(DerivedTestAdapter {
                    registration: derived_registration(),
                    derivation: derived_signing_derivation(),
                })
                .expect("derived adapter");
            manager
        })
        .collect()
}

#[derive(Debug, Clone, Default)]
struct FailingNonceStore {
    inner: MemoryNonceStore,
    fail_mutation: Arc<Mutex<Option<usize>>>,
}

impl FailingNonceStore {
    fn arm(&self, mutation: usize) {
        *self.fail_mutation.lock().expect("failure lock") = Some(mutation);
    }

    fn fail_if_armed(&self) -> Result<(), Error> {
        let mut failure = self.fail_mutation.lock().expect("failure lock");
        match failure.as_mut() {
            Some(remaining) if *remaining == 1 => {
                *failure = None;
                Err(Error::NoncePersistence(
                    "injected durable transaction failure".to_string(),
                ))
            }
            Some(remaining) => {
                *remaining -= 1;
                Ok(())
            }
            None => Ok(()),
        }
    }
}

impl DurableNonceStore for FailingNonceStore {
    fn activate_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        self.fail_if_armed()?;
        self.inner.activate_attempt(context)
    }

    fn reserve(&self, record: NonceRecord) -> Result<NonceReservation, Error> {
        self.fail_if_armed()?;
        self.inner.reserve(record)
    }

    fn load_nonce(&self, nonce_id: NonceId) -> Result<Option<NonceRecord>, Error> {
        self.inner.load_nonce(nonce_id)
    }

    fn load_session(&self, session_id: SigningSessionId) -> Result<Option<NonceRecord>, Error> {
        self.inner.load_session(session_id)
    }

    fn compare_and_swap(
        &self,
        expected: &NonceRecord,
        replacement: NonceRecord,
    ) -> Result<(), Error> {
        self.fail_if_armed()?;
        self.inner.compare_and_swap(expected, replacement)
    }

    fn nonterminal_records(&self) -> Result<Vec<NonceRecord>, Error> {
        self.inner.nonterminal_records()
    }

    fn compact_terminal(&self, retain_detailed: usize) -> Result<usize, Error> {
        self.inner.compact_terminal(retain_detailed)
    }
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
    let context = signing_context(
        registration,
        &SigningDerivation::application_key(),
        session_id,
        message,
        signer_indexes,
    );
    let session_id = context.session_id();
    let mut rng = ChaCha20Rng::from_seed([seed; 32]);
    let commitments = signer_indexes
        .iter()
        .map(|index| {
            managers[*index]
                .commit_nonce(&context, &mut rng)
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
fn signing_transport_wire_round_trips_and_rejects_mutation() {
    let mut managers = managers(3, 2, 87);
    let registration = wallet_registration();
    let derivation = SigningDerivation::application_key();
    let message = [87; 32];
    let context = signing_context(registration, &derivation, session(87), message, &[0, 1]);
    let request = authorized_request(
        registration,
        context.session_id(),
        message,
        b"wallet-test-policy/v1",
    );
    let mut rng = ChaCha20Rng::from_seed([88; 32]);
    let commitments = managers[..2]
        .iter_mut()
        .map(|manager| {
            manager
                .commit_nonce(&context, &mut rng)
                .expect("commitment")
        })
        .collect::<Vec<_>>();
    let set = NonceCommitmentSet::new(commitments).expect("commitment set");

    assert_eq!(
        SigningSessionContext::decode(&context.encode()).expect("context wire"),
        context
    );
    assert_eq!(
        SigningDerivation::decode(&derivation.encode()).expect("derivation wire"),
        derivation
    );
    assert_eq!(
        AuthorizationRequest::decode(&request.encode()).expect("authorization wire"),
        request
    );
    assert_eq!(
        NonceCommitmentSet::decode(&set.encode()).expect("commitment-set wire"),
        set
    );

    let context_bytes = context.encode();
    let mut trailing = context_bytes.clone();
    trailing.push(0);
    assert!(SigningSessionContext::decode(&trailing).is_err());
    assert!(SigningSessionContext::decode(&context_bytes[..context_bytes.len() - 1]).is_err());
    let mut mutated = context_bytes;
    mutated[0] ^= 1;
    assert!(SigningSessionContext::decode(&mutated).is_err());

    let mut authorization = request.encode();
    authorization[0] ^= 1;
    assert!(AuthorizationRequest::decode(&authorization).is_err());
    let mut commitment_set = set.encode();
    commitment_set.push(0);
    assert!(NonceCommitmentSet::decode(&commitment_set).is_err());
}

#[test]
fn additively_derived_taproot_target_signs_and_rejects_target_swaps() {
    let mut managers = managers(5, 3, 31);
    let registration = derived_registration();
    let derivation = derived_signing_derivation();
    let message = [31; 32];
    let context = signing_context(registration, &derivation, session(31), message, &[0, 1, 2]);
    let session_id = context.session_id();
    let mut rng = ChaCha20Rng::from_seed([31; 32]);
    let commitments = managers[..3]
        .iter_mut()
        .map(|manager| {
            manager
                .commit_nonce_for_derivation(&context, derivation.clone(), &mut rng)
                .expect("derived nonce")
        })
        .collect::<Vec<_>>();
    let set = NonceCommitmentSet::new(commitments.clone()).expect("commitment set");
    let request = authorized_request(registration, session_id, message, b"derived-test-policy/v1");
    let shares = managers[..3]
        .iter_mut()
        .zip(&commitments)
        .map(|(manager, commitment)| {
            manager
                .sign_share(commitment.nonce_id(), &request, &set)
                .expect("derived share")
        })
        .collect::<Vec<_>>();
    let application = managers[0]
        .public_package(registration.id())
        .expect("application public")
        .clone();
    let target = managers[0]
        .signing_public_package(registration.id(), &derivation)
        .expect("target public");
    let signature = ParticipantKeyManager::aggregate_for_derivation(
        &application,
        &derivation,
        session_id,
        &message,
        &set,
        shares.clone(),
    )
    .expect("derived aggregate");
    assert_eq!(
        hex_bytes(&signature.to_bytes()),
        "89c6b2725df6cb48282d7a62c7f39822c7a59ed9c1f9a4e010b50e3cd683e0c232d5436373fdf29fa939e2f80c9c39e4cf7e50eede29ac8ea1962a1d8c069984"
    );
    let secp = Secp256k1::verification_only();
    let key = PublicKey::from_slice(&target.output_group_key()).expect("output key");
    let (x_only, _) = key.x_only_public_key();
    let signature = schnorr::Signature::from_slice(&signature.to_bytes()).expect("signature");
    secp.verify_schnorr(&signature, &Message::from_digest(message), &x_only)
        .expect("BIP341 output-key signature");

    let wrong_targets = [
        SigningDerivation::new(
            derivation.context(),
            vec![[1; 32], [3; 32]],
            derivation.signature_mode(),
        )
        .expect("wrong child"),
        SigningDerivation::new(
            Sha256::digest(b"wrong-chain-code-or-network-context").into(),
            derivation.tweaks().to_vec(),
            derivation.signature_mode(),
        )
        .expect("wrong context"),
        SigningDerivation::new(
            derivation.context(),
            derivation.tweaks().to_vec(),
            SignatureMode::Plain,
        )
        .expect("wrong Taproot mode"),
    ];
    for wrong in wrong_targets {
        assert!(matches!(
            ParticipantKeyManager::aggregate_for_derivation(
                &application,
                &wrong,
                session_id,
                &message,
                &set,
                shares.clone(),
            ),
            Err(Error::WrongSession)
        ));
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
    let message = [9; 32];
    let context = signing_context(
        registration,
        &SigningDerivation::application_key(),
        session(12),
        message,
        &[0, 1],
    );
    let session_id = context.session_id();
    let mut rng = ChaCha20Rng::from_seed([55; 32]);
    let commitment = managers[0]
        .commit_nonce(&context, &mut rng)
        .expect("commitment");
    let set = NonceCommitmentSet::new([commitment.clone()]).expect("set");
    let request = authorized_request(registration, session_id, message, b"wallet-test-policy/v1");
    assert!(matches!(
        managers[0].sign_share(commitment.nonce_id(), &request, &set),
        Err(Error::InsufficientThreshold { .. })
    ));

    let second = managers[1]
        .commit_nonce(&context, &mut rng)
        .expect("second commitment");
    let full_set = NonceCommitmentSet::new([commitment.clone(), second]).expect("threshold set");
    let wrong = authorized_request(
        compatibility_registration(),
        session_id,
        message,
        b"compatibility-policy/v1",
    );
    assert!(matches!(
        managers[0].sign_share(commitment.nonce_id(), &wrong, &full_set),
        Err(Error::WrongSession)
    ));
    let wrong_version = AuthorizationRequest::new(
        registration.id(),
        registration.key_epoch(),
        ApplicationDerivationVersion::new(2).expect("version"),
        session_id,
        vec![],
    )
    .expect("request");
    assert!(matches!(
        managers[0].sign_share(commitment.nonce_id(), &wrong_version, &full_set),
        Err(Error::Application(_))
    ));
    let wrong_epoch = AuthorizationRequest::new(
        registration.id(),
        ApplicationKeyEpoch::new(2).expect("key epoch"),
        registration.derivation_version(),
        session_id,
        vec![],
    )
    .expect("request");
    assert!(matches!(
        managers[0].sign_share(commitment.nonce_id(), &wrong_epoch, &full_set),
        Err(Error::Application(_))
    ));

    let wrong_policy = ApplicationRegistration::new(
        registration.id(),
        registration.key_epoch(),
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
                value.derivation_binding,
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
        "0318e749ec4732cc0758d6ef5c2998652c1d8e40d3cdbdbabaf8b09902e7640030"
    );
    assert_eq!(
        hex_bytes(&compatibility.group_key()),
        "03396bb048b17ba7a46fb6624d6328005d46cec043588f26a5dd9d06b6d08d34e9"
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
    let message = [13; 32];
    let context = signing_context(
        registration,
        &SigningDerivation::application_key(),
        session(13),
        message,
        &[0, 1],
    );
    let session_id = context.session_id();
    let mut rng = ChaCha20Rng::from_seed([56; 32]);
    let commitments = [0, 1]
        .into_iter()
        .map(|index| {
            managers[index]
                .commit_nonce(&context, &mut rng)
                .expect("commitment")
        })
        .collect::<Vec<_>>();
    let set = NonceCommitmentSet::new(commitments.clone()).expect("set");
    let mut invalid_payload = message.to_vec();
    invalid_payload.extend_from_slice(&[0; 32]);
    let invalid = AuthorizationRequest::new(
        registration.id(),
        registration.key_epoch(),
        registration.derivation_version(),
        session_id,
        invalid_payload,
    )
    .expect("invalid authorization request");
    assert!(matches!(
        managers[0].sign_share(commitments[0].nonce_id(), &invalid, &set),
        Err(Error::Application(_))
    ));
    let valid = authorized_request(registration, session_id, message, b"wallet-test-policy/v1");
    assert!(matches!(
        managers[0].sign_share(commitments[0].nonce_id(), &valid, &set),
        Err(Error::InvalidCommitment(_))
    ));
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
    let signed_session = set
        .commitments()
        .values()
        .next()
        .expect("signed commitment")
        .session_id();

    assert!(ParticipantKeyManager::aggregate(
        &public,
        signed_session,
        &[15; 32],
        &set,
        shares.clone(),
    )
    .is_err());

    let mut invalid_share = shares.clone();
    invalid_share[0].share[0] ^= 1;
    assert!(ParticipantKeyManager::aggregate(
        &public,
        signed_session,
        &message,
        &set,
        invalid_share,
    )
    .is_err());
    let (recovered_public, _, _, recovered_signature) = sign_with_subset(
        &mut primary_managers,
        &[1, 2, 3],
        wallet_registration(),
        session(16),
        message,
        b"wallet-test-policy/v1",
        61,
    );
    verify_with_rust_bitcoin(&recovered_public, message, recovered_signature);

    let mut other_managers = managers(5, 3, 15);
    let other_public = other_managers[0]
        .public_package(wallet_registration().id())
        .expect("other public")
        .clone();
    let mut wrong_group = public.clone();
    wrong_group.group_key = other_public.group_key;
    assert!(ParticipantKeyManager::aggregate(
        &wrong_group,
        signed_session,
        &message,
        &set,
        shares.clone(),
    )
    .is_err());

    let extra_context = signing_context(
        wallet_registration(),
        &SigningDerivation::application_key(),
        session(16),
        [16; 32],
        &[0, 1, 3],
    );
    let extra = other_managers[3]
        .commit_nonce(&extra_context, &mut ChaCha20Rng::from_seed([61; 32]))
        .expect("extra commitment");
    let mut mismatched_commitments = set.commitments().values().cloned().collect::<Vec<_>>();
    mismatched_commitments.push(extra);
    let mismatched_set = NonceCommitmentSet::new(mismatched_commitments).expect("set");
    assert!(ParticipantKeyManager::aggregate(
        &public,
        signed_session,
        &message,
        &mismatched_set,
        shares,
    )
    .is_err());

    let mut signer_set_managers = managers(5, 3, 20);
    let registration = wallet_registration();
    let signer_context = signing_context(
        registration,
        &SigningDerivation::application_key(),
        session(20),
        [20; 32],
        &[0, 1, 2, 3],
    );
    let signer_session = signer_context.session_id();
    let mut rng = ChaCha20Rng::from_seed([63; 32]);
    let signer_commitments = [0, 1, 2, 3]
        .into_iter()
        .map(|index| {
            signer_set_managers[index]
                .commit_nonce(&signer_context, &mut rng)
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
    let wire_context = signing_context(
        wallet_registration(),
        &SigningDerivation::application_key(),
        session(17),
        [17; 32],
        &[0, 1],
    );
    let wire_session = wire_context.session_id();
    let commitments = [0, 1]
        .into_iter()
        .map(|index| {
            managers[index]
                .commit_nonce(&wire_context, &mut rng)
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
        wire_session,
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
        wire_session,
        [17; 32],
        b"wallet-test-policy/v1",
    );
    let share = managers[1]
        .sign_share(commitments[1].nonce_id(), &request, &set)
        .expect("share");
    assert_eq!(
        SignatureShare::decode(&share.encode()).expect("share decode"),
        share
    );

    let oversized = vec![0; crate::types::MAX_COMMITMENT_BYTES + 193];
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
        registration.key_epoch(),
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
    let property_context = signing_context(
        wallet_registration(),
        &SigningDerivation::application_key(),
        session(21),
        [21; 32],
        &[0, 1],
    );
    let commitments = [0, 1]
        .into_iter()
        .map(|index| {
            managers[index]
                .commit_nonce(&property_context, &mut rng)
                .expect("commitment")
        })
        .collect::<Vec<_>>();
    let commitment_set = NonceCommitmentSet::new(commitments.clone()).expect("set");
    let request = authorized_request(
        wallet_registration(),
        property_context.session_id(),
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

#[test]
fn signing_session_ids_bind_every_scope_attempt_and_signer_field() {
    let registration = wallet_registration();
    let alternate_registration = ApplicationRegistration::new(
        registration.id(),
        ApplicationKeyEpoch::new(2).expect("alternate key epoch"),
        registration.derivation_version(),
        registration.authorization_policy(),
    )
    .expect("alternate registration");
    let member_one = MemberId::new(1).expect("member one");
    let member_two = MemberId::new(2).expect("member two");
    let member_three = MemberId::new(3).expect("member three");
    let make = |federation_id,
                root_epoch,
                registration,
                derivation_binding,
                authorization_reference,
                message,
                attempt,
                signers| {
        let scope = SigningAuthorizationScope::new(
            federation_id,
            root_epoch,
            registration,
            derivation_binding,
            authorization_reference,
            message,
        )
        .expect("authorization scope");
        SigningSessionContext::new(scope, attempt, signers).expect("session context")
    };
    let base = make(
        [1; 32],
        RootKeyEpoch::new(1).expect("root epoch"),
        registration,
        [2; 32],
        [3; 32],
        [4; 32],
        0,
        vec![member_one, member_two],
    );
    let variants = [
        make(
            [9; 32],
            base.root_epoch(),
            registration,
            [2; 32],
            [3; 32],
            [4; 32],
            0,
            vec![member_one, member_two],
        ),
        make(
            [1; 32],
            RootKeyEpoch::new(2).expect("alternate root epoch"),
            registration,
            [2; 32],
            [3; 32],
            [4; 32],
            0,
            vec![member_one, member_two],
        ),
        make(
            [1; 32],
            base.root_epoch(),
            alternate_registration,
            [2; 32],
            [3; 32],
            [4; 32],
            0,
            vec![member_one, member_two],
        ),
        make(
            [1; 32],
            base.root_epoch(),
            registration,
            [8; 32],
            [3; 32],
            [4; 32],
            0,
            vec![member_one, member_two],
        ),
        make(
            [1; 32],
            base.root_epoch(),
            registration,
            [2; 32],
            [8; 32],
            [4; 32],
            0,
            vec![member_one, member_two],
        ),
        make(
            [1; 32],
            base.root_epoch(),
            registration,
            [2; 32],
            [3; 32],
            [8; 32],
            0,
            vec![member_one, member_two],
        ),
        make(
            [1; 32],
            base.root_epoch(),
            registration,
            [2; 32],
            [3; 32],
            [4; 32],
            1,
            vec![member_one, member_two],
        ),
        make(
            [1; 32],
            base.root_epoch(),
            registration,
            [2; 32],
            [3; 32],
            [4; 32],
            0,
            vec![member_one, member_three],
        ),
    ];
    let session_ids = std::iter::once(base.session_id())
        .chain(variants.iter().map(SigningSessionContext::session_id))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(session_ids.len(), variants.len() + 1);
    assert_eq!(variants[6].slot_id(), base.slot_id());
    assert_eq!(variants[7].slot_id(), base.slot_id());
    assert!(variants[..6]
        .iter()
        .all(|variant| variant.slot_id() != base.slot_id()));
    let invalid_scope = SigningAuthorizationScope::new(
        [1; 32],
        base.root_epoch(),
        registration,
        [2; 32],
        [3; 32],
        [4; 32],
    )
    .expect("invalid-order scope");
    assert!(SigningSessionContext::new(invalid_scope, 0, [member_two, member_one],).is_err());
}

#[test]
fn durable_nonce_lifecycle_retries_attempts_and_cleanup_fail_closed() {
    let stores = (0..3)
        .map(|_| FailingNonceStore::default())
        .collect::<Vec<_>>();
    let stores_for_managers = stores.clone();
    let mut managers = managers_with_store(3, 2, 70, move |member| {
        stores_for_managers[usize::from(member.get() - 1)].clone()
    });
    let registration = wallet_registration();
    let message = [70; 32];
    let context = signing_context(
        registration,
        &SigningDerivation::application_key(),
        session(70),
        message,
        &[0, 1],
    );
    let mut rng = ChaCha20Rng::from_seed([71; 32]);
    let first = managers[0]
        .commit_nonce(&context, &mut rng)
        .expect("first commitment");
    assert_eq!(
        managers[0]
            .commit_nonce(&context, &mut rng)
            .expect("exact round-one retry"),
        first
    );
    let second = managers[1]
        .commit_nonce(&context, &mut rng)
        .expect("second commitment");
    let first_record = stores[0]
        .load_nonce(first.nonce_id())
        .expect("load first record")
        .expect("first record");
    assert_eq!(first_record.lifecycle(), &NonceLifecycle::CommitmentSent);
    let durable_bytes = first_record.to_bytes().expect("durable nonce bytes");
    assert_eq!(
        NonceRecord::from_bytes(&durable_bytes).expect("durable nonce decode"),
        first_record
    );
    let mut malformed =
        serde_json::from_slice::<serde_json::Value>(&durable_bytes).expect("nonce record json");
    malformed["lifecycle"] = serde_json::json!({
        "state": "consumed",
        "data": {"package_hash": vec![7; 32]}
    });
    assert!(NonceRecord::from_bytes(
        &serde_json::to_vec(&malformed).expect("malformed nonce bytes")
    )
    .is_err());
    let skipped_round_two = first_record
        .transition(
            NonceLifecycle::Consumed {
                package_hash: [7; 32],
            },
            false,
            None,
        )
        .expect("construct invalid transition");
    assert!(stores[0]
        .compare_and_swap(&first_record, skipped_round_two)
        .is_err());
    let record_debug = format!("{first_record:?}");
    assert!(record_debug.contains("<sealed>"));
    assert!(!record_debug.contains("SigningNonces"));

    let next_scope = SigningAuthorizationScope::new(
        context.federation_id(),
        context.root_epoch(),
        context.registration(),
        context.derivation_binding(),
        context.authorization_reference(),
        context.message_commitment(),
    )
    .expect("next attempt scope");
    let next_attempt =
        SigningSessionContext::new(next_scope, 1, context.signer_set().iter().copied())
            .expect("next attempt");
    let replacement = managers[1]
        .commit_nonce(&next_attempt, &mut rng)
        .expect("newer attempt atomically replaces the old nonce");
    assert_ne!(replacement.nonce_id(), second.nonce_id());
    assert_eq!(
        stores[1]
            .load_nonce(second.nonce_id())
            .expect("load superseded attempt")
            .expect("superseded attempt record")
            .lifecycle(),
        &NonceLifecycle::Burned {
            reason: NonceBurnReason::ReplacedAttempt,
        }
    );

    let set = NonceCommitmentSet::new([first.clone(), second]).expect("commitment set");
    let request = authorized_request(
        registration,
        context.session_id(),
        message,
        b"wallet-test-policy/v1",
    );
    let share = managers[0]
        .sign_share(first.nonce_id(), &request, &set)
        .expect("signature share");
    assert_eq!(
        managers[0]
            .sign_share(first.nonce_id(), &request, &set)
            .expect("exact round-two retry"),
        share
    );
    assert!(matches!(
        stores[0]
            .load_nonce(first.nonce_id())
            .expect("load consumed")
            .expect("consumed record")
            .lifecycle(),
        NonceLifecycle::Consumed { .. }
    ));
    assert_eq!(
        managers[0]
            .commit_nonce(&context, &mut rng)
            .expect("another coordinator recovers the consumed commitment"),
        first
    );
    assert_eq!(
        managers[0]
            .cleanup_terminal_nonces(0)
            .expect("compact terminal record"),
        1
    );
    assert!(stores[0].load_nonce(first.nonce_id()).is_err());
    assert!(stores[0]
        .inner
        .tombstone(first.nonce_id())
        .expect("load tombstone")
        .is_some());
}

#[test]
fn restart_and_transaction_failures_never_resurrect_nonce_material() {
    for failed_mutation in 1..=4 {
        let stores = (0..3)
            .map(|_| FailingNonceStore::default())
            .collect::<Vec<_>>();
        let stores_for_managers = stores.clone();
        let mut managers = managers_with_store(3, 2, 80 + failed_mutation as u8, move |member| {
            stores_for_managers[usize::from(member.get() - 1)].clone()
        });
        let context = signing_context(
            wallet_registration(),
            &SigningDerivation::application_key(),
            session(80 + failed_mutation as u8),
            [80 + failed_mutation as u8; 32],
            &[0, 1],
        );
        stores[0].arm(failed_mutation);
        assert!(managers[0]
            .commit_nonce(&context, &mut ChaCha20Rng::from_seed([83; 32]))
            .is_err());
        let records = stores[0]
            .nonterminal_records()
            .expect("round-one partial records");
        if failed_mutation <= 2 {
            assert!(records.is_empty());
        } else {
            assert_eq!(records.len(), 1);
            managers[0]
                .recover_after_restart()
                .expect("round-one restart recovery");
            assert!(matches!(
                stores[0]
                    .load_nonce(records[0].nonce_id())
                    .expect("load recovered record")
                    .expect("recovered record")
                    .lifecycle(),
                NonceLifecycle::Burned {
                    reason: NonceBurnReason::RestartRecovery
                }
            ));
        }
    }

    for failed_mutation in 1..=3 {
        let stores = (0..3)
            .map(|_| FailingNonceStore::default())
            .collect::<Vec<_>>();
        let stores_for_managers = stores.clone();
        let mut managers = managers_with_store(3, 2, 90 + failed_mutation as u8, move |member| {
            stores_for_managers[usize::from(member.get() - 1)].clone()
        });
        let registration = wallet_registration();
        let message = [90 + failed_mutation as u8; 32];
        let context = signing_context(
            registration,
            &SigningDerivation::application_key(),
            session(90 + failed_mutation as u8),
            message,
            &[0, 1],
        );
        let mut rng = ChaCha20Rng::from_seed([94; 32]);
        let first = managers[0]
            .commit_nonce(&context, &mut rng)
            .expect("first commitment");
        let second = managers[1]
            .commit_nonce(&context, &mut rng)
            .expect("second commitment");
        let set = NonceCommitmentSet::new([first.clone(), second]).expect("commitment set");
        let request = authorized_request(
            registration,
            context.session_id(),
            message,
            b"wallet-test-policy/v1",
        );
        stores[0].arm(failed_mutation);
        assert!(managers[0]
            .sign_share(first.nonce_id(), &request, &set)
            .is_err());
        let record = stores[0]
            .load_nonce(first.nonce_id())
            .expect("load failed round-two record")
            .expect("failed round-two record");
        match failed_mutation {
            1 => assert_eq!(record.lifecycle(), &NonceLifecycle::CommitmentSent),
            2 => assert!(matches!(record.lifecycle(), NonceLifecycle::Signing { .. })),
            3 => assert!(matches!(
                record.lifecycle(),
                NonceLifecycle::ShareProduced { .. }
            )),
            _ => unreachable!("bounded failure cases"),
        }
        managers[0]
            .recover_after_restart()
            .expect("round-two restart recovery");
        assert!(matches!(
            stores[0]
                .load_nonce(first.nonce_id())
                .expect("load burned record")
                .expect("burned record")
                .lifecycle(),
            NonceLifecycle::Burned {
                reason: NonceBurnReason::RestartRecovery
            }
        ));
    }
}

#[test]
fn restart_burns_pending_sessions_for_two_applications_in_one_shared_nonce_ledger() {
    let stores = (0..3)
        .map(|_| MemoryNonceStore::default())
        .collect::<Vec<_>>();
    let stores_for_managers = stores.clone();
    let mut managers = managers_with_store(3, 2, 110, move |member| {
        stores_for_managers[usize::from(member.get() - 1)].clone()
    });
    let wallet_context = signing_context(
        wallet_registration(),
        &SigningDerivation::application_key(),
        session(110),
        [110; 32],
        &[0, 1],
    );
    let compatibility_context = signing_context(
        compatibility_registration(),
        &SigningDerivation::application_key(),
        session(111),
        [111; 32],
        &[0, 1],
    );
    let mut rng = ChaCha20Rng::from_seed([112; 32]);
    let wallet = managers[0]
        .commit_nonce(&wallet_context, &mut rng)
        .expect("wallet commitment");
    let compatibility = managers[0]
        .commit_nonce(&compatibility_context, &mut rng)
        .expect("compatibility commitment");
    assert_ne!(wallet.nonce_id(), compatibility.nonce_id());
    assert_eq!(
        stores[0]
            .nonterminal_records()
            .expect("pending application sessions")
            .len(),
        2
    );

    managers[0]
        .recover_after_restart()
        .expect("shared-ledger restart recovery");
    managers[0]
        .recover_after_restart()
        .expect("idempotent shared-ledger recovery");
    for nonce_id in [wallet.nonce_id(), compatibility.nonce_id()] {
        assert!(matches!(
            stores[0]
                .load_nonce(nonce_id)
                .expect("recovered nonce")
                .expect("durable recovered record")
                .lifecycle(),
            NonceLifecycle::Burned {
                reason: NonceBurnReason::RestartRecovery
            }
        ));
    }

    let fresh_context = |prior: &SigningSessionContext| {
        let scope = SigningAuthorizationScope::new(
            prior.federation_id(),
            prior.root_epoch(),
            prior.registration(),
            prior.derivation_binding(),
            prior.authorization_reference(),
            prior.message_commitment(),
        )
        .expect("retry authorization scope");
        SigningSessionContext::new(scope, 1, prior.signer_set().iter().copied())
            .expect("fresh signing attempt")
    };
    let wallet_retry = managers[0]
        .commit_nonce(&fresh_context(&wallet_context), &mut rng)
        .expect("fresh wallet commitment");
    let compatibility_retry = managers[0]
        .commit_nonce(&fresh_context(&compatibility_context), &mut rng)
        .expect("fresh compatibility commitment");
    assert_ne!(wallet_retry.nonce_id(), wallet.nonce_id());
    assert_ne!(compatibility_retry.nonce_id(), compatibility.nonce_id());
    assert_ne!(wallet_retry.nonce_id(), compatibility_retry.nonce_id());
}

#[test]
fn durable_attempt_high_water_rejects_delayed_stale_and_same_attempt_substitution() {
    let directory = tempfile::tempdir().expect("attempt ledger directory");
    let database_path = directory.path().join("attempt-high-water.redb");
    let base = signing_context(
        wallet_registration(),
        &SigningDerivation::application_key(),
        session(113),
        [113; 32],
        &[0, 1],
    );
    let scope = SigningAuthorizationScope::new(
        base.federation_id(),
        base.root_epoch(),
        base.registration(),
        base.derivation_binding(),
        base.authorization_reference(),
        base.message_commitment(),
    )
    .expect("attempt scope");
    let newer = SigningSessionContext::new(scope.clone(), 1, base.signer_set().iter().copied())
        .expect("newer attempt");
    let substituted = SigningSessionContext::new(
        scope,
        1,
        [
            MemberId::new(1).expect("first signer"),
            MemberId::new(3).expect("substituted signer"),
        ],
    )
    .expect("same-attempt signer substitution");
    {
        let store = RedbNonceStore::open(&database_path).expect("attempt store");
        store.activate_attempt(&base).expect("base attempt");
        store.activate_attempt(&newer).expect("newer attempt");
    }
    let reopened = RedbNonceStore::open(&database_path).expect("reopened attempt store");
    assert!(reopened.activate_attempt(&base).is_err());
    assert!(reopened.activate_attempt(&substituted).is_err());
    reopened
        .activate_attempt(&newer)
        .expect("exact active-attempt replay");
}

#[test]
fn abort_timeout_ambiguity_and_signer_substitution_burn_records() {
    let stores = (0..3)
        .map(|_| FailingNonceStore::default())
        .collect::<Vec<_>>();
    let stores_for_managers = stores.clone();
    let mut managers = managers_with_store(3, 2, 108, move |member| {
        stores_for_managers[usize::from(member.get() - 1)].clone()
    });
    let reasons = [
        NonceBurnReason::Aborted,
        NonceBurnReason::AmbiguousResponse,
        NonceBurnReason::Timeout,
        NonceBurnReason::CoordinatorLost,
        NonceBurnReason::SignerSetChanged,
    ];
    let mut rng = ChaCha20Rng::from_seed([109; 32]);
    for (index, reason) in reasons.into_iter().enumerate() {
        let marker = index as u8 + 1;
        let scope = SigningAuthorizationScope::new(
            [108; 32],
            RootKeyEpoch::new(1).expect("root epoch"),
            wallet_registration(),
            SigningDerivation::application_key().binding(),
            [marker; 32],
            [marker.wrapping_add(20); 32],
        )
        .expect("burn scope");
        let context = SigningSessionContext::new(
            scope,
            0,
            [
                MemberId::new(1).expect("member one"),
                MemberId::new(2).expect("member two"),
            ],
        )
        .expect("burn context");
        let commitment = managers[0]
            .commit_nonce(&context, &mut rng)
            .expect("burn commitment");
        managers[0]
            .burn_nonce(commitment.nonce_id(), reason)
            .expect("burn nonce");
        assert_eq!(
            stores[0]
                .load_nonce(commitment.nonce_id())
                .expect("load burned nonce")
                .expect("burned nonce")
                .lifecycle(),
            &NonceLifecycle::Burned { reason }
        );
    }

    let registration = wallet_registration();
    let message = [119; 32];
    let original = signing_context(
        registration,
        &SigningDerivation::application_key(),
        session(119),
        message,
        &[0, 1],
    );
    let substituted_scope = SigningAuthorizationScope::new(
        original.federation_id(),
        original.root_epoch(),
        original.registration(),
        original.derivation_binding(),
        original.authorization_reference(),
        original.message_commitment(),
    )
    .expect("substituted scope");
    let substituted = SigningSessionContext::new(
        substituted_scope,
        original.attempt(),
        [
            MemberId::new(1).expect("member one"),
            MemberId::new(3).expect("member three"),
        ],
    )
    .expect("substituted context");
    let first = managers[0]
        .commit_nonce(&original, &mut rng)
        .expect("original commitment");
    let substituted_commitment = managers[2]
        .commit_nonce(&substituted, &mut rng)
        .expect("substituted commitment");
    let mixed =
        NonceCommitmentSet::new([first.clone(), substituted_commitment]).expect("mixed set");
    let request = authorized_request(
        registration,
        original.session_id(),
        message,
        b"wallet-test-policy/v1",
    );
    assert!(managers[0]
        .sign_share(first.nonce_id(), &request, &mixed)
        .is_err());
    assert_eq!(
        stores[0]
            .load_nonce(first.nonce_id())
            .expect("load substituted record")
            .expect("substituted record")
            .lifecycle(),
        &NonceLifecycle::Burned {
            reason: NonceBurnReason::InvalidPackage,
        }
    );
}

#[test]
fn concurrent_session_reservation_has_one_atomic_winner() {
    let source_stores = (0..3)
        .map(|_| FailingNonceStore::default())
        .collect::<Vec<_>>();
    let stores_for_managers = source_stores.clone();
    let mut managers = managers_with_store(3, 2, 101, move |member| {
        stores_for_managers[usize::from(member.get() - 1)].clone()
    });
    let context = signing_context(
        wallet_registration(),
        &SigningDerivation::application_key(),
        session(101),
        [101; 32],
        &[0, 1],
    );
    source_stores[0].arm(3);
    assert!(managers[0]
        .commit_nonce(&context, &mut ChaCha20Rng::from_seed([102; 32]))
        .is_err());
    let record = source_stores[0]
        .nonterminal_records()
        .expect("load source records")
        .into_iter()
        .next()
        .expect("source generated record");
    let competing_store = MemoryNonceStore::default();
    competing_store
        .activate_attempt(&context)
        .expect("activate competing attempt");
    let results = (0..8)
        .map(|_| {
            let store = competing_store.clone();
            let record = record.clone();
            thread::spawn(move || store.reserve(record).expect("atomic reservation"))
        })
        .map(|handle| handle.join().expect("reservation thread"))
        .collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, NonceReservation::Inserted))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, NonceReservation::Existing(_)))
            .count(),
        7
    );
    assert_eq!(
        competing_store
            .nonterminal_records()
            .expect("competing records")
            .len(),
        1
    );
}

#[test]
fn redb_nonce_store_survives_reopen_and_compacts_to_replay_tombstone() {
    let source_stores = (0..3)
        .map(|_| FailingNonceStore::default())
        .collect::<Vec<_>>();
    let stores_for_managers = source_stores.clone();
    let mut managers = managers_with_store(3, 2, 105, move |member| {
        stores_for_managers[usize::from(member.get() - 1)].clone()
    });
    let context = signing_context(
        wallet_registration(),
        &SigningDerivation::application_key(),
        session(105),
        [105; 32],
        &[0, 1],
    );
    source_stores[0].arm(3);
    assert!(managers[0]
        .commit_nonce(&context, &mut ChaCha20Rng::from_seed([106; 32]))
        .is_err());
    let record = source_stores[0]
        .nonterminal_records()
        .expect("load source records")
        .into_iter()
        .next()
        .expect("source generated record");

    let directory = tempfile::tempdir().expect("temporary nonce directory");
    let database_path = directory.path().join("participant-nonces.redb");
    {
        let store = RedbNonceStore::open(&database_path).expect("open redb store");
        store
            .activate_attempt(&context)
            .expect("activate redb attempt");
        assert_eq!(
            store.reserve(record.clone()).expect("reserve in redb"),
            NonceReservation::Inserted
        );
    }
    let store = RedbNonceStore::open(&database_path).expect("reopen redb store");
    assert_eq!(
        store
            .load_nonce(record.nonce_id())
            .expect("load reopened record")
            .expect("reopened record"),
        record
    );
    let burned = record
        .transition(
            NonceLifecycle::Burned {
                reason: NonceBurnReason::Timeout,
            },
            false,
            None,
        )
        .expect("burned replacement");
    store
        .compare_and_swap(&record, burned)
        .expect("terminal redb transition");
    assert_eq!(store.compact_terminal(0).expect("compact redb record"), 1);
    drop(store);
    let reopened = RedbNonceStore::open(&database_path).expect("reopen compacted store");
    assert!(reopened.load_nonce(record.nonce_id()).is_err());
    assert!(reopened.reserve(record).is_err());
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

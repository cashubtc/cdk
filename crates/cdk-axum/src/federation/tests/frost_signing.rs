use cdk::federation::{FederationBarkVirtualMusigPartialRequest, FederationFrostCommitmentRequest};
use cdk_frost::{
    advance_dkg, derive_application_public, finish_dkg, start_dkg, ApplicationAuthorization,
    ApplicationRegistration, AuthorizationRequest, AuthorizedSigningRequest, CustodyProtocol,
    DkgContext, DkgRound1Package, DkgRound2Package, MemoryNonceStore, ParticipantKeyManager,
    RootKeyEpoch, SecretEncryptionKey, SigningAuthorizationScope, SigningDerivation,
    SigningSessionContext, ThresholdDhRequest, VirtualMusigTranscript,
};
use cdk_frost_bip32::application_registration;
use musig2::secp::Scalar as MusigScalar;
use musig2::{BinaryEncoding, SecNonce};

use super::*;

#[derive(Debug, Clone)]
struct TestAuthorizationAdapter {
    registration: ApplicationRegistration,
    payload: Vec<u8>,
    message: [u8; 32],
    derivation: SigningDerivation,
}

impl ApplicationAuthorization for TestAuthorizationAdapter {
    fn registration(&self) -> ApplicationRegistration {
        self.registration
    }

    fn authorize(
        &self,
        request: &AuthorizationRequest,
    ) -> Result<AuthorizedSigningRequest, cdk_frost::Error> {
        if request.payload() != self.payload
            || request.application_id() != self.registration.id()
            || request.application_key_epoch() != self.registration.key_epoch()
            || request.derivation_version() != self.registration.derivation_version()
        {
            return Err(cdk_frost::Error::Application(
                "test authorization mismatch".to_string(),
            ));
        }
        Ok(AuthorizedSigningRequest::new(
            self.message,
            self.derivation.clone(),
        ))
    }
}

fn frost_roots(
    federation_id: [u8; 32],
    members: u16,
    threshold: u16,
) -> Vec<cdk_frost::CanonicalRootKeyPackage> {
    let participants = (1..=members)
        .map(|member| cdk_frost::MemberId::new(member).expect("member"))
        .collect::<BTreeSet<_>>();
    let context = DkgContext::new(
        federation_id,
        [42; 32],
        RootKeyEpoch::new(1).expect("root epoch"),
        threshold,
        participants.clone(),
    )
    .expect("DKG context");
    let round1 = participants
        .iter()
        .copied()
        .map(|participant| {
            start_dkg(context.clone(), participant, cdk_frost::rand_core::OsRng).expect("start DKG")
        })
        .collect::<Vec<_>>();
    let public_round1 = round1
        .iter()
        .map(|(_, package)| package.clone())
        .collect::<Vec<DkgRound1Package>>();
    let mut round2 = round1
        .into_iter()
        .map(|(secret, own)| {
            let others = public_round1
                .iter()
                .filter(|package| package.sender() != own.sender())
                .cloned();
            let (secret, outgoing) = advance_dkg(secret, others).expect("advance DKG");
            (Some(secret), outgoing)
        })
        .collect::<Vec<_>>();
    let mut roots = Vec::with_capacity(round2.len());
    for local_index in 0..round2.len() {
        let local = cdk_frost::MemberId::new(u16::try_from(local_index + 1).expect("member index"))
            .expect("member");
        let incoming = round2
            .iter()
            .enumerate()
            .filter(|(sender_index, _)| *sender_index != local_index)
            .map(|(_, (_, outgoing))| outgoing.get(&local).expect("incoming package").clone())
            .collect::<Vec<DkgRound2Package>>();
        let secret = round2[local_index].0.take().expect("round two secret");
        let others = public_round1
            .iter()
            .filter(|package| package.sender() != local)
            .cloned();
        roots.push(finish_dkg(secret, others, incoming).expect("finish DKG"));
    }
    roots
}

async fn run_live_signing_case(
    public_config: FederationConfig,
    private_configs: Vec<MemberConfigPrivate>,
    threshold: u16,
    failure: SigningFailure,
) {
    let registration = application_registration().expect("application registration");
    let derivation = SigningDerivation::application_key();
    let message = [91; 32];
    let payload = b"typed-test-authorization-v1".to_vec();
    let roots = frost_roots(
        public_config.federation_id.to_bytes(),
        u16::try_from(private_configs.len()).expect("member count"),
        threshold,
    );
    let public = derive_application_public(roots[0].public(), registration)
        .expect("application public package");
    let signer_set = (1..=threshold)
        .map(|member| cdk_frost::MemberId::new(member).expect("signer"))
        .collect::<Vec<_>>();
    let scope = SigningAuthorizationScope::new(
        public_config.federation_id.to_bytes(),
        RootKeyEpoch::new(1).expect("root epoch"),
        registration,
        derivation.binding(),
        [73; 32],
        message,
    )
    .expect("authorization scope");
    let context = SigningSessionContext::new(scope, 1, signer_set).expect("signing context");
    let authorization = AuthorizationRequest::new(
        registration.id(),
        registration.key_epoch(),
        registration.derivation_version(),
        context.session_id(),
        payload.clone(),
    )
    .expect("authorization request");
    let retry_context = SigningSessionContext::new(
        SigningAuthorizationScope::new(
            public_config.federation_id.to_bytes(),
            RootKeyEpoch::new(1).expect("root epoch"),
            registration,
            derivation.binding(),
            [73; 32],
            message,
        )
        .expect("retry scope"),
        2,
        context.signer_set().iter().copied(),
    )
    .expect("retry context");
    let retry_authorization = AuthorizationRequest::new(
        registration.id(),
        registration.key_epoch(),
        registration.derivation_version(),
        retry_context.session_id(),
        payload.clone(),
    )
    .expect("retry authorization request");

    let transport = RouterFederationTransportRegistry::default();
    for (index, (private, root)) in private_configs.iter().zip(roots).enumerate() {
        let mut manager = ParticipantKeyManager::with_nonce_store(
            root,
            SecretEncryptionKey::new([u8::try_from(index + 1).expect("key byte"); 32])
                .expect("nonce encryption key"),
            MemoryNonceStore::default(),
        )
        .expect("key manager");
        manager
            .register_application(TestAuthorizationAdapter {
                registration,
                payload: payload.clone(),
                message,
                derivation: derivation.clone(),
            })
            .expect("register application");
        let runtime = super::super::frost_signing::FederationFrostSigningRuntime::for_test(
            BTreeMap::from([(1, Arc::new(tokio::sync::Mutex::new(manager)))]),
            [
                (context.clone(), authorization.clone(), derivation.clone()),
                (
                    retry_context.clone(),
                    retry_authorization.clone(),
                    derivation.clone(),
                ),
            ],
        );
        let mut state =
            federation_route_state_for_private(public_config.clone(), private.clone()).await;
        state.federation.set_frost_signing_for_test(Some(runtime));
        let router = Router::new()
            .nest("/api/federation/v1", create_federation_router())
            .with_state(state);
        if index < usize::from(threshold) {
            transport.insert(
                public_config
                    .member(private.member_id)
                    .expect("configured member")
                    .federation_api_url
                    .clone(),
                router,
            );
        }
    }
    let client = FederationRestPeerClient::new(
        transport.clone(),
        &public_config,
        private_configs[0].clone(),
    )
    .expect("peer client");
    let coordinator = super::super::frost_signing::FederationFrostSigningCoordinator::new(
        &client,
        public.clone(),
    );
    if matches!(failure, SigningFailure::Commitment | SigningFailure::Share) {
        let path = match failure {
            SigningFailure::Commitment => "frost/signing/commitments",
            SigningFailure::Share => "frost/signing/packages",
            SigningFailure::None | SigningFailure::AggregateResult => unreachable!(),
        };
        transport.fail_post_responses_after_delivery(path, 1);
        coordinator
            .sign(&public_config, &context, &derivation, &authorization)
            .await
            .expect_err("lost response must fail the first attempt");
        coordinator
            .sign(
                &public_config,
                &retry_context,
                &derivation,
                &retry_authorization,
            )
            .await
            .expect("abort must burn ambiguous nonces before retry attempt");
    } else if failure == SigningFailure::AggregateResult {
        transport.fail_post_responses_after_delivery("frost/signing/results", 1);
        coordinator
            .sign(&public_config, &context, &derivation, &authorization)
            .await
            .expect_err("lost aggregate notification response must fail the coordinator");
        let failover_client = FederationRestPeerClient::new(
            transport,
            &public_config,
            private_configs
                .last()
                .expect("replacement coordinator config")
                .clone(),
        )
        .expect("replacement coordinator client");
        super::super::frost_signing::FederationFrostSigningCoordinator::new(
            &failover_client,
            public,
        )
        .sign(&public_config, &context, &derivation, &authorization)
        .await
        .expect("another member aggregates cached shares for the exact active attempt");
    } else {
        coordinator
            .sign(&public_config, &context, &derivation, &authorization)
            .await
            .expect("threshold signature over live private REST routes");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SigningFailure {
    None,
    Commitment,
    Share,
    AggregateResult,
}

#[tokio::test]
async fn live_private_rest_frost_signing_succeeds_two_of_three() {
    let (public, first, second, third) = transport_config_three();
    run_live_signing_case(public, vec![first, second, third], 2, SigningFailure::None).await;
}

#[tokio::test]
async fn live_private_rest_frost_signing_succeeds_three_of_five() {
    let (public, private) =
        transport_config_five_with_threshold(ThresholdParams::new(5, 3, 3).expect("threshold"));
    run_live_signing_case(public, private, 3, SigningFailure::None).await;
}

#[tokio::test]
async fn lost_post_signer_response_burns_nonce_and_retry_attempt_succeeds() {
    let (public, first, second, third) = transport_config_three();
    run_live_signing_case(
        public,
        vec![first, second, third],
        2,
        SigningFailure::Commitment,
    )
    .await;
}

#[tokio::test]
async fn lost_share_response_burns_attempt_and_fresh_attempt_succeeds() {
    let (public, first, second, third) = transport_config_three();
    run_live_signing_case(public, vec![first, second, third], 2, SigningFailure::Share).await;
}

#[tokio::test]
async fn another_member_aggregates_cached_shares_after_coordinator_loss() {
    let (public, first, second, third) = transport_config_three();
    run_live_signing_case(
        public,
        vec![first, second, third],
        2,
        SigningFailure::AggregateResult,
    )
    .await;
}

#[tokio::test]
async fn catching_up_member_refuses_frost_commitments_before_nonce_creation() {
    let (public, first, second, _) = transport_config_three();
    let registration = application_registration().expect("application registration");
    let derivation = SigningDerivation::application_key();
    let message = [52; 32];
    let payload = b"typed-timeout-authorization".to_vec();
    let root = frost_roots(public.federation_id.to_bytes(), 3, 2)
        .into_iter()
        .next()
        .expect("first root");
    let mut manager = ParticipantKeyManager::with_nonce_store(
        root,
        SecretEncryptionKey::new([121; 32]).expect("nonce encryption key"),
        MemoryNonceStore::default(),
    )
    .expect("key manager");
    manager
        .register_application(TestAuthorizationAdapter {
            registration,
            payload: payload.clone(),
            message,
            derivation: derivation.clone(),
        })
        .expect("register application");
    let (context, _, authorization, request) = transport_request_fixture(
        public.federation_id,
        vec![cdk_frost::MemberId::new(1).expect("first signer")],
    );
    let manager = Arc::new(tokio::sync::Mutex::new(manager));
    let runtime = super::super::frost_signing::FederationFrostSigningRuntime::for_test(
        BTreeMap::from([(1, Arc::clone(&manager))]),
        [(context, authorization, derivation)],
    );
    let mut state = federation_route_state_for_private(public.clone(), first.clone()).await;
    state.federation.set_frost_signing_for_test(Some(runtime));
    state.mark_federation_catching_up(second.member_id).await;

    let transport = RouterFederationTransportRegistry::default();
    transport.insert(
        public
            .member(first.member_id)
            .expect("first member")
            .federation_api_url
            .clone(),
        Router::new()
            .nest("/api/federation/v1", create_federation_router())
            .with_state(state),
    );
    let client = FederationRestPeerClient::new(transport, &public, second).expect("peer client");
    let error = client
        .request_frost_commitment(
            public.member(first.member_id).expect("first member"),
            &request,
        )
        .await
        .expect_err("catching-up signer must fail closed");
    assert!(error.to_string().contains("503"));
    assert!(manager
        .lock()
        .await
        .nonce_store()
        .nonterminal_records()
        .expect("nonce records")
        .is_empty());
}

fn transport_request_fixture(
    federation_id: cdk::federation::FederationId,
    signers: Vec<cdk_frost::MemberId>,
) -> (
    SigningSessionContext,
    SigningDerivation,
    AuthorizationRequest,
    FederationFrostCommitmentRequest,
) {
    let registration = application_registration().expect("application registration");
    let derivation = SigningDerivation::application_key();
    let context = SigningSessionContext::new(
        SigningAuthorizationScope::new(
            federation_id.to_bytes(),
            RootKeyEpoch::new(1).expect("root epoch"),
            registration,
            derivation.binding(),
            [51; 32],
            [52; 32],
        )
        .expect("scope"),
        1,
        signers,
    )
    .expect("context");
    let authorization = AuthorizationRequest::new(
        registration.id(),
        registration.key_epoch(),
        registration.derivation_version(),
        context.session_id(),
        b"typed-timeout-authorization".to_vec(),
    )
    .expect("authorization");
    let request = FederationFrostCommitmentRequest::new(&context, &derivation, &authorization);
    (context, derivation, authorization, request)
}

#[tokio::test]
async fn frost_private_rest_times_out_and_enforces_outbound_backpressure() {
    let (public, first, second, _) = transport_config_three();
    let (_, _, _, request) = transport_request_fixture(
        public.federation_id,
        vec![
            cdk_frost::MemberId::new(1).expect("first"),
            cdk_frost::MemberId::new(2).expect("second"),
        ],
    );
    let sync_config = FederationPeerSyncConfig {
        peer_request_timeout: Duration::from_millis(5),
        private_rest_outbound_concurrency_limit: 1,
        ..FederationPeerSyncConfig::default()
    };
    let client = FederationRestPeerClient::new_with_config(
        DelayTransport {
            delay: Duration::from_millis(50),
            request_started: None,
        },
        &public,
        first,
        sync_config,
    )
    .expect("peer client");
    let member = public.member(second.member_id).expect("second member");
    let timeout = client
        .request_frost_commitment(member, &request)
        .await
        .expect_err("request must time out");
    assert!(timeout
        .downcast_ref::<FederationRestPeerClientError>()
        .is_some_and(|error| matches!(
            error,
            FederationRestPeerClientError::RequestTimeout { .. }
        )));

    let _permit = client
        .outbound_admission
        .clone()
        .try_acquire_owned()
        .expect("hold outbound permit");
    let full = client
        .request_frost_commitment(member, &request)
        .await
        .expect_err("outbound queue must reject excess work");
    assert!(full
        .downcast_ref::<FederationRestPeerClientError>()
        .is_some_and(|error| matches!(
            error,
            FederationRestPeerClientError::OutboundAdmissionFull { .. }
        )));
}

#[tokio::test]
async fn frost_coordinator_rejects_threshold_minus_one_before_transport() {
    let (public_config, first, _, _) = transport_config_three();
    let roots = frost_roots(public_config.federation_id.to_bytes(), 3, 2);
    let registration = application_registration().expect("registration");
    let public = derive_application_public(roots[0].public(), registration).expect("public");
    let (context, derivation, authorization, _) = transport_request_fixture(
        public_config.federation_id,
        vec![cdk_frost::MemberId::new(1).expect("one signer")],
    );
    let client = FederationRestPeerClient::new(
        RouterFederationTransportRegistry::default(),
        &public_config,
        first,
    )
    .expect("peer client");
    let coordinator =
        super::super::frost_signing::FederationFrostSigningCoordinator::new(&client, public);
    assert!(coordinator
        .sign(&public_config, &context, &derivation, &authorization)
        .await
        .is_err());
}

#[tokio::test]
async fn bark_custody_transport_rotates_attempts_and_retries_exact_results() {
    let (public_config, first, second, third) = transport_config_three();
    let private_configs = [first.clone(), second, third.clone()];
    let registration = application_registration().expect("application registration");
    let derivation = SigningDerivation::application_key();
    let message = [141; 32];
    let payload = b"typed-bark-transport-authorization-v1".to_vec();
    let roots = frost_roots(public_config.federation_id.to_bytes(), 3, 2);
    let public = derive_application_public(roots[0].public(), registration)
        .expect("application public package");
    let scope = SigningAuthorizationScope::new(
        public_config.federation_id.to_bytes(),
        RootKeyEpoch::new(1).expect("root epoch"),
        registration,
        derivation.binding(),
        [142; 32],
        message,
    )
    .expect("Bark authorization scope");
    let member = |value| cdk_frost::MemberId::new(value).expect("member");
    let initial = SigningSessionContext::new_for_protocol(
        scope.clone(),
        1,
        [member(1), member(2)],
        CustodyProtocol::VirtualMusig2,
    )
    .expect("initial virtual MuSig2 context");
    let mixed_same_attempt = SigningSessionContext::new_for_protocol(
        scope.clone(),
        1,
        [member(1), member(3)],
        CustodyProtocol::VirtualMusig2,
    )
    .expect("mixed same-attempt context");
    let retry = SigningSessionContext::new_for_protocol(
        scope.clone(),
        2,
        [member(1), member(3)],
        CustodyProtocol::VirtualMusig2,
    )
    .expect("higher virtual MuSig2 attempt");
    let dh_context = SigningSessionContext::new_for_protocol(
        scope,
        1,
        [member(1), member(3)],
        CustodyProtocol::ThresholdDh,
    )
    .expect("threshold-DH context");
    let authorization_for = |context: &SigningSessionContext| {
        AuthorizationRequest::new(
            registration.id(),
            registration.key_epoch(),
            registration.derivation_version(),
            context.session_id(),
            payload.clone(),
        )
        .expect("Bark authorization")
    };
    let initial_authorization = authorization_for(&initial);
    let mixed_authorization = authorization_for(&mixed_same_attempt);
    let retry_authorization = authorization_for(&retry);
    let dh_authorization = authorization_for(&dh_context);
    let accepted = [
        (
            initial.clone(),
            initial_authorization.clone(),
            derivation.clone(),
        ),
        (
            mixed_same_attempt.clone(),
            mixed_authorization.clone(),
            derivation.clone(),
        ),
        (
            retry.clone(),
            retry_authorization.clone(),
            derivation.clone(),
        ),
        (
            dh_context.clone(),
            dh_authorization.clone(),
            derivation.clone(),
        ),
    ];

    let transport = RouterFederationTransportRegistry::default();
    for (index, (private, root)) in private_configs.iter().zip(roots).enumerate() {
        let mut manager = ParticipantKeyManager::with_nonce_store(
            root,
            SecretEncryptionKey::new([u8::try_from(index + 151).expect("key byte"); 32])
                .expect("nonce encryption key"),
            MemoryNonceStore::default(),
        )
        .expect("key manager");
        manager
            .register_application(TestAuthorizationAdapter {
                registration,
                payload: payload.clone(),
                message,
                derivation: derivation.clone(),
            })
            .expect("register Bark test application");
        let runtime = super::super::frost_signing::FederationFrostSigningRuntime::for_test(
            BTreeMap::from([(1, Arc::new(tokio::sync::Mutex::new(manager)))]),
            accepted.clone(),
        );
        let mut state =
            federation_route_state_for_private(public_config.clone(), private.clone()).await;
        state.federation.set_frost_signing_for_test(Some(runtime));
        if index != 1 {
            transport.insert(
                public_config
                    .member(private.member_id)
                    .expect("configured member")
                    .federation_api_url
                    .clone(),
                Router::new()
                    .nest("/api/federation/v1", create_federation_router())
                    .with_state(state),
            );
        }
    }
    let client = FederationRestPeerClient::new_with_config(
        transport,
        &public_config,
        first.clone(),
        FederationPeerSyncConfig {
            private_rest_outbound_concurrency_limit: 1,
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("bounded Bark peer client");
    let coordinator =
        super::super::bark_custody::FederationBarkCustodyCoordinator::new(&client, public.clone());

    coordinator
        .collect_virtual_musig_commitments(
            &public_config,
            &initial,
            &derivation,
            &initial_authorization,
        )
        .await
        .expect_err("unavailable selected signer must fail the accepted attempt");
    coordinator
        .collect_virtual_musig_commitments(
            &public_config,
            &mixed_same_attempt,
            &derivation,
            &mixed_authorization,
        )
        .await
        .expect_err("signer replacement at the same attempt must remain rejected");
    let commitments = coordinator
        .collect_virtual_musig_commitments(
            &public_config,
            &retry,
            &derivation,
            &retry_authorization,
        )
        .await
        .expect("higher accepted attempt replaces the unavailable signer");
    assert_eq!(
        coordinator
            .collect_virtual_musig_commitments(
                &public_config,
                &retry,
                &derivation,
                &retry_authorization,
            )
            .await
            .expect("exact commitment retry"),
        commitments
    );

    let target = cdk_frost::derive_signing_public(&public, &derivation)
        .expect("derived Bark signing target");
    let user_key =
        musig2::secp::Point::try_from(&target.output_group_key()).expect("virtual user key");
    let server_secret = MusigScalar::try_from([161; 32]).expect("server scalar");
    let server_key = server_secret.base_point_mul();
    let server_nonce = SecNonce::new(
        MusigScalar::try_from([162; 32]).expect("server nonce one"),
        MusigScalar::try_from([163; 32]).expect("server nonce two"),
        server_key,
    );
    let transcript = VirtualMusigTranscript::new(
        retry.session_id(),
        [user_key.serialize(), server_key.serialize()],
        user_key.serialize(),
        commitments
            .aggregate_public_nonce()
            .expect("aggregate virtual public nonce"),
        server_nonce.public_nonce().to_bytes(),
        message,
        None,
    )
    .expect("final Bark outer transcript");
    let combined = coordinator
        .collect_virtual_musig_partial(
            &public_config,
            &retry,
            &derivation,
            &retry_authorization,
            &commitments,
            &transcript,
        )
        .await
        .expect("remote virtual MuSig2 partial");
    assert_eq!(
        coordinator
            .collect_virtual_musig_partial(
                &public_config,
                &retry,
                &derivation,
                &retry_authorization,
                &commitments,
                &transcript,
            )
            .await
            .expect("exact completed virtual partial retry"),
        combined
    );
    let own = commitments
        .commitments()
        .get(&member(1))
        .expect("first virtual commitment");
    let mut mixed_request = FederationBarkVirtualMusigPartialRequest::new(
        &mixed_same_attempt,
        own.nonce_id(),
        &mixed_authorization,
        &commitments,
        &transcript,
    );
    mixed_request.transcript_binding[0] ^= 1;
    assert!(mixed_request
        .decode(public_config.federation_id, first.member_id)
        .is_err());

    let dh_request =
        ThresholdDhRequest::new(dh_context.session_id(), server_key.serialize(), message)
            .expect("threshold-DH request");
    let shared = coordinator
        .threshold_dh(
            &public_config,
            &dh_context,
            &derivation,
            &dh_authorization,
            &dh_request,
        )
        .await
        .expect("remote threshold-DH");
    assert_eq!(
        coordinator
            .threshold_dh(
                &public_config,
                &dh_context,
                &derivation,
                &dh_authorization,
                &dh_request,
            )
            .await
            .expect("exact threshold-DH retry"),
        shared
    );
    coordinator
        .collect_virtual_musig_commitments(
            &public_config,
            &initial,
            &derivation,
            &initial_authorization,
        )
        .await
        .expect_err("stale delayed attempt must remain rejected");
}

#[test]
fn bark_custody_authorization_tracks_attempt_across_durable_checkpoints() {
    use cdk::federation::{FederationBarkActionKind as Kind, FederationBarkSemanticPhase as Phase};

    let is_current = super::super::frost_signing::bark_custody_attempt_is_current;
    assert!(is_current(
        Kind::IncomingClaim,
        Phase::SigningAttemptAdvanced,
        Some(Phase::ClaimPlanned),
        4,
        4,
    ));
    assert!(is_current(
        Kind::IncomingClaim,
        Phase::ReceivePreimageDisclosed,
        None,
        4,
        4,
    ));
    assert!(is_current(
        Kind::EmergencyExit,
        Phase::ExitRegistered,
        None,
        2,
        2,
    ));
    assert!(!is_current(
        Kind::IncomingClaim,
        Phase::ReceivePreimageDisclosed,
        None,
        4,
        3,
    ));
    assert!(!is_current(
        Kind::IncomingClaim,
        Phase::ReceiveClaimed,
        None,
        4,
        4,
    ));
}

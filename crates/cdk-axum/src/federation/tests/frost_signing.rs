#[cfg(feature = "federation-bark")]
use cdk::federation::FederationBarkVirtualMusigPartialRequest;
use cdk::federation::{
    FederationFrostBatchCommitmentItem, FederationFrostBatchCommitmentRequest,
    FederationFrostBatchSigningAbortItem, FederationFrostBatchSigningAbortRequest,
    FederationFrostCommitmentRequest, FEDERATION_MAX_FROST_SIGNING_BATCH_ITEMS,
};
use cdk_federation::test_support::cdk_frost::{
    advance_dkg, derive_application_public, finish_dkg, start_dkg, ApplicationAuthorization,
    ApplicationRegistration, AuthorizationRequest, AuthorizedSigningRequest, DkgContext,
    DkgRound1Package, DkgRound2Package, MemoryNonceStore, NonceBurnReason, ParticipantKeyManager,
    RedbNonceStore, RootKeyEpoch, SecretEncryptionKey, SigningAuthorizationScope,
    SigningDerivation, SigningSessionContext,
};
#[cfg(feature = "federation-bark")]
use cdk_federation::test_support::cdk_frost::{
    CustodyProtocol, ThresholdDhRequest, VirtualMusigTranscript,
};
use cdk_federation::test_support::cdk_frost_bip32::application_registration;
#[cfg(feature = "federation-bark")]
use cdk_federation::test_support::musig2::secp::Scalar as MusigScalar;
#[cfg(feature = "federation-bark")]
use cdk_federation::test_support::musig2::{BinaryEncoding, SecNonce};

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
    ) -> Result<AuthorizedSigningRequest, cdk_federation::test_support::cdk_frost::Error> {
        if request.payload() != self.payload
            || request.application_id() != self.registration.id()
            || request.application_key_epoch() != self.registration.key_epoch()
            || request.derivation_version() != self.registration.derivation_version()
        {
            return Err(cdk_federation::test_support::cdk_frost::Error::Application(
                "test authorization mismatch".to_string(),
            ));
        }
        Ok(AuthorizedSigningRequest::new(
            self.message,
            self.derivation.clone(),
        ))
    }
}

pub(super) fn frost_roots(
    federation_id: [u8; 32],
    members: u16,
    threshold: u16,
) -> Vec<cdk_federation::test_support::cdk_frost::CanonicalRootKeyPackage> {
    let participants = (1..=members)
        .map(|member| {
            cdk_federation::test_support::cdk_frost::MemberId::new(member).expect("member")
        })
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
            start_dkg(
                context.clone(),
                participant,
                cdk_federation::test_support::cdk_frost::rand_core::OsRng,
            )
            .expect("start DKG")
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
        let local = cdk_federation::test_support::cdk_frost::MemberId::new(
            u16::try_from(local_index + 1).expect("member index"),
        )
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
    phase_delays: &[Duration],
) -> Duration {
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
        .map(|member| {
            cdk_federation::test_support::cdk_frost::MemberId::new(member).expect("signer")
        })
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
    let mut local_signer = None;
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
        state
            .federation
            .set_frost_signing_for_test(Some(runtime.clone()));
        if index == 0 {
            local_signer = Some((private.member_id, Arc::clone(&state.mint), runtime.clone()));
        }
        let router = Router::new()
            .nest("/api/federation/v1", create_federation_router())
            .with_state(state);
        if index > 0 && index < usize::from(threshold) {
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
    if !phase_delays.is_empty() {
        assert_eq!(phase_delays.len(), context.signer_set().len());
        for (signer, phase_delay) in context.signer_set().iter().zip(phase_delays) {
            let receiver = MemberId::new(signer.get()).expect("configured signer");
            for path in [
                "frost/signing/commitments",
                "frost/signing/packages",
                "frost/signing/results",
            ] {
                transport.delay_next_posts_before_delivery(path, receiver, [*phase_delay]);
            }
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
    let (local_member_id, local_mint, local_runtime) =
        local_signer.expect("first member local signer");
    let coordinator =
        coordinator.with_local_signer(local_member_id, local_mint.as_ref(), &local_runtime);
    let started = tokio::time::Instant::now();
    if failure != SigningFailure::None {
        let path = match failure {
            SigningFailure::Commitment => "frost/signing/commitments",
            SigningFailure::Share => "frost/signing/packages",
            SigningFailure::AggregateResult => "frost/signing/results",
            SigningFailure::None => unreachable!(),
        };
        transport.fail_post_responses_after_delivery(path, 1);
        coordinator
            .sign(&public_config, &context, &derivation, &authorization)
            .await
            .expect("lost idempotent response must recover within the first signing attempt");
    } else {
        coordinator
            .sign(&public_config, &context, &derivation, &authorization)
            .await
            .expect("threshold signature over live private REST routes");
    }
    started.elapsed()
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
    run_live_signing_case(
        public,
        vec![first, second, third],
        2,
        SigningFailure::None,
        &[],
    )
    .await;
}

#[tokio::test]
async fn live_private_rest_frost_signing_succeeds_three_of_five() {
    let (public, private) =
        transport_config_five_with_threshold(ThresholdParams::new(5, 3, 3).expect("threshold"));
    run_live_signing_case(public, private, 3, SigningFailure::None, &[]).await;
}

#[tokio::test]
async fn live_private_rest_frost_phases_wait_for_slowest_peer_not_sum() {
    let (public, private) =
        transport_config_five_with_threshold(ThresholdParams::new(5, 3, 3).expect("threshold"));
    let elapsed = run_live_signing_case(
        public,
        private,
        3,
        SigningFailure::None,
        &[
            Duration::from_millis(40),
            Duration::from_millis(70),
            Duration::from_millis(100),
        ],
    )
    .await;

    assert!(
        elapsed < Duration::from_millis(600),
        "three differently delayed private RPC phases should wait for the 100 ms slowest signer in each phase: {elapsed:?}"
    );
}

#[tokio::test]
async fn lost_commitment_response_recovers_in_same_frost_attempt() {
    let (public, first, second, third) = transport_config_three();
    run_live_signing_case(
        public,
        vec![first, second, third],
        2,
        SigningFailure::Commitment,
        &[],
    )
    .await;
}

#[tokio::test]
async fn lost_share_response_recovers_in_same_frost_attempt() {
    let (public, first, second, third) = transport_config_three();
    run_live_signing_case(
        public,
        vec![first, second, third],
        2,
        SigningFailure::Share,
        &[],
    )
    .await;
}

#[tokio::test]
async fn lost_aggregate_ack_recovers_in_same_frost_attempt() {
    let (public, first, second, third) = transport_config_three();
    run_live_signing_case(
        public,
        vec![first, second, third],
        2,
        SigningFailure::AggregateResult,
        &[],
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
        vec![cdk_federation::test_support::cdk_frost::MemberId::new(1).expect("first signer")],
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
    signers: Vec<cdk_federation::test_support::cdk_frost::MemberId>,
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
            cdk_federation::test_support::cdk_frost::MemberId::new(1).expect("first"),
            cdk_federation::test_support::cdk_frost::MemberId::new(2).expect("second"),
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
        vec![cdk_federation::test_support::cdk_frost::MemberId::new(1).expect("one signer")],
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

#[cfg(feature = "federation-bark")]
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
    let member =
        |value| cdk_federation::test_support::cdk_frost::MemberId::new(value).expect("member");
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

    let target =
        cdk_federation::test_support::cdk_frost::derive_signing_public(&public, &derivation)
            .expect("derived Bark signing target");
    let user_key =
        cdk_federation::test_support::musig2::secp::Point::try_from(&target.output_group_key())
            .expect("virtual user key");
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

#[cfg(feature = "federation-bark")]
#[test]
fn bark_custody_authorization_tracks_attempt_across_durable_checkpoints() {
    use cdk::federation::{FederationBarkActionKind as Kind, FederationBarkSemanticPhase as Phase};

    let is_current = cdk::federation::federation_bark_custody_attempt_is_current;
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

#[derive(Debug, Clone)]
struct MultiInputTestAuthorizationAdapter {
    registration: ApplicationRegistration,
    entries: BTreeMap<Vec<u8>, ([u8; 32], SigningDerivation)>,
}

impl ApplicationAuthorization for MultiInputTestAuthorizationAdapter {
    fn registration(&self) -> ApplicationRegistration {
        self.registration
    }

    fn authorize(
        &self,
        request: &AuthorizationRequest,
    ) -> Result<AuthorizedSigningRequest, cdk_federation::test_support::cdk_frost::Error> {
        if request.application_id() != self.registration.id()
            || request.application_key_epoch() != self.registration.key_epoch()
            || request.derivation_version() != self.registration.derivation_version()
        {
            return Err(cdk_federation::test_support::cdk_frost::Error::Application(
                "multi-input test authorization metadata mismatch".to_string(),
            ));
        }
        let Some((message, derivation)) = self.entries.get(request.payload()) else {
            return Err(cdk_federation::test_support::cdk_frost::Error::Application(
                "multi-input test authorization payload mismatch".to_string(),
            ));
        };
        Ok(AuthorizedSigningRequest::new(*message, derivation.clone()))
    }
}

async fn run_live_batch_signing_case(
    public_config: FederationConfig,
    private_configs: Vec<MemberConfigPrivate>,
    threshold: u16,
    input_count: usize,
    fail_paths: &[&str],
    phase_delays: &[Duration],
    result_delay: Duration,
) -> Duration {
    let registration = application_registration().expect("application registration");
    let derivation = SigningDerivation::application_key();
    let roots = frost_roots(
        public_config.federation_id.to_bytes(),
        u16::try_from(private_configs.len()).expect("member count"),
        threshold,
    );
    let public = derive_application_public(roots[0].public(), registration)
        .expect("application public package");
    let signer_set = (1..=threshold)
        .map(|member| {
            cdk_federation::test_support::cdk_frost::MemberId::new(member).expect("signer")
        })
        .collect::<Vec<_>>();

    let mut entries = BTreeMap::new();
    let mut batch_inputs = Vec::with_capacity(input_count);
    let mut fixed_authorizations = Vec::with_capacity(input_count);
    for idx in 0..input_count {
        let marker = u8::try_from(idx + 1).expect("input marker");
        let message = [marker.wrapping_add(100); 32];
        let payload = format!("batch-typed-authorization-v1-input-{idx}").into_bytes();
        let scope = SigningAuthorizationScope::new(
            public_config.federation_id.to_bytes(),
            RootKeyEpoch::new(1).expect("root epoch"),
            registration,
            derivation.binding(),
            [marker.wrapping_add(50); 32],
            message,
        )
        .expect("batch scope");
        let context = SigningSessionContext::new(scope, 1, signer_set.iter().copied())
            .expect("batch context");
        let authorization = AuthorizationRequest::new(
            registration.id(),
            registration.key_epoch(),
            registration.derivation_version(),
            context.session_id(),
            payload.clone(),
        )
        .expect("batch authorization");
        entries.insert(payload, (message, derivation.clone()));
        batch_inputs.push((
            u32::try_from(idx).expect("input index"),
            context.clone(),
            derivation.clone(),
            authorization.clone(),
        ));
        fixed_authorizations.push((context, authorization, derivation.clone()));
    }

    let transport = RouterFederationTransportRegistry::default();
    let mut local_signer = None;
    let mut _directories = Vec::new();
    for (index, (private, root)) in private_configs.iter().zip(roots).enumerate() {
        let directory = tempfile::tempdir().expect("member redb directory");
        let store =
            RedbNonceStore::open(directory.path().join("nonces.redb")).expect("redb nonce store");
        let mut manager = ParticipantKeyManager::with_nonce_store(
            root,
            SecretEncryptionKey::new([u8::try_from(index + 11).expect("key byte"); 32])
                .expect("nonce encryption key"),
            store,
        )
        .expect("key manager");
        manager
            .register_application(MultiInputTestAuthorizationAdapter {
                registration,
                entries: entries.clone(),
            })
            .expect("register multi-input adapter");
        let runtime = super::super::frost_signing::FederationFrostSigningRuntime::for_test(
            BTreeMap::from([(1, Arc::new(tokio::sync::Mutex::new(manager)))]),
            fixed_authorizations.clone(),
        );
        let mut state =
            federation_route_state_for_private(public_config.clone(), private.clone()).await;
        state
            .federation
            .set_frost_signing_for_test(Some(runtime.clone()));
        if index == 0 {
            local_signer = Some((private.member_id, Arc::clone(&state.mint), runtime.clone()));
        }
        let router = Router::new()
            .nest("/api/federation/v1", create_federation_router())
            .with_state(state);
        if index > 0 && index < usize::from(threshold) {
            transport.insert(
                public_config
                    .member(private.member_id)
                    .expect("configured member")
                    .federation_api_url
                    .clone(),
                router,
            );
        }
        _directories.push(directory);
    }

    if !phase_delays.is_empty() {
        assert_eq!(phase_delays.len(), signer_set.len());
        for (signer, phase_delay) in signer_set.iter().zip(phase_delays) {
            let receiver = MemberId::new(signer.get()).expect("configured signer");
            for path in [
                "frost/signing/commitments/batch",
                "frost/signing/packages/batch",
            ] {
                transport.delay_next_posts_before_delivery(path, receiver, [*phase_delay]);
            }
            if !result_delay.is_zero() {
                transport.delay_next_posts_before_delivery(
                    "frost/signing/results/batch",
                    receiver,
                    [result_delay],
                );
            }
        }
    }
    for path in fail_paths {
        transport.fail_post_responses_after_delivery(path, 1);
    }

    let client = FederationRestPeerClient::new(
        transport.clone(),
        &public_config,
        private_configs[0].clone(),
    )
    .expect("peer client");
    let (local_member_id, local_mint, local_runtime) =
        local_signer.expect("first member local signer");
    let coordinator = super::super::frost_signing::FederationFrostSigningCoordinator::new(
        &client,
        public.clone(),
    )
    .with_local_signer(local_member_id, local_mint.as_ref(), &local_runtime);

    let started = tokio::time::Instant::now();
    let signatures = coordinator
        .sign_batch(&public_config, &batch_inputs)
        .await
        .expect("multi-input batch FROST signing over live private REST routes");
    let elapsed = started.elapsed();

    assert_eq!(signatures.len(), input_count);
    for ((_, context, derivation, _), signature) in batch_inputs.iter().zip(signatures) {
        ParticipantKeyManager::verify_aggregate_for_derivation(
            &public,
            derivation,
            &context.message_commitment(),
            signature,
        )
        .expect("verified aggregate signature for batched input");
    }
    elapsed
}

#[tokio::test]
async fn live_private_rest_batch_frost_signing_eight_inputs_two_fanouts_and_non_blocking_round3() {
    let (public, private) =
        transport_config_five_with_threshold(ThresholdParams::new(5, 3, 3).expect("threshold"));
    let elapsed = run_live_batch_signing_case(
        public,
        private,
        3,
        8,
        &[],
        &[
            Duration::from_millis(40),
            Duration::from_millis(70),
            Duration::from_millis(100),
        ],
        Duration::from_millis(1_500),
    )
    .await;

    assert!(
        elapsed < Duration::from_millis(700),
        "8-input batch signing must complete in 2 parallel peer fanouts (~200ms) without waiting on the 1.5s round-3 aggregate notification: {elapsed:?}"
    );
}

#[tokio::test]
async fn lost_batch_commitment_and_share_responses_recover_idempotently() {
    let (public, first, second, third) = transport_config_three();
    run_live_batch_signing_case(
        public,
        vec![first, second, third],
        2,
        6,
        &[
            "frost/signing/commitments/batch",
            "frost/signing/packages/batch",
        ],
        &[],
        Duration::ZERO,
    )
    .await;
}

#[test]
fn batch_frost_wire_rejects_empty_oversized_duplicate_and_out_of_order_inputs() {
    let (public, first, second, _) = transport_config_three();
    let signers = vec![
        cdk_federation::test_support::cdk_frost::MemberId::new(1).expect("first"),
        cdk_federation::test_support::cdk_frost::MemberId::new(2).expect("second"),
    ];
    let (context0, derivation0, auth0, req0) =
        transport_request_fixture(public.federation_id, signers.clone());
    let registration = application_registration().expect("registration");
    let context1 = SigningSessionContext::new(
        SigningAuthorizationScope::new(
            public.federation_id.to_bytes(),
            RootKeyEpoch::new(1).expect("root epoch"),
            registration,
            derivation0.binding(),
            [61; 32],
            [62; 32],
        )
        .expect("scope 1"),
        1,
        signers,
    )
    .expect("context 1");
    let auth1 = AuthorizationRequest::new(
        registration.id(),
        registration.key_epoch(),
        registration.derivation_version(),
        context1.session_id(),
        b"typed-timeout-authorization-1".to_vec(),
    )
    .expect("auth 1");
    let req1 = FederationFrostCommitmentRequest::new(&context1, &derivation0, &auth1);

    // Empty batch is rejected.
    let empty = FederationFrostBatchCommitmentRequest::new(Vec::new());
    assert!(empty.decode(public.federation_id, first.member_id).is_err());

    // Duplicate input_index is rejected.
    let duplicate_index = FederationFrostBatchCommitmentRequest::new(vec![
        FederationFrostBatchCommitmentItem {
            input_index: 0,
            request: req0.clone(),
        },
        FederationFrostBatchCommitmentItem {
            input_index: 0,
            request: req1.clone(),
        },
    ]);
    assert!(duplicate_index
        .decode(public.federation_id, second.member_id)
        .is_err());

    // Non-ascending input_index is rejected.
    let out_of_order = FederationFrostBatchCommitmentRequest::new(vec![
        FederationFrostBatchCommitmentItem {
            input_index: 1,
            request: req1.clone(),
        },
        FederationFrostBatchCommitmentItem {
            input_index: 0,
            request: req0.clone(),
        },
    ]);
    assert!(out_of_order
        .decode(public.federation_id, second.member_id)
        .is_err());

    // Duplicate session_id across distinct input_index is rejected.
    let duplicate_session = FederationFrostBatchCommitmentRequest::new(vec![
        FederationFrostBatchCommitmentItem {
            input_index: 0,
            request: req0.clone(),
        },
        FederationFrostBatchCommitmentItem {
            input_index: 1,
            request: req0.clone(),
        },
    ]);
    assert!(duplicate_session
        .decode(public.federation_id, second.member_id)
        .is_err());

    // Oversized batch exceeding FEDERATION_MAX_FROST_SIGNING_BATCH_ITEMS is rejected.
    let oversized = FederationFrostBatchCommitmentRequest::new(
        (0..=FEDERATION_MAX_FROST_SIGNING_BATCH_ITEMS)
            .map(|idx| FederationFrostBatchCommitmentItem {
                input_index: u32::try_from(idx).expect("idx"),
                request: req0.clone(),
            })
            .collect(),
    );
    assert!(oversized
        .decode(public.federation_id, second.member_id)
        .is_err());

    // Valid strictly ascending 2-input batch decodes cleanly.
    let valid = FederationFrostBatchCommitmentRequest::new(vec![
        FederationFrostBatchCommitmentItem {
            input_index: 0,
            request: req0,
        },
        FederationFrostBatchCommitmentItem {
            input_index: 1,
            request: req1,
        },
    ]);
    let decoded = valid
        .decode(public.federation_id, second.member_id)
        .expect("valid batch commitment decodes");
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0].input_index, 0);
    assert_eq!(decoded[1].input_index, 1);

    // Batch abort rejects duplicate input indices too.
    let dup_abort = FederationFrostBatchSigningAbortRequest::new(
        NonceBurnReason::Aborted,
        vec![
            FederationFrostBatchSigningAbortItem {
                input_index: 0,
                context: context0.encode(),
                authorization: auth0.encode(),
            },
            FederationFrostBatchSigningAbortItem {
                input_index: 0,
                context: context1.encode(),
                authorization: auth1.encode(),
            },
        ],
    );
    assert!(dup_abort
        .decode(public.federation_id, second.member_id)
        .is_err());
}

#[cfg(feature = "federation-bdk")]
#[tokio::test]
async fn multi_input_onchain_bdk_transaction_signs_in_two_batch_fanouts_with_redb() {
    use cdk::federation::{
        federation_wallet_signing_attempt_at, FederationAddressAllocation,
        FederationAddressKeychain, FederationAllocatedAddressRef, FederationChainObservationState,
        FederationChainSourceDescriptor, FederationChainSourceScope, FederationDepositConfirmation,
        FederationDepositObservation, FederationFrostApplicationConfig,
        FederationFrostRootPublicConfig, FederationFrostWalletConfig,
        FederationOnchainConfirmationPolicy, FederationOnchainPaymentBatch,
        FederationOnchainPaymentIntent, FederationOnchainPaymentIntentRecord,
        FederationTransactionChangeOutput, FederationTransactionInput,
        FederationTransactionProposal, FederationTransactionRecipientOutput,
        FederationTransactionScope, FederationTransactionState, FederationWalletLivenessSchedule,
    };
    use cdk_common::bitcoin::hashes::Hash;
    use cdk_common::bitcoin::{
        absolute, blockdata::constants::genesis_block, transaction, Amount as BitcoinAmount,
        FeeRate, Network as BitcoinNetwork, OutPoint, ScriptBuf, Sequence, Transaction, TxIn,
        TxOut, Txid, Witness,
    };
    use cdk_federation::test_support::cdk_bitcoin_wallet::{
        ChainSource, EsploraConfig, FederatedBdkWallet, FederatedWalletConfig,
        FederatedWalletKeychain, FederatedWalletScope,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    let (mut config, first, second, third) = transport_config_three();
    let roots = frost_roots(config.federation_id.to_bytes(), 3, 2);
    let public = roots[0].public();
    let root = FederationFrostRootPublicConfig {
        version: 2,
        federation_id: config.federation_id,
        setup_authorization: config.setup_authorization,
        ceremony_id: FederationDkgCeremonyId::from_bytes([43; 32]),
        root_key_epoch: 1,
        dkg_protocol_version: public.dkg_protocol_version(),
        ciphersuite_version: 1,
        threshold: 2,
        participant_mapping: config
            .members
            .keys()
            .map(|member| (*member, member.get()))
            .collect(),
        verification_shares: public
            .verifying_shares()
            .iter()
            .map(|(member, share)| (MemberId::new(member.get()).expect("member"), *share))
            .collect(),
        untweaked_group_key: public.group_key(),
        transcript_binding: public.transcript_binding(),
        canonical_public_package: public.encode(),
    };
    let application = FederationFrostApplicationConfig::new_cdk_onchain(
        &config,
        &root,
        1,
        BitcoinNetwork::Regtest,
        FederationOnchainConfirmationPolicy::new(1).expect("confirmations"),
        1,
    )
    .expect("application");
    config.frost_wallet = Some(
        FederationFrostWalletConfig::new(
            &config,
            root,
            Some(application.clone()),
            Default::default(),
        )
        .expect("wallet config"),
    );
    let scope = FederationTransactionScope {
        root_key_epoch: 1,
        application_id: application.application_id,
        application_key_epoch: 1,
        network: BitcoinNetwork::Regtest,
        policy_version: 1,
    };

    // Minimal mock Esplora backend for watch-only BDK wallet init.
    let genesis = genesis_block(BitcoinNetwork::Regtest);
    let blocks = serde_json::json!([{
        "id": genesis.block_hash(), "height": 0, "version": 1,
        "timestamp": genesis.header.time, "tx_count": 1,
        "size": 285, "weight": 1140, "merkle_root": genesis.header.merkle_root,
        "previousblockhash": null, "mediantime": genesis.header.time,
        "nonce": genesis.header.nonce, "bits": genesis.header.bits.to_consensus(),
        "difficulty": 1.0
    }]);
    let esplora_router = Router::new().route(
        "/blocks",
        axum::routing::get(move || async move { axum::Json(blocks) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock esplora");
    let source = ChainSource::Esplora(EsploraConfig {
        url: format!("http://{}", listener.local_addr().expect("esplora addr")),
        parallel_requests: 1,
        request_timeout_secs: 2,
        max_retries: 0,
    });
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, esplora_router).await;
    });

    let input_count = 6_usize;
    let transport = RouterFederationTransportRegistry::default();
    let mut states = Vec::new();
    let mut wallets = Vec::new();
    let mut _directories = Vec::new();
    for (private, root) in [first, second, third].into_iter().zip(roots) {
        let mut state = federation_route_state_for_private(config.clone(), private.clone()).await;
        let directory = tempfile::tempdir().expect("member dir");
        let store = RedbNonceStore::open(directory.path().join("frost-nonces.redb"))
            .expect("redb nonce store");
        let mut manager = ParticipantKeyManager::with_nonce_store(
            root,
            SecretEncryptionKey::new([private.member_id.get() as u8 + 30; 32]).expect("key"),
            store,
        )
        .expect("FROST manager");
        manager
            .register_application(
                cdk::federation::FederationCdkOnchainFrostAuthorizationAdapter::new(&application)
                    .expect("authorization adapter"),
            )
            .expect("register onchain application");
        let signing = super::super::frost_signing::FederationFrostSigningRuntime::for_test(
            BTreeMap::from([(1, Arc::new(tokio::sync::Mutex::new(manager)))]),
            [],
        );
        state.federation.set_frost_signing_for_test(Some(signing));
        let wallet_scope = FederatedWalletScope::new(
            config.federation_id.to_bytes(),
            application.application_id.to_bytes(),
            1,
            1,
            application.descriptor_commitment,
        )
        .expect("wallet scope");
        let wallet = FederatedBdkWallet::open(
            FederatedWalletConfig::new(
                wallet_scope,
                &application.external_descriptor,
                &application.internal_descriptor,
                BitcoinNetwork::Regtest,
            )
            .expect("descriptors"),
            source.clone(),
            directory.path(),
        )
        .expect("BDK wallet");
        wallet.sync_once().await.expect("initial BDK sync");
        wallet
            .reconcile_revealed_counters(u64::try_from(input_count).expect("inputs"), 2)
            .await
            .expect("reveal allocations");
        state
            .federation
            .components
            .wallet
            .onchain_wallets
            .push(wallet.clone());
        transport.insert(
            config
                .member(private.member_id)
                .expect("member")
                .federation_api_url
                .clone(),
            Router::new()
                .nest("/api/federation/v1", create_federation_router())
                .with_state(state.clone()),
        );
        wallets.push(wallet);
        states.push(state);
        _directories.push(directory);
    }

    let mut chain = FederationChainObservationState::new();
    let mut selected = Vec::with_capacity(input_count);
    for idx in 0..input_count {
        let output = TxOut {
            value: BitcoinAmount::from_sat(25_000),
            script_pubkey: config
                .frost_wallet
                .as_ref()
                .expect("wallet")
                .derive_cdk_onchain_script(
                    &config,
                    application.application_id,
                    1,
                    FederationAddressKeychain::External,
                    u32::try_from(idx).expect("idx"),
                )
                .expect("script"),
        };
        selected.push((FederationAddressKeychain::External, idx as u32, output));
    }
    let funding = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([77; 32]), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        }],
        output: selected.iter().map(|(_, _, out)| out.clone()).collect(),
    };
    let mut inputs = Vec::with_capacity(input_count);
    for (vout, (keychain, index, output)) in selected.into_iter().enumerate() {
        let allocation = FederationAddressAllocation {
            quote: format!("batch-deposit-{vout}"),
            keychain,
            derivation_index: index,
            operation_id: FederationOperationId::from_bytes([vout as u8 + 10; 32]),
            operation_index: vout as u64 + 1,
        };
        let allocation_ref = FederationAllocatedAddressRef {
            application_id: application.application_id,
            application_key_epoch: 1,
            quote: allocation.quote.clone(),
            keychain,
            derivation_index: index,
        };
        let outpoint = OutPoint::new(funding.compute_txid(), vout as u32);
        for observer in [member_id(1), member_id(2)] {
            chain
                .apply_deposit(
                    &FederationDepositObservation {
                        allocation: allocation_ref.clone(),
                        outpoint,
                        txout: output.clone(),
                        confirmation: Some(FederationDepositConfirmation {
                            block_hash: genesis.block_hash(),
                            block_height: 0,
                            confirmation_depth: 1,
                        }),
                        observation_epoch: 0,
                        observer,
                        source: FederationChainSourceDescriptor::new(
                            FederationChainSourceScope::Independent,
                            format!("batch-source-{observer}"),
                        )
                        .expect("source"),
                    },
                    &allocation,
                    &output.script_pubkey,
                    2,
                    1,
                    FederationOperationId::from_bytes([observer.get() as u8 + vout as u8 + 40; 32]),
                    10 + vout as u64,
                )
                .expect("confirmed deposit");
        }
        inputs.push(FederationTransactionInput {
            outpoint,
            previous_output: output,
            allocation: allocation_ref,
        });
    }
    for wallet in &wallets {
        wallet
            .engine()
            .apply_accepted_transaction(funding.clone(), 1)
            .await
            .expect("apply funding");
    }

    let recipient = ScriptBuf::from_bytes([vec![0, 32], vec![9; 32]].concat());
    let intent = FederationOnchainPaymentIntent::new(
        "batch-multi-input-payment".to_string(),
        FederationOperationId::from_bytes([88; 32]),
        recipient.clone(),
        120_000,
        20_000,
        0,
        1,
        1,
        scope,
    )
    .expect("payment intent");
    let batch = FederationOnchainPaymentBatch::new(vec![intent.clone()], 1).expect("batch");
    let change_script = wallets[0]
        .peek_federated_script(FederatedWalletKeychain::Internal, 0)
        .await;
    let psbt = wallets[0]
        .engine()
        .build_federated_psbt(
            &[(recipient.clone(), 120_000)],
            FeeRate::from_sat_per_vb(2).expect("fee rate"),
            change_script.clone(),
            &inputs
                .iter()
                .map(|input| input.outpoint)
                .collect::<Vec<_>>(),
        )
        .await
        .expect("build 6-input psbt");
    assert_eq!(psbt.unsigned_tx.input.len(), input_count);
    let fee = psbt.fee().expect("fee").to_sat();
    let recipient_index = psbt
        .unsigned_tx
        .output
        .iter()
        .position(|output| output.script_pubkey == recipient)
        .expect("recipient");
    let change_index = psbt
        .unsigned_tx
        .output
        .iter()
        .position(|output| output.script_pubkey == change_script)
        .expect("change");
    let proposal = FederationTransactionProposal::new(
        &batch,
        member_id(1),
        psbt.unsigned_tx.clone(),
        psbt.unsigned_tx
            .input
            .iter()
            .map(|input| {
                inputs
                    .iter()
                    .find(|selected| selected.outpoint == input.previous_output)
                    .expect("selected input")
                    .clone()
            })
            .collect(),
        vec![FederationTransactionRecipientOutput {
            intent_id: intent.intent_id().expect("intent id"),
            output_index: recipient_index as u32,
            output: psbt.unsigned_tx.output[recipient_index].clone(),
            fee_contribution_sat: fee,
        }],
        Some(FederationTransactionChangeOutput {
            derivation_index: 0,
            output_index: change_index as u32,
            output: psbt.unsigned_tx.output[change_index].clone(),
        }),
        fee,
        None,
    )
    .expect("6-input proposal");

    let mut transactions = FederationTransactionState::new();
    transactions
        .add_pending_intent(
            FederationOnchainPaymentIntentRecord::pending(intent).expect("intent record"),
        )
        .expect("pending intent");
    for state in &states {
        let mut tx = state
            .mint
            .localstore()
            .begin_transaction()
            .await
            .expect("seed tx");
        cdk_common::federation::write_federation_state(&mut tx, transactions.clone())
            .await
            .expect("seed intents");
        cdk_common::federation::write_federation_state(&mut tx, chain.clone())
            .await
            .expect("seed deposits");
        tx.commit().await.expect("commit seed");
    }
    let change_allocation = states[0]
        .mint
        .federation_config()
        .expect("runtime")
        .address_allocation_operation(
            "batch-change",
            application.application_id,
            1,
            FederationAddressKeychain::Internal,
        )
        .expect("change allocation");
    let proposal_envelope = states[0]
        .mint
        .federation_config()
        .expect("runtime")
        .transaction_proposal_operation(proposal.clone())
        .expect("proposal envelope");
    for state in &states {
        state
            .submit_federation_envelope(change_allocation.clone())
            .await
            .expect("submit change")
            .ensure_accepted()
            .expect("accept change");
        state
            .submit_federation_envelope(proposal_envelope.clone())
            .await
            .expect("submit proposal")
            .ensure_accepted()
            .expect("accept proposal");
    }

    let attempt = federation_wallet_signing_attempt_at(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    );
    let plan = FederationWalletLivenessSchedule::new(
        proposal.proposal_id().expect("proposal id").to_bytes(),
        config.members.keys().copied(),
        2,
    )
    .expect("schedule")
    .signing_plan(attempt)
    .expect("signing plan");

    // Delay round 1 & 2 batch RPCs by 50ms and round 3 aggregate notification by 1.5s.
    for signer in &plan.signer_set {
        if *signer != member_id(1) {
            transport.delay_next_posts_before_delivery(
                "frost/signing/commitments/batch",
                *signer,
                [Duration::from_millis(50)],
            );
            transport.delay_next_posts_before_delivery(
                "frost/signing/packages/batch",
                *signer,
                [Duration::from_millis(50)],
            );
            transport.delay_next_posts_before_delivery(
                "frost/signing/results/batch",
                *signer,
                [Duration::from_millis(1_500)],
            );
        }
    }

    let state = &states[0];
    let runtime = state.mint.federation_config().expect("runtime");
    let app_public = application
        .derived_public_package(
            runtime.public_config(),
            &runtime
                .public_config()
                .frost_wallet
                .as_ref()
                .expect("wallet")
                .root,
        )
        .expect("app public");
    let client = FederationRestPeerClient::new(
        transport.clone(),
        runtime.public_config(),
        runtime.private_config().clone(),
    )
    .expect("client");

    let started = tokio::time::Instant::now();
    let signed =
        super::super::frost_signing::FederationFrostSigningCoordinator::new(&client, app_public)
            .with_local_signer(
                runtime.member_id(),
                state.mint.as_ref(),
                state.federation.frost_signing().expect("signing runtime"),
            )
            .sign_accepted_transaction(
                state,
                &wallets[0],
                proposal.proposal_id().expect("proposal id"),
                attempt,
                plan.signer_set
                    .iter()
                    .map(|member| {
                        cdk_federation::test_support::cdk_frost::MemberId::new(member.get())
                            .expect("signer")
                    })
                    .collect(),
            )
            .await
            .expect("6-input on-chain transaction batch signs and finalizes");
    let elapsed = started.elapsed();

    assert_eq!(signed.input_signatures.len(), input_count);
    signed
        .verify_signatures(&proposal)
        .expect("every batched Taproot input signature verifies");
    let unique_sessions = signed
        .input_signatures
        .iter()
        .map(|audit| audit.session_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(unique_sessions.len(), input_count);
    assert!(
        elapsed < Duration::from_millis(900),
        "6-input on-chain signing must finish in 2 batch fanouts without blocking on the 1.5s round-3 notification: {elapsed:?}"
    );

    // Every signer's durable redb store must have zero remaining nonterminal records.
    for (idx, state) in states.iter().enumerate() {
        let member = MemberId::new(u16::try_from(idx + 1).expect("member")).expect("member");
        if plan.signer_set.contains(&member) {
            let signing_runtime = state.federation.frost_signing().expect("signing runtime");
            assert_eq!(
                signing_runtime
                    .root_epoch_nonterminal_count(1)
                    .await
                    .expect("count"),
                0
            );
        }
    }
    server.abort();
}

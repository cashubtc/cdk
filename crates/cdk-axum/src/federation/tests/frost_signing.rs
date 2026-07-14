use cdk::federation::{
    FederationAddressKeychain, FederationAllocatedAddressRef,
    FederationCdkOnchainFrostAuthorization, FederationFrostApplicationId,
    FederationFrostCommitmentRequest, FederationOperationId, FederationTransactionChangeOutput,
    FederationTransactionInput, FederationTransactionPolicyAudit, FederationTransactionProposal,
    FederationTransactionScope, FederationTransactionState,
};
use cdk_common::bitcoin::absolute::LockTime;
use cdk_common::bitcoin::hashes::Hash;
use cdk_common::bitcoin::sighash::{Prevouts, SighashCache, TapSighashType};
use cdk_common::bitcoin::transaction::Version;
use cdk_common::bitcoin::{
    Amount as BitcoinAmount, Network as BitcoinNetwork, OutPoint, ScriptBuf, Sequence, Transaction,
    TxIn, TxOut, Txid, Witness,
};
use cdk_frost::{
    advance_dkg, derive_application_public, finish_dkg, start_dkg, ApplicationAuthorization,
    ApplicationRegistration, AuthorizationRequest, AuthorizedSigningRequest, DkgContext,
    DkgRound1Package, DkgRound2Package, MemoryNonceStore, ParticipantKeyManager, RootKeyEpoch,
    SecretEncryptionKey, SigningAuthorizationScope, SigningDerivation, SigningSessionContext,
};
use cdk_frost_bip32::{
    application_registration, application_registration_for_epoch, FrostBip32, WalletBranch,
    WalletIndex,
};

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
    payload_kind: SigningPayloadKind,
) {
    let registration = application_registration().expect("application registration");
    let roots = frost_roots(
        public_config.federation_id.to_bytes(),
        u16::try_from(private_configs.len()).expect("member count"),
        threshold,
    );
    let public = derive_application_public(roots[0].public(), registration)
        .expect("application public package");
    let (derivation, message, payload, authorization_reference) = match payload_kind {
        SigningPayloadKind::Generic => (
            SigningDerivation::application_key(),
            [91; 32],
            b"typed-test-authorization-v1".to_vec(),
            [73; 32],
        ),
        SigningPayloadKind::EpochSweep => epoch_sweep_signing_fixture(roots[0].public(), &public),
    };
    let signer_set = (1..=threshold)
        .map(|member| cdk_frost::MemberId::new(member).expect("signer"))
        .collect::<Vec<_>>();
    let scope = SigningAuthorizationScope::new(
        public_config.federation_id.to_bytes(),
        RootKeyEpoch::new(1).expect("root epoch"),
        registration,
        derivation.binding(),
        authorization_reference,
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
            authorization_reference,
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
        state.federation.frost_signing = Some(runtime);
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

fn epoch_sweep_signing_fixture(
    root: &cdk_frost::RootPublicPackage,
    source_public: &cdk_frost::DerivedPublicPackage,
) -> (SigningDerivation, [u8; 32], Vec<u8>, [u8; 32]) {
    let source_registration = source_public.registration();
    let source_key = FrostBip32::new(source_public.clone(), BitcoinNetwork::Regtest)
        .expect("source BIP32 adapter")
        .derive(
            WalletBranch::External,
            WalletIndex::new(0).expect("source index"),
        )
        .expect("source wallet key");
    let destination_registration =
        application_registration_for_epoch(2).expect("destination registration");
    let destination_public = derive_application_public(root, destination_registration)
        .expect("destination public package");
    let destination_key = FrostBip32::new(destination_public, BitcoinNetwork::Regtest)
        .expect("destination BIP32 adapter")
        .derive(
            WalletBranch::Internal,
            WalletIndex::new(0).expect("destination index"),
        )
        .expect("destination wallet key");
    let source_scope = FederationTransactionScope {
        root_key_epoch: 1,
        application_id: FederationFrostApplicationId::from_bytes(
            source_registration.id().to_bytes(),
        )
        .expect("source application id"),
        application_key_epoch: source_registration.key_epoch().get(),
        network: BitcoinNetwork::Regtest,
        policy_version: 1,
    };
    let destination_scope = FederationTransactionScope {
        application_key_epoch: destination_registration.key_epoch().get(),
        ..source_scope
    };
    let input = FederationTransactionInput {
        outpoint: OutPoint::new(Txid::from_byte_array([61; 32]), 0),
        previous_output: TxOut {
            value: BitcoinAmount::from_sat(50_000),
            script_pubkey: source_key
                .address(BitcoinNetwork::Regtest)
                .expect("source address")
                .script_pubkey(),
        },
        allocation: FederationAllocatedAddressRef {
            application_id: source_scope.application_id,
            application_key_epoch: source_scope.application_key_epoch,
            quote: "retiring-epoch-deposit".to_string(),
            keychain: FederationAddressKeychain::External,
            derivation_index: 0,
        },
    };
    let destination_allocation = FederationAllocatedAddressRef {
        application_id: destination_scope.application_id,
        application_key_epoch: destination_scope.application_key_epoch,
        quote: "active-epoch-sweep".to_string(),
        keychain: FederationAddressKeychain::Internal,
        derivation_index: 0,
    };
    let output = TxOut {
        value: BitcoinAmount::from_sat(49_500),
        script_pubkey: destination_key
            .address(BitcoinNetwork::Regtest)
            .expect("destination address")
            .script_pubkey(),
    };
    let transaction = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: input.outpoint,
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        }],
        output: vec![output.clone()],
    };
    let proposal = FederationTransactionProposal::new_epoch_sweep(
        source_scope,
        destination_scope,
        destination_allocation,
        41,
        MemberId::new(1).expect("proposer"),
        transaction,
        vec![input.clone()],
        FederationTransactionChangeOutput {
            derivation_index: 0,
            output_index: 0,
            output,
        },
        500,
    )
    .expect("epoch sweep proposal");
    let proposal_id = proposal.proposal_id().expect("proposal id");
    let sighash = SighashCache::new(&proposal.unsigned_transaction)
        .taproot_key_spend_signature_hash(
            0,
            &Prevouts::All(std::slice::from_ref(&input.previous_output)),
            TapSighashType::Default,
        )
        .expect("sweep sighash")
        .to_byte_array();
    let audit = FederationTransactionPolicyAudit {
        proposal_id,
        txid: proposal.unsigned_transaction.compute_txid(),
        fee_sat: proposal.fee_sat,
        fee_rate_sat_per_kwu: proposal.fee_rate_sat_per_kwu,
        weight: proposal.estimated_signed_weight(),
        fee_contributions: BTreeMap::new(),
        taproot_sighashes: vec![sighash],
    };
    let mut state = FederationTransactionState::new();
    state
        .accept_transaction(
            proposal,
            audit,
            FederationOperationId::from_bytes([62; 32]),
            42,
        )
        .expect("accepted sweep");
    let accepted = state
        .accepted_transaction(proposal_id)
        .expect("accepted sweep transaction");
    let typed =
        FederationCdkOnchainFrostAuthorization::new(accepted, 0, source_key.signing_derivation())
            .expect("typed sweep authorization");
    assert_eq!(typed.message, sighash);
    (
        source_key.signing_derivation().clone(),
        typed.message,
        typed.to_bytes().expect("typed authorization bytes"),
        typed.authorization_reference,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SigningFailure {
    None,
    Commitment,
    Share,
    AggregateResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SigningPayloadKind {
    Generic,
    EpochSweep,
}

#[tokio::test]
async fn live_private_rest_frost_signing_succeeds_two_of_three() {
    let (public, first, second, third) = transport_config_three();
    run_live_signing_case(
        public,
        vec![first, second, third],
        2,
        SigningFailure::None,
        SigningPayloadKind::Generic,
    )
    .await;
}

#[tokio::test]
async fn retiring_epoch_sweep_signs_two_of_three_with_one_member_offline() {
    let (public, first, second, third) = transport_config_three();
    run_live_signing_case(
        public,
        vec![first, second, third],
        2,
        SigningFailure::None,
        SigningPayloadKind::EpochSweep,
    )
    .await;
}

#[tokio::test]
async fn live_private_rest_frost_signing_succeeds_three_of_five() {
    let (public, private) =
        transport_config_five_with_threshold(ThresholdParams::new(5, 3, 3).expect("threshold"));
    run_live_signing_case(
        public,
        private,
        3,
        SigningFailure::None,
        SigningPayloadKind::Generic,
    )
    .await;
}

#[tokio::test]
async fn lost_post_signer_response_burns_nonce_and_retry_attempt_succeeds() {
    let (public, first, second, third) = transport_config_three();
    run_live_signing_case(
        public,
        vec![first, second, third],
        2,
        SigningFailure::Commitment,
        SigningPayloadKind::Generic,
    )
    .await;
}

#[tokio::test]
async fn lost_share_response_burns_attempt_and_fresh_attempt_succeeds() {
    let (public, first, second, third) = transport_config_three();
    run_live_signing_case(
        public,
        vec![first, second, third],
        2,
        SigningFailure::Share,
        SigningPayloadKind::Generic,
    )
    .await;
}

#[tokio::test]
async fn another_member_aggregates_cached_shares_after_coordinator_loss() {
    let (public, first, second, third) = transport_config_three();
    run_live_signing_case(
        public,
        vec![first, second, third],
        2,
        SigningFailure::AggregateResult,
        SigningPayloadKind::Generic,
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
    state.federation.frost_signing = Some(runtime);
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

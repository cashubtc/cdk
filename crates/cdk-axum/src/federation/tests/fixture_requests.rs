//! Signed peer requests, catch-up responses and DKG fixtures.

use super::*;

pub(super) fn signed_operation_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    sequence: u64,
    kind: FederationRestMessageKind,
    envelope: FederationOperationEnvelope,
) -> FederationSignedRequest<FederationSubmitOperationRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        member_id(1),
        sequence,
        kind,
        FederationSubmitOperationRequest::new(envelope),
        private_config,
    )
    .expect("signed operation request")
}

pub(super) fn signed_status_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
) -> FederationSignedRequest<FederationPeerStatusRequest> {
    signed_status_request_with_payload(
        public_config,
        private_config,
        receiver,
        sequence,
        FederationPeerStatusRequest::default(),
    )
}

pub(super) fn signed_status_request_with_payload(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    payload: FederationPeerStatusRequest,
) -> FederationSignedRequest<FederationPeerStatusRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::Status,
        payload,
        private_config,
    )
    .expect("signed status request")
}

pub(super) fn signed_catch_up_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationJournalCatchUpRequest,
) -> FederationSignedRequest<FederationJournalCatchUpRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::CatchUp,
        request,
        private_config,
    )
    .expect("signed catch-up request")
}

pub(super) fn signed_catch_up_request_with_kind(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    kind: FederationRestMessageKind,
    request: FederationJournalCatchUpRequest,
) -> FederationSignedRequest<FederationJournalCatchUpRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        kind,
        request,
        private_config,
    )
    .expect("signed catch-up request")
}

pub(super) fn finalized_aleph_bft_history<I>(
    public_config: &FederationConfig,
    submitting_member: MemberId,
    envelopes: I,
) -> FederationConsensusRuntime
where
    I: IntoIterator<Item = FederationOperationEnvelope>,
{
    let mut adapter = FederationConsensusRuntime::for_members(
        public_config.federation_id,
        submitting_member,
        0,
        public_config.members.keys().copied(),
    )
    .expect("test AlephBFT adapter");
    for (unit_index, envelope) in (0_u64..).zip(envelopes) {
        let unit = FederationAlephBftUnitPayload::new(
            public_config.federation_id,
            submitting_member,
            0,
            unit_index,
            vec![FederationConsensusItem::operation(envelope)],
        )
        .expect("test AlephBFT finalized unit");
        adapter
            .finalize_unit(unit)
            .expect("finalize test AlephBFT history");
    }
    adapter
}

pub(super) fn test_aleph_bft_peer(
    federation_id: cdk::federation::FederationId,
    member_id: MemberId,
) -> FederationConsensusRuntime {
    FederationConsensusRuntime::new(federation_id, member_id, 0)
}

pub(super) async fn backup_recovery_test_state() -> (
    MintState,
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    MemberConfigPublic,
    FederationConsensusRuntime,
) {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let peer_member = public_config
        .member(member_id(2))
        .expect("peer member")
        .clone();
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    let envelope = mint_quote_envelope(&state, &QuoteId::new());
    finalize_test_operation(&mut peer_consensus, envelope).expect("finalize peer operation");

    let catch_up_request = state
        .federation
        .consensus()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let response = aleph_bft_catch_up_response_with_materialized_digest(
        &state,
        &public_config,
        &peer_consensus,
        &catch_up_request,
    )
    .await;
    state
        .apply_federation_catch_up_response(&response)
        .await
        .expect("seed exact finalized cursor");

    (
        state,
        public_config,
        local_private,
        peer_private,
        peer_member,
        peer_consensus,
    )
}

pub(super) fn aleph_bft_catch_up_response(
    public_config: &FederationConfig,
    adapter: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let finalized_from = usize::try_from(request.aleph_bft_next_item_index).unwrap_or_default();
    FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        request,
        50,
        100,
    )
    .with_aleph_bft_finalized_items(
        adapter
            .finalized_items()
            .get(finalized_from..)
            .unwrap_or_default()
            .to_vec(),
        true,
    )
}

pub(super) fn test_catch_up_response(
    adapter: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let status = adapter.status();
    let finalized_from = usize::try_from(request.aleph_bft_next_item_index).unwrap_or_default();
    FederationJournalCatchUpResponse::from_journal(
        status.federation_id,
        adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        request,
        50,
        100,
    )
    .with_aleph_bft_finalized_items(
        adapter
            .finalized_items()
            .get(finalized_from..)
            .unwrap_or_default()
            .to_vec(),
        true,
    )
}

pub(super) fn checked_test_catch_up_response(
    adapter: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> Result<FederationJournalCatchUpResponse, FederationError> {
    if let Some(known_order_digest) = request.known_order_digest {
        if let Some(expected_digest) = adapter.journal().order_digest_before(request.next_index) {
            if expected_digest != known_order_digest {
                return Err(FederationError::CatchUpDigestMismatch {
                    index: request.next_index,
                    expected: expected_digest,
                    found: known_order_digest,
                });
            }
        }
    }

    Ok(test_catch_up_response(adapter, request))
}

pub(super) fn dkg_reveal(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> cdk::federation::FederationDkgParticipantCommitmentReveal {
    cdk::federation::FederationDkgParticipantCommitmentReveal {
        federation_id: public_config.federation_id,
        setup_authorization: public_config.setup_authorization,
        ceremony_id: cdk::federation::FederationDkgCeremonyId::from_bytes([55; 32]),
        member_id,
        keyset_spec: cdk::federation::FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [Amount::from(1)].into_iter().collect(),
        )
        .expect("DKG keyset spec"),
        coefficient_commitments: BTreeMap::from([(
            Amount::from(1),
            vec![
                BlsSecretKey::from_reduced_bytes(&[seed; 32]).public_key_g2(),
                BlsSecretKey::from_reduced_bytes(&[seed + 10; 32]).public_key_g2(),
            ],
        )]),
    }
}

pub(super) fn dkg_commitment_request(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> FederationDkgCommitmentRequest {
    let reveal = dkg_reveal(public_config, member_id, seed);
    let commitment = cdk::federation::FederationDkgParticipantCommitment::from_reveal(&reveal)
        .expect("dkg commitment");
    FederationDkgCommitmentRequest::new(commitment)
}

pub(super) fn dkg_reveal_request(
    public_config: &FederationConfig,
    member_id: MemberId,
    seed: u8,
) -> FederationDkgRevealRequest {
    FederationDkgRevealRequest::new(dkg_reveal(public_config, member_id, seed))
}

pub(super) fn dkg_secret_share_request(
    public_config: &FederationConfig,
    sender: MemberId,
    receiver: MemberId,
    seed: u8,
) -> FederationDkgSecretShareRequest {
    FederationDkgSecretShareRequest::new(
        public_config.federation_id,
        public_config.setup_authorization,
        cdk::federation::FederationDkgCeremonyId::from_bytes([55; 32]),
        sender,
        receiver,
        BTreeMap::from([(
            Amount::from(1),
            evaluate_secret_key_coefficients(
                receiver.to_bls_signer_id(),
                &[
                    BlsSecretKey::from_reduced_bytes(&[seed; 32]),
                    BlsSecretKey::from_reduced_bytes(&[seed + 10; 32]),
                ],
            )
            .expect("dkg secret share"),
        )]),
    )
}

pub(super) fn signed_dkg_commitment_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationDkgCommitmentRequest,
) -> FederationSignedRequest<FederationDkgCommitmentRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::DkgCommitment,
        request,
        private_config,
    )
    .expect("signed DKG commitment request")
}

pub(super) fn signed_dkg_reveal_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationDkgRevealRequest,
) -> FederationSignedRequest<FederationDkgRevealRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::DkgReveal,
        request,
        private_config,
    )
    .expect("signed DKG reveal request")
}

pub(super) fn signed_dkg_secret_share_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationDkgSecretShareRequest,
) -> FederationSignedRequest<FederationDkgSecretShareRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::DkgSecretShare,
        request,
        private_config,
    )
    .expect("signed DKG secret-share request")
}

pub(super) async fn persist_complete_dkg_round(
    state: &MintState,
    public_config: &FederationConfig,
    first_private: &MemberConfigPrivate,
    second_private: &MemberConfigPrivate,
) {
    for (private_config, seed) in [(first_private, 11), (second_private, 21)] {
        let commitment =
            dkg_commitment_request(public_config, private_config.member_id, seed).commitment;
        state
            .persist_federation_dkg_commitment(&commitment, private_config.member_id, 1)
            .await
            .expect("persist DKG commitment");
        let reveal = dkg_reveal_request(public_config, private_config.member_id, seed).reveal;
        state
            .persist_federation_dkg_reveal(&reveal, private_config.member_id, 2)
            .await
            .expect("persist DKG reveal");
    }
}

pub(super) fn dkg_transcript_signature_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
) -> FederationDkgTranscriptSignatureRequest {
    let transcript = public_config
        .setup_transcript
        .as_ref()
        .expect("setup transcript");
    let signature = cdk::federation::FederationDkgTranscriptSignature::sign(
        transcript.transcript_hash,
        private_config,
    )
    .expect("transcript signature");
    FederationDkgTranscriptSignatureRequest::new(
        public_config.federation_id,
        public_config.setup_authorization,
        private_config.member_id,
        transcript.transcript_hash,
        signature,
    )
}

pub(super) async fn derive_and_persist_dkg_member_keyset_share(
    state: &MintState,
    public_config: &FederationConfig,
) -> FederationDkgMemberKeysetShare {
    let db = state.mint.localstore();
    let commitments = db
        .get_federation_dkg_commitments()
        .await
        .expect("DKG commitments")
        .into_iter()
        .map(|commitment| (commitment.member_id, commitment))
        .collect();
    let round = FederationDkgCommitmentRound::new(public_config, commitments)
        .expect("complete DKG commitment round");
    let reveals = db
        .get_federation_dkg_reveals()
        .await
        .expect("DKG reveals")
        .into_iter()
        .map(|reveal| (reveal.member_id, reveal))
        .collect::<BTreeMap<_, _>>();
    round
        .validate_reveals(public_config, &reveals)
        .expect("complete DKG reveals");
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let contributions = db
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("DKG secret-share contributions")
        .into_iter()
        .filter(|contribution| contribution.receiver == local_member)
        .map(|contribution| (contribution.sender, contribution))
        .collect::<BTreeMap<_, _>>();
    let keyset_share = round
        .derive_member_keyset_share(public_config, &reveals, local_member, &contributions)
        .expect("driver-derived member keyset share");
    let mut tx = db.begin_transaction().await.expect("begin keyset share");
    tx.add_federation_dkg_member_keyset_share(&keyset_share)
        .await
        .expect("persist driver-derived keyset share");
    tx.commit()
        .await
        .expect("commit driver-derived keyset share");
    keyset_share
}

pub(super) async fn durable_dkg_transcript_fixture() -> (
    MintState,
    FederationConfig,
    MemberConfigPrivate,
    MemberConfigPrivate,
    FederationDkgTranscriptHash,
) {
    let (trusted_public_config, local_private, peer_private) = transport_config();
    let mut setup_public_config = trusted_public_config.clone();
    setup_public_config.keysets.clear();
    setup_public_config.setup_transcript = None;
    setup_public_config.validate().expect("setup public config");

    let mut setup_local_private = local_private.clone();
    setup_local_private.keyset_secret_shares.clear();
    setup_local_private.setup_transcript_hash = None;
    setup_local_private
        .validate_against_public(&setup_public_config)
        .expect("setup local private config");
    let mut setup_peer_private = peer_private.clone();
    setup_peer_private.keyset_secret_shares.clear();
    setup_peer_private.setup_transcript_hash = None;
    setup_peer_private
        .validate_against_public(&setup_public_config)
        .expect("setup peer private config");

    let state = federation_route_state_for_private(
        setup_public_config.clone(),
        setup_local_private.clone(),
    )
    .await;
    let local_member = setup_local_private.member_id;
    persist_complete_dkg_round(
        &state,
        &setup_public_config,
        &setup_local_private,
        &setup_peer_private,
    )
    .await;
    let local_contribution =
        dkg_secret_share_request(&setup_public_config, local_member, local_member, 11);
    state
        .persist_federation_dkg_secret_share_contribution(&local_contribution, local_member, 3)
        .await
        .expect("persist local secret-share contribution");
    let peer_contribution = dkg_secret_share_request(
        &setup_public_config,
        setup_peer_private.member_id,
        local_member,
        21,
    );
    state
        .persist_federation_dkg_secret_share_contribution(
            &peer_contribution,
            setup_peer_private.member_id,
            3,
        )
        .await
        .expect("persist peer secret-share contribution");
    let keyset_share =
        derive_and_persist_dkg_member_keyset_share(&state, &setup_public_config).await;
    let mut final_public_config = FederationConfig::new(
        setup_public_config.setup_authorization,
        setup_public_config.threshold_params,
        setup_public_config.members.clone(),
        vec![keyset_share.keyset.clone()],
    )
    .expect("final public config");
    final_public_config.payment_observation_policy =
        setup_public_config.payment_observation_policy.clone();
    let public_ceremonies = vec![keyset_share.public_ceremony];
    let transcript_hash = FederationDkgTranscript::hash_for_config(
        FederationDkgSetupMethod::PedersenDkg,
        &final_public_config,
        &public_ceremonies,
    )
    .expect("durable transcript hash");

    (
        state,
        setup_public_config,
        setup_local_private,
        setup_peer_private,
        transcript_hash,
    )
}

pub(super) fn signed_dkg_transcript_signature_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver: MemberId,
    sequence: u64,
    request: FederationDkgTranscriptSignatureRequest,
) -> FederationSignedRequest<FederationDkgTranscriptSignatureRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver,
        sequence,
        FederationRestMessageKind::DkgTranscriptSignature,
        request,
        private_config,
    )
    .expect("signed DKG transcript signature request")
}

pub(super) async fn consensus_operation_count(state: &MintState) -> u64 {
    state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .peer_status()
        .operation_count
}

pub(super) fn reset_federation_runtime_journal(state: &mut MintState) {
    let runtime_config = state.mint.federation_config().expect("federation config");
    state.federation.member_mut_for_test().consensus =
        Arc::new(tokio::sync::Mutex::new(FederationConsensusRuntime::new(
            runtime_config.federation_id(),
            runtime_config.member_id(),
            0,
        )));
}

pub(super) fn activate_local_test_finality(state: &mut MintState) {
    state.federation.finality = cdk_federation::FederationFinalityMode::Local;
}

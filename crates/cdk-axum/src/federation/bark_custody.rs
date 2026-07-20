//! Authenticated private transport for Bark-specific FROST custody protocols.

use async_trait::async_trait;
use axum::extract::{Json, State};
use axum::response::Response;
use cdk::federation::{
    FederationBarkCustodyAbortRequest, FederationBarkThresholdDhRequest,
    FederationBarkThresholdDhResponse, FederationBarkVirtualMusigCommitmentRequest,
    FederationBarkVirtualMusigCommitmentResponse, FederationBarkVirtualMusigPartialRequest,
    FederationBarkVirtualMusigPartialResponse, FederationConfig, FederationFrostSigningAck,
    FederationRestMessageKind, FederationSignedRequest, MemberConfigPublic,
};
use cdk_frost::{
    AuthorizationRequest, CustodyProtocol, DerivedPublicPackage, SigningDerivation,
    SigningSessionContext, ThresholdDhRequest, VirtualMusigNonceCommitmentSet,
    VirtualMusigTranscript,
};
use futures::{stream, StreamExt, TryStreamExt};
use tracing::instrument;

use crate::MintState;

use super::frost_signing::{signing_error_response, signing_runtime, signing_unavailable_response};
use super::{admit_private_request, federation_error_response, federation_runtime_config};

/// Owned authenticated peer client installed behind `cdk-bark`'s custody
/// boundary. It contains public verification material but no root share.
#[derive(Debug)]
pub(crate) struct FederatedBarkCustodyPeerClient {
    client: super::peer_client::FederationRestPeerClient<cdk_http_client::Async>,
    public_config: FederationConfig,
    public: DerivedPublicPackage,
}

impl FederatedBarkCustodyPeerClient {
    pub(crate) fn new(
        client: super::peer_client::FederationRestPeerClient<cdk_http_client::Async>,
        public_config: FederationConfig,
        public: DerivedPublicPackage,
    ) -> Self {
        Self {
            client,
            public_config,
            public,
        }
    }
}

#[async_trait]
impl cdk_bark::FederatedBarkCustodyClient for FederatedBarkCustodyPeerClient {
    async fn sign_frost(
        &self,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
    ) -> anyhow::Result<cdk_frost::AggregateSignature> {
        super::frost_signing::FederationFrostSigningCoordinator::new(
            &self.client,
            self.public.clone(),
        )
        .sign(&self.public_config, context, derivation, authorization)
        .await
    }

    async fn collect_virtual_musig_commitments(
        &self,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
    ) -> anyhow::Result<VirtualMusigNonceCommitmentSet> {
        FederationBarkCustodyCoordinator::new(&self.client, self.public.clone())
            .collect_virtual_musig_commitments(
                &self.public_config,
                context,
                derivation,
                authorization,
            )
            .await
    }

    async fn collect_virtual_musig_partial(
        &self,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
        commitments: &VirtualMusigNonceCommitmentSet,
        transcript: &VirtualMusigTranscript,
    ) -> anyhow::Result<[u8; 32]> {
        FederationBarkCustodyCoordinator::new(&self.client, self.public.clone())
            .collect_virtual_musig_partial(
                &self.public_config,
                context,
                derivation,
                authorization,
                commitments,
                transcript,
            )
            .await
    }

    async fn abort(
        &self,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
        reason: cdk_frost::NonceBurnReason,
    ) {
        FederationBarkCustodyCoordinator::new(&self.client, self.public.clone())
            .abort_selected(&self.public_config, context, authorization, reason)
            .await;
    }
}

/// Coordinator for one accepted Bark signer attempt and exact signer subset.
#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FederationBarkCustodyCoordinator<'a, T> {
    client: &'a super::peer_client::FederationRestPeerClient<T>,
    public: DerivedPublicPackage,
}

#[cfg_attr(not(test), allow(dead_code))]
impl<'a, T> FederationBarkCustodyCoordinator<'a, T>
where
    T: cdk_http_client::Transport,
{
    /// Construct a coordinator containing public verification material only.
    pub(crate) fn new(
        client: &'a super::peer_client::FederationRestPeerClient<T>,
        public: DerivedPublicPackage,
    ) -> Self {
        Self { client, public }
    }

    /// Collect one durable virtual MuSig2 nonce from every accepted signer.
    pub(crate) async fn collect_virtual_musig_commitments(
        &self,
        public_config: &FederationConfig,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
    ) -> anyhow::Result<VirtualMusigNonceCommitmentSet> {
        self.validate_context(context, CustodyProtocol::VirtualMusig2)?;
        let request =
            FederationBarkVirtualMusigCommitmentRequest::new(context, derivation, authorization);
        let responses = stream::iter(context.signer_set().iter().copied())
            .map(|signer| {
                let request = request.clone();
                async move {
                    let member = member_for_signer(public_config, signer)?;
                    self.client
                        .request_bark_virtual_musig_commitment(member, &request)
                        .await
                }
            })
            .buffer_unordered(self.parallel_limit(context))
            .try_collect::<Vec<_>>()
            .await;
        let commitments = match responses {
            Ok(commitments) => commitments,
            Err(error) => {
                self.abort_selected(
                    public_config,
                    context,
                    authorization,
                    cdk_frost::NonceBurnReason::AmbiguousResponse,
                )
                .await;
                return Err(error);
            }
        };
        let commitments = VirtualMusigNonceCommitmentSet::new(commitments)?;
        if commitments
            .commitments()
            .keys()
            .copied()
            .collect::<Vec<_>>()
            != context.signer_set()
        {
            anyhow::bail!("Bark virtual commitment responses differ from accepted signer set");
        }
        Ok(commitments)
    }

    /// Collect, independently verify, and combine one finalized virtual partial
    /// from every accepted signer.
    pub(crate) async fn collect_virtual_musig_partial(
        &self,
        public_config: &FederationConfig,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
        commitments: &VirtualMusigNonceCommitmentSet,
        transcript: &VirtualMusigTranscript,
    ) -> anyhow::Result<[u8; 32]> {
        self.validate_context(context, CustodyProtocol::VirtualMusig2)?;
        if commitments
            .commitments()
            .keys()
            .copied()
            .collect::<Vec<_>>()
            != context.signer_set()
            || transcript.session_id() != context.session_id()
        {
            anyhow::bail!("Bark virtual partial inputs differ from accepted signer attempt");
        }
        let responses = stream::iter(context.signer_set().iter().copied())
            .map(|signer| async move {
                let member = member_for_signer(public_config, signer)?;
                let commitment = commitments
                    .commitments()
                    .get(&signer)
                    .ok_or_else(|| anyhow::anyhow!("accepted Bark signer commitment is missing"))?;
                let request = FederationBarkVirtualMusigPartialRequest::new(
                    context,
                    commitment.nonce_id(),
                    authorization,
                    commitments,
                    transcript,
                );
                self.client
                    .request_bark_virtual_musig_partial(member, &request)
                    .await
                    .map(|partial| (commitment.clone(), partial))
            })
            .buffer_unordered(self.parallel_limit(context))
            .try_collect::<Vec<_>>()
            .await;
        let contributions = match responses {
            Ok(contributions) => contributions,
            Err(error) => {
                self.abort_selected(
                    public_config,
                    context,
                    authorization,
                    cdk_frost::NonceBurnReason::AmbiguousResponse,
                )
                .await;
                return Err(error);
            }
        };
        let target = cdk_frost::derive_signing_public(&self.public, derivation)?;
        Ok(cdk_frost::combine_virtual_musig_partial_shares(
            &target,
            context.signer_set(),
            transcript,
            contributions,
        )?)
    }

    /// Collect, verify, and interpolate an operation-bound threshold-DH point.
    pub(crate) async fn threshold_dh(
        &self,
        public_config: &FederationConfig,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
        request: &ThresholdDhRequest,
    ) -> anyhow::Result<[u8; 33]> {
        self.validate_context(context, CustodyProtocol::ThresholdDh)?;
        let payload =
            FederationBarkThresholdDhRequest::new(context, derivation, authorization, request);
        let responses = stream::iter(context.signer_set().iter().copied())
            .map(|signer| {
                let payload = payload.clone();
                async move {
                    let member = member_for_signer(public_config, signer)?;
                    self.client
                        .request_bark_threshold_dh_share(member, &payload)
                        .await
                }
            })
            .buffer_unordered(self.parallel_limit(context))
            .try_collect::<Vec<_>>()
            .await;
        let shares = match responses {
            Ok(shares) => shares,
            Err(error) => {
                self.abort_selected(
                    public_config,
                    context,
                    authorization,
                    cdk_frost::NonceBurnReason::AmbiguousResponse,
                )
                .await;
                return Err(error);
            }
        };
        let target = cdk_frost::derive_signing_public(&self.public, derivation)?;
        Ok(cdk_frost::combine_threshold_dh_shares(
            &target,
            context.signer_set(),
            request,
            shares,
        )?)
    }

    fn validate_context(
        &self,
        context: &SigningSessionContext,
        protocol: CustodyProtocol,
    ) -> anyhow::Result<()> {
        if context.protocol() != protocol
            || context.root_epoch() != self.public.root_epoch()
            || context.registration() != self.public.registration()
            || context.signer_set().len() < usize::from(self.public.threshold())
            || context
                .signer_set()
                .iter()
                .any(|member| !self.public.verifying_shares().contains_key(member))
        {
            anyhow::bail!("Bark custody context differs from the configured application");
        }
        Ok(())
    }

    fn parallel_limit(&self, context: &SigningSessionContext) -> usize {
        context
            .signer_set()
            .len()
            .min(
                self.client
                    .sync_config
                    .private_rest_outbound_concurrency_limit,
            )
            .max(1)
    }

    async fn abort_selected(
        &self,
        public_config: &FederationConfig,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
        reason: cdk_frost::NonceBurnReason,
    ) {
        let request = FederationBarkCustodyAbortRequest::new(context, authorization, reason);
        stream::iter(context.signer_set().iter().copied())
            .map(|signer| {
                let request = request.clone();
                async move {
                    let Ok(member_id) = cdk::federation::MemberId::new(signer.get()) else {
                        return;
                    };
                    let Some(member) = public_config.member(member_id) else {
                        return;
                    };
                    let _ = self.client.abort_bark_custody(member, &request).await;
                }
            })
            .buffer_unordered(self.parallel_limit(context))
            .collect::<Vec<_>>()
            .await;
    }
}

fn member_for_signer(
    public_config: &FederationConfig,
    signer: cdk_frost::MemberId,
) -> anyhow::Result<&MemberConfigPublic> {
    let member_id = cdk::federation::MemberId::new(signer.get())?;
    public_config
        .member(member_id)
        .ok_or_else(|| cdk::federation::FederationError::UnknownMember { member_id }.into())
}

#[instrument(skip_all)]
pub(super) async fn post_bark_virtual_musig_commitment(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkVirtualMusigCommitmentRequest>>,
) -> Result<Json<FederationSignedRequest<FederationBarkVirtualMusigCommitmentResponse>>, Response> {
    let kind = FederationRestMessageKind::BarkVirtualMusigCommitment;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(signing_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, derivation, authorization) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    let commitment = signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .commit_bark_virtual_musig(&state.mint, &context, &derivation, &authorization)
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationBarkVirtualMusigCommitmentResponse::new(&commitment),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub(super) async fn post_bark_virtual_musig_partial(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkVirtualMusigPartialRequest>>,
) -> Result<Json<FederationSignedRequest<FederationBarkVirtualMusigPartialResponse>>, Response> {
    let kind = FederationRestMessageKind::BarkVirtualMusigPartial;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(signing_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, nonce_id, authorization, commitments, transcript) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    let partial = signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .sign_bark_virtual_musig(
            &state.mint,
            &context,
            nonce_id,
            &authorization,
            &commitments,
            &transcript,
        )
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationBarkVirtualMusigPartialResponse::new(&partial),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub(super) async fn post_bark_threshold_dh(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkThresholdDhRequest>>,
) -> Result<Json<FederationSignedRequest<FederationBarkThresholdDhResponse>>, Response> {
    let kind = FederationRestMessageKind::BarkThresholdDh;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(signing_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, derivation, authorization, request) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    let share = signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .bark_threshold_dh(&state.mint, &context, &derivation, &authorization, &request)
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationBarkThresholdDhResponse::new(&share),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub(super) async fn post_bark_custody_abort(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationBarkCustodyAbortRequest>>,
) -> Result<Json<FederationSignedRequest<FederationFrostSigningAck>>, Response> {
    let kind = FederationRestMessageKind::BarkCustodyAbort;
    let verified = admit_private_request(&state, &payload, kind).await?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, authorization, reason) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .abort_bark_custody(&state.mint, &context, &authorization, reason)
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationFrostSigningAck::new(context.session_id(), kind),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

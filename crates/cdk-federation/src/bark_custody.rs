//! Authenticated private transport for Bark-specific FROST custody protocols.

use std::sync::Arc;

use async_trait::async_trait;
#[cfg(any(test, feature = "test-utils"))]
use cdk::federation::FederationBarkThresholdDhRequest;
use cdk::federation::{
    FederationBarkCustodyAbortRequest, FederationBarkVirtualMusigCommitmentRequest,
    FederationBarkVirtualMusigPartialRequest, FederationConfig, MemberConfigPublic,
};
#[cfg(any(test, feature = "test-utils"))]
use cdk_frost::ThresholdDhRequest;
use cdk_frost::{
    AuthorizationRequest, CustodyProtocol, DerivedPublicPackage, SigningDerivation,
    SigningSessionContext, VirtualMusigNonceCommitmentSet, VirtualMusigTranscript,
};
use futures::{stream, StreamExt, TryStreamExt};

/// Owned authenticated peer client installed behind `cdk-bark`'s custody
/// boundary. It contains public verification material but no root share.
#[derive(Debug)]
pub struct FederatedBarkCustodyPeerClient {
    client: super::peer_client::FederationRestPeerClient<super::FederationTransport>,
    public_config: FederationConfig,
    custody_public: DerivedPublicPackage,
    recovery_public: DerivedPublicPackage,
    local_member_id: cdk::federation::MemberId,
    mint: Arc<cdk::mint::Mint>,
    local_signing: super::frost_signing::FederationFrostSigningRuntime,
}

impl FederatedBarkCustodyPeerClient {
    pub fn new(
        client: super::peer_client::FederationRestPeerClient<super::FederationTransport>,
        public_config: FederationConfig,
        custody_public: DerivedPublicPackage,
        recovery_public: DerivedPublicPackage,
        local_member_id: cdk::federation::MemberId,
        mint: Arc<cdk::mint::Mint>,
        local_signing: super::frost_signing::FederationFrostSigningRuntime,
    ) -> Self {
        Self {
            client,
            public_config,
            custody_public,
            recovery_public,
            local_member_id,
            mint,
            local_signing,
        }
    }

    fn signing_public(
        &self,
        authorization: &AuthorizationRequest,
    ) -> anyhow::Result<DerivedPublicPackage> {
        let application_id = authorization.application_id();
        if application_id == self.custody_public.registration().id() {
            Ok(self.custody_public.clone())
        } else if application_id == self.recovery_public.registration().id() {
            Ok(self.recovery_public.clone())
        } else {
            anyhow::bail!("Bark requested an unregistered FROST application")
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
        let public = self.signing_public(authorization)?;
        let latency = super::frost_signing::FrostLatencyContext::new(
            super::latency::FederationLatencyCorrelation::Session,
            cdk::federation::FederationOperationId::from_bytes(context.session_id().to_bytes())
                .to_string(),
            super::latency::FederationLatencyBackend::Bark,
            super::latency::FederationLatencyDirection::Internal,
        );
        super::frost_signing::FederationFrostSigningCoordinator::new(&self.client, public)
            .with_local_signer(self.local_member_id, &self.mint, &self.local_signing)
            .sign_with_latency(
                &self.public_config,
                context,
                derivation,
                authorization,
                &latency,
            )
            .await
    }

    async fn collect_virtual_musig_commitments(
        &self,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
    ) -> anyhow::Result<VirtualMusigNonceCommitmentSet> {
        FederationBarkCustodyCoordinator::new(&self.client, self.custody_public.clone())
            .with_local_signer(self.local_member_id, &self.mint, &self.local_signing)
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
        FederationBarkCustodyCoordinator::new(&self.client, self.custody_public.clone())
            .with_local_signer(self.local_member_id, &self.mint, &self.local_signing)
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
        FederationBarkCustodyCoordinator::new(&self.client, self.custody_public.clone())
            .with_local_signer(self.local_member_id, &self.mint, &self.local_signing)
            .abort_selected(&self.public_config, context, authorization, reason)
            .await;
    }
}

/// Coordinator for one accepted Bark signer attempt and exact signer subset.
#[derive(Debug)]
pub struct FederationBarkCustodyCoordinator<'a, T> {
    client: &'a super::peer_client::FederationRestPeerClient<T>,
    public: DerivedPublicPackage,
    local_signer: Option<FederationBarkLocalSigner<'a>>,
}

#[derive(Debug, Clone, Copy)]
struct FederationBarkLocalSigner<'a> {
    member_id: cdk::federation::MemberId,
    mint: &'a cdk::mint::Mint,
    runtime: &'a super::frost_signing::FederationFrostSigningRuntime,
}

impl<'a, T> FederationBarkCustodyCoordinator<'a, T>
where
    T: cdk_http_client::Transport,
{
    /// Construct a coordinator containing public verification material only.
    pub fn new(
        client: &'a super::peer_client::FederationRestPeerClient<T>,
        public: DerivedPublicPackage,
    ) -> Self {
        Self {
            client,
            public,
            local_signer: None,
        }
    }

    fn with_local_signer(
        mut self,
        member_id: cdk::federation::MemberId,
        mint: &'a cdk::mint::Mint,
        runtime: &'a super::frost_signing::FederationFrostSigningRuntime,
    ) -> Self {
        self.local_signer = Some(FederationBarkLocalSigner {
            member_id,
            mint,
            runtime,
        });
        self
    }

    /// Collect one durable virtual MuSig2 nonce from every accepted signer.
    pub async fn collect_virtual_musig_commitments(
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
                    match self
                        .local_signer
                        .filter(|local| local.member_id == member.member_id)
                    {
                        Some(local) => local
                            .runtime
                            .commit_bark_virtual_musig(
                                local.mint,
                                context,
                                derivation,
                                authorization,
                            )
                            .await
                            .map_err(anyhow::Error::from),
                        None => {
                            self.client
                                .request_bark_virtual_musig_commitment(member, &request)
                                .await
                        }
                    }
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
    pub async fn collect_virtual_musig_partial(
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
                match self
                    .local_signer
                    .filter(|local| local.member_id == member.member_id)
                {
                    Some(local) => local
                        .runtime
                        .sign_bark_virtual_musig(
                            local.mint,
                            context,
                            commitment.nonce_id(),
                            authorization,
                            commitments,
                            transcript,
                        )
                        .await
                        .map_err(anyhow::Error::from)
                        .map(|partial| (commitment.clone(), partial)),
                    None => self
                        .client
                        .request_bark_virtual_musig_partial(member, &request)
                        .await
                        .map(|partial| (commitment.clone(), partial)),
                }
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
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn threshold_dh(
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
                    match self
                        .local_signer
                        .filter(|local| local.member_id == member.member_id)
                    {
                        Some(local) => local
                            .runtime
                            .bark_threshold_dh(
                                local.mint,
                                context,
                                derivation,
                                authorization,
                                request,
                            )
                            .await
                            .map_err(anyhow::Error::from),
                        None => {
                            self.client
                                .request_bark_threshold_dh_share(member, &payload)
                                .await
                        }
                    }
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
                    if let Some(local) = self
                        .local_signer
                        .filter(|local| local.member_id == member.member_id)
                    {
                        let _ = local
                            .runtime
                            .abort_bark_custody(local.mint, context, authorization, reason)
                            .await;
                        return;
                    }
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

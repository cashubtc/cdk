use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cdk::federation::{
    FederationAlephBftFinalizedUnitRequest, FederationAlephBftFinalizedUnitResponse,
    FederationConfig, FederationDkgCommitmentRequest, FederationDkgRevealRequest,
    FederationDkgSecretShareRequest, FederationDkgSetupResponse,
    FederationDkgTranscriptSignatureRequest, FederationId, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationPeerStatus, FederationPeerStatusRequest,
    FederationRestMessageKind, FederationSignedRequest, FederationSubmitOperationRequest,
    FederationSubmitOperationResponse, MemberConfigPrivate, MemberConfigPublic,
    FEDERATION_REST_API_VERSION,
};
use cdk_http_client::{Async, Transport};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::time;

use super::FederationPeerSyncConfig;

static FEDERATION_REST_SEQUENCE_SEED: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FederationRestPeerClient<T = Async> {
    transport: T,
    public_config: FederationConfig,
    federation_id: FederationId,
    private_config: MemberConfigPrivate,
    pub(super) sync_config: FederationPeerSyncConfig,
    next_sequence: Arc<AtomicU64>,
    pub(super) outbound_admission: Arc<tokio::sync::Semaphore>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl<T> FederationRestPeerClient<T>
where
    T: Transport,
{
    pub(crate) fn new(
        transport: T,
        public_config: &FederationConfig,
        private_config: MemberConfigPrivate,
    ) -> anyhow::Result<Self> {
        Self::new_with_config(
            transport,
            public_config,
            private_config,
            FederationPeerSyncConfig::default(),
        )
    }

    pub(crate) fn new_with_config(
        transport: T,
        public_config: &FederationConfig,
        private_config: MemberConfigPrivate,
        sync_config: FederationPeerSyncConfig,
    ) -> anyhow::Result<Self> {
        sync_config.validate()?;
        private_config.validate_against_public(public_config)?;
        Ok(Self {
            transport,
            public_config: public_config.clone(),
            federation_id: public_config.federation_id,
            private_config,
            sync_config,
            next_sequence: Arc::new(AtomicU64::new(initial_federation_rest_sequence())),
            outbound_admission: Arc::new(tokio::sync::Semaphore::new(
                sync_config.private_rest_outbound_concurrency_limit,
            )),
        })
    }

    async fn http_post_with_timeout<P, R>(
        &self,
        member: &MemberConfigPublic,
        endpoint: &str,
        operation: &str,
        payload: &P,
        timeout: Duration,
    ) -> anyhow::Result<R>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        let _permit = match self.outbound_admission.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                anyhow::bail!(
                    "federation outbound REST queue is full for peer {} {}",
                    member.member_id,
                    operation
                );
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                anyhow::bail!(
                    "federation outbound REST admission is closed for peer {} {}",
                    member.member_id,
                    operation
                );
            }
        };
        let request =
            self.transport
                .http_post(federation_endpoint(member, endpoint)?, None, payload);
        match time::timeout(timeout, request).await {
            Ok(result) => Ok(result?),
            Err(_) => anyhow::bail!(
                "federation peer {} {} timed out after {:?}",
                member.member_id,
                operation,
                timeout
            ),
        }
    }

    pub(crate) async fn peer_status(
        &self,
        member: &MemberConfigPublic,
    ) -> anyhow::Result<FederationPeerStatus> {
        let request = self.signed_request(
            member,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
        )?;
        let status = self
            .http_post_with_timeout(
                member,
                "status",
                "status request",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        validate_peer_status(self.federation_id, member, &status)?;
        Ok(status)
    }

    pub(crate) async fn submit_operation(
        &self,
        member: &MemberConfigPublic,
        request: &FederationSubmitOperationRequest,
    ) -> anyhow::Result<FederationSubmitOperationResponse> {
        request.ensure_admissible_size()?;
        let expected_operation_id = request.operation_id()?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::SubmitOperation,
            request.clone(),
        )?;
        let response: FederationSubmitOperationResponse = self
            .http_post_with_timeout(
                member,
                "operations",
                "operation submission",
                &request,
                self.sync_config.operation_submission_timeout,
            )
            .await?;
        response.validate(expected_operation_id)?;

        Ok(response)
    }

    pub(crate) async fn submit_aleph_bft_finalized_unit(
        &self,
        member: &MemberConfigPublic,
        request: &FederationAlephBftFinalizedUnitRequest,
    ) -> anyhow::Result<FederationAlephBftFinalizedUnitResponse> {
        request.ensure_admissible_size()?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::AlephBftFinalizedUnit,
            request.clone(),
        )?;
        let response: FederationAlephBftFinalizedUnitResponse = self
            .http_post_with_timeout(
                member,
                "aleph-bft/finalized-units",
                "AlephBFT finalized-unit submission",
                &request,
                self.sync_config.operation_submission_timeout,
            )
            .await?;
        response.validate(self.federation_id, &request.payload.unit)?;

        Ok(response)
    }

    pub(crate) async fn submit_dkg_commitment(
        &self,
        member: &MemberConfigPublic,
        request: &FederationDkgCommitmentRequest,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.validate_sender(&self.public_config, self.private_config.member_id)?;
        let request =
            self.signed_request(member, FederationRestMessageKind::DkgCommitment, *request)?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/commitments",
                "DKG commitment submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgCommitment,
            self.private_config.member_id,
        )?;
        Ok(response)
    }

    pub(crate) async fn submit_dkg_reveal(
        &self,
        member: &MemberConfigPublic,
        request: &FederationDkgRevealRequest,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.validate_sender(&self.public_config, self.private_config.member_id)?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::DkgReveal,
            request.clone(),
        )?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/reveals",
                "DKG reveal submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgReveal,
            self.private_config.member_id,
        )?;
        Ok(response)
    }

    pub(crate) async fn submit_dkg_secret_share(
        &self,
        member: &MemberConfigPublic,
        request: &FederationDkgSecretShareRequest,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.validate_sender_receiver(
            &self.public_config,
            self.private_config.member_id,
            member.member_id,
        )?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::DkgSecretShare,
            request.clone(),
        )?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/secret-shares",
                "DKG secret-share submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgSecretShare,
            self.private_config.member_id,
        )?;
        Ok(response)
    }

    pub(crate) async fn submit_dkg_transcript_signature(
        &self,
        member: &MemberConfigPublic,
        request: &FederationDkgTranscriptSignatureRequest,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.validate_sender(&self.public_config, self.private_config.member_id)?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::DkgTranscriptSignature,
            *request,
        )?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/transcript-signatures",
                "DKG transcript-signature submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgTranscriptSignature,
            self.private_config.member_id,
        )?;
        Ok(response)
    }

    pub(crate) async fn catch_up(
        &self,
        member: &MemberConfigPublic,
        request: &FederationJournalCatchUpRequest,
    ) -> anyhow::Result<FederationJournalCatchUpResponse> {
        let request =
            self.signed_request(member, FederationRestMessageKind::CatchUp, request.clone())?;
        let response: FederationJournalCatchUpResponse = self
            .http_post_with_timeout(
                member,
                "journal",
                "catch-up request",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        if response.version != FEDERATION_REST_API_VERSION {
            anyhow::bail!(
                "federation peer {} returned unsupported REST version {}",
                member.member_id,
                response.version
            );
        }
        if response.federation_id != self.federation_id {
            anyhow::bail!(
                "federation peer {} returned catch-up for federation {}, expected {}",
                member.member_id,
                response.federation_id,
                self.federation_id
            );
        }
        response.verify_session_outcome(member)?;
        Ok(response)
    }

    fn signed_request<P>(
        &self,
        member: &MemberConfigPublic,
        kind: FederationRestMessageKind,
        payload: P,
    ) -> anyhow::Result<FederationSignedRequest<P>>
    where
        P: Serialize,
    {
        self.ensure_configured_peer(member)?;
        Ok(FederationSignedRequest::new_signed(
            self.federation_id,
            member.member_id,
            next_federation_rest_sequence(&self.next_sequence),
            kind,
            payload,
            &self.private_config,
        )?)
    }

    fn ensure_configured_peer(&self, member: &MemberConfigPublic) -> anyhow::Result<()> {
        let Some(configured) = self.public_config.members.get(&member.member_id) else {
            anyhow::bail!(
                "federation peer {} is not in the configured member roster",
                member.member_id
            );
        };
        if configured != member {
            anyhow::bail!(
                "federation peer {} does not match the configured member roster",
                member.member_id
            );
        }

        Ok(())
    }
}

fn federation_rest_sequence_time_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| {
            duration
                .as_secs()
                .saturating_mul(1_000_000_000)
                .saturating_add(u64::from(duration.subsec_nanos()))
        })
        .unwrap_or_default()
}

fn initial_federation_rest_sequence() -> u64 {
    let now = federation_rest_sequence_time_seed();

    loop {
        let last = FEDERATION_REST_SEQUENCE_SEED.load(Ordering::Relaxed);
        let candidate = now.max(last.saturating_add(1));
        match FEDERATION_REST_SEQUENCE_SEED.compare_exchange(
            last,
            candidate,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return candidate,
            Err(_) => continue,
        }
    }
}

fn next_federation_rest_sequence(local_sequence: &AtomicU64) -> u64 {
    loop {
        let now = federation_rest_sequence_time_seed();
        let local_last = local_sequence.load(Ordering::Relaxed);
        let global_last = FEDERATION_REST_SEQUENCE_SEED.load(Ordering::Relaxed);
        let candidate = now
            .max(local_last.saturating_add(1))
            .max(global_last.saturating_add(1));
        match FEDERATION_REST_SEQUENCE_SEED.compare_exchange(
            global_last,
            candidate,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                local_sequence.store(candidate, Ordering::Relaxed);
                return candidate;
            }
            Err(_) => continue,
        }
    }
}

fn federation_endpoint(member: &MemberConfigPublic, endpoint: &str) -> anyhow::Result<url::Url> {
    Ok(member
        .federation_api_url
        .join_paths(&["federation", "v1", endpoint])?)
}

fn validate_peer_status(
    expected_federation_id: FederationId,
    member: &MemberConfigPublic,
    status: &FederationPeerStatus,
) -> anyhow::Result<()> {
    if status.version != FEDERATION_REST_API_VERSION {
        anyhow::bail!(
            "federation peer {} returned unsupported REST version {}",
            member.member_id,
            status.version
        );
    }
    if status.member_id != member.member_id {
        anyhow::bail!(
            "federation peer status member mismatch: expected {}, found {}",
            member.member_id,
            status.member_id
        );
    }
    if status.federation_id != expected_federation_id {
        anyhow::bail!(
            "federation peer status federation mismatch: expected {}, found {}",
            expected_federation_id,
            status.federation_id
        );
    }

    Ok(())
}

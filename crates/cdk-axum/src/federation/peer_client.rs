use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cdk::federation::{
    FederationAlephBftActivityRequest, FederationAlephBftActivityResponse,
    FederationAlephBftBackupRequest, FederationAlephBftBackupResponse,
    FederationAlephBftNetworkMessageRequest, FederationAlephBftNetworkMessageResponse,
    FederationCheckpoint, FederationCheckpointFetchRequest, FederationCheckpointFetchResponse,
    FederationCheckpointId, FederationCheckpointQuorum, FederationCheckpointQuorumRequest,
    FederationCheckpointQuorumResponse, FederationCheckpointSignatureRequest,
    FederationCheckpointSignatureResponse, FederationCheckpointSummaryRequest,
    FederationCheckpointSummaryResponse, FederationConfig, FederationDkgActivationRequest,
    FederationDkgCommitmentRequest, FederationDkgReadinessRequest, FederationDkgRevealRequest,
    FederationDkgSecretShareRequest, FederationDkgSetupResponse,
    FederationDkgTranscriptSignatureRequest, FederationError,
    FederationFrostAggregateResultNotification, FederationFrostCommitmentRequest,
    FederationFrostCommitmentResponse, FederationFrostDkgConfirmationRequest,
    FederationFrostDkgRound1Request, FederationFrostDkgRound2Request,
    FederationFrostSignatureShareResponse, FederationFrostSigningAbortRequest,
    FederationFrostSigningAck, FederationFrostSigningPackageRequest, FederationId,
    FederationJournalCatchUpRequest, FederationJournalCatchUpResponse, FederationPeerStatus,
    FederationPeerStatusRequest, FederationRestMessageKind, FederationSignedRequest,
    FederationSubmitOperationRequest, FederationSubmitOperationResponse, MemberConfigPrivate,
    MemberConfigPublic, FEDERATION_MAX_ALEPH_BFT_BACKUP_PAGE_BYTES,
    FEDERATION_MAX_ALEPH_BFT_BACKUP_REQUEST_BYTES, FEDERATION_MAX_CHECKPOINT_CATCH_UP_PAGE_BYTES,
    FEDERATION_MAX_CHECKPOINT_CATCH_UP_REQUEST_BYTES, FEDERATION_REST_API_VERSION,
};
#[cfg(feature = "federation-bark")]
use cdk::federation::{
    FederationBarkCustodyAbortRequest, FederationBarkThresholdDhRequest,
    FederationBarkThresholdDhResponse, FederationBarkVirtualMusigCommitmentRequest,
    FederationBarkVirtualMusigCommitmentResponse, FederationBarkVirtualMusigPartialRequest,
    FederationBarkVirtualMusigPartialResponse,
};
#[cfg(feature = "federation-bark")]
use cdk::federation::{
    FederationBarkEvidenceAttestationRequest, FederationBarkEvidenceAttestationResponse,
    FederationBarkPrepareReceiveSecretRequest, FederationBarkReceiveSecretAcknowledgement,
    FEDERATION_MAX_BARK_EVIDENCE_ATTESTATION_REQUEST_BYTES,
};
use cdk_http_client::{Async, HttpError, Transport};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::time;

use super::FederationPeerSyncConfig;

static FEDERATION_REST_SEQUENCE_SEED: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, thiserror::Error)]
pub(super) enum FederationRestPeerClientError {
    #[error("federation outbound REST queue is full for peer {member_id} {operation}")]
    OutboundAdmissionFull {
        member_id: cdk::federation::MemberId,
        operation: String,
    },
    #[error("federation outbound REST admission is closed for peer {member_id} {operation}")]
    OutboundAdmissionClosed {
        member_id: cdk::federation::MemberId,
        operation: String,
    },
    #[error("federation peer {member_id} {operation} timed out after {timeout:?}")]
    RequestTimeout {
        member_id: cdk::federation::MemberId,
        operation: String,
        timeout: Duration,
    },
}

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
    pub(super) fn cloned_transport(&self) -> T {
        self.transport.clone()
    }

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
        public_config.validate()?;
        Self::new_with_validated_config(transport, public_config, private_config, sync_config)
    }

    pub(crate) fn new_with_validated_config(
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
                return Err(FederationRestPeerClientError::OutboundAdmissionFull {
                    member_id: member.member_id,
                    operation: operation.to_string(),
                }
                .into());
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(FederationRestPeerClientError::OutboundAdmissionClosed {
                    member_id: member.member_id,
                    operation: operation.to_string(),
                }
                .into());
            }
        };
        let request =
            self.transport
                .http_post(federation_endpoint(member, endpoint)?, None, payload);
        match time::timeout(timeout, request).await {
            Ok(result) => Ok(result?.json_or_status_error()?),
            Err(_) => Err(FederationRestPeerClientError::RequestTimeout {
                member_id: member.member_id,
                operation: operation.to_string(),
                timeout,
            }
            .into()),
        }
    }

    async fn http_post_bounded_with_timeout<P, R>(
        &self,
        member: &MemberConfigPublic,
        endpoint: &str,
        operation: &str,
        payload: &P,
        timeout: Duration,
        max_response_bytes: usize,
    ) -> anyhow::Result<R>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned + Serialize,
    {
        let _permit = match self.outbound_admission.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                return Err(FederationRestPeerClientError::OutboundAdmissionFull {
                    member_id: member.member_id,
                    operation: operation.to_string(),
                }
                .into());
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(FederationRestPeerClientError::OutboundAdmissionClosed {
                    member_id: member.member_id,
                    operation: operation.to_string(),
                }
                .into());
            }
        };
        let request = self.transport.http_post_bounded(
            federation_endpoint(member, endpoint)?,
            None,
            payload,
            max_response_bytes,
        );
        match time::timeout(timeout, request).await {
            Ok(Err(HttpError::ResponseTooLarge { actual, max })) => {
                Err(FederationError::CheckpointCatchUpTooLarge {
                    resource: "decoded response",
                    actual: u64::try_from(actual).unwrap_or(u64::MAX),
                    max: u64::try_from(max).unwrap_or(u64::MAX),
                }
                .into())
            }
            Ok(result) => Ok(result?),
            Err(_) => Err(FederationRestPeerClientError::RequestTimeout {
                member_id: member.member_id,
                operation: operation.to_string(),
                timeout,
            }
            .into()),
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
        validate_peer_status(&self.public_config, member, &status)?;
        Ok(status)
    }

    pub(crate) async fn checkpoint_summary(
        &self,
        member: &MemberConfigPublic,
        request_recovery_checkpoint: bool,
        next_scheduled_quorum_after: Option<u64>,
    ) -> anyhow::Result<FederationCheckpointSummaryResponse> {
        let mut payload = if request_recovery_checkpoint {
            FederationCheckpointSummaryRequest::new().with_recovery_checkpoint()
        } else {
            FederationCheckpointSummaryRequest::new()
        };
        if let Some(sequence) = next_scheduled_quorum_after {
            payload = payload.with_next_scheduled_quorum_after(sequence);
        }
        let request = self.signed_request(
            member,
            FederationRestMessageKind::CheckpointSummary,
            payload,
        )?;
        let request_sequence = request.sequence;
        let response: FederationSignedRequest<FederationCheckpointSummaryResponse> = self
            .http_post_with_timeout(
                member,
                "checkpoints/summary",
                "checkpoint summary request",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        self.validate_signed_response(
            &response,
            member,
            FederationRestMessageKind::CheckpointSummary,
        )?;
        if response.sequence != request_sequence {
            anyhow::bail!(
                "signed checkpoint summary response has sequence {}, expected {}",
                response.sequence,
                request_sequence
            );
        }
        response
            .payload
            .validate_for_validated_config(&self.public_config, member.member_id)?;
        if let (Some(sequence), Some(next)) = (
            next_scheduled_quorum_after,
            response.payload.next_scheduled_quorum.as_ref(),
        ) {
            let expected =
                sequence
                    .checked_add(1)
                    .ok_or_else(|| FederationError::CheckpointInvalid {
                        reason: "scheduled checkpoint sequence overflow".to_string(),
                    })?;
            if next.scheduled_checkpoint_sequence != expected {
                return Err(FederationError::CheckpointInvalid {
                    reason: format!(
                        "peer returned scheduled checkpoint sequence {}, expected {}",
                        next.scheduled_checkpoint_sequence, expected
                    ),
                }
                .into());
            }
        }
        Ok(response.payload)
    }

    pub(crate) async fn fetch_checkpoint(
        &self,
        member: &MemberConfigPublic,
        checkpoint_id: FederationCheckpointId,
    ) -> anyhow::Result<FederationCheckpointFetchResponse> {
        #[cfg(feature = "prometheus")]
        let transfer_started_at = std::time::Instant::now();
        let request = self.signed_request(
            member,
            FederationRestMessageKind::CheckpointFetch,
            FederationCheckpointFetchRequest::new(checkpoint_id),
        )?;
        let response: FederationCheckpointFetchResponse = self
            .http_post_with_timeout(
                member,
                "checkpoints/fetch",
                "checkpoint fetch request",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        let quorum_signature_count = response
            .quorum
            .as_ref()
            .map(|quorum| quorum.signatures.len());
        #[cfg(feature = "prometheus")]
        let verification_started_at = std::time::Instant::now();
        let validation = response.validate_for_validated_config(
            &self.public_config,
            member.member_id,
            checkpoint_id,
        );
        #[cfg(feature = "prometheus")]
        if let Some(signature_count) = quorum_signature_count {
            let verification_duration = verification_started_at.elapsed().as_secs_f64();
            let local_member = self.private_config.member_id.to_string();
            cdk_prometheus::METRICS.record_federation_checkpoint_work(
                &local_member,
                cdk_prometheus::FederationCheckpointWork::SignaturesVerified,
                u64::try_from(signature_count).unwrap_or(u64::MAX),
            );
            cdk_prometheus::METRICS.record_federation_checkpoint_duration(
                &local_member,
                cdk_prometheus::FederationCheckpointOperation::SignatureVerification,
                verification_duration,
            );
            super::record_checkpoint_quorum_verification_performance(
                super::CheckpointQuorumVerificationContext::PeerFetch,
                validation.is_ok(),
                verification_duration,
            );
        }
        validation?;
        #[cfg(feature = "prometheus")]
        if quorum_signature_count.is_some() {
            let local_member = self.private_config.member_id.to_string();
            cdk_prometheus::METRICS.record_federation_checkpoint_work(
                &local_member,
                cdk_prometheus::FederationCheckpointWork::QuorumsTransferred,
                1,
            );
            cdk_prometheus::METRICS.record_federation_checkpoint_duration(
                &local_member,
                cdk_prometheus::FederationCheckpointOperation::QuorumTransfer,
                transfer_started_at.elapsed().as_secs_f64(),
            );
        }
        #[cfg(not(feature = "prometheus"))]
        let _ = quorum_signature_count;
        Ok(response)
    }

    pub(crate) async fn request_checkpoint_signature(
        &self,
        member: &MemberConfigPublic,
        checkpoint: &FederationCheckpoint,
    ) -> anyhow::Result<FederationCheckpointSignatureResponse> {
        checkpoint.validate_against_validated_config(&self.public_config)?;
        let checkpoint_id = checkpoint.id()?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::CheckpointSignature,
            FederationCheckpointSignatureRequest::new(checkpoint_id),
        )?;
        let response: FederationCheckpointSignatureResponse = self
            .http_post_with_timeout(
                member,
                "checkpoints/signature",
                "checkpoint signature request",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        #[cfg(feature = "prometheus")]
        let verification_started_at = std::time::Instant::now();
        let has_signature = response.signature.is_some();
        let verification = response.validate(&self.public_config, member.member_id, checkpoint);
        #[cfg(feature = "prometheus")]
        if has_signature {
            let verification_duration = verification_started_at.elapsed().as_secs_f64();
            let local_member = self.private_config.member_id.to_string();
            cdk_prometheus::METRICS.record_federation_checkpoint_work(
                &local_member,
                cdk_prometheus::FederationCheckpointWork::SignaturesVerified,
                1,
            );
            cdk_prometheus::METRICS.record_federation_checkpoint_duration(
                &local_member,
                cdk_prometheus::FederationCheckpointOperation::SignatureVerification,
                verification_duration,
            );
            super::record_checkpoint_signature_verification_performance(
                super::CheckpointSignatureVerificationContext::PeerFetch,
                verification.is_ok(),
                verification_duration,
            );
        }
        #[cfg(not(feature = "prometheus"))]
        let _ = has_signature;
        verification?;
        Ok(response)
    }

    pub(crate) async fn publish_checkpoint_quorum(
        &self,
        member: &MemberConfigPublic,
        quorum: &FederationCheckpointQuorum,
    ) -> anyhow::Result<()> {
        #[cfg(feature = "prometheus")]
        let transfer_started_at = std::time::Instant::now();
        super::verify_checkpoint_quorum_with_metrics(
            quorum,
            &self.public_config,
            self.private_config.member_id,
            super::CheckpointQuorumVerificationContext::Publish,
        )?;
        let checkpoint_id = quorum.checkpoint.id()?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::CheckpointQuorum,
            FederationCheckpointQuorumRequest::new(quorum.clone()),
        )?;
        let response: FederationCheckpointQuorumResponse = self
            .http_post_with_timeout(
                member,
                "checkpoints/quorum",
                "checkpoint quorum publication",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(self.federation_id, member.member_id, checkpoint_id)?;
        #[cfg(feature = "prometheus")]
        {
            let local_member = self.private_config.member_id.to_string();
            cdk_prometheus::METRICS.record_federation_checkpoint_work(
                &local_member,
                cdk_prometheus::FederationCheckpointWork::QuorumsTransferred,
                1,
            );
            cdk_prometheus::METRICS.record_federation_checkpoint_duration(
                &local_member,
                cdk_prometheus::FederationCheckpointOperation::QuorumTransfer,
                transfer_started_at.elapsed().as_secs_f64(),
            );
        }
        Ok(())
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

    pub(crate) async fn submit_aleph_bft_network_message(
        &self,
        member: &MemberConfigPublic,
        request: &FederationAlephBftNetworkMessageRequest,
    ) -> anyhow::Result<FederationAlephBftNetworkMessageResponse> {
        request.ensure_admissible_size()?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::AlephBftNetworkMessage,
            request.clone(),
        )?;
        let response: FederationAlephBftNetworkMessageResponse = self
            .http_post_with_timeout(
                member,
                "aleph-bft/messages",
                "AlephBFT network-message submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(self.federation_id, request.payload.session_id)?;

        Ok(response)
    }

    pub(crate) async fn submit_aleph_bft_activity(
        &self,
        member: &MemberConfigPublic,
        request: &FederationAlephBftActivityRequest,
    ) -> anyhow::Result<FederationAlephBftActivityResponse> {
        let request = self.signed_request(
            member,
            FederationRestMessageKind::AlephBftActivity,
            *request,
        )?;
        let response: FederationAlephBftActivityResponse = self
            .http_post_with_timeout(
                member,
                "aleph-bft/activity",
                "AlephBFT activity submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            request.payload.session_id,
            request.payload.operation_id,
        )?;

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

    pub(crate) async fn submit_dkg_readiness(
        &self,
        member: &MemberConfigPublic,
        request: &FederationDkgReadinessRequest,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.validate(
            &self.public_config,
            request.ceremony_id,
            &request.keyset_spec,
        )?;
        let request = self.signed_request(
            member,
            FederationRestMessageKind::DkgReadiness,
            request.clone(),
        )?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/readiness",
                "DKG readiness probe",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgReadiness,
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

    pub(crate) async fn submit_frost_dkg_round1(
        &self,
        member: &MemberConfigPublic,
        request: &FederationFrostDkgRound1Request,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.decode_package(&self.public_config, self.private_config.member_id)?;
        let kind = FederationRestMessageKind::FrostDkgRound1;
        let request = self.signed_request(member, kind, request.clone())?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/frost/round1",
                "FROST DKG round-one submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            kind,
            self.private_config.member_id,
        )?;
        Ok(response)
    }

    pub(crate) async fn submit_frost_dkg_round2(
        &self,
        member: &MemberConfigPublic,
        request: &FederationFrostDkgRound2Request,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.decode_package(
            &self.public_config,
            self.private_config.member_id,
            member.member_id,
        )?;
        let kind = FederationRestMessageKind::FrostDkgRound2;
        let request = self.signed_request(member, kind, request.clone())?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/frost/round2",
                "FROST DKG round-two submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            kind,
            self.private_config.member_id,
        )?;
        Ok(response)
    }

    pub(crate) async fn submit_frost_dkg_confirmation(
        &self,
        member: &MemberConfigPublic,
        request: &FederationFrostDkgConfirmationRequest,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.validate(&self.public_config, self.private_config.member_id)?;
        let kind = FederationRestMessageKind::FrostDkgConfirmation;
        let request = self.signed_request(member, kind, request.clone())?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/frost/confirmations",
                "FROST DKG confirmation submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            kind,
            self.private_config.member_id,
        )?;
        Ok(response)
    }

    pub(crate) async fn request_frost_commitment(
        &self,
        member: &MemberConfigPublic,
        request: &FederationFrostCommitmentRequest,
    ) -> anyhow::Result<cdk_frost::NonceCommitment> {
        let kind = FederationRestMessageKind::FrostSigningCommitment;
        let expected_session =
            cdk_frost::SigningSessionContext::decode(&request.context)?.session_id();
        let request = self.signed_request(member, kind, request.clone())?;
        let response: FederationSignedRequest<FederationFrostCommitmentResponse> = self
            .http_post_with_timeout(
                member,
                "frost/signing/commitments",
                "FROST commitment request",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        self.validate_signed_response(&response, member, kind)?;
        Ok(response
            .payload
            .decode(member.member_id, expected_session)?)
    }

    pub(crate) async fn request_frost_signature_share(
        &self,
        member: &MemberConfigPublic,
        request: &FederationFrostSigningPackageRequest,
    ) -> anyhow::Result<cdk_frost::SignatureShare> {
        let kind = FederationRestMessageKind::FrostSigningPackage;
        let expected_session =
            cdk_frost::SigningSessionContext::decode(&request.context)?.session_id();
        let package_hash = request.package_hash;
        let request = self.signed_request(member, kind, request.clone())?;
        let response: FederationSignedRequest<FederationFrostSignatureShareResponse> = self
            .http_post_with_timeout(
                member,
                "frost/signing/packages",
                "FROST signature-share request",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        self.validate_signed_response(&response, member, kind)?;
        Ok(response
            .payload
            .decode(member.member_id, expected_session, package_hash)?)
    }

    pub(crate) async fn notify_frost_aggregate_result(
        &self,
        member: &MemberConfigPublic,
        notification: &FederationFrostAggregateResultNotification,
    ) -> anyhow::Result<()> {
        let kind = FederationRestMessageKind::FrostAggregateResult;
        let expected_session =
            cdk_frost::SigningSessionContext::decode(&notification.context)?.session_id();
        let request = self.signed_request(member, kind, notification.clone())?;
        let response: FederationSignedRequest<FederationFrostSigningAck> = self
            .http_post_with_timeout(
                member,
                "frost/signing/results",
                "FROST aggregate-result notification",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        self.validate_signed_response(&response, member, kind)?;
        if response.payload.version != FEDERATION_REST_API_VERSION
            || response.payload.kind != kind
            || response.payload.session_id != expected_session.to_bytes()
        {
            return Err(FederationError::FrostSigningInvalid {
                reason: "aggregate-result acknowledgement bindings are inconsistent".to_string(),
            }
            .into());
        }
        Ok(())
    }

    pub(crate) async fn abort_frost_signing(
        &self,
        member: &MemberConfigPublic,
        request: &FederationFrostSigningAbortRequest,
    ) -> anyhow::Result<()> {
        let kind = FederationRestMessageKind::FrostSigningAbort;
        let expected_session =
            cdk_frost::SigningSessionContext::decode(&request.context)?.session_id();
        let request = self.signed_request(member, kind, request.clone())?;
        let response: FederationSignedRequest<FederationFrostSigningAck> = self
            .http_post_with_timeout(
                member,
                "frost/signing/aborts",
                "FROST signing abort",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        self.validate_signed_response(&response, member, kind)?;
        if response.payload.version != FEDERATION_REST_API_VERSION
            || response.payload.kind != kind
            || response.payload.session_id != expected_session.to_bytes()
        {
            return Err(FederationError::FrostSigningInvalid {
                reason: "signing-abort acknowledgement bindings are inconsistent".to_string(),
            }
            .into());
        }
        Ok(())
    }

    #[cfg(feature = "federation-bark")]
    pub(crate) async fn request_bark_virtual_musig_commitment(
        &self,
        member: &MemberConfigPublic,
        request: &FederationBarkVirtualMusigCommitmentRequest,
    ) -> anyhow::Result<cdk_frost::VirtualMusigNonceCommitment> {
        let kind = FederationRestMessageKind::ApplicationVirtualMusigCommitment;
        let context = cdk_frost::SigningSessionContext::decode(&request.context)?;
        let signed = self.signed_request(member, kind, request.clone())?;
        let response: FederationSignedRequest<FederationBarkVirtualMusigCommitmentResponse> = self
            .http_post_with_timeout(
                member,
                "frost/bark/musig2/commitments",
                "Bark virtual MuSig2 commitment request",
                &signed,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        self.validate_signed_response(&response, member, kind)?;
        Ok(response.payload.decode(member.member_id, &context)?)
    }

    #[cfg(feature = "federation-bark")]
    pub(crate) async fn request_bark_virtual_musig_partial(
        &self,
        member: &MemberConfigPublic,
        request: &FederationBarkVirtualMusigPartialRequest,
    ) -> anyhow::Result<cdk_frost::VirtualMusigPartialShare> {
        let kind = FederationRestMessageKind::ApplicationVirtualMusigPartial;
        let context = cdk_frost::SigningSessionContext::decode(&request.context)?;
        let transcript_binding = request.transcript_binding;
        let signed = self.signed_request(member, kind, request.clone())?;
        let response: FederationSignedRequest<FederationBarkVirtualMusigPartialResponse> = self
            .http_post_with_timeout(
                member,
                "frost/bark/musig2/partials",
                "Bark virtual MuSig2 partial request",
                &signed,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        self.validate_signed_response(&response, member, kind)?;
        Ok(response
            .payload
            .decode(member.member_id, context.session_id(), transcript_binding)?)
    }

    #[cfg(feature = "federation-bark")]
    pub(crate) async fn request_bark_threshold_dh_share(
        &self,
        member: &MemberConfigPublic,
        request: &FederationBarkThresholdDhRequest,
    ) -> anyhow::Result<cdk_frost::ThresholdDhShare> {
        let kind = FederationRestMessageKind::ApplicationThresholdDh;
        let context = cdk_frost::SigningSessionContext::decode(&request.context)?;
        let request_binding = request.request_binding;
        let signed = self.signed_request(member, kind, request.clone())?;
        let response: FederationSignedRequest<FederationBarkThresholdDhResponse> = self
            .http_post_with_timeout(
                member,
                "frost/bark/threshold-dh",
                "Bark threshold-DH share request",
                &signed,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        self.validate_signed_response(&response, member, kind)?;
        Ok(response
            .payload
            .decode(member.member_id, context.session_id(), request_binding)?)
    }

    #[cfg(feature = "federation-bark")]
    pub(crate) async fn abort_bark_custody(
        &self,
        member: &MemberConfigPublic,
        request: &FederationBarkCustodyAbortRequest,
    ) -> anyhow::Result<()> {
        let kind = FederationRestMessageKind::ApplicationCustodyAbort;
        let expected_session =
            cdk_frost::SigningSessionContext::decode(&request.context)?.session_id();
        let signed = self.signed_request(member, kind, request.clone())?;
        let response: FederationSignedRequest<FederationFrostSigningAck> = self
            .http_post_with_timeout(
                member,
                "frost/bark/aborts",
                "Bark custody abort",
                &signed,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        self.validate_signed_response(&response, member, kind)?;
        if response.payload.version != FEDERATION_REST_API_VERSION
            || response.payload.kind != kind
            || response.payload.session_id != expected_session.to_bytes()
        {
            return Err(FederationError::FrostSigningInvalid {
                reason: "Bark custody abort acknowledgement is inconsistent".to_string(),
            }
            .into());
        }
        Ok(())
    }

    #[cfg(feature = "federation-bark")]
    pub(crate) async fn prepare_bark_receive_secret(
        &self,
        member: &MemberConfigPublic,
        request: &FederationBarkPrepareReceiveSecretRequest,
    ) -> anyhow::Result<FederationBarkReceiveSecretAcknowledgement> {
        if request.recipient != member.member_id {
            anyhow::bail!("Bark receive-secret request recipient differs from target member");
        }
        let kind = FederationRestMessageKind::ApplicationSecretPrepare;
        let signed = self.signed_request(member, kind, request.clone())?;
        let response: FederationSignedRequest<FederationBarkReceiveSecretAcknowledgement> = self
            .http_post_with_timeout(
                member,
                "bark/receive-secrets",
                "Bark receive-secret preparation",
                &signed,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        self.validate_signed_response(&response, member, kind)?;
        response
            .payload
            .verify(&self.public_config, &request.binding)?;
        if response.payload.member != member.member_id {
            anyhow::bail!("Bark receive-secret acknowledgement signer differs from target member");
        }
        Ok(response.payload)
    }

    #[cfg(feature = "federation-bark")]
    pub(crate) async fn deliver_bark_evidence_attestation(
        &self,
        member: &MemberConfigPublic,
        request: &FederationBarkEvidenceAttestationRequest,
    ) -> anyhow::Result<FederationBarkEvidenceAttestationResponse> {
        request.validate_relayed(&self.public_config, self.private_config.member_id)?;
        let kind = FederationRestMessageKind::ApplicationEvidenceAttestation;
        let signed = self.signed_request(member, kind, request.clone())?;
        let response: FederationSignedRequest<FederationBarkEvidenceAttestationResponse> = self
            .http_post_bounded_with_timeout(
                member,
                "bark/evidence-attestations",
                "Bark evidence-attestation delivery",
                &signed,
                self.sync_config.peer_request_timeout,
                FEDERATION_MAX_BARK_EVIDENCE_ATTESTATION_REQUEST_BYTES,
            )
            .await?;
        self.validate_signed_response(&response, member, kind)?;
        response.payload.validate(
            &self.public_config,
            &request.binding,
            request.attestation.signer,
        )?;
        Ok(response.payload)
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

    pub(crate) async fn submit_dkg_activation(
        &self,
        member: &MemberConfigPublic,
        request: &FederationDkgActivationRequest,
    ) -> anyhow::Result<FederationDkgSetupResponse> {
        request.validate_signature_sender(&self.public_config, self.private_config.member_id)?;
        let request =
            self.signed_request(member, FederationRestMessageKind::DkgActivation, *request)?;
        let response: FederationDkgSetupResponse = self
            .http_post_with_timeout(
                member,
                "dkg/activations",
                "DKG activation submission",
                &request,
                self.sync_config.peer_request_timeout,
            )
            .await?;
        response.validate(
            self.federation_id,
            member.member_id,
            FederationRestMessageKind::DkgActivation,
            self.private_config.member_id,
        )?;
        Ok(response)
    }

    pub(crate) async fn aleph_bft_backup(
        &self,
        member: &MemberConfigPublic,
        request: FederationAlephBftBackupRequest,
    ) -> anyhow::Result<FederationAlephBftBackupResponse> {
        request.validate()?;
        let request =
            self.signed_request(member, FederationRestMessageKind::AlephBftBackup, request)?;
        let request_bytes = serde_json::to_vec(&request)?.len();
        if request_bytes > FEDERATION_MAX_ALEPH_BFT_BACKUP_REQUEST_BYTES {
            return Err(FederationError::CheckpointCatchUpTooLarge {
                resource: "AlephBFT backup request",
                actual: u64::try_from(request_bytes).unwrap_or(u64::MAX),
                max: FEDERATION_MAX_ALEPH_BFT_BACKUP_REQUEST_BYTES as u64,
            }
            .into());
        }
        let request_sequence = request.sequence;
        let response: FederationSignedRequest<FederationAlephBftBackupResponse> = self
            .http_post_bounded_with_timeout(
                member,
                "aleph-bft/backup",
                "AlephBFT backup request",
                &request,
                self.sync_config.peer_request_timeout,
                FEDERATION_MAX_ALEPH_BFT_BACKUP_PAGE_BYTES,
            )
            .await?;
        self.validate_signed_response(
            &response,
            member,
            FederationRestMessageKind::AlephBftBackup,
        )?;
        if response.sequence != request_sequence {
            return Err(FederationError::CheckpointInvalid {
                reason: "AlephBFT backup response sequence mismatch".to_string(),
            }
            .into());
        }
        response
            .payload
            .validate(self.federation_id, member.member_id, request.payload)?;
        Ok(response.payload)
    }

    pub(crate) async fn catch_up_to_checkpoint(
        &self,
        member: &MemberConfigPublic,
        request: &FederationJournalCatchUpRequest,
        checkpoint: &FederationCheckpoint,
    ) -> anyhow::Result<FederationJournalCatchUpResponse> {
        let checkpoint_id = checkpoint.id()?;
        if request.target_checkpoint_id != Some(checkpoint_id) {
            return Err(FederationError::CheckpointInvalid {
                reason: "catch-up request does not bind the supplied target checkpoint".to_string(),
            }
            .into());
        }
        let request =
            self.signed_request(member, FederationRestMessageKind::CatchUp, request.clone())?;
        ensure_checkpoint_catch_up_request_size(&request)?;
        let response: FederationJournalCatchUpResponse = self
            .http_post_bounded_with_timeout(
                member,
                "journal",
                "checkpoint catch-up request",
                &request,
                self.sync_config.peer_request_timeout,
                FEDERATION_MAX_CHECKPOINT_CATCH_UP_PAGE_BYTES,
            )
            .await?;
        if response.version != FEDERATION_REST_API_VERSION {
            return Err(FederationError::RestVersionMismatch {
                expected: FEDERATION_REST_API_VERSION,
                found: response.version,
            }
            .into());
        }
        if response.federation_id != self.federation_id {
            return Err(FederationError::CatchUpFederationMismatch {
                expected: self.federation_id,
                found: response.federation_id,
            }
            .into());
        }
        response.ensure_within_checkpoint_page_limit()?;
        response.validate_checkpoint_binding(checkpoint)?;
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

    fn validate_signed_response<P>(
        &self,
        response: &FederationSignedRequest<P>,
        member: &MemberConfigPublic,
        kind: FederationRestMessageKind,
    ) -> anyhow::Result<()>
    where
        P: Serialize,
    {
        response.verify(&self.public_config, self.private_config.member_id)?;
        if response.sender != member.member_id {
            anyhow::bail!(
                "signed federation response came from member {}, expected {}",
                response.sender,
                member.member_id
            );
        }
        if response.kind != kind {
            return Err(FederationError::RestMessageKindMismatch {
                expected: kind,
                found: response.kind,
            }
            .into());
        }
        Ok(())
    }

    fn ensure_configured_peer(&self, member: &MemberConfigPublic) -> anyhow::Result<()> {
        let Some(configured) = self.public_config.members.get(&member.member_id) else {
            return Err(FederationError::UnknownMember {
                member_id: member.member_id,
            }
            .into());
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

fn ensure_checkpoint_catch_up_request_size<P>(
    request: &FederationSignedRequest<P>,
) -> anyhow::Result<usize>
where
    P: Serialize,
{
    let actual = serde_json::to_vec(request)?.len();
    if actual > FEDERATION_MAX_CHECKPOINT_CATCH_UP_REQUEST_BYTES {
        return Err(FederationError::CheckpointCatchUpTooLarge {
            resource: "request",
            actual: u64::try_from(actual).unwrap_or(u64::MAX),
            max: FEDERATION_MAX_CHECKPOINT_CATCH_UP_REQUEST_BYTES as u64,
        }
        .into());
    }
    Ok(actual)
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
    public_config: &FederationConfig,
    member: &MemberConfigPublic,
    status: &FederationPeerStatus,
) -> anyhow::Result<()> {
    if status.version != FEDERATION_REST_API_VERSION {
        return Err(FederationError::RestVersionMismatch {
            expected: FEDERATION_REST_API_VERSION,
            found: status.version,
        }
        .into());
    }
    if status.member_id != member.member_id {
        return Err(FederationError::MemberIdMismatch {
            expected: member.member_id,
            found: status.member_id,
        }
        .into());
    }
    if status.federation_id != public_config.federation_id {
        return Err(FederationError::RestFederationMismatch {
            expected: public_config.federation_id,
            found: status.federation_id,
        }
        .into());
    }
    if status.aleph_bft_next_item_index.is_none() {
        return Err(FederationError::AlephBftFinalizedCursorMissing {
            member_id: member.member_id,
        }
        .into());
    }
    if status
        .aleph_bft_next_unit_index_by_member
        .keys()
        .copied()
        .ne(public_config.members.keys().copied())
    {
        return Err(FederationError::CheckpointInvalid {
            reason: "peer status per-member AlephBFT cursor roster mismatch".to_string(),
        }
        .into());
    }

    Ok(())
}

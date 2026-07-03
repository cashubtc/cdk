//! Private federation member REST routes.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use cdk::federation::{
    FederationConfig, FederationError, FederationId, FederationJournalCatchUpRequest,
    FederationJournalCatchUpResponse, FederationJournalDigest, FederationJournalEntry,
    FederationOperationId, FederationPeerStatus, FederationPeerStatusRequest,
    FederationRestMessageKind, FederationRestSequenceTracker, FederationSessionOutcome,
    FederationSessionOutcomeQuorum, FederationSignedRequest, FederationStateDigest,
    FederationSubmitOperationRequest, FederationSubmitOperationResponse,
    InMemoryFederationConsensus, MemberConfigPrivate, MemberConfigPublic,
    FEDERATION_REST_API_VERSION,
};
use cdk_http_client::{Async, Transport};
use serde::{Deserialize, Serialize};
use tokio::time::{self, Duration, MissedTickBehavior};
use tracing::instrument;

use crate::MintState;

type ConsensusHandle = std::sync::Arc<tokio::sync::Mutex<InMemoryFederationConsensus>>;
type SequenceHandle = std::sync::Arc<tokio::sync::Mutex<FederationRestSequenceTracker>>;

const FEDERATION_PEER_SYNC_INTERVAL: Duration = Duration::from_secs(5);
static FEDERATION_REST_SEQUENCE_SEED: AtomicU64 = AtomicU64::new(0);

pub(crate) fn create_federation_router() -> Router<MintState> {
    Router::new()
        .route("/status", post(post_status))
        .route("/local-status", post(post_local_status))
        .route("/operations", post(post_operation))
        .route("/journal", post(post_journal))
}

pub(crate) fn spawn_federation_peer_sync_loop(state: MintState) {
    let Some(config) = state.mint.federation_config().cloned() else {
        return;
    };
    let public_config = config.public_config().clone();
    let local_member = config.member_id();
    let peers = public_config
        .members
        .values()
        .filter(|member| member.member_id != local_member)
        .cloned()
        .collect::<Vec<_>>();

    if peers.is_empty() {
        return;
    }

    let private_config = config.private_config().clone();
    tokio::spawn(async move {
        let client =
            match FederationRestPeerClient::new(Async::default(), &public_config, private_config) {
                Ok(client) => client,
                Err(err) => {
                    tracing::error!("Could not start federation peer sync loop: {}", err);
                    return;
                }
            };
        let mut interval = time::interval(FEDERATION_PEER_SYNC_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            run_federation_peer_sync_round(&state, &client, &peers).await;
        }
    });
}

async fn run_federation_peer_sync_round<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
) -> usize
where
    T: Transport,
{
    let mut synced = 0;
    for peer in peers {
        match client.sync_once_from_member(state, peer).await {
            Ok(status) => {
                synced += 1;
                tracing::debug!(
                    member_id = %peer.member_id,
                    operation_count = status.operation_count,
                    latest_index = ?status.latest_index,
                    "Federation peer sync completed"
                );
            }
            Err(err) => {
                tracing::warn!(
                    member_id = %peer.member_id,
                    "Federation peer sync failed: {}",
                    err
                );
            }
        }
    }
    synced
}

#[instrument(skip_all)]
async fn post_status(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationPeerStatusRequest>>,
) -> Result<Json<FederationPeerStatus>, Response> {
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::Status).await?;
    state
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
    let consensus = federation_consensus(&state)?;
    let consensus = consensus.lock().await;
    Ok(Json(consensus.peer_status()))
}

#[instrument(skip_all)]
async fn post_local_status(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationPeerStatusRequest>>,
) -> Result<Json<FederationLocalStatusResponse>, Response> {
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::Status).await?;
    state
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
    let consensus = federation_consensus(&state)?;
    let consensus = consensus.lock().await;
    let peer_status = consensus.peer_status();
    let sync_status = state.federation_sync_status().await.unwrap_or_default();
    let session_outcome_quorum = session_outcome_quorum_status(&state)
        .await
        .map_err(cdk_error_response)?;
    Ok(Json(FederationLocalStatusResponse::new(
        peer_status,
        sync_status,
        session_outcome_quorum,
    )))
}

#[instrument(skip_all)]
async fn post_operation(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationSubmitOperationRequest>>,
) -> Result<Json<FederationSubmitOperationResponse>, Response> {
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::SubmitOperation)
            .await?;
    let response = state
        .submit_federation_envelope_with_sequence(
            payload.payload.envelope,
            Some((verified.sender, verified.sequence)),
        )
        .await
        .map_err(cdk_error_response)?;
    tracing::debug!("Accepted federation operation {}", response.operation_id);
    Ok(Json(response))
}

#[instrument(skip_all)]
async fn post_journal(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationJournalCatchUpRequest>>,
) -> Result<Json<FederationJournalCatchUpResponse>, Response> {
    let verified =
        verify_federation_request(&state, &payload, FederationRestMessageKind::CatchUp).await?;
    state
        .persist_federation_sequence(verified.sender, verified.sequence)
        .await
        .map_err(cdk_error_response)?;
    let consensus = federation_consensus(&state)?;
    let private_config = federation_runtime_config(&state)?.private_config().clone();
    tracing::debug!(
        next_index = payload.payload.next_index,
        limit = ?payload.payload.limit,
        "Received federation journal catch-up request"
    );
    let consensus = consensus.lock().await;
    let response = consensus
        .catch_up_checked(&payload.payload)
        .map_err(federation_error_response)?
        .with_signed_session_outcome(&private_config)
        .map_err(federation_error_response)?;
    Ok(Json(response))
}

async fn verify_federation_request<T>(
    state: &MintState,
    request: &FederationSignedRequest<T>,
    expected_kind: FederationRestMessageKind,
) -> Result<VerifiedFederationRequest, Response>
where
    T: Serialize,
{
    if request.kind != expected_kind {
        return Err(federation_error_response(
            FederationError::RestMessageKindMismatch {
                expected: expected_kind,
                found: request.kind,
            },
        ));
    }

    let runtime_config = federation_runtime_config(state)?;
    let sequences = federation_sequences(state)?;
    let sequences = sequences.lock().await;
    sequences
        .verify_next(
            request,
            runtime_config.public_config(),
            runtime_config.member_id(),
        )
        .map_err(federation_error_response)?;

    Ok(VerifiedFederationRequest {
        sender: request.sender,
        sequence: request.sequence,
    })
}

#[derive(Debug, Clone, Copy)]
struct VerifiedFederationRequest {
    sender: cdk::federation::MemberId,
    sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FederationLocalStatusResponse {
    pub(crate) version: u16,
    pub(crate) peer_status: FederationPeerStatus,
    pub(crate) sync_state: String,
    pub(crate) sync_ready: bool,
    pub(crate) sync_message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_outcome_quorum_operation_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) session_outcome_quorum_signers: Vec<cdk::federation::MemberId>,
}

impl FederationLocalStatusResponse {
    fn new(
        peer_status: FederationPeerStatus,
        sync_status: crate::FederationSyncStatus,
        session_outcome_quorum: Option<FederationSessionOutcomeQuorum>,
    ) -> Self {
        let session_outcome_quorum_operation_count = session_outcome_quorum
            .as_ref()
            .map(|quorum| quorum.latest_index.saturating_add(1));
        let session_outcome_quorum_signers = session_outcome_quorum
            .map(|quorum| quorum.signers.into_iter().collect())
            .unwrap_or_default();
        Self {
            version: FEDERATION_REST_API_VERSION,
            peer_status,
            sync_state: sync_status.state_name().to_string(),
            sync_ready: sync_status.is_ready(),
            sync_message: sync_status.not_ready_message(),
            session_outcome_quorum_operation_count,
            session_outcome_quorum_signers,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SessionOutcomeRangeKey {
    from_index: u64,
    operation_count: u64,
    latest_index: u64,
    previous_order_digest: FederationJournalDigest,
    range_order_digest: FederationJournalDigest,
    latest_order_digest: FederationJournalDigest,
    state_digest: FederationStateDigest,
    operation_ids: Vec<FederationOperationId>,
}

impl From<&FederationSessionOutcome> for SessionOutcomeRangeKey {
    fn from(outcome: &FederationSessionOutcome) -> Self {
        Self {
            from_index: outcome.from_index,
            operation_count: outcome.operation_count,
            latest_index: outcome.latest_index,
            previous_order_digest: outcome.previous_order_digest,
            range_order_digest: outcome.range_order_digest,
            latest_order_digest: outcome.latest_order_digest,
            state_digest: outcome.state_digest,
            operation_ids: outcome.operation_ids.clone(),
        }
    }
}

async fn session_outcome_quorum_status(
    state: &MintState,
) -> Result<Option<FederationSessionOutcomeQuorum>, cdk::Error> {
    let Some(runtime_config) = state.mint.federation_config() else {
        return Ok(None);
    };
    let db = state.mint.localstore();
    let entries = db.get_federation_journal_entries().await?;
    let outcomes = db.get_federation_session_outcomes().await?;
    if entries.is_empty() || outcomes.is_empty() {
        return Ok(None);
    }

    let entries_by_index = entries
        .into_iter()
        .map(|entry| (entry.index, entry))
        .collect::<BTreeMap<_, _>>();
    let mut grouped_outcomes: BTreeMap<SessionOutcomeRangeKey, Vec<FederationSessionOutcome>> =
        BTreeMap::new();
    for outcome in outcomes {
        grouped_outcomes
            .entry(SessionOutcomeRangeKey::from(&outcome))
            .or_default()
            .push(outcome);
    }

    let mut best_quorum: Option<FederationSessionOutcomeQuorum> = None;
    for (range, outcomes) in grouped_outcomes {
        let Some(entries) = entries_for_session_outcome_range(&entries_by_index, &range) else {
            continue;
        };
        let response = FederationJournalCatchUpResponse {
            version: FEDERATION_REST_API_VERSION,
            federation_id: runtime_config.public_config().federation_id,
            from_index: range.from_index,
            entries,
            latest_index: Some(range.latest_index),
            latest_order_digest: range.latest_order_digest,
            state_digest: range.state_digest,
            session_outcome: None,
            complete: true,
        };

        let quorum = match response
            .verify_session_outcome_quorum(runtime_config.public_config(), &outcomes)
        {
            Ok(quorum) => quorum,
            Err(FederationError::SessionOutcomeQuorumNotReached { .. }) => continue,
            Err(_) => continue,
        };
        match &best_quorum {
            Some(best) if best.latest_index >= quorum.latest_index => {}
            _ => best_quorum = Some(quorum),
        }
    }

    Ok(best_quorum)
}

fn entries_for_session_outcome_range(
    entries_by_index: &BTreeMap<u64, FederationJournalEntry>,
    range: &SessionOutcomeRangeKey,
) -> Option<Vec<FederationJournalEntry>> {
    if range.operation_count == 0
        || range.latest_index < range.from_index
        || range.operation_count != range.latest_index.saturating_sub(range.from_index) + 1
    {
        return None;
    }

    let mut entries = Vec::with_capacity(range.operation_count as usize);
    for index in range.from_index..=range.latest_index {
        entries.push(entries_by_index.get(&index)?.clone());
    }
    Some(entries)
}

fn federation_runtime_config(
    state: &MintState,
) -> Result<&cdk::mint::FederationRuntimeConfig, Response> {
    state
        .mint
        .federation_config()
        .ok_or_else(federation_not_configured)
}

fn federation_consensus(state: &MintState) -> Result<ConsensusHandle, Response> {
    state
        .federation_consensus
        .clone()
        .ok_or_else(federation_not_configured)
}

fn federation_sequences(state: &MintState) -> Result<SequenceHandle, Response> {
    state
        .federation_sequences
        .clone()
        .ok_or_else(federation_not_configured)
}

fn federation_not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        "mint is not configured as a federation member",
    )
        .into_response()
}

fn federation_error_response(err: FederationError) -> Response {
    let status = match err {
        FederationError::ConflictingOperation { .. } => StatusCode::CONFLICT,
        FederationError::OperationFederationMismatch { .. }
        | FederationError::CatchUpFederationMismatch { .. }
        | FederationError::CatchUpIndexMismatch { .. }
        | FederationError::CatchUpDigestMismatch { .. }
        | FederationError::CatchUpOperationIdMismatch { .. }
        | FederationError::JournalConflictKeysMismatch { .. } => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_REQUEST,
    };
    tracing::warn!("Federation request failed: {}", err);
    (status, err.to_string()).into_response()
}

fn cdk_error_response(err: cdk::Error) -> Response {
    match err {
        cdk::Error::Federation(err) => federation_error_response(err),
        err => {
            tracing::error!("Federation storage mutation failed: {}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FederationRestPeerClient<T = Async> {
    transport: T,
    federation_id: FederationId,
    private_config: MemberConfigPrivate,
    next_sequence: Arc<AtomicU64>,
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
        private_config.validate_against_public(public_config)?;
        Ok(Self {
            transport,
            federation_id: public_config.federation_id,
            private_config,
            next_sequence: Arc::new(AtomicU64::new(initial_federation_rest_sequence())),
        })
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
            .transport
            .http_post(federation_endpoint(member, "status")?, None, &request)
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
            .transport
            .http_post(federation_endpoint(member, "operations")?, None, &request)
            .await?;

        if response.operation_id != expected_operation_id {
            anyhow::bail!(
                "federation peer {} returned operation id {} for submitted operation {}",
                member.member_id,
                response.operation_id,
                expected_operation_id
            );
        }

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
            .transport
            .http_post(federation_endpoint(member, "journal")?, None, &request)
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

    pub(crate) async fn catch_up_state_from_member(
        &self,
        state: &MintState,
        member: &MemberConfigPublic,
    ) -> anyhow::Result<Vec<FederationJournalEntry>> {
        let consensus = state
            .federation_consensus
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("mint is not configured as a federation member"))?;
        let request = {
            let consensus = consensus.lock().await;
            consensus.catch_up_request()
        };
        let response = self.catch_up(member, &request).await?;
        Ok(state.apply_federation_catch_up_response(&response).await?)
    }

    pub(crate) async fn sync_once_from_member(
        &self,
        state: &MintState,
        member: &MemberConfigPublic,
    ) -> anyhow::Result<FederationPeerStatus> {
        let peer_status = self.peer_status(member).await?;
        let mut local_status = local_peer_status(state).await?;

        if peer_status.operation_count > local_status.operation_count {
            state
                .mark_federation_lagging(
                    member.member_id,
                    local_status.operation_count,
                    peer_status.operation_count,
                )
                .await;
            state.mark_federation_catching_up(member.member_id).await;

            while local_status.operation_count < peer_status.operation_count {
                let before = local_status.operation_count;
                if let Err(err) = self.catch_up_state_from_member(state, member).await {
                    state
                        .mark_federation_lagging(
                            member.member_id,
                            before,
                            peer_status.operation_count,
                        )
                        .await;
                    return Err(err);
                }

                local_status = local_peer_status(state).await?;
                if local_status.operation_count == before {
                    state
                        .mark_federation_halted(format!(
                            "peer {} returned no catch-up progress from operation {}",
                            member.member_id, before
                        ))
                        .await;
                    anyhow::bail!(
                        "federation peer {} returned no catch-up progress from operation {}",
                        member.member_id,
                        before
                    );
                }
            }
        }

        local_status = local_peer_status(state).await?;
        let digest_mismatch = local_status.latest_order_digest != peer_status.latest_order_digest
            || local_status.state_digest != peer_status.state_digest;
        if local_status.operation_count == peer_status.operation_count && digest_mismatch {
            state
                .mark_federation_halted(format!(
                    "peer {} digest mismatch at operation count {}",
                    member.member_id, peer_status.operation_count
                ))
                .await;
            anyhow::bail!(
                "federation peer {} digest mismatch at operation count {}",
                member.member_id,
                peer_status.operation_count
            );
        }

        if local_status.operation_count >= peer_status.operation_count {
            state
                .mark_federation_healthy_if_caught_up(local_status.operation_count)
                .await;
        }

        Ok(local_status)
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
        Ok(FederationSignedRequest::new_signed(
            self.federation_id,
            member.member_id,
            self.next_sequence.fetch_add(1, Ordering::Relaxed) + 1,
            kind,
            payload,
            &self.private_config,
        )?)
    }
}

fn initial_federation_rest_sequence() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| {
            duration
                .as_secs()
                .saturating_mul(1_000_000_000)
                .saturating_add(u64::from(duration.subsec_nanos()))
        })
        .unwrap_or_default();

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

#[cfg_attr(not(test), allow(dead_code))]
fn federation_endpoint(member: &MemberConfigPublic, endpoint: &str) -> anyhow::Result<url::Url> {
    Ok(member
        .federation_api_url
        .join_paths(&["federation", "v1", endpoint])?)
}

#[cfg_attr(not(test), allow(dead_code))]
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

async fn local_peer_status(state: &MintState) -> anyhow::Result<FederationPeerStatus> {
    let consensus = state
        .federation_consensus
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("mint is not configured as a federation member"))?;
    Ok(consensus.lock().await.peer_status())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use bip39::Mnemonic;
    use cdk::federation::{
        generate_trusted_dealer_configs, FederationMintQuotePaymentObservation,
        FederationOperation, FederationOperationEnvelope, FederationOperationKind,
        FederationOperationStatus, MemberId, ThresholdParams, TrustedDealerKeysetSpec,
        TrustedDealerMember,
    };
    use cdk::mint::{MintBuilder, MintInput, MintMeltLimits, QuoteId};
    use cdk::mint_url::MintUrl;
    use cdk::nuts::nut00::KnownMethod;
    use cdk::nuts::nut01::{BlsG1PublicKey, BlsSecretKey};
    use cdk::nuts::nut22::AuthToken;
    use cdk::nuts::{
        BlindedMessage, CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState,
        MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteState, MintRequest,
        PaymentMethod, PublicKey, RestoreRequest, SecretKey,
    };
    use cdk::types::FeeReserve;
    use cdk::Amount;
    use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription, FakeWallet};
    use cdk_http_client::HttpError;
    use serde::de::DeserializeOwned;
    use serde::Serialize;

    use super::*;

    #[derive(Debug, Default)]
    struct MockFederationTransportState {
        status_response: Option<FederationPeerStatus>,
        submit_response: Option<FederationSubmitOperationResponse>,
        catch_up_response: Option<FederationJournalCatchUpResponse>,
        status_requests: Vec<FederationSignedRequest<FederationPeerStatusRequest>>,
        submitted_operations: Vec<FederationSignedRequest<FederationSubmitOperationRequest>>,
        catch_up_requests: Vec<FederationSignedRequest<FederationJournalCatchUpRequest>>,
    }

    #[derive(Debug, Clone, Default)]
    struct MockFederationTransport {
        state: Arc<Mutex<MockFederationTransportState>>,
    }

    impl MockFederationTransport {
        fn with_status_response(self, response: FederationPeerStatus) -> Self {
            self.state
                .lock()
                .expect("mock transport lock")
                .status_response = Some(response);
            self
        }

        fn with_submit_response(self, response: FederationSubmitOperationResponse) -> Self {
            self.state
                .lock()
                .expect("mock transport lock")
                .submit_response = Some(response);
            self
        }

        fn with_catch_up_response(self, response: FederationJournalCatchUpResponse) -> Self {
            self.state
                .lock()
                .expect("mock transport lock")
                .catch_up_response = Some(response);
            self
        }

        fn status_requests(&self) -> Vec<FederationSignedRequest<FederationPeerStatusRequest>> {
            self.state
                .lock()
                .expect("mock transport lock")
                .status_requests
                .clone()
        }

        fn submitted_operations(
            &self,
        ) -> Vec<FederationSignedRequest<FederationSubmitOperationRequest>> {
            self.state
                .lock()
                .expect("mock transport lock")
                .submitted_operations
                .clone()
        }

        fn catch_up_requests(
            &self,
        ) -> Vec<FederationSignedRequest<FederationJournalCatchUpRequest>> {
            self.state
                .lock()
                .expect("mock transport lock")
                .catch_up_requests
                .clone()
        }
    }

    #[async_trait]
    impl Transport for MockFederationTransport {
        fn with_proxy(
            &mut self,
            _proxy: url::Url,
            _host_matcher: Option<&str>,
            _accept_invalid_certs: bool,
        ) -> Result<(), HttpError> {
            Ok(())
        }

        async fn http_get<R>(&self, url: url::Url, _auth: Option<AuthToken>) -> Result<R, HttpError>
        where
            R: DeserializeOwned,
        {
            Err(HttpError::Other(format!(
                "unexpected GET path {}",
                url.path()
            )))
        }

        async fn http_post<P, R>(
            &self,
            url: url::Url,
            _auth_token: Option<AuthToken>,
            payload: &P,
        ) -> Result<R, HttpError>
        where
            P: Serialize + Send + Sync,
            R: DeserializeOwned,
        {
            let mut state = self.state.lock().expect("mock transport lock");
            match url.path() {
                path if path.ends_with("/federation/v1/status") => {
                    let request = serde_json::from_value::<
                        FederationSignedRequest<FederationPeerStatusRequest>,
                    >(serde_json::to_value(payload)?)?;
                    state.status_requests.push(request);
                    let response = state
                        .status_response
                        .clone()
                        .ok_or_else(|| HttpError::Other("missing status response".to_string()))?;
                    json_response(response)
                }
                path if path.ends_with("/federation/v1/operations") => {
                    let request = serde_json::from_value::<
                        FederationSignedRequest<FederationSubmitOperationRequest>,
                    >(serde_json::to_value(payload)?)?;
                    state.submitted_operations.push(request);
                    let response = state
                        .submit_response
                        .clone()
                        .ok_or_else(|| HttpError::Other("missing submit response".to_string()))?;
                    json_response(response)
                }
                path if path.ends_with("/federation/v1/journal") => {
                    let request = serde_json::from_value::<
                        FederationSignedRequest<FederationJournalCatchUpRequest>,
                    >(serde_json::to_value(payload)?)?;
                    state.catch_up_requests.push(request);
                    let response = state
                        .catch_up_response
                        .clone()
                        .ok_or_else(|| HttpError::Other("missing catch-up response".to_string()))?;
                    json_response(response)
                }
                path => Err(HttpError::Other(format!("unexpected POST path {path}"))),
            }
        }
    }

    fn json_response<T, R>(value: T) -> Result<R, HttpError>
    where
        T: Serialize,
        R: DeserializeOwned,
    {
        Ok(serde_json::from_value(serde_json::to_value(value)?)?)
    }

    fn member_id(value: u16) -> MemberId {
        MemberId::new(value).expect("member id")
    }

    fn private_member(value: u16) -> MemberConfigPrivate {
        MemberConfigPrivate {
            member_id: member_id(value),
            identity_secret_key: SecretKey::from_slice(&[value as u8; 32])
                .expect("identity secret"),
            keyset_secret_shares: BTreeMap::new(),
        }
    }

    fn transport_config() -> (FederationConfig, MemberConfigPrivate, MemberConfigPrivate) {
        let members = vec![
            TrustedDealerMember::new(
                member_id(1),
                MintUrl::from_str("https://mint1.example.com").expect("public mint url"),
                MintUrl::from_str("https://fed1.example.com/api").expect("federation api url"),
                SecretKey::from_slice(&[1; 32]).expect("identity secret"),
            ),
            TrustedDealerMember::new(
                member_id(2),
                MintUrl::from_str("https://mint2.example.com").expect("public mint url"),
                MintUrl::from_str("https://fed2.example.com/api").expect("federation api url"),
                SecretKey::from_slice(&[2; 32]).expect("identity secret"),
            ),
        ];
        let keyset = TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            BTreeMap::from([(
                Amount::from(1),
                vec![
                    BlsSecretKey::from_reduced_bytes(&[11; 32]),
                    BlsSecretKey::from_reduced_bytes(&[12; 32]),
                ],
            )]),
        );
        let bundle = generate_trusted_dealer_configs(
            ThresholdParams::new(2, 2, 2).expect("threshold params"),
            members,
            vec![keyset],
        )
        .expect("trusted dealer bundle");
        let first_private = bundle
            .private_configs
            .get(&member_id(1))
            .expect("first private config")
            .clone();
        let second_private = bundle
            .private_configs
            .get(&member_id(2))
            .expect("second private config")
            .clone();
        (bundle.public_config, first_private, second_private)
    }

    async fn federation_route_state_with_local_private() -> (
        MintState,
        FederationConfig,
        MemberConfigPrivate,
        MemberConfigPrivate,
    ) {
        let (public_config, local_private, peer_private) = transport_config();
        let runtime_config =
            cdk::mint::FederationRuntimeConfig::new(public_config.clone(), local_private.clone())
                .expect("runtime config");
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let mut builder = MintBuilder::new(db.clone())
            .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
            .with_federation_config(runtime_config.clone());
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(fake),
            )
            .await
            .expect("payment processor");
        let mint = builder
            .build_with_seed(db, &mnemonic.to_seed_normalized(""))
            .await
            .expect("mint");
        let state = MintState {
            mint: Arc::new(mint),
            cache: Arc::new(crate::cache::HttpCache::default()),
            federation_consensus: Some(Arc::new(tokio::sync::Mutex::new(
                InMemoryFederationConsensus::new(
                    runtime_config.federation_id(),
                    runtime_config.member_id(),
                ),
            ))),
            federation_sequences: Some(Arc::new(tokio::sync::Mutex::new(
                FederationRestSequenceTracker::new(),
            ))),
            federation_sync: Some(Arc::new(tokio::sync::Mutex::new(
                crate::FederationSyncState::default(),
            ))),
        };

        (state, public_config, local_private, peer_private)
    }

    async fn federation_route_state() -> (MintState, FederationConfig, MemberConfigPrivate) {
        let (state, public_config, _, peer_private) =
            federation_route_state_with_local_private().await;
        (state, public_config, peer_private)
    }

    fn signed_operation_request(
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

    fn signed_status_request(
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        receiver: MemberId,
        sequence: u64,
    ) -> FederationSignedRequest<FederationPeerStatusRequest> {
        FederationSignedRequest::new_signed(
            public_config.federation_id,
            receiver,
            sequence,
            FederationRestMessageKind::Status,
            FederationPeerStatusRequest::default(),
            private_config,
        )
        .expect("signed status request")
    }

    fn signed_catch_up_request(
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

    fn signed_catch_up_response(
        response: FederationJournalCatchUpResponse,
        private_config: &MemberConfigPrivate,
    ) -> FederationJournalCatchUpResponse {
        response
            .with_signed_session_outcome(private_config)
            .expect("signed catch-up response")
    }

    async fn consensus_operation_count(state: &MintState) -> u64 {
        state
            .federation_consensus
            .as_ref()
            .expect("consensus")
            .lock()
            .await
            .peer_status()
            .operation_count
    }

    fn payment_observation_envelope(federation_id: FederationId) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id,
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: "quote-paid".to_string(),
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(21),
                amount_issued: Amount::ZERO,
                payment_proof: Some("fake-wallet-paid".to_string()),
            }),
        )
    }

    fn oversized_payment_observation_envelope(
        federation_id: FederationId,
    ) -> FederationOperationEnvelope {
        let mut envelope = payment_observation_envelope(federation_id);
        let FederationOperation::MintQuotePayment(observation) = &mut envelope.operation else {
            panic!("mint quote payment observation");
        };
        observation.payment_proof =
            Some("x".repeat(cdk::federation::FEDERATION_MAX_OPERATION_ENVELOPE_BYTES));
        envelope
    }

    fn mint_quote_envelope(state: &MintState, quote_id: &QuoteId) -> FederationOperationEnvelope {
        let config = state.mint.federation_config().expect("federation config");
        config.mint_quote_operation(
            quote_id.clone(),
            cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                amount: Amount::from(1),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }),
            cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                quote: quote_id.to_string(),
                request: format!("invoice-{quote_id}"),
                amount: Some(Amount::from(1)),
                unit: Some(CurrencyUnit::Sat),
                state: MintQuoteState::Unpaid,
                expiry: Some(10_000),
                pubkey: None,
            }),
        )
    }

    fn paid_observation_envelope(
        state: &MintState,
        quote_id: &QuoteId,
    ) -> FederationOperationEnvelope {
        let config = state.mint.federation_config().expect("federation config");
        config.mint_quote_payment_observation(
            quote_id.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some(format!("paid-{quote_id}")),
        )
    }

    fn mint_output(state: &MintState, quote_id: &QuoteId) -> BlindedMessage {
        let config = state.mint.federation_config().expect("federation config");
        BlindedMessage::new(
            Amount::from(1),
            config.public_config().keysets[0].id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(
                format!("federation replay output {quote_id}").as_bytes(),
            )),
        )
    }

    fn aggregate_secret_for_amount(amount: u64) -> BlsSecretKey {
        match amount {
            1 => BlsSecretKey::from_reduced_bytes(&[11; 32]),
            _ => panic!("missing aggregate secret fixture for amount {amount}"),
        }
    }

    fn proof_for_amount(state: &MintState, amount: u64, label: &str) -> cdk::nuts::Proof {
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let secret = cdk::secret::Secret::new(format!("federation replay proof {label}"));
        let signature = BlsG1PublicKey::hash_to_curve(secret.as_bytes())
            .mul(&aggregate_secret_for_amount(amount));

        cdk::nuts::Proof::new(
            Amount::from(amount),
            keyset.id,
            secret,
            PublicKey::from(signature),
        )
    }

    fn swap_output(state: &MintState, label: &str) -> BlindedMessage {
        let config = state.mint.federation_config().expect("federation config");
        BlindedMessage::new(
            Amount::from(1),
            config.public_config().keysets[0].id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(
                format!("federation replay swap output {label}").as_bytes(),
            )),
        )
    }

    async fn melt_quote(state: &MintState, amount_msat: u64) -> QuoteId {
        melt_quote_with_description(state, amount_msat, "".to_string()).await
    }

    async fn melt_quote_with_description(
        state: &MintState,
        amount_msat: u64,
        description: String,
    ) -> QuoteId {
        let response = state
            .mint
            .get_melt_quote(cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: create_fake_invoice(amount_msat, description),
                unit: CurrencyUnit::Sat,
                options: None,
            }))
            .await
            .expect("melt quote");

        match response {
            cdk::MeltQuoteCreateResponse::Bolt11(response) => response.quote,
            _ => panic!("expected bolt11 melt quote"),
        }
    }

    fn melt_payment_observation_envelope(
        state: &MintState,
        quote_id: &QuoteId,
        state_observed: MeltQuoteState,
        payment_proof: Option<String>,
    ) -> FederationOperationEnvelope {
        let config = state.mint.federation_config().expect("federation config");
        config.melt_quote_payment_observation(
            quote_id.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            state_observed,
            Amount::from(1),
            Amount::from(1),
            payment_proof,
            None,
        )
    }

    fn restore_envelope(
        federation_id: FederationId,
        label: &'static str,
    ) -> FederationOperationEnvelope {
        FederationOperationEnvelope::new(
            federation_id,
            FederationOperation::Restore(cdk::federation::FederationRestoreOperation {
                request: RestoreRequest {
                    outputs: vec![cdk::nuts::BlindedMessage::new(
                        Amount::from(1),
                        cdk::nuts::Id::from_str("00bfa73302d12ffd").expect("keyset id"),
                        PublicKey::from_hex(label).expect("public key"),
                    )],
                },
            }),
        )
    }

    #[tokio::test]
    async fn rest_peer_client_validates_peer_status() {
        let (public_config, local_private, _) = transport_config();
        let federation_id = public_config.federation_id;
        let member = public_config.member(member_id(2)).expect("peer member");
        let status = FederationPeerStatus::new(
            federation_id,
            member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status.clone());
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let response = client.peer_status(member).await.expect("peer status");

        assert_eq!(response, status);
        let requests = transport.status_requests();
        assert_eq!(requests.len(), 1);
        requests[0]
            .verify(&public_config, member.member_id)
            .expect("signed status request");
        assert_eq!(requests[0].sender, member_id(1));
        assert_eq!(requests[0].kind, FederationRestMessageKind::Status);
    }

    #[tokio::test]
    async fn rest_peer_client_initial_sequence_advances_across_clients() {
        let (public_config, local_private, _) = transport_config();
        let federation_id = public_config.federation_id;
        let member = public_config.member(member_id(2)).expect("peer member");
        let status = FederationPeerStatus::new(
            federation_id,
            member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status);
        let first_client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
                .expect("first client");
        first_client
            .peer_status(member)
            .await
            .expect("first peer status");

        let second_client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("second client");
        second_client
            .peer_status(member)
            .await
            .expect("second peer status");

        let requests = transport.status_requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].sequence > 1);
        assert!(
            requests[1].sequence > requests[0].sequence,
            "fresh clients must not replay stale federation REST sequences"
        );
    }

    #[tokio::test]
    async fn rest_peer_client_rejects_status_wrong_federation() {
        let (public_config, local_private, _) = transport_config();
        let member = public_config.member(member_id(2)).expect("peer member");
        let status = FederationPeerStatus::new(
            super::FederationId::from_bytes([32; 32]),
            member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status);
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        assert!(client.peer_status(member).await.is_err());
    }

    #[tokio::test]
    async fn rest_peer_client_submits_payment_observation_operations() {
        let (public_config, local_private, _) = transport_config();
        let federation_id = public_config.federation_id;
        let member = public_config.member(member_id(2)).expect("peer member");
        let envelope = payment_observation_envelope(federation_id);
        let request = FederationSubmitOperationRequest::new(envelope.clone());
        let mut peer_consensus = InMemoryFederationConsensus::new(federation_id, member.member_id);
        let submit_response = peer_consensus
            .submit_operation(FederationSubmitOperationRequest::new(envelope))
            .expect("peer submit response");
        let transport =
            MockFederationTransport::default().with_submit_response(submit_response.clone());
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let response = client
            .submit_operation(member, &request)
            .await
            .expect("submit operation");

        assert_eq!(response.operation_id, submit_response.operation_id);
        let submitted = transport.submitted_operations();
        assert_eq!(submitted.len(), 1);
        submitted[0]
            .verify(&public_config, member.member_id)
            .expect("signed submit request");
        assert_eq!(
            submitted[0].payload.operation_id().expect("operation id"),
            response.operation_id
        );
        assert_eq!(
            submitted[0].payload.envelope.operation.kind(),
            FederationOperationKind::MintQuotePayment
        );
    }

    #[tokio::test]
    async fn rest_peer_client_rejects_oversized_operation_before_broadcast() {
        let (public_config, local_private, _) = transport_config();
        let member = public_config.member(member_id(2)).expect("peer member");
        let request = FederationSubmitOperationRequest::new(
            oversized_payment_observation_envelope(public_config.federation_id),
        );
        let transport = MockFederationTransport::default();
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let err = client
            .submit_operation(member, &request)
            .await
            .expect_err("oversized operation should not be broadcast");

        assert!(matches!(
            err.downcast_ref::<FederationError>(),
            Some(FederationError::OperationEnvelopeTooLarge { .. })
        ));
        assert!(transport.submitted_operations().is_empty());
    }

    #[tokio::test]
    async fn rest_peer_client_catches_up_local_consensus_from_member() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let first = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        state
            .submit_federation_envelope(first.clone())
            .await
            .expect("local first operation");
        peer_consensus
            .submit_envelope(first)
            .expect("peer first operation");
        peer_consensus
            .submit_envelope(second)
            .expect("peer second operation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("peer catch-up response");
        let transport = MockFederationTransport::default()
            .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private));
        let client =
            FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
                .expect("client");

        let applied = client
            .catch_up_state_from_member(&state, peer_member)
            .await
            .expect("catch up");

        assert_eq!(applied.len(), 1);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .peer_status()
                .latest_order_digest,
            peer_consensus.peer_status().latest_order_digest
        );
        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("persisted journal");
        assert_eq!(persisted.len(), 2);
        assert_eq!(persisted[1].operation_id, applied[0].operation_id);
        let persisted_outcomes = state
            .mint
            .localstore()
            .get_federation_session_outcomes()
            .await
            .expect("persisted session outcomes");
        assert_eq!(persisted_outcomes.len(), 1);
        assert_eq!(persisted_outcomes[0].from_index, 1);
        assert_eq!(persisted_outcomes[0].signer, peer_member.member_id);
        let requests = transport.catch_up_requests();
        assert_eq!(requests.len(), 1);
        requests[0]
            .verify(&public_config, peer_member.member_id)
            .expect("signed catch-up request");
        assert_eq!(requests[0].payload.next_index, 1);
    }

    #[tokio::test]
    async fn rest_peer_client_rejects_unsigned_nonempty_catch_up_response() {
        let (state, public_config, local_private, _) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let first = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        state
            .submit_federation_envelope(first.clone())
            .await
            .expect("local first operation");
        peer_consensus
            .submit_envelope(first)
            .expect("peer first operation");
        peer_consensus
            .submit_envelope(second)
            .expect("peer second operation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("peer catch-up response");
        assert!(catch_up_response.session_outcome.is_none());
        let transport =
            MockFederationTransport::default().with_catch_up_response(catch_up_response);
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        let err = client
            .catch_up_state_from_member(&state, peer_member)
            .await
            .expect_err("unsigned response must fail");

        assert!(matches!(
            err.downcast_ref::<FederationError>(),
            Some(FederationError::SessionOutcomeMismatch { .. })
        ));
        assert_eq!(
            local_consensus.lock().await.peer_status().operation_count,
            1
        );
    }

    #[tokio::test]
    async fn lagging_member_refuses_public_federation_submit_until_healthy() {
        let (state, public_config, _) = federation_route_state().await;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        state
            .mark_federation_lagging(peer_member.member_id, 0, 1)
            .await;
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );

        let blocked = state.submit_federation_envelope(envelope.clone()).await;

        assert!(matches!(blocked, Err(cdk::Error::Custom(_))));
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Lagging { .. })
        ));

        state.mark_federation_healthy().await;
        state
            .submit_federation_envelope(envelope)
            .await
            .expect("healthy member can submit");
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Healthy)
        ));
    }

    #[tokio::test]
    async fn rest_peer_client_syncs_lagging_member_before_public_submit() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let peer_entry = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(peer_entry)
            .expect("peer accepted operation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("peer catch-up response");
        let transport = MockFederationTransport::default()
            .with_status_response(peer_consensus.peer_status())
            .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private));
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        let synced_status = client
            .sync_once_from_member(&state, peer_member)
            .await
            .expect("peer sync");

        assert_eq!(synced_status.operation_count, 1);
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Healthy)
        ));

        let next = restore_envelope(
            federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        state
            .submit_federation_envelope(next)
            .await
            .expect("synced member can submit");
    }

    #[tokio::test]
    async fn peer_sync_round_catches_up_from_configured_peer() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let peer_entry = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(peer_entry)
            .expect("peer accepted operation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("peer catch-up response");
        let transport = MockFederationTransport::default()
            .with_status_response(peer_consensus.peer_status())
            .with_catch_up_response(signed_catch_up_response(catch_up_response, &peer_private));
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        let synced =
            run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member))
                .await;

        assert_eq!(synced, 1);
        assert_eq!(
            local_consensus.lock().await.peer_status().operation_count,
            1
        );
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Healthy)
        ));
    }

    #[tokio::test]
    async fn peer_sync_round_does_not_clear_higher_known_lag_with_lower_peer() {
        let (state, public_config, local_private, _) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        state
            .mark_federation_lagging(peer_member.member_id, 0, 2)
            .await;
        let status = FederationPeerStatus::new_with_state_digest(
            federation_id,
            peer_member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
            cdk::federation::FederationStateDigest::genesis(),
        );
        let transport = MockFederationTransport::default().with_status_response(status);
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        let synced =
            run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member))
                .await;

        assert_eq!(synced, 1);
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Lagging {
                peer_operation_count: 2,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn rest_peer_client_halts_on_equal_height_digest_mismatch() {
        let (state, public_config, local_private, _) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let peer_entry = restore_envelope(
            federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let local_entry = restore_envelope(
            federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(peer_entry)
            .expect("peer accepted operation");
        state
            .submit_federation_envelope(local_entry)
            .await
            .expect("local accepted operation");
        let transport =
            MockFederationTransport::default().with_status_response(peer_consensus.peer_status());
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        let sync = client.sync_once_from_member(&state, peer_member).await;

        assert!(sync.is_err());
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Halted { .. })
        ));

        let blocked = state
            .submit_federation_envelope(restore_envelope(
                federation_id,
                "03b45d181a961a91a284e8c64eb72839d43300afaa4e6b3076f5183bb14f50c2cc",
            ))
            .await;
        assert!(matches!(blocked, Err(cdk::Error::Custom(_))));
    }

    #[tokio::test]
    async fn rest_peer_client_halts_on_equal_height_state_digest_mismatch() {
        let (state, public_config, local_private, _) =
            federation_route_state_with_local_private().await;
        let federation_id = public_config.federation_id;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let status = FederationPeerStatus::new_with_state_digest(
            federation_id,
            peer_member.member_id,
            0,
            None,
            cdk::federation::FederationJournalDigest::genesis(),
            cdk::federation::FederationStateDigest::from_bytes([9; 32]),
        );
        let transport = MockFederationTransport::default().with_status_response(status);
        let client = FederationRestPeerClient::new(transport, &public_config, local_private)
            .expect("client");

        let sync = client.sync_once_from_member(&state, peer_member).await;

        assert!(sync.is_err());
        assert!(matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Halted { .. })
        ));
    }

    #[tokio::test]
    async fn catch_up_applies_mint_quote_operation_to_local_store() {
        let (state, public_config, _) = federation_route_state().await;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let quote_id = QuoteId::new();
        let envelope = mint_quote_envelope(&state, &quote_id);
        let operation_id = envelope.operation_id().expect("operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted quote");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.request, format!("invoice-{quote_id}"));

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn catch_up_applies_melt_quote_operation_to_local_store() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let quote_id = QuoteId::new();
        let invoice = create_fake_invoice(1000, "".to_string());
        let request = cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice.clone(),
            unit: CurrencyUnit::Sat,
            options: None,
        });
        let envelope = config.melt_quote_operation(quote_id.clone(), request);
        let operation_id = envelope.operation_id().expect("operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted melt quote");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.id, quote_id);
        assert_eq!(
            quote.payment_method,
            PaymentMethod::Known(KnownMethod::Bolt11)
        );
        assert_eq!(quote.request.to_string(), invoice.to_string());
        assert_eq!(quote.state, MeltQuoteState::Unpaid);
        assert_eq!(quote.amount(), Amount::new(1, CurrencyUnit::Sat));
        assert_eq!(quote.fee_reserve(), Amount::new(1, CurrencyUnit::Sat));

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn catch_up_applies_swap_operation_to_local_store() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let input = proof_for_amount(&state, 1, "catch-up");
        let input_y = input.y().expect("input y");
        let output = swap_output(&state, "catch-up");
        let request = cdk::nuts::SwapRequest::new(vec![input], vec![output.clone()]);
        let envelope = config
            .swap_operation(request.clone())
            .expect("swap envelope");
        let operation_id = envelope.operation_id().expect("operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted swap");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup");
        let expected_response = config
            .swap_share_response(operation_id, &request)
            .expect("expected share response");
        assert_eq!(
            signatures,
            vec![Some(expected_response.signatures[0].clone())]
        );

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn catch_up_applies_melt_operation_to_local_store() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let quote_id = melt_quote(&state, 1000).await;
        let inputs = vec![
            proof_for_amount(&state, 1, "melt-catch-up-a"),
            proof_for_amount(&state, 1, "melt-catch-up-b"),
            proof_for_amount(&state, 1, "melt-catch-up-c"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let mut change_output = swap_output(&state, "melt-catch-up-change");
        change_output.amount = Amount::ZERO;
        let request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            inputs,
            Some(vec![change_output.clone()]),
        );
        let envelope = config.melt_operation(request).expect("melt envelope");
        let operation_id = envelope.operation_id().expect("operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(envelope)
            .expect("peer accepted melt");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Paid);

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("proof state lookup");
        assert_eq!(
            proof_states,
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("signature lookup");
        let signature = signatures[0].clone().expect("change share persisted");
        assert_eq!(signature.amount, Amount::from(1));
        assert_eq!(signature.keyset_id, change_output.keyset_id);

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn catch_up_applies_paid_melt_payment_observation_to_pending_melt() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let quote_id = melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let inputs = vec![
            proof_for_amount(&state, 1, "paid-observation-a"),
            proof_for_amount(&state, 1, "paid-observation-b"),
            proof_for_amount(&state, 1, "paid-observation-c"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let mut change_output = swap_output(&state, "paid-observation-change");
        change_output.amount = Amount::ZERO;
        let melt_request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            inputs,
            Some(vec![change_output.clone()]),
        );
        let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
        let melt_operation_id = melt_envelope.operation_id().expect("melt operation id");
        let paid_envelope = melt_payment_observation_envelope(
            &state,
            &quote_id,
            MeltQuoteState::Paid,
            Some("federation-paid-preimage".to_string()),
        );
        let paid_operation_id = paid_envelope.operation_id().expect("paid operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(melt_envelope)
            .expect("peer accepted pending melt");
        peer_consensus
            .submit_envelope(paid_envelope)
            .expect("peer accepted paid observation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Paid);
        assert_eq!(
            quote.payment_proof.as_deref(),
            Some("federation-paid-preimage")
        );

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("proof state lookup");
        assert_eq!(
            proof_states,
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("signature lookup");
        let signature = signatures[0].clone().expect("change share persisted");
        assert_eq!(signature.amount, Amount::from(1));
        assert_eq!(signature.keyset_id, change_output.keyset_id);

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 2);
        assert!(persisted
            .iter()
            .all(|entry| entry.status == FederationOperationStatus::Applied));
        let runtime = local_consensus.lock().await;
        assert_eq!(
            runtime
                .journal()
                .entry_by_operation_id(melt_operation_id)
                .expect("melt runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
        assert_eq!(
            runtime
                .journal()
                .entry_by_operation_id(paid_operation_id)
                .expect("paid runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn catch_up_applies_unpaid_melt_payment_observation_to_pending_melt() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let quote_id = melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let inputs = vec![
            proof_for_amount(&state, 1, "unpaid-observation-a"),
            proof_for_amount(&state, 1, "unpaid-observation-b"),
        ];
        let input_ys = inputs
            .iter()
            .map(|proof| proof.y().expect("input y"))
            .collect::<Vec<_>>();
        let melt_request = cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, None);
        let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
        let unpaid_envelope =
            melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Unpaid, None);
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(melt_envelope)
            .expect("peer accepted pending melt");
        peer_consensus
            .submit_envelope(unpaid_envelope)
            .expect("peer accepted unpaid observation");
        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply catch-up");

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);
        assert!(quote.payment_proof.is_none());
        assert!(
            state
                .mint
                .localstore()
                .get_melt_saga_by_quote_id(&quote_id)
                .await
                .expect("saga lookup")
                .is_none(),
            "rollback should remove pending melt saga metadata"
        );

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&input_ys)
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![None, None]);

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 2);
        assert!(persisted
            .iter()
            .all(|entry| entry.status == FederationOperationStatus::Applied));
    }

    #[tokio::test]
    async fn offline_member_catches_up_multiple_operations_and_converges() {
        let (state, public_config, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);

        let mint_quote_id = QuoteId::new();
        let minted_output = mint_output(&state, &mint_quote_id);
        let mint_request = MintInput::Single(MintRequest {
            quote: mint_quote_id.clone(),
            outputs: vec![minted_output.clone()],
            signature: None,
        });

        let swap_input = proof_for_amount(&state, 1, "offline-catch-up-swap-input");
        let swap_input_y = swap_input.y().expect("swap input y");
        let swapped_output = swap_output(&state, "offline-catch-up-swap-output");
        let swap_request =
            cdk::nuts::SwapRequest::new(vec![swap_input], vec![swapped_output.clone()]);

        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let melt_quote_id = QuoteId::new();
        let melt_invoice = create_fake_invoice(
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        );
        let melt_quote_envelope = config.melt_quote_operation(
            melt_quote_id.clone(),
            cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: melt_invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }),
        );
        let melt_inputs = vec![
            proof_for_amount(&state, 1, "offline-catch-up-melt-a"),
            proof_for_amount(&state, 1, "offline-catch-up-melt-b"),
            proof_for_amount(&state, 1, "offline-catch-up-melt-c"),
        ];
        let melt_input_ys = melt_inputs
            .iter()
            .map(|proof| proof.y().expect("melt input y"))
            .collect::<Vec<_>>();
        let mut melt_change_output = swap_output(&state, "offline-catch-up-melt-change");
        melt_change_output.amount = Amount::ZERO;
        let melt_request = cdk::nuts::MeltRequest::new(
            melt_quote_id.clone(),
            melt_inputs,
            Some(vec![melt_change_output.clone()]),
        );
        let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
        let melt_payment_envelope = melt_payment_observation_envelope(
            &state,
            &melt_quote_id,
            MeltQuoteState::Paid,
            Some("offline-catch-up-preimage".to_string()),
        );

        for envelope in [
            mint_quote_envelope(&state, &mint_quote_id),
            paid_observation_envelope(&state, &mint_quote_id),
            config
                .mint_operation(mint_request.clone())
                .expect("mint envelope"),
            config
                .swap_operation(swap_request.clone())
                .expect("swap envelope"),
            melt_quote_envelope,
            melt_envelope,
            melt_payment_envelope,
        ] {
            peer_consensus
                .submit_envelope(envelope)
                .expect("peer accepted operation");
        }

        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        assert_eq!(
            local_consensus.lock().await.peer_status().operation_count,
            0
        );
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("peer catch-up response");

        let applied = state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply offline catch-up");

        assert_eq!(applied.len(), 7);
        assert_eq!(
            local_consensus
                .lock()
                .await
                .peer_status()
                .latest_order_digest,
            peer_consensus.peer_status().latest_order_digest
        );

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 7);
        assert!(persisted
            .iter()
            .all(|entry| entry.status == FederationOperationStatus::Applied));

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&mint_quote_id)
            .await
            .expect("mint quote lookup")
            .expect("mint quote persisted");
        assert_eq!(quote.state(), MintQuoteState::Issued);

        let mint_signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[minted_output.blinded_secret])
            .await
            .expect("mint signature lookup");
        assert!(mint_signatures[0].is_some());

        let swap_states = state
            .mint
            .localstore()
            .get_proofs_states(&[swap_input_y])
            .await
            .expect("swap state lookup");
        assert_eq!(swap_states, vec![Some(cdk::nuts::State::Spent)]);
        let swap_signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[swapped_output.blinded_secret])
            .await
            .expect("swap signature lookup");
        assert!(swap_signatures[0].is_some());

        let melt_quote = state
            .mint
            .localstore()
            .get_melt_quote(&melt_quote_id)
            .await
            .expect("melt quote lookup")
            .expect("melt quote persisted");
        assert_eq!(melt_quote.state, MeltQuoteState::Paid);
        assert_eq!(
            melt_quote.payment_proof.as_deref(),
            Some("offline-catch-up-preimage")
        );
        assert!(
            state
                .mint
                .localstore()
                .get_melt_saga_by_quote_id(&melt_quote_id)
                .await
                .expect("melt saga lookup")
                .is_none(),
            "paid observation replay should complete pending melt saga metadata"
        );

        let melt_states = state
            .mint
            .localstore()
            .get_proofs_states(&melt_input_ys)
            .await
            .expect("melt state lookup");
        assert_eq!(
            melt_states,
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );
        let melt_change_signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[melt_change_output.blinded_secret])
            .await
            .expect("melt change signature lookup");
        assert!(melt_change_signatures[0].is_some());

        let conflicting_output = swap_output(&state, "offline-catch-up-conflicting-mint");
        let conflicting_mint = config
            .mint_operation(MintInput::Single(MintRequest {
                quote: mint_quote_id,
                outputs: vec![conflicting_output],
                signature: None,
            }))
            .expect("conflicting mint envelope");
        let conflict = state.submit_federation_envelope(conflicting_mint).await;
        assert!(matches!(
            conflict,
            Err(cdk::Error::Federation(
                FederationError::ConflictingOperation { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn diverged_member_rejects_peer_catch_up_digest_mismatch() {
        let (state, public_config, _) = federation_route_state().await;
        let peer_member = public_config.member(member_id(2)).expect("peer member");
        let peer_entry = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let local_entry = restore_envelope(
            public_config.federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut peer_consensus =
            InMemoryFederationConsensus::new(public_config.federation_id, peer_member.member_id);
        peer_consensus
            .submit_envelope(peer_entry)
            .expect("peer accepted operation");
        state
            .submit_federation_envelope(local_entry)
            .await
            .expect("local accepted divergent operation");

        let local_consensus = state.federation_consensus.as_ref().expect("consensus");
        let request = local_consensus.lock().await.catch_up_request();
        assert_eq!(request.next_index, 1);
        assert!(matches!(
            peer_consensus.catch_up_checked(&request),
            Err(FederationError::CatchUpDigestMismatch { .. })
        ));

        let unchecked_response = peer_consensus.catch_up(&request);
        let apply = state
            .apply_federation_catch_up_response(&unchecked_response)
            .await;
        assert!(matches!(
            apply,
            Err(cdk::Error::Federation(
                FederationError::CatchUpDigestMismatch { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn federation_runtime_state_replays_persisted_journal_and_sequences() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let peer_member = peer_private.member_id;
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let response = state
            .submit_federation_envelope(envelope)
            .await
            .expect("persist operation");
        state
            .persist_federation_sequence(peer_member, 42)
            .await
            .expect("persist sequence");

        let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("runtime state");
        let consensus = consensus.expect("consensus");
        let sequences = sequences.expect("sequences");
        let consensus = consensus.lock().await;
        let sequences = sequences.lock().await;

        assert_eq!(consensus.peer_status().operation_count, 1);
        assert_eq!(
            consensus
                .journal()
                .entries()
                .first()
                .expect("journal entry")
                .operation_id,
            response.operation_id
        );
        assert_eq!(sequences.sequences().get(&peer_member), Some(&42));
    }

    #[tokio::test]
    async fn submit_mint_quote_operation_applies_quote_to_local_store() {
        let (state, _, _) = federation_route_state().await;
        let quote_id = QuoteId::new();
        let envelope = mint_quote_envelope(&state, &quote_id);
        let operation_id = envelope.operation_id().expect("operation id");

        state
            .submit_federation_envelope(envelope)
            .await
            .expect("submit mint quote");

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.request, format!("invoice-{quote_id}"));
        assert_eq!(quote.amount, Some(Amount::new(1, CurrencyUnit::Sat)));
        assert_eq!(quote.state(), MintQuoteState::Unpaid);

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted[0].status, FederationOperationStatus::Applied);
        assert_eq!(
            state
                .federation_consensus
                .as_ref()
                .expect("consensus")
                .lock()
                .await
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn submit_mint_operation_applies_issuance_and_persists_share() {
        let (state, _, _) = federation_route_state().await;
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        let quote_id = QuoteId::new();
        state
            .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
            .await
            .expect("submit mint quote");
        state
            .submit_federation_envelope(paid_observation_envelope(&state, &quote_id))
            .await
            .expect("submit paid observation");

        let output = mint_output(&state, &quote_id);
        let input = MintInput::Single(MintRequest {
            quote: quote_id.clone(),
            outputs: vec![output.clone()],
            signature: None,
        });
        let envelope = config.mint_operation(input.clone()).expect("mint envelope");
        let operation_id = envelope.operation_id().expect("operation id");
        state
            .submit_federation_envelope(envelope)
            .await
            .expect("submit mint");

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.amount_paid(), Amount::new(1, CurrencyUnit::Sat));
        assert_eq!(quote.amount_issued(), Amount::new(1, CurrencyUnit::Sat));
        assert_eq!(quote.state(), MintQuoteState::Issued);

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup");
        let persisted_signature = signatures[0].clone().expect("signature persisted");
        let expected_response = config
            .mint_share_response(operation_id, &input)
            .expect("expected share response");
        assert_eq!(persisted_signature, expected_response.signatures[0]);

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(
            persisted
                .iter()
                .find(|entry| entry.operation_id == operation_id)
                .expect("mint entry")
                .status,
            FederationOperationStatus::Applied
        );
    }

    #[tokio::test]
    async fn private_local_status_route_returns_consensus_and_sync_state() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        state
            .submit_federation_envelope(restore_envelope(
                public_config.federation_id,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .await
            .expect("submit restore");

        let request = signed_status_request(&public_config, &peer_private, local_member, 1);
        let response = post_local_status(State(state), Json(request))
            .await
            .expect("local status response")
            .0;

        assert_eq!(response.version, FEDERATION_REST_API_VERSION);
        assert_eq!(response.peer_status.member_id, local_member);
        assert_eq!(response.peer_status.operation_count, 1);
        assert_eq!(response.sync_state, "healthy");
        assert!(response.sync_ready);
        assert_eq!(response.sync_message, "federation member is healthy");
        assert_eq!(response.session_outcome_quorum_operation_count, None);
        assert!(response.session_outcome_quorum_signers.is_empty());
    }

    #[tokio::test]
    async fn private_local_status_route_reports_session_outcome_quorum_evidence() {
        let (state, public_config, local_private, peer_private) =
            federation_route_state_with_local_private().await;
        let local_member = local_private.member_id;
        state
            .submit_federation_envelope(restore_envelope(
                public_config.federation_id,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .await
            .expect("submit restore");
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal entries");
        let peer_status = local_peer_status(&state).await.expect("local status");
        let local_outcome = FederationSessionOutcome::new_signed(
            public_config.federation_id,
            &entries,
            peer_status.latest_order_digest,
            peer_status.state_digest,
            &local_private,
        )
        .expect("local session outcome");
        let peer_outcome = FederationSessionOutcome::new_signed(
            public_config.federation_id,
            &entries,
            peer_status.latest_order_digest,
            peer_status.state_digest,
            &peer_private,
        )
        .expect("peer session outcome");
        let mut tx = state
            .mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin transaction");
        tx.add_federation_session_outcome(&local_outcome)
            .await
            .expect("persist local outcome");
        tx.add_federation_session_outcome(&peer_outcome)
            .await
            .expect("persist peer outcome");
        tx.commit().await.expect("commit outcomes");

        let request = signed_status_request(&public_config, &peer_private, local_member, 1);
        let response = post_local_status(State(state), Json(request))
            .await
            .expect("local status response")
            .0;

        assert_eq!(response.peer_status.operation_count, 1);
        assert_eq!(response.session_outcome_quorum_operation_count, Some(1));
        assert_eq!(response.session_outcome_quorum_signers.len(), 2);
        assert!(response
            .session_outcome_quorum_signers
            .contains(&local_private.member_id));
        assert!(response
            .session_outcome_quorum_signers
            .contains(&peer_private.member_id));
    }

    #[tokio::test]
    async fn private_local_status_route_reports_lagging_sync_state() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        let peer_member = peer_private.member_id;
        state.mark_federation_lagging(peer_member, 0, 3).await;

        let request = signed_status_request(&public_config, &peer_private, local_member, 1);
        let response = post_local_status(State(state), Json(request))
            .await
            .expect("local status response")
            .0;

        assert_eq!(response.peer_status.operation_count, 0);
        assert_eq!(response.sync_state, "lagging");
        assert!(!response.sync_ready);
        assert!(response
            .sync_message
            .contains("federation member is lagging behind peer"));
    }

    #[tokio::test]
    async fn private_journal_route_returns_signed_session_outcome() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let local_member = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        state
            .submit_federation_envelope(restore_envelope(
                public_config.federation_id,
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ))
            .await
            .expect("submit restore");
        let request = signed_catch_up_request(
            &public_config,
            &peer_private,
            local_member,
            1,
            FederationJournalCatchUpRequest::new(0),
        );

        let response = post_journal(State(state), Json(request))
            .await
            .expect("journal response")
            .0;

        assert_eq!(response.entries.len(), 1);
        assert!(response.session_outcome.is_some());
        response
            .verify_session_outcome(public_config.member(local_member).expect("local member"))
            .expect("session outcome verifies");
    }

    #[tokio::test]
    async fn private_operation_route_accepts_signed_peer_request() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            envelope.clone(),
        );

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect("operation response");

        assert_eq!(
            response.0.operation_id,
            envelope.operation_id().expect("operation id")
        );
        assert_eq!(consensus_operation_count(&state).await, 1);
        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("persisted journal");
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].operation_id, response.0.operation_id);
        assert_eq!(sequences.get(&peer_private.member_id), Some(&1));
    }

    #[tokio::test]
    async fn private_operation_route_rejects_oversized_request_before_mutation() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            oversized_payment_observation_envelope(public_config.federation_id),
        );

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect_err("oversized request must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 0);
        let sequences = state
            .mint
            .localstore()
            .get_federation_member_sequences()
            .await
            .expect("persisted sequences");
        assert!(sequences.is_empty());
    }

    #[tokio::test]
    async fn private_operation_route_rejects_tampered_payload_before_mutation() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let first = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            public_config.federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let mut request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            first,
        );
        request.payload = FederationSubmitOperationRequest::new(second);

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect_err("tampered request must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 0);
    }

    #[tokio::test]
    async fn private_operation_route_rejects_unknown_sender_before_mutation() {
        let (state, public_config, _) = federation_route_state().await;
        let unknown_private = private_member(3);
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let request = signed_operation_request(
            &public_config,
            &unknown_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            envelope,
        );

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect_err("unknown sender must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 0);
    }

    #[tokio::test]
    async fn private_operation_route_rejects_kind_mismatch_before_mutation() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let envelope = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::Status,
            envelope,
        );

        let response = post_operation(State(state.clone()), Json(request))
            .await
            .expect_err("wrong message kind must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 0);
    }

    #[tokio::test]
    async fn private_operation_route_rejects_stale_sequence_before_second_mutation() {
        let (state, public_config, peer_private) = federation_route_state().await;
        let first = restore_envelope(
            public_config.federation_id,
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        );
        let second = restore_envelope(
            public_config.federation_id,
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        );
        let first_request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            first,
        );
        let second_request = signed_operation_request(
            &public_config,
            &peer_private,
            1,
            FederationRestMessageKind::SubmitOperation,
            second,
        );

        let _ = post_operation(State(state.clone()), Json(first_request))
            .await
            .expect("first operation accepted");
        let response = post_operation(State(state.clone()), Json(second_request))
            .await
            .expect_err("stale sequence must fail");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(consensus_operation_count(&state).await, 1);
    }
}

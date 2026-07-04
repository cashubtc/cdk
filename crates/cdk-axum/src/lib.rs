//! Axum server for Mint

#![doc = include_str!("../README.md")]

#[cfg(feature = "federation")]
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use auth::create_auth_router;
use axum::middleware::from_fn;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use cache::HttpCache;
#[cfg(feature = "federation")]
use cdk::federation::{
    FederationConfig, FederationConsensusEngine, FederationConsensusRuntime,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgSetupState, FederationJournalAppendOutcome, FederationJournalEntry,
    FederationOperationEnvelope, FederationOperationId, FederationOperationStatus,
    FederationRestSequenceTracker, FederationSessionOutcome, FederationSubmitOperationResponse,
    MemberId,
};
use cdk::mint::Mint;
use router_handlers::*;

mod metrics;

mod auth;
pub mod cache;
mod custom_handlers;
mod custom_router;
#[cfg(feature = "federation")]
mod federation;
mod router_handlers;
mod ws;

#[cfg(feature = "federation")]
type FederationConsensusHandle = Arc<tokio::sync::Mutex<FederationConsensusRuntime>>;
#[cfg(feature = "federation")]
type FederationSequenceHandle = Arc<tokio::sync::Mutex<FederationRestSequenceTracker>>;
#[cfg(feature = "federation")]
type FederationSyncHandle = Arc<tokio::sync::Mutex<FederationSyncState>>;
#[cfg(feature = "federation")]
type FederationNotificationHandle = Arc<FederationOperationNotifications>;
#[cfg(feature = "federation")]
type FederationFinalizationHandle = tokio::sync::mpsc::Sender<FederationFinalizationRequest>;

#[cfg(feature = "federation")]
struct FederationFinalizationRequest {
    response: FederationSubmitOperationResponse,
    entry: FederationJournalEntry,
    sequence: Option<(MemberId, u64)>,
    result: tokio::sync::oneshot::Sender<Result<FederationSubmitOperationResponse, cdk::Error>>,
}

#[cfg(feature = "federation")]
#[derive(Debug, Default)]
struct FederationOperationNotifications {
    inner: tokio::sync::Mutex<FederationOperationNotificationState>,
}

#[cfg(feature = "federation")]
#[derive(Debug, Default)]
struct FederationOperationNotificationState {
    outcomes: HashMap<FederationOperationId, FederationSubmitOperationResponse>,
    notifiers: HashMap<FederationOperationId, Arc<tokio::sync::Notify>>,
}

#[cfg(feature = "federation")]
impl FederationOperationNotifications {
    async fn notify(&self, response: FederationSubmitOperationResponse) {
        let operation_id = response.operation_id;
        let mut inner = self.inner.lock().await;
        inner.outcomes.insert(operation_id, response);
        if let Some(notifier) = inner.notifiers.remove(&operation_id) {
            notifier.notify_waiters();
        }
    }

    async fn wait_for(
        &self,
        operation_id: FederationOperationId,
    ) -> FederationSubmitOperationResponse {
        loop {
            let notified = {
                let mut inner = self.inner.lock().await;
                if let Some(response) = inner.outcomes.get(&operation_id) {
                    return response.clone();
                }
                inner
                    .notifiers
                    .entry(operation_id)
                    .or_default()
                    .clone()
                    .notified_owned()
            };
            notified.await;
        }
    }
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FederationSyncStatus {
    Healthy,
    Lagging {
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    },
    CatchingUp {
        peer: MemberId,
    },
    VerifyingSessionOutcome {
        peer: MemberId,
        operation_count: u64,
    },
    MaterializedStateMismatch {
        peer: MemberId,
        operation_count: u64,
    },
    Halted {
        reason: String,
    },
}

#[cfg(feature = "federation")]
impl Default for FederationSyncStatus {
    fn default() -> Self {
        Self::Healthy
    }
}

#[cfg(feature = "federation")]
impl FederationSyncStatus {
    pub(crate) fn is_ready(&self) -> bool {
        matches!(self, Self::Healthy)
    }

    pub(crate) fn state_name(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Lagging { .. } => "lagging",
            Self::CatchingUp { .. } => "catching_up",
            Self::VerifyingSessionOutcome { .. } => "verifying_session_outcome",
            Self::MaterializedStateMismatch { .. } => "materialized_state_mismatch",
            Self::Halted { .. } => "halted",
        }
    }

    pub(crate) fn not_ready_message(&self) -> String {
        match self {
            Self::Healthy => "federation member is healthy".to_string(),
            Self::Lagging {
                peer,
                local_operation_count,
                peer_operation_count,
            } => format!(
                "federation member is lagging behind peer {peer}: local_operations={local_operation_count}, peer_operations={peer_operation_count}"
            ),
            Self::CatchingUp { peer } => {
                format!("federation member is catching up from peer {peer}")
            }
            Self::VerifyingSessionOutcome {
                peer,
                operation_count,
            } => format!(
                "federation member is verifying quorum session outcome evidence after catch-up from peer {peer}: operation_count={operation_count}"
            ),
            Self::MaterializedStateMismatch {
                peer,
                operation_count,
            } => format!(
                "federation member is waiting for materialized state digest convergence with peer {peer}: operation_count={operation_count}"
            ),
            Self::Halted { reason } => format!("federation member is halted: {reason}"),
        }
    }
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Default)]
pub(crate) struct FederationSyncState {
    status: FederationSyncStatus,
}

#[cfg(feature = "federation")]
impl FederationSyncState {
    fn status(&self) -> FederationSyncStatus {
        self.status.clone()
    }

    fn mark_lagging(
        &mut self,
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    ) {
        self.status = FederationSyncStatus::Lagging {
            peer,
            local_operation_count,
            peer_operation_count,
        };
    }

    fn mark_catching_up(&mut self, peer: MemberId) {
        self.status = FederationSyncStatus::CatchingUp { peer };
    }

    fn mark_verifying_session_outcome(&mut self, peer: MemberId, operation_count: u64) {
        match self.status {
            FederationSyncStatus::Halted { .. } => {}
            FederationSyncStatus::Lagging {
                peer_operation_count,
                ..
            } if peer_operation_count > operation_count => {}
            _ => {
                self.status = FederationSyncStatus::VerifyingSessionOutcome {
                    peer,
                    operation_count,
                };
            }
        }
    }

    fn mark_materialized_state_mismatch(&mut self, peer: MemberId, operation_count: u64) {
        match self.status {
            FederationSyncStatus::Halted { .. } => {}
            FederationSyncStatus::Lagging {
                peer_operation_count,
                ..
            } if peer_operation_count > operation_count => {}
            FederationSyncStatus::VerifyingSessionOutcome {
                operation_count: pending_operation_count,
                ..
            } if pending_operation_count > operation_count => {}
            _ => {
                self.status = FederationSyncStatus::MaterializedStateMismatch {
                    peer,
                    operation_count,
                };
            }
        }
    }

    fn mark_healthy_if_session_outcome_verified(&mut self, verified_operation_count: u64) {
        match self.status {
            FederationSyncStatus::Halted { .. } => {}
            FederationSyncStatus::Lagging {
                peer_operation_count,
                ..
            } if peer_operation_count > verified_operation_count => {}
            FederationSyncStatus::VerifyingSessionOutcome {
                operation_count, ..
            } if operation_count > verified_operation_count => {}
            _ => self.mark_healthy(),
        }
    }

    fn mark_healthy(&mut self) {
        self.status = FederationSyncStatus::Healthy;
    }

    fn mark_halted(&mut self, reason: String) {
        self.status = FederationSyncStatus::Halted { reason };
    }
}

/// CDK Mint State
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct MintState {
    mint: Arc<Mint>,
    cache: Arc<cache::HttpCache>,
    #[cfg(feature = "federation")]
    federation_consensus: Option<FederationConsensusHandle>,
    #[cfg(feature = "federation")]
    federation_sequences: Option<FederationSequenceHandle>,
    #[cfg(feature = "federation")]
    federation_sync: Option<FederationSyncHandle>,
    #[cfg(feature = "federation")]
    federation_notifications: Option<FederationNotificationHandle>,
    #[cfg(feature = "federation")]
    federation_finalizer: Option<FederationFinalizationHandle>,
}

#[cfg(feature = "federation")]
impl MintState {
    fn start_federation_finalization_loop(&mut self) {
        if self.federation_consensus.is_none() || self.federation_finalizer.is_some() {
            return;
        }

        let (sender, receiver) = tokio::sync::mpsc::channel(128);
        self.federation_finalizer = Some(sender);
        let mut finalizer_state = self.clone();
        finalizer_state.federation_finalizer = None;
        tokio::spawn(async move {
            finalizer_state
                .run_federation_finalization_loop(receiver)
                .await;
        });
    }

    async fn run_federation_finalization_loop(
        &self,
        mut receiver: tokio::sync::mpsc::Receiver<FederationFinalizationRequest>,
    ) {
        while let Some(request) = receiver.recv().await {
            let result = self
                .finalize_federation_operation_acceptance_inline(
                    request.response,
                    request.entry,
                    request.sequence,
                )
                .await;
            let _ = request.result.send(result);
        }
    }

    pub(crate) async fn submit_federation_envelope(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        if let Some(response) =
            crate::federation::submit_operation_to_primary_if_needed(self, envelope.clone(), None)
                .await?
        {
            return Ok(response);
        }

        self.ensure_federation_ready_for_public_submit().await?;
        self.submit_federation_envelope_with_sequence(envelope, None)
            .await
    }

    pub(crate) async fn submit_federation_envelope_with_sequence(
        &self,
        envelope: FederationOperationEnvelope,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        let consensus = self.federation_consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let mut consensus = consensus.lock().await;
        let mut next_consensus = consensus.clone();
        let response = next_consensus.submit_envelope(envelope)?;
        let entry = response.outcome.entry().clone();

        self.persist_federation_append_outcome(&response.outcome, sequence)
            .await?;
        *consensus = next_consensus;
        drop(consensus);

        let operation_id = response.operation_id;
        self.finalize_federation_operation_acceptance(response.clone(), entry, sequence)
            .await?;

        Ok(self
            .wait_for_federation_operation_acceptance(operation_id)
            .await
            .unwrap_or(response))
    }

    async fn finalize_federation_operation_acceptance(
        &self,
        response: FederationSubmitOperationResponse,
        entry: FederationJournalEntry,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        let Some(finalizer) = &self.federation_finalizer else {
            return self
                .finalize_federation_operation_acceptance_inline(response, entry, sequence)
                .await;
        };
        let (result, receiver) = tokio::sync::oneshot::channel();
        finalizer
            .send(FederationFinalizationRequest {
                response,
                entry,
                sequence,
                result,
            })
            .await
            .map_err(|_| {
                cdk::Error::Custom(
                    "federation finalization loop stopped before accepting operation".to_string(),
                )
            })?;
        receiver.await.map_err(|_| {
            cdk::Error::Custom(
                "federation finalization loop stopped before completing operation".to_string(),
            )
        })?
    }

    async fn finalize_federation_operation_acceptance_inline(
        &self,
        response: FederationSubmitOperationResponse,
        entry: FederationJournalEntry,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        if let Some((sender, sequence)) = sequence {
            self.record_federation_sequence(sender, sequence).await;
        }
        self.apply_federation_journal_entries(std::slice::from_ref(&entry))
            .await?;
        self.notify_federation_operation(response.clone()).await;

        Ok(response)
    }

    pub(crate) async fn wait_for_federation_operation_acceptance(
        &self,
        operation_id: FederationOperationId,
    ) -> Option<FederationSubmitOperationResponse> {
        let notifications = self.federation_notifications.as_ref()?;
        Some(notifications.wait_for(operation_id).await)
    }

    async fn notify_federation_operation(&self, response: FederationSubmitOperationResponse) {
        if let Some(notifications) = &self.federation_notifications {
            notifications.notify(response).await;
        }
    }

    pub(crate) async fn apply_federation_catch_up_response(
        &self,
        response: &cdk::federation::FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        let consensus = self.federation_consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let mut consensus = consensus.lock().await;
        let mut next_consensus = consensus.clone();
        let applied = next_consensus.apply_catch_up_response(response)?;

        self.persist_federation_journal_entries(&applied, response.session_outcome.as_ref(), None)
            .await?;
        *consensus = next_consensus;
        drop(consensus);
        self.apply_federation_journal_entries(&applied).await?;

        Ok(applied)
    }

    pub(crate) async fn persist_federation_sequence(
        &self,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.persist_federation_journal_entries(&[], None, Some((sender, sequence)))
            .await?;
        self.record_federation_sequence(sender, sequence).await;
        Ok(())
    }

    pub(crate) async fn persist_federation_dkg_commitment(
        &self,
        commitment: &FederationDkgParticipantCommitment,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.federation_public_config()?;
        let mut setup_state = self.load_federation_dkg_setup_state(&public_config).await?;
        setup_state.insert_commitment(&public_config, *commitment)?;

        let db = self.mint.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_commitment(commitment).await?;
        tx.set_federation_member_sequence(sender, sequence).await?;
        tx.commit().await?;
        self.record_federation_sequence(sender, sequence).await;
        Ok(())
    }

    pub(crate) async fn persist_federation_dkg_reveal(
        &self,
        reveal: &FederationDkgParticipantCommitmentReveal,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.federation_public_config()?;
        let mut setup_state = self.load_federation_dkg_setup_state(&public_config).await?;
        setup_state.insert_reveal(&public_config, reveal.clone())?;

        let db = self.mint.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_reveal(reveal).await?;
        tx.set_federation_member_sequence(sender, sequence).await?;
        tx.commit().await?;
        self.record_federation_sequence(sender, sequence).await;
        Ok(())
    }

    fn federation_public_config(&self) -> Result<FederationConfig, cdk::Error> {
        self.mint
            .federation_config()
            .map(|config| config.public_config().clone())
            .ok_or_else(|| {
                cdk::Error::Custom("federation mint is missing its runtime config".to_string())
            })
    }

    async fn load_federation_dkg_setup_state(
        &self,
        public_config: &FederationConfig,
    ) -> Result<FederationDkgSetupState, cdk::Error> {
        let db = self.mint.localstore();
        let commitments = db.get_federation_dkg_commitments().await?;
        let reveals = db.get_federation_dkg_reveals().await?;
        let mut setup_state = FederationDkgSetupState::new(public_config);
        for commitment in commitments {
            setup_state.insert_commitment(public_config, commitment)?;
        }
        for reveal in reveals {
            setup_state.insert_reveal(public_config, reveal)?;
        }
        Ok(setup_state)
    }

    pub(crate) async fn persist_federation_session_outcome(
        &self,
        outcome: &FederationSessionOutcome,
    ) -> Result<(), cdk::Error> {
        self.persist_federation_journal_entries(&[], Some(outcome), None)
            .await
    }

    async fn persist_federation_append_outcome(
        &self,
        outcome: &FederationJournalAppendOutcome,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<(), cdk::Error> {
        match outcome {
            FederationJournalAppendOutcome::Appended(entry) => {
                self.persist_federation_journal_entries(std::slice::from_ref(entry), None, sequence)
                    .await
            }
            FederationJournalAppendOutcome::Duplicate(_) => {
                self.persist_federation_journal_entries(&[], None, sequence)
                    .await
            }
        }
    }

    async fn persist_federation_journal_entries(
        &self,
        entries: &[FederationJournalEntry],
        session_outcome: Option<&FederationSessionOutcome>,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<(), cdk::Error> {
        let db = self.mint.localstore();
        let mut tx = db.begin_transaction().await?;
        for entry in entries {
            tx.add_federation_journal_entry(entry).await?;
        }
        if let Some(session_outcome) = session_outcome {
            tx.add_federation_session_outcome(session_outcome).await?;
        }
        if let Some((sender, sequence)) = sequence {
            tx.set_federation_member_sequence(sender, sequence).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn record_federation_sequence(&self, sender: MemberId, sequence: u64) {
        if let Some(sequences) = &self.federation_sequences {
            sequences.lock().await.record_sequence(sender, sequence);
        }
    }

    async fn apply_federation_journal_entries(
        &self,
        entries: &[FederationJournalEntry],
    ) -> Result<Vec<FederationOperationId>, cdk::Error> {
        let applied = self.mint.apply_federation_journal_entries(entries).await?;
        self.mark_federation_operations_applied(&applied).await?;
        Ok(applied)
    }

    async fn mark_federation_operations_applied(
        &self,
        operation_ids: &[FederationOperationId],
    ) -> Result<(), cdk::Error> {
        let Some(consensus) = &self.federation_consensus else {
            return Ok(());
        };
        let mut consensus = consensus.lock().await;
        for operation_id in operation_ids {
            consensus.update_status(*operation_id, FederationOperationStatus::Applied)?;
        }
        Ok(())
    }

    pub(crate) async fn federation_sync_status(&self) -> Option<FederationSyncStatus> {
        let sync = self.federation_sync.as_ref()?;
        Some(sync.lock().await.status())
    }

    pub(crate) async fn mark_federation_lagging(
        &self,
        peer: MemberId,
        local_operation_count: u64,
        peer_operation_count: u64,
    ) {
        if let Some(sync) = &self.federation_sync {
            sync.lock()
                .await
                .mark_lagging(peer, local_operation_count, peer_operation_count);
        }
    }

    pub(crate) async fn mark_federation_catching_up(&self, peer: MemberId) {
        if let Some(sync) = &self.federation_sync {
            sync.lock().await.mark_catching_up(peer);
        }
    }

    pub(crate) async fn mark_federation_verifying_session_outcome(
        &self,
        peer: MemberId,
        operation_count: u64,
    ) {
        if let Some(sync) = &self.federation_sync {
            sync.lock()
                .await
                .mark_verifying_session_outcome(peer, operation_count);
        }
    }

    pub(crate) async fn mark_federation_materialized_state_mismatch(
        &self,
        peer: MemberId,
        operation_count: u64,
    ) {
        if let Some(sync) = &self.federation_sync {
            sync.lock()
                .await
                .mark_materialized_state_mismatch(peer, operation_count);
        }
    }

    pub(crate) async fn mark_federation_healthy_if_session_outcome_verified(
        &self,
        verified_operation_count: u64,
    ) {
        if let Some(sync) = &self.federation_sync {
            sync.lock()
                .await
                .mark_healthy_if_session_outcome_verified(verified_operation_count);
        }
    }

    #[cfg(test)]
    pub(crate) async fn mark_federation_healthy(&self) {
        if let Some(sync) = &self.federation_sync {
            sync.lock().await.mark_healthy();
        }
    }

    pub(crate) async fn mark_federation_healthy_if_caught_up(&self, local_operation_count: u64) {
        if let Some(sync) = &self.federation_sync {
            let mut sync = sync.lock().await;
            match sync.status() {
                FederationSyncStatus::Lagging {
                    peer_operation_count,
                    ..
                } if peer_operation_count > local_operation_count => {}
                FederationSyncStatus::VerifyingSessionOutcome { .. } => {}
                FederationSyncStatus::Halted { .. } => {}
                _ => sync.mark_healthy(),
            }
        }
    }

    pub(crate) async fn mark_federation_halted<S>(&self, reason: S)
    where
        S: Into<String>,
    {
        if let Some(sync) = &self.federation_sync {
            sync.lock().await.mark_halted(reason.into());
        }
    }

    async fn ensure_federation_ready_for_public_submit(&self) -> Result<(), cdk::Error> {
        let Some(sync) = &self.federation_sync else {
            return Ok(());
        };
        let status = sync.lock().await.status();
        if status.is_ready() {
            return Ok(());
        }

        Err(cdk::Error::Custom(status.not_ready_message()))
    }
}

/// Create mint [`Router`] with required endpoints for cashu mint with the default cache
///
/// The `custom_methods` parameter should include all custom payment methods supported
/// by the payment processor, including "bolt11" and "bolt12" if they are supported.
pub async fn create_mint_router(mint: Arc<Mint>, custom_methods: Vec<String>) -> Result<Router> {
    create_mint_router_with_custom_cache(mint, Default::default(), custom_methods, false).await
}

async fn cors_middleware(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    let allowed_headers = "*";

    // Handle preflight requests
    if req.method() == axum::http::Method::OPTIONS {
        let mut response = Response::new("".into());
        response.headers_mut().insert(
            "Access-Control-Allow-Origin",
            "*".parse().expect("Valid header value"),
        );
        response.headers_mut().insert(
            "Access-Control-Allow-Methods",
            "GET, POST, OPTIONS".parse().expect("Valid header value"),
        );
        response.headers_mut().insert(
            "Access-Control-Allow-Headers",
            allowed_headers.parse().expect("Valid header value"),
        );
        return response;
    }

    // Call the next handler
    let mut response = next.run(req).await;

    response.headers_mut().insert(
        "Access-Control-Allow-Origin",
        "*".parse().expect("Valid header value"),
    );
    response.headers_mut().insert(
        "Access-Control-Allow-Methods",
        "GET, POST, OPTIONS".parse().expect("Valid header value"),
    );
    response.headers_mut().insert(
        "Access-Control-Allow-Headers",
        allowed_headers.parse().expect("Valid header value"),
    );

    response
}

/// Create mint [`Router`] with required endpoints for cashu mint with a custom
/// backend for cache
///
/// The `custom_methods` parameter should include all custom payment methods supported
/// by the payment processor, including "bolt11" and "bolt12" if they are supported.
#[allow(unused_mut, unused_variables)]
pub async fn create_mint_router_with_custom_cache(
    mint: Arc<Mint>,
    cache: HttpCache,
    custom_methods: Vec<String>,
    enable_info_page: bool,
) -> Result<Router> {
    #[cfg(feature = "federation")]
    let (federation_consensus, federation_sequences) =
        create_federation_runtime_state(&mint).await?;
    let mut state = MintState {
        mint,
        cache: Arc::new(cache),
        #[cfg(feature = "federation")]
        federation_consensus: federation_consensus.clone(),
        #[cfg(feature = "federation")]
        federation_sequences,
        #[cfg(feature = "federation")]
        federation_sync: federation_consensus
            .as_ref()
            .map(|_| Arc::new(tokio::sync::Mutex::new(FederationSyncState::default()))),
        #[cfg(feature = "federation")]
        federation_notifications: federation_consensus
            .as_ref()
            .map(|_| Arc::new(FederationOperationNotifications::default())),
        #[cfg(feature = "federation")]
        federation_finalizer: None,
    };
    #[cfg(feature = "federation")]
    state.start_federation_finalization_loop();

    let v1_router = Router::new()
        .route("/keys", get(get_keys))
        .route("/keysets", get(get_keysets))
        .route("/keys/{keyset_id}", get(get_keyset_pubkeys))
        .route("/swap", post(cache_post_swap))
        .route("/ws", get(ws_handler))
        .route("/checkstate", post(post_check))
        .route("/info", get(get_mint_info))
        .route("/restore", post(post_restore));

    let mut mint_router = Router::new().nest("/v1", v1_router);

    #[cfg(feature = "federation")]
    if state.federation_consensus.is_some() {
        federation::spawn_federation_peer_sync_loop(state.clone());
        mint_router = mint_router.nest("/federation/v1", federation::create_federation_router());
    }

    #[cfg(feature = "info-page")]
    if enable_info_page {
        mint_router = mint_router.route("/", get(get_index));
    }

    let mint_router = {
        let auth_router = create_auth_router(state.clone());
        mint_router.nest("/v1", auth_router)
    };

    // Create and merge custom payment method routers
    // This now includes bolt11 and bolt12 if they are in custom_methods
    let mint_router = if !custom_methods.is_empty() {
        // Validate custom method names
        custom_router::validate_custom_method_names(&custom_methods)
            .map_err(|e| anyhow::anyhow!("Invalid custom method names: {}", e))?;

        tracing::info!(
            "Creating routes for {} payment methods: {:?}",
            custom_methods.len(),
            custom_methods
        );

        let custom_router = custom_router::create_custom_routers(state.clone(), custom_methods);
        mint_router.nest("/v1", custom_router)
    } else {
        mint_router
    };

    #[cfg(feature = "prometheus")]
    let mint_router = mint_router.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        metrics::global_metrics_middleware,
    ));
    let mint_router = mint_router
        .layer(from_fn(cors_middleware))
        .with_state(state);

    Ok(mint_router)
}

#[cfg(feature = "federation")]
async fn create_federation_runtime_state(
    mint: &Arc<Mint>,
) -> Result<(
    Option<FederationConsensusHandle>,
    Option<FederationSequenceHandle>,
)> {
    let Some(config) = mint.federation_config() else {
        return Ok((None, None));
    };

    let db = mint.localstore();
    let entries = db.get_federation_journal_entries().await?;
    let sequences = db.get_federation_member_sequences().await?;
    let mut consensus = FederationConsensusRuntime::in_memory_from_entries(
        config.federation_id(),
        config.member_id(),
        entries.clone(),
    )?;
    let applied = mint.apply_federation_journal_entries(&entries).await?;
    for operation_id in applied {
        consensus.update_status(operation_id, FederationOperationStatus::Applied)?;
    }
    let sequences = FederationRestSequenceTracker::from_sequences(sequences);

    Ok((
        Some(Arc::new(tokio::sync::Mutex::new(consensus))),
        Some(Arc::new(tokio::sync::Mutex::new(sequences))),
    ))
}

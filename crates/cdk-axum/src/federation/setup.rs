//! Bounded setup-only coordinator HTTP service and authenticated client.

use core::future::Future;
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::error_handling::HandleErrorLayer;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Json, State};
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use cdk_common::database::mint::{Database, DynMintDatabase};
use cdk_common::federation::{
    FederationError, FederationSetupApproval, FederationSetupCoordinatorPhase,
    FederationSetupCoordinatorPublicKey, FederationSetupCorrelationNonce,
    FederationSetupEnrollment, FederationSetupEnrollmentAcknowledgement, FederationSetupId,
    FederationSetupInvitation, FederationSetupInvitationId, FederationSetupInvitationRecord,
    FederationSetupInvitationState, FederationSetupManifest, FederationSetupProposal,
    FederationSetupResponseBody, FederationSetupResponseRouteKind, FederationSetupSession,
    FederationSetupSignedResponse, FederationSetupStatus, FederationSetupUrlPolicy,
    FEDERATION_MAX_SETUP_ARTIFACT_BYTES, FEDERATION_MAX_SETUP_ENROLLMENT_BYTES,
    FEDERATION_MAX_SETUP_INVITATION_BYTES, FEDERATION_MAX_SETUP_STATUS_BYTES,
    FEDERATION_SETUP_CORRELATION_HEADER, FEDERATION_SETUP_PROTOCOL_VERSION,
};
use cdk_common::SecretKey;
use cdk_http_client::{HttpClient, HttpError};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore};
use tower::{BoxError, ServiceBuilder};
use url::Url;
use uuid::Uuid;

const SETUP_ENROLLMENTS_PATH: &str = "/federation/setup/v1/enrollments";
const SETUP_PROPOSAL_PATH: &str = "/federation/setup/v1/proposal";
const SETUP_APPROVALS_PATH: &str = "/federation/setup/v1/approvals";
const SETUP_MANIFEST_PATH: &str = "/federation/setup/v1/manifest";
const SETUP_STATUS_PATH: &str = "/federation/setup/v1/status";
const SETUP_APPROVAL_REQUEST_BYTES: usize = 4 * 1024;
const SETUP_ERROR_RESPONSE_BYTES: usize = 4 * 1024;

/// Stable structured error codes returned by the setup-only API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationSetupHttpErrorCode {
    /// Invitation is absent or malformed.
    InvalidInvitation,
    /// Invitation has expired.
    Expired,
    /// One-use authorization was replayed with different content.
    Replay,
    /// Immutable or compare-and-set state conflicts.
    Conflict,
    /// Requested setup artifact is not yet available.
    NotReady,
    /// A member or coordinator signature is invalid.
    InvalidSignature,
    /// Request or artifact uses an unsupported protocol version.
    UnsupportedVersion,
    /// Request is structurally invalid.
    InvalidRequest,
    /// Durable state could not be read or written.
    StorageFailure,
    /// Bounded admission was saturated.
    RateLimited,
    /// Request deadline elapsed while owned work continued safely.
    Timeout,
    /// Coordinator is shutting down.
    Unavailable,
}

/// Redacted setup HTTP failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("federation setup HTTP error: {code:?}")]
pub struct FederationSetupHttpError {
    /// Stable machine-readable code.
    pub code: FederationSetupHttpErrorCode,
    message: &'static str,
}

impl FederationSetupHttpError {
    const fn new(code: FederationSetupHttpErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }

    fn invalid_invitation() -> Self {
        Self::new(
            FederationSetupHttpErrorCode::InvalidInvitation,
            "setup invitation authentication failed",
        )
    }

    fn storage() -> Self {
        Self::new(
            FederationSetupHttpErrorCode::StorageFailure,
            "setup durable state is unavailable",
        )
    }

    fn from_federation(error: FederationError) -> Self {
        match error {
            FederationError::SetupVersionMismatch { .. } => Self::new(
                FederationSetupHttpErrorCode::UnsupportedVersion,
                "unsupported setup protocol version",
            ),
            FederationError::InvalidSetupSignature { .. }
            | FederationError::SetupSignatureVerificationFailed { .. } => Self::new(
                FederationSetupHttpErrorCode::InvalidSignature,
                "setup signature validation failed",
            ),
            FederationError::SetupPersistenceConflict { .. }
            | FederationError::SetupProposalHashMismatch { .. }
            | FederationError::SetupManifestHashMismatch { .. } => Self::new(
                FederationSetupHttpErrorCode::Conflict,
                "setup artifact conflicts with durable state",
            ),
            _ => Self::new(
                FederationSetupHttpErrorCode::InvalidRequest,
                "setup request validation failed",
            ),
        }
    }

    const fn status(&self) -> StatusCode {
        match self.code {
            FederationSetupHttpErrorCode::InvalidInvitation
            | FederationSetupHttpErrorCode::InvalidSignature => StatusCode::UNAUTHORIZED,
            FederationSetupHttpErrorCode::Expired => StatusCode::GONE,
            FederationSetupHttpErrorCode::Replay | FederationSetupHttpErrorCode::Conflict => {
                StatusCode::CONFLICT
            }
            FederationSetupHttpErrorCode::NotReady => StatusCode::TOO_EARLY,
            FederationSetupHttpErrorCode::UnsupportedVersion => StatusCode::UPGRADE_REQUIRED,
            FederationSetupHttpErrorCode::InvalidRequest => StatusCode::BAD_REQUEST,
            FederationSetupHttpErrorCode::StorageFailure => StatusCode::INTERNAL_SERVER_ERROR,
            FederationSetupHttpErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            FederationSetupHttpErrorCode::Timeout => StatusCode::GATEWAY_TIMEOUT,
            FederationSetupHttpErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FederationSetupHttpErrorBody {
    code: FederationSetupHttpErrorCode,
    message: String,
}

impl IntoResponse for FederationSetupHttpError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = FederationSetupHttpErrorBody {
            code: self.code,
            message: self.message.to_owned(),
        };
        no_store((status, Json(body)).into_response())
    }
}

/// Resource and deadline policy for a setup-only coordinator service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederationSetupCoordinatorServiceConfig {
    /// Maximum wall time a caller waits for one owned operation.
    pub request_deadline: Duration,
    /// Maximum concurrent requests from one source address.
    pub per_source_concurrency: usize,
    /// Maximum concurrent requests for one invitation.
    pub per_invitation_concurrency: usize,
    /// Maximum concurrent requests across this setup coordinator.
    pub global_concurrency: usize,
    /// Maximum distinct source buckets retained in memory.
    pub max_source_buckets: usize,
}

impl Default for FederationSetupCoordinatorServiceConfig {
    fn default() -> Self {
        Self {
            request_deadline: Duration::from_secs(5),
            per_source_concurrency: 4,
            per_invitation_concurrency: 1,
            global_concurrency: 32,
            max_source_buckets: 256,
        }
    }
}

impl FederationSetupCoordinatorServiceConfig {
    fn validate(self) -> Result<Self, FederationSetupHttpError> {
        if self.request_deadline.is_zero()
            || self.per_source_concurrency == 0
            || self.per_invitation_concurrency == 0
            || self.global_concurrency == 0
            || self.max_source_buckets == 0
        {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::InvalidRequest,
                "setup coordinator resource policy is invalid",
            ));
        }
        Ok(self)
    }
}

/// Setup-only coordinator state. It cannot expose wallet or runtime routes.
#[allow(missing_debug_implementations)]
pub struct FederationSetupCoordinatorService {
    database: DynMintDatabase,
    setup_id: FederationSetupId,
    member_count: u16,
    coordinator_secret_key: SecretKey,
    coordinator_identity: FederationSetupCoordinatorPublicKey,
    url_policy: FederationSetupUrlPolicy,
    config: FederationSetupCoordinatorServiceConfig,
    global_admission: Arc<Semaphore>,
    source_admission: Mutex<BTreeMap<IpAddr, Weak<Semaphore>>>,
    invitation_admission: Mutex<BTreeMap<FederationSetupInvitationId, Weak<Semaphore>>>,
    approval_admission: Arc<Semaphore>,
    shutting_down: AtomicBool,
    active_tasks: AtomicUsize,
    idle: Notify,
    #[cfg(test)]
    operation_delay: Mutex<Option<Duration>>,
    #[cfg(test)]
    fail_storage: AtomicBool,
}

impl FederationSetupCoordinatorService {
    /// Construct a bounded coordinator for one locally created setup.
    pub fn new(
        database: DynMintDatabase,
        setup_id: FederationSetupId,
        member_count: u16,
        coordinator_secret_key: SecretKey,
        url_policy: FederationSetupUrlPolicy,
        config: FederationSetupCoordinatorServiceConfig,
    ) -> Result<Self, FederationSetupHttpError> {
        let config = config.validate()?;
        if member_count == 0 {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::InvalidRequest,
                "setup coordinator member count is invalid",
            ));
        }
        let coordinator_identity =
            FederationSetupCoordinatorPublicKey::new(coordinator_secret_key.public_key())
                .map_err(FederationSetupHttpError::from_federation)?;
        Ok(Self {
            database,
            setup_id,
            member_count,
            coordinator_secret_key,
            coordinator_identity,
            url_policy,
            config,
            global_admission: Arc::new(Semaphore::new(config.global_concurrency)),
            source_admission: Mutex::new(BTreeMap::new()),
            invitation_admission: Mutex::new(BTreeMap::new()),
            approval_admission: Arc::new(Semaphore::new(1)),
            shutting_down: AtomicBool::new(false),
            active_tasks: AtomicUsize::new(0),
            idle: Notify::new(),
            #[cfg(test)]
            operation_delay: Mutex::new(None),
            #[cfg(test)]
            fail_storage: AtomicBool::new(false),
        })
    }

    /// Reject new work and wait until every detached timed-out operation finishes.
    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        while self.active_tasks.load(Ordering::Acquire) != 0 {
            self.idle.notified().await;
        }
    }

    async fn run_owned<T, F, Fut>(
        self: Arc<Self>,
        source: IpAddr,
        invitation_id: FederationSetupInvitationId,
        operation: F,
    ) -> Result<T, FederationSetupHttpError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<Self>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, FederationSetupHttpError>> + Send + 'static,
    {
        self.active_tasks.fetch_add(1, Ordering::AcqRel);
        let active = ActiveTaskGuard {
            service: self.clone(),
        };
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Unavailable,
                "setup coordinator is shutting down",
            ));
        }
        let permits = self.try_admit(source, invitation_id).await?;
        let service = self.clone();
        let mut task = tokio::spawn(async move {
            let _active = active;
            let _permits = permits;
            #[cfg(test)]
            if let Some(delay) = *service.operation_delay.lock().await {
                tokio::time::sleep(delay).await;
            }
            #[cfg(test)]
            if service.fail_storage.load(Ordering::Acquire) {
                return Err(FederationSetupHttpError::storage());
            }
            operation(service).await
        });
        match tokio::time::timeout(self.config.request_deadline, &mut task).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(FederationSetupHttpError::storage()),
            Err(_) => Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Timeout,
                "setup request deadline elapsed",
            )),
        }
    }

    async fn try_admit(
        &self,
        source: IpAddr,
        invitation_id: FederationSetupInvitationId,
    ) -> Result<SetupAdmission, FederationSetupHttpError> {
        let global = self
            .global_admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| rate_limited())?;
        let source_semaphore = {
            let mut sources = self.source_admission.lock().await;
            sources.retain(|_, semaphore| semaphore.strong_count() != 0);
            match sources.get(&source).and_then(Weak::upgrade) {
                Some(semaphore) => semaphore,
                None if sources.len() >= self.config.max_source_buckets => {
                    return Err(rate_limited());
                }
                None => {
                    let semaphore = Arc::new(Semaphore::new(self.config.per_source_concurrency));
                    sources.insert(source, Arc::downgrade(&semaphore));
                    semaphore
                }
            }
        };
        let source = source_semaphore
            .try_acquire_owned()
            .map_err(|_| rate_limited())?;
        let invitation_semaphore = {
            let mut invitations = self.invitation_admission.lock().await;
            invitations.retain(|_, semaphore| semaphore.strong_count() != 0);
            match invitations.get(&invitation_id).and_then(Weak::upgrade) {
                Some(semaphore) => semaphore,
                None => {
                    let semaphore =
                        Arc::new(Semaphore::new(self.config.per_invitation_concurrency));
                    invitations.insert(invitation_id, Arc::downgrade(&semaphore));
                    semaphore
                }
            }
        };
        let invitation = invitation_semaphore
            .try_acquire_owned()
            .map_err(|_| rate_limited())?;
        Ok(SetupAdmission {
            _global: global,
            _source: source,
            _invitation: invitation,
        })
    }

    async fn authenticate(
        &self,
        invitation: FederationSetupInvitation,
        require_consumed: bool,
    ) -> Result<AuthenticatedInvitation, FederationSetupHttpError> {
        let now = current_unix_time()?;
        let credential_expired = invitation.expires_at_unix <= now;
        let validation_time = match credential_expired {
            true => invitation.expires_at_unix.saturating_sub(1),
            false => now,
        };
        invitation
            .validate(validation_time, self.url_policy)
            .map_err(FederationSetupHttpError::from_federation)?;
        if invitation.setup_id != self.setup_id
            || invitation.coordinator_identity != self.coordinator_identity
        {
            return Err(FederationSetupHttpError::invalid_invitation());
        }
        let record = self
            .database
            .get_federation_setup_invitation(invitation.setup_id, invitation.invitation_id)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?
            .ok_or_else(FederationSetupHttpError::invalid_invitation)?;
        if credential_expired && record.state != FederationSetupInvitationState::Consumed {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Expired,
                "setup invitation is expired",
            ));
        }
        if record.member_id != invitation.member_id
            || record.expires_at_unix != invitation.expires_at_unix
            || record.capability_hash != invitation.capability_hash()
        {
            return Err(FederationSetupHttpError::invalid_invitation());
        }
        match record.state {
            FederationSetupInvitationState::Available if !require_consumed => {}
            FederationSetupInvitationState::Consumed => {}
            FederationSetupInvitationState::Expired => {
                return Err(FederationSetupHttpError::new(
                    FederationSetupHttpErrorCode::Expired,
                    "setup invitation is expired",
                ));
            }
            FederationSetupInvitationState::Available => {
                return Err(FederationSetupHttpError::new(
                    FederationSetupHttpErrorCode::Replay,
                    "setup invitation has not completed enrollment",
                ));
            }
        }
        Ok(AuthenticatedInvitation { invitation, record })
    }

    async fn enroll(
        &self,
        authenticated: AuthenticatedInvitation,
        enrollment: FederationSetupEnrollment,
    ) -> Result<FederationSetupEnrollment, FederationSetupHttpError> {
        enrollment
            .validate(self.url_policy)
            .map_err(FederationSetupHttpError::from_federation)?;
        if enrollment.setup_id != authenticated.invitation.setup_id
            || enrollment.invitation_id != authenticated.invitation.invitation_id
            || enrollment.member_id != authenticated.invitation.member_id
        {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::InvalidRequest,
                "setup enrollment does not match its invitation",
            ));
        }
        if authenticated.record.state == FederationSetupInvitationState::Consumed {
            return self.exact_enrollment_replay(&enrollment).await;
        }
        let consumed = authenticated
            .record
            .consumed()
            .map_err(FederationSetupHttpError::from_federation)?;
        let mut transaction = Database::begin_transaction(&*self.database)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?;
        let result = transaction
            .consume_federation_setup_invitation(&authenticated.record, &consumed, &enrollment)
            .await;
        if result.is_ok() {
            transaction
                .commit()
                .await
                .map_err(|_| FederationSetupHttpError::storage())?;
            return Ok(enrollment);
        }
        let _ = transaction.rollback().await;
        self.exact_enrollment_replay(&enrollment).await
    }

    async fn exact_enrollment_replay(
        &self,
        enrollment: &FederationSetupEnrollment,
    ) -> Result<FederationSetupEnrollment, FederationSetupHttpError> {
        let existing = self
            .database
            .get_federation_setup_enrollments(self.setup_id)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?
            .into_iter()
            .find(|existing| {
                existing.member_id == enrollment.member_id
                    || existing.invitation_id == enrollment.invitation_id
            });
        match existing {
            Some(existing) if existing == *enrollment => Ok(existing),
            Some(_) => Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Replay,
                "setup invitation was consumed by a different enrollment",
            )),
            None => Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Conflict,
                "setup invitation state conflicts with enrollment storage",
            )),
        }
    }

    async fn proposal(&self) -> Result<FederationSetupProposal, FederationSetupHttpError> {
        let proposal = self
            .database
            .get_federation_setup_proposal(self.setup_id)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?
            .ok_or_else(not_ready)?;
        proposal
            .validate(self.url_policy)
            .map_err(FederationSetupHttpError::from_federation)?;
        Ok(proposal)
    }

    async fn approve(
        &self,
        authenticated: AuthenticatedInvitation,
        approval: FederationSetupApproval,
    ) -> Result<FederationSetupApproval, FederationSetupHttpError> {
        let _approval_serial = self
            .approval_admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| rate_limited())?;
        let proposal = self.proposal().await?;
        if authenticated.invitation.member_id != approval.member_id {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::InvalidSignature,
                "setup approval signer does not match its invitation",
            ));
        }
        approval
            .validate(&proposal)
            .map_err(FederationSetupHttpError::from_federation)?;
        let existing = self
            .database
            .get_federation_setup_approvals(self.setup_id)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?;
        if let Some(existing) = existing
            .iter()
            .find(|existing| existing.member_id == approval.member_id)
        {
            return match existing == &approval {
                true => Ok(existing.clone()),
                false => Err(FederationSetupHttpError::new(
                    FederationSetupHttpErrorCode::Conflict,
                    "setup member already approved different content",
                )),
            };
        }
        let mut approvals = existing
            .into_iter()
            .map(|existing| (existing.member_id, existing))
            .collect::<BTreeMap<_, _>>();
        approvals.insert(approval.member_id, approval.clone());
        let certification = match approvals.len() == proposal.body.enrollments.len() {
            true => {
                let manifest = FederationSetupManifest::certify(
                    proposal,
                    approvals,
                    &self.coordinator_secret_key,
                    self.url_policy,
                )
                .map_err(FederationSetupHttpError::from_federation)?;
                let session = self.session().await?;
                let certified = session
                    .with_coordinator_phase(
                        FederationSetupCoordinatorPhase::Certified,
                        current_unix_time()?,
                    )
                    .map_err(FederationSetupHttpError::from_federation)?;
                Some((manifest, session, certified))
            }
            false => None,
        };
        let mut transaction = Database::begin_transaction(&*self.database)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?;
        transaction
            .add_federation_setup_approval(&approval)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?;
        if let Some((manifest, session, certified)) = certification {
            transaction
                .store_federation_setup_manifest(&session, &certified, &manifest)
                .await
                .map_err(|_| FederationSetupHttpError::storage())?;
        }
        transaction
            .commit()
            .await
            .map_err(|_| FederationSetupHttpError::storage())?;
        Ok(approval)
    }

    async fn manifest(&self) -> Result<FederationSetupManifest, FederationSetupHttpError> {
        let manifest = self
            .database
            .get_federation_setup_manifest(self.setup_id)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?
            .ok_or_else(not_ready)?;
        manifest
            .validate(self.url_policy)
            .map_err(FederationSetupHttpError::from_federation)?;
        Ok(manifest)
    }

    async fn status(&self) -> Result<FederationSetupStatus, FederationSetupHttpError> {
        let session = self.session().await?;
        let enrollments = self
            .database
            .get_federation_setup_enrollments(self.setup_id)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?;
        let approvals = self
            .database
            .get_federation_setup_approvals(self.setup_id)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?;
        let proposal = self
            .database
            .get_federation_setup_proposal(self.setup_id)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?;
        let manifest = self
            .database
            .get_federation_setup_manifest(self.setup_id)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?;
        let status = FederationSetupStatus {
            version: FEDERATION_SETUP_PROTOCOL_VERSION,
            setup_id: self.setup_id,
            coordinator_phase: session.coordinator_phase.ok_or_else(|| {
                FederationSetupHttpError::new(
                    FederationSetupHttpErrorCode::Conflict,
                    "setup coordinator lifecycle is missing",
                )
            })?,
            member_count: self.member_count,
            enrolled_members: enrollments
                .into_iter()
                .map(|enrollment| enrollment.member_id)
                .collect(),
            approved_members: approvals
                .into_iter()
                .map(|approval| approval.member_id)
                .collect(),
            proposal_hash: proposal.map(|proposal| proposal.proposal_hash),
            manifest_hash: manifest.map(|manifest| manifest.manifest_hash),
        };
        status
            .validate()
            .map_err(FederationSetupHttpError::from_federation)?;
        Ok(status)
    }

    /// Read validated local coordinator progress without network authentication.
    pub async fn local_status(&self) -> Result<FederationSetupStatus, FederationSetupHttpError> {
        self.status().await
    }

    async fn session(&self) -> Result<FederationSetupSession, FederationSetupHttpError> {
        self.database
            .get_federation_setup_session(self.setup_id)
            .await
            .map_err(|_| FederationSetupHttpError::storage())?
            .ok_or_else(|| {
                FederationSetupHttpError::new(
                    FederationSetupHttpErrorCode::Conflict,
                    "setup coordinator lifecycle is missing",
                )
            })
    }

    #[cfg(test)]
    async fn set_operation_delay(&self, delay: Option<Duration>) {
        *self.operation_delay.lock().await = delay;
    }

    #[cfg(test)]
    fn set_storage_failure(&self, fail: bool) {
        self.fail_storage.store(fail, Ordering::Release);
    }
}

struct ActiveTaskGuard {
    service: Arc<FederationSetupCoordinatorService>,
}

impl Drop for ActiveTaskGuard {
    fn drop(&mut self) {
        if self.service.active_tasks.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.service.idle.notify_waiters();
        }
    }
}

struct SetupAdmission {
    _global: OwnedSemaphorePermit,
    _source: OwnedSemaphorePermit,
    _invitation: OwnedSemaphorePermit,
}

struct AuthenticatedInvitation {
    invitation: FederationSetupInvitation,
    record: FederationSetupInvitationRecord,
}

/// Create the isolated router containing only the five setup protocol routes.
pub fn create_federation_setup_router(service: Arc<FederationSetupCoordinatorService>) -> Router {
    let request_deadline = service.config.request_deadline;
    Router::new()
        .route(
            SETUP_ENROLLMENTS_PATH,
            post(post_enrollment).layer(DefaultBodyLimit::max(
                FEDERATION_MAX_SETUP_ENROLLMENT_BYTES + SETUP_ERROR_RESPONSE_BYTES,
            )),
        )
        .route(SETUP_PROPOSAL_PATH, get(get_proposal))
        .route(
            SETUP_APPROVALS_PATH,
            post(post_approval).layer(DefaultBodyLimit::max(SETUP_APPROVAL_REQUEST_BYTES)),
        )
        .route(SETUP_MANIFEST_PATH, get(get_manifest))
        .route(SETUP_STATUS_PATH, get(get_status))
        .fallback(setup_not_found)
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_setup_timeout))
                .timeout(request_deadline),
        )
        .layer(axum::middleware::from_fn(add_no_store))
        .with_state(service)
}

async fn add_no_store(request: Request<Body>, next: Next) -> Response {
    no_store(next.run(request).await)
}

async fn handle_setup_timeout(error: BoxError) -> FederationSetupHttpError {
    match error.is::<tower::timeout::error::Elapsed>() {
        true => FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::Timeout,
            "setup request deadline elapsed",
        ),
        false => FederationSetupHttpError::storage(),
    }
}

async fn post_enrollment(
    State(service): State<Arc<FederationSetupCoordinatorService>>,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(enrollment): Json<FederationSetupEnrollment>,
) -> Result<Response, FederationSetupHttpError> {
    let (invitation, correlation_nonce) = request_authorization(&headers)?;
    let invitation_id = invitation.invitation_id;
    let response = service
        .clone()
        .run_owned(source.ip(), invitation_id, move |service| async move {
            let authenticated = service.authenticate(invitation, false).await?;
            let enrollment = service.enroll(authenticated, enrollment).await?;
            let acknowledgement = FederationSetupEnrollmentAcknowledgement::sign(
                &enrollment,
                correlation_nonce,
                &service.coordinator_secret_key,
            )
            .map_err(FederationSetupHttpError::from_federation)?;
            sign_response(
                &service,
                FederationSetupResponseRouteKind::EnrollmentAck,
                correlation_nonce,
                acknowledgement,
            )
        })
        .await?;
    bounded_json_response(response, FEDERATION_MAX_SETUP_ENROLLMENT_BYTES)
}

async fn get_proposal(
    State(service): State<Arc<FederationSetupCoordinatorService>>,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, FederationSetupHttpError> {
    let (invitation, correlation_nonce) = request_authorization(&headers)?;
    let invitation_id = invitation.invitation_id;
    let response = service
        .clone()
        .run_owned(source.ip(), invitation_id, move |service| async move {
            service.authenticate(invitation, true).await?;
            let proposal = service.proposal().await?;
            sign_response(
                &service,
                FederationSetupResponseRouteKind::Proposal,
                correlation_nonce,
                proposal,
            )
        })
        .await?;
    bounded_json_response(response, FEDERATION_MAX_SETUP_ARTIFACT_BYTES)
}

async fn post_approval(
    State(service): State<Arc<FederationSetupCoordinatorService>>,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(approval): Json<FederationSetupApproval>,
) -> Result<Response, FederationSetupHttpError> {
    let (invitation, correlation_nonce) = request_authorization(&headers)?;
    let invitation_id = invitation.invitation_id;
    let response = service
        .clone()
        .run_owned(source.ip(), invitation_id, move |service| async move {
            let authenticated = service.authenticate(invitation, true).await?;
            let approval = service.approve(authenticated, approval).await?;
            sign_response(
                &service,
                FederationSetupResponseRouteKind::ApprovalAck,
                correlation_nonce,
                approval,
            )
        })
        .await?;
    bounded_json_response(response, SETUP_APPROVAL_REQUEST_BYTES)
}

async fn get_manifest(
    State(service): State<Arc<FederationSetupCoordinatorService>>,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, FederationSetupHttpError> {
    let (invitation, correlation_nonce) = request_authorization(&headers)?;
    let invitation_id = invitation.invitation_id;
    let response = service
        .clone()
        .run_owned(source.ip(), invitation_id, move |service| async move {
            service.authenticate(invitation, true).await?;
            let manifest = service.manifest().await?;
            sign_response(
                &service,
                FederationSetupResponseRouteKind::Manifest,
                correlation_nonce,
                manifest,
            )
        })
        .await?;
    bounded_json_response(response, FEDERATION_MAX_SETUP_ARTIFACT_BYTES)
}

async fn get_status(
    State(service): State<Arc<FederationSetupCoordinatorService>>,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, FederationSetupHttpError> {
    let (invitation, correlation_nonce) = request_authorization(&headers)?;
    let invitation_id = invitation.invitation_id;
    let response = service
        .clone()
        .run_owned(source.ip(), invitation_id, move |service| async move {
            service.authenticate(invitation, true).await?;
            let status = service.status().await?;
            sign_response(
                &service,
                FederationSetupResponseRouteKind::Status,
                correlation_nonce,
                status,
            )
        })
        .await?;
    bounded_json_response(response, FEDERATION_MAX_SETUP_STATUS_BYTES)
}

async fn setup_not_found() -> FederationSetupHttpError {
    FederationSetupHttpError::new(
        FederationSetupHttpErrorCode::InvalidRequest,
        "unknown setup-only route",
    )
}

fn sign_response<T>(
    service: &FederationSetupCoordinatorService,
    route_kind: FederationSetupResponseRouteKind,
    correlation_nonce: FederationSetupCorrelationNonce,
    body: T,
) -> Result<FederationSetupSignedResponse<T>, FederationSetupHttpError>
where
    T: FederationSetupResponseBody,
{
    FederationSetupSignedResponse::sign(
        route_kind,
        correlation_nonce,
        body,
        &service.coordinator_secret_key,
    )
    .map_err(FederationSetupHttpError::from_federation)
}

fn request_authorization(
    headers: &HeaderMap,
) -> Result<(FederationSetupInvitation, FederationSetupCorrelationNonce), FederationSetupHttpError>
{
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(FederationSetupHttpError::invalid_invitation)?;
    if authorization.len() > FEDERATION_MAX_SETUP_INVITATION_BYTES + 7 {
        return Err(FederationSetupHttpError::invalid_invitation());
    }
    let invitation = authorization
        .strip_prefix("Bearer ")
        .ok_or_else(FederationSetupHttpError::invalid_invitation)?
        .parse()
        .map_err(|_| FederationSetupHttpError::invalid_invitation())?;
    let correlation_nonce = headers
        .get(FEDERATION_SETUP_CORRELATION_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::InvalidRequest,
                "setup correlation nonce is missing",
            )
        })?
        .parse()
        .map_err(|_| {
            FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::InvalidRequest,
                "setup correlation nonce is invalid",
            )
        })?;
    Ok((invitation, correlation_nonce))
}

fn bounded_json_response<T>(value: T, max: usize) -> Result<Response, FederationSetupHttpError>
where
    T: Serialize,
{
    let body = serde_json::to_vec(&value).map_err(|_| FederationSetupHttpError::storage())?;
    if body.len() > max {
        return Err(FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::StorageFailure,
            "setup response exceeds its configured bound",
        ));
    }
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(no_store(response))
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn rate_limited() -> FederationSetupHttpError {
    FederationSetupHttpError::new(
        FederationSetupHttpErrorCode::RateLimited,
        "setup request admission is saturated",
    )
}

fn not_ready() -> FederationSetupHttpError {
    FederationSetupHttpError::new(
        FederationSetupHttpErrorCode::NotReady,
        "setup artifact is not ready",
    )
}

fn current_unix_time() -> Result<u64, FederationSetupHttpError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| FederationSetupHttpError::storage())
}

/// Authenticated, no-redirect client for the setup-only coordinator API.
#[derive(Debug, Clone)]
pub struct FederationSetupHttpClient {
    client: HttpClient,
    request_deadline: Duration,
    retry_attempts: usize,
}

impl Default for FederationSetupHttpClient {
    fn default() -> Self {
        Self {
            client: HttpClient::builder()
                .no_redirects()
                .build()
                .expect("default no-redirect setup client"),
            request_deadline: Duration::from_secs(7),
            retry_attempts: 3,
        }
    }
}

impl FederationSetupHttpClient {
    /// Enroll an invitation's exact signed member record.
    pub async fn enroll(
        &self,
        invitation: &FederationSetupInvitation,
        enrollment: &FederationSetupEnrollment,
    ) -> Result<FederationSetupEnrollment, FederationSetupHttpError> {
        let nonce = random_correlation_nonce()?;
        let response: FederationSetupSignedResponse<FederationSetupEnrollmentAcknowledgement> =
            self.post(
                invitation,
                SETUP_ENROLLMENTS_PATH,
                nonce,
                enrollment,
                FEDERATION_MAX_SETUP_ENROLLMENT_BYTES,
            )
            .await?;
        response
            .validate(
                FederationSetupResponseRouteKind::EnrollmentAck,
                nonce,
                invitation.coordinator_identity,
            )
            .map_err(FederationSetupHttpError::from_federation)?;
        response
            .body
            .validate(enrollment, nonce, invitation.coordinator_identity)
            .map_err(FederationSetupHttpError::from_federation)?;
        Ok(enrollment.clone())
    }

    /// Fetch and authenticate the immutable setup proposal.
    pub async fn proposal(
        &self,
        invitation: &FederationSetupInvitation,
    ) -> Result<FederationSetupProposal, FederationSetupHttpError> {
        let nonce = random_correlation_nonce()?;
        let response: FederationSetupSignedResponse<FederationSetupProposal> = self
            .get(
                invitation,
                SETUP_PROPOSAL_PATH,
                nonce,
                FEDERATION_MAX_SETUP_ARTIFACT_BYTES,
            )
            .await?;
        response
            .validate(
                FederationSetupResponseRouteKind::Proposal,
                nonce,
                invitation.coordinator_identity,
            )
            .map_err(FederationSetupHttpError::from_federation)?;
        response
            .body
            .validate(url_policy_for(invitation))
            .map_err(FederationSetupHttpError::from_federation)?;
        if response.body.body.setup_id != invitation.setup_id
            || response.body.body.coordinator_identity != invitation.coordinator_identity
            || !response
                .body
                .body
                .enrollments
                .contains_key(&invitation.member_id)
        {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Conflict,
                "setup proposal does not match the invitation",
            ));
        }
        Ok(response.body)
    }

    /// Submit and authenticate acknowledgement of one durable approval.
    pub async fn approve(
        &self,
        invitation: &FederationSetupInvitation,
        approval: &FederationSetupApproval,
    ) -> Result<FederationSetupApproval, FederationSetupHttpError> {
        let nonce = random_correlation_nonce()?;
        let response: FederationSetupSignedResponse<FederationSetupApproval> = self
            .post(
                invitation,
                SETUP_APPROVALS_PATH,
                nonce,
                approval,
                SETUP_APPROVAL_REQUEST_BYTES,
            )
            .await?;
        response
            .validate(
                FederationSetupResponseRouteKind::ApprovalAck,
                nonce,
                invitation.coordinator_identity,
            )
            .map_err(FederationSetupHttpError::from_federation)?;
        if response.body != *approval {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Conflict,
                "setup approval acknowledgement changed the approval",
            ));
        }
        Ok(response.body)
    }

    /// Fetch and authenticate the complete certified manifest.
    pub async fn manifest(
        &self,
        invitation: &FederationSetupInvitation,
    ) -> Result<FederationSetupManifest, FederationSetupHttpError> {
        let nonce = random_correlation_nonce()?;
        let response: FederationSetupSignedResponse<FederationSetupManifest> = self
            .get(
                invitation,
                SETUP_MANIFEST_PATH,
                nonce,
                FEDERATION_MAX_SETUP_ARTIFACT_BYTES,
            )
            .await?;
        response
            .validate(
                FederationSetupResponseRouteKind::Manifest,
                nonce,
                invitation.coordinator_identity,
            )
            .map_err(FederationSetupHttpError::from_federation)?;
        response
            .body
            .validate(url_policy_for(invitation))
            .map_err(FederationSetupHttpError::from_federation)?;
        if response.body.proposal.body.setup_id != invitation.setup_id
            || response.body.proposal.body.coordinator_identity != invitation.coordinator_identity
        {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Conflict,
                "setup manifest does not match the invitation",
            ));
        }
        Ok(response.body)
    }

    /// Fetch authenticated aggregate setup progress.
    pub async fn status(
        &self,
        invitation: &FederationSetupInvitation,
    ) -> Result<FederationSetupStatus, FederationSetupHttpError> {
        let nonce = random_correlation_nonce()?;
        let response: FederationSetupSignedResponse<FederationSetupStatus> = self
            .get(
                invitation,
                SETUP_STATUS_PATH,
                nonce,
                FEDERATION_MAX_SETUP_STATUS_BYTES,
            )
            .await?;
        response
            .validate(
                FederationSetupResponseRouteKind::Status,
                nonce,
                invitation.coordinator_identity,
            )
            .map_err(FederationSetupHttpError::from_federation)?;
        response
            .body
            .validate()
            .map_err(FederationSetupHttpError::from_federation)?;
        if response.body.setup_id != invitation.setup_id {
            return Err(FederationSetupHttpError::new(
                FederationSetupHttpErrorCode::Conflict,
                "setup status does not match the invitation",
            ));
        }
        Ok(response.body)
    }

    async fn get<T>(
        &self,
        invitation: &FederationSetupInvitation,
        path: &str,
        nonce: FederationSetupCorrelationNonce,
        max_response_bytes: usize,
    ) -> Result<T, FederationSetupHttpError>
    where
        T: DeserializeOwned,
    {
        let url = setup_url(invitation, path)?;
        let authorization = format!(
            "Bearer {}",
            invitation
                .to_invitation_code()
                .map_err(FederationSetupHttpError::from_federation)?
        );
        self.retry_request(|| {
            self.client
                .get(url.as_str())
                .header(header::AUTHORIZATION.as_str(), &authorization)
                .header(FEDERATION_SETUP_CORRELATION_HEADER, nonce.to_string())
                .send_bounded(max_response_bytes)
        })
        .await
    }

    async fn post<P, T>(
        &self,
        invitation: &FederationSetupInvitation,
        path: &str,
        nonce: FederationSetupCorrelationNonce,
        payload: &P,
        max_response_bytes: usize,
    ) -> Result<T, FederationSetupHttpError>
    where
        P: Serialize + Sync,
        T: DeserializeOwned,
    {
        let url = setup_url(invitation, path)?;
        let authorization = format!(
            "Bearer {}",
            invitation
                .to_invitation_code()
                .map_err(FederationSetupHttpError::from_federation)?
        );
        self.retry_request(|| {
            self.client
                .post(url.as_str())
                .header(header::AUTHORIZATION.as_str(), &authorization)
                .header(FEDERATION_SETUP_CORRELATION_HEADER, nonce.to_string())
                .json(payload)
                .send_bounded(max_response_bytes)
        })
        .await
    }

    async fn retry_request<T, F, Fut>(&self, mut request: F) -> Result<T, FederationSetupHttpError>
    where
        T: DeserializeOwned,
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<cdk_http_client::RawResponse, HttpError>>,
    {
        let mut delay = Duration::from_millis(100);
        for attempt in 0..self.retry_attempts {
            let result = tokio::time::timeout(self.request_deadline, request()).await;
            match result {
                Ok(Ok(response)) if response.is_success() => {
                    return response.json().await.map_err(map_transport_error);
                }
                Ok(Ok(response)) => {
                    let status = response.status();
                    let body = response
                        .json::<FederationSetupHttpErrorBody>()
                        .await
                        .map_err(map_transport_error)?;
                    let error = FederationSetupHttpError::new(body.code, safe_message(body.code));
                    if status < 500 || attempt + 1 == self.retry_attempts {
                        return Err(error);
                    }
                }
                Ok(Err(error)) => {
                    if attempt + 1 == self.retry_attempts {
                        return Err(map_transport_error(error));
                    }
                }
                Err(_) => {
                    if attempt + 1 == self.retry_attempts {
                        return Err(FederationSetupHttpError::new(
                            FederationSetupHttpErrorCode::Timeout,
                            "setup request deadline elapsed",
                        ));
                    }
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(1));
        }
        Err(FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::Unavailable,
            "setup coordinator is unavailable",
        ))
    }
}

fn setup_url(
    invitation: &FederationSetupInvitation,
    path: &str,
) -> Result<Url, FederationSetupHttpError> {
    let mut url = Url::parse(&invitation.coordinator_url.to_string()).map_err(|_| {
        FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::InvalidInvitation,
            "setup coordinator URL is invalid",
        )
    })?;
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn random_correlation_nonce() -> Result<FederationSetupCorrelationNonce, FederationSetupHttpError> {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut bytes = [0; 32];
    bytes[..16].copy_from_slice(first.as_bytes());
    bytes[16..].copy_from_slice(second.as_bytes());
    FederationSetupCorrelationNonce::from_bytes(bytes)
        .map_err(FederationSetupHttpError::from_federation)
}

fn url_policy_for(invitation: &FederationSetupInvitation) -> FederationSetupUrlPolicy {
    match invitation
        .coordinator_url
        .to_string()
        .starts_with("http://")
    {
        true => FederationSetupUrlPolicy::AllowInsecureLocalhost,
        false => FederationSetupUrlPolicy::RequireHttps,
    }
}

fn map_transport_error(error: HttpError) -> FederationSetupHttpError {
    match error {
        HttpError::Timeout => FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::Timeout,
            "setup request deadline elapsed",
        ),
        HttpError::ResponseTooLarge { .. } => FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::InvalidRequest,
            "setup response exceeds its configured bound",
        ),
        _ => FederationSetupHttpError::new(
            FederationSetupHttpErrorCode::Unavailable,
            "setup coordinator is unavailable",
        ),
    }
}

const fn safe_message(code: FederationSetupHttpErrorCode) -> &'static str {
    match code {
        FederationSetupHttpErrorCode::InvalidInvitation => "setup invitation authentication failed",
        FederationSetupHttpErrorCode::Expired => "setup invitation is expired",
        FederationSetupHttpErrorCode::Replay => "setup request replay conflicts with durable state",
        FederationSetupHttpErrorCode::Conflict => "setup artifact conflicts with durable state",
        FederationSetupHttpErrorCode::NotReady => "setup artifact is not ready",
        FederationSetupHttpErrorCode::InvalidSignature => "setup signature validation failed",
        FederationSetupHttpErrorCode::UnsupportedVersion => "unsupported setup protocol version",
        FederationSetupHttpErrorCode::InvalidRequest => "setup request validation failed",
        FederationSetupHttpErrorCode::StorageFailure => "setup durable state is unavailable",
        FederationSetupHttpErrorCode::RateLimited => "setup request admission is saturated",
        FederationSetupHttpErrorCode::Timeout => "setup request deadline elapsed",
        FederationSetupHttpErrorCode::Unavailable => "setup coordinator is unavailable",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::str::FromStr;

    use cdk_common::database::MintFederationSetupDatabase;
    use cdk_common::federation::{
        FederationCheckpointPolicy, FederationDkgKeysetSpec, FederationPaymentObservationPolicy,
        FederationSetupBitcoinNetwork, FederationSetupFrostApplication, FederationSetupFrostPolicy,
        FederationSetupInvitationCapability, FederationSetupLocalRole,
        FederationSetupParticipantPhase, FederationSetupProposalBody,
        FederationSetupProtocolVersions, MemberId, ThresholdParams,
        FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
    };
    use cdk_common::mint_url::MintUrl;
    use cdk_common::{Amount, CurrencyUnit};
    use cdk_sqlite::MintSqliteDatabase;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::oneshot;

    use super::*;

    struct SetupHttpFixture {
        database: Arc<MintSqliteDatabase>,
        service: Arc<FederationSetupCoordinatorService>,
        invitations: BTreeMap<MemberId, FederationSetupInvitation>,
        enrollments: BTreeMap<MemberId, FederationSetupEnrollment>,
        member_secrets: BTreeMap<MemberId, SecretKey>,
        coordinator_secret: SecretKey,
        url_policy: FederationSetupUrlPolicy,
        shutdown: oneshot::Sender<()>,
        server: tokio::task::JoinHandle<()>,
    }

    impl SetupHttpFixture {
        async fn stop(self) {
            let _ = self.shutdown.send(());
            self.server.await.expect("setup server task");
            self.service.shutdown().await;
        }

        fn invitation(&self, member: u16) -> &FederationSetupInvitation {
            self.invitations
                .get(&member_id(member))
                .expect("member invitation")
        }

        fn enrollment(&self, member: u16) -> &FederationSetupEnrollment {
            self.enrollments
                .get(&member_id(member))
                .expect("member enrollment")
        }
    }

    #[tokio::test]
    async fn real_http_setup_flow_is_authenticated_idempotent_and_certifies() {
        let fixture = setup_http_fixture(FederationSetupCoordinatorServiceConfig::default()).await;
        let client = FederationSetupHttpClient::default();

        let not_ready = client
            .proposal(fixture.invitation(2))
            .await
            .expect_err("proposal must not exist before publication");
        assert_eq!(not_ready.code, FederationSetupHttpErrorCode::Replay);

        let (member_two, member_three) = tokio::join!(
            client.enroll(fixture.invitation(2), fixture.enrollment(2)),
            client.enroll(fixture.invitation(3), fixture.enrollment(3)),
        );
        assert_eq!(
            member_two.expect("member two enrollment"),
            *fixture.enrollment(2)
        );
        assert_eq!(
            member_three.expect("member three enrollment"),
            *fixture.enrollment(3)
        );
        assert_eq!(
            client
                .enroll(fixture.invitation(2), fixture.enrollment(2))
                .await
                .expect("exact enrollment replay"),
            *fixture.enrollment(2)
        );

        let mut invalid_enrollment = fixture.enrollment(2).clone();
        invalid_enrollment.signature = fixture.enrollment(3).signature;
        let invalid_signature = client
            .enroll(fixture.invitation(2), &invalid_enrollment)
            .await
            .expect_err("invalid enrollment signature");
        assert_eq!(
            invalid_signature.code,
            FederationSetupHttpErrorCode::InvalidSignature
        );

        let proposal = publish_proposal(&fixture).await;
        assert_eq!(
            client
                .proposal(fixture.invitation(2))
                .await
                .expect("authenticated proposal"),
            proposal
        );

        let mut equivocating_body = proposal.body.clone();
        equivocating_body
            .checkpoint_policy
            .checkpoint_interval_journal_operations += 1;
        let equivocating_proposal = FederationSetupProposal::sign(
            equivocating_body,
            &fixture.coordinator_secret,
            fixture.url_policy,
        )
        .expect("alternate proposal");
        let mut transaction = Database::begin_transaction(&*fixture.database)
            .await
            .expect("equivocation transaction");
        assert!(transaction
            .add_federation_setup_proposal(&equivocating_proposal)
            .await
            .is_err());
        transaction.rollback().await.expect("rollback equivocation");

        let mut wrong_identity = fixture.invitation(2).clone();
        wrong_identity.coordinator_identity =
            FederationSetupCoordinatorPublicKey::new(SecretKey::generate().public_key())
                .expect("alternate identity");
        let wrong_identity_error = client
            .proposal(&wrong_identity)
            .await
            .expect_err("wrong setup identity");
        assert_eq!(
            wrong_identity_error.code,
            FederationSetupHttpErrorCode::InvalidInvitation
        );

        let creator_approval = FederationSetupApproval::sign(
            &proposal,
            member_id(1),
            fixture.member_secrets.get(&member_id(1)).expect("secret"),
            fixture.url_policy,
        )
        .expect("creator approval");
        let mut transaction = Database::begin_transaction(&*fixture.database)
            .await
            .expect("creator approval transaction");
        transaction
            .add_federation_setup_approval(&creator_approval)
            .await
            .expect("creator approval");
        transaction.commit().await.expect("commit creator approval");

        let member_two_approval = FederationSetupApproval::sign(
            &proposal,
            member_id(2),
            fixture.member_secrets.get(&member_id(2)).expect("secret"),
            fixture.url_policy,
        )
        .expect("member two approval");
        client
            .approve(fixture.invitation(2), &member_two_approval)
            .await
            .expect("member two approval accepted");
        let alternate_member_two_approval = FederationSetupApproval::sign(
            &proposal,
            member_id(2),
            fixture.member_secrets.get(&member_id(2)).expect("secret"),
            fixture.url_policy,
        )
        .expect("alternate valid approval signature");
        if alternate_member_two_approval != member_two_approval {
            let conflict = client
                .approve(fixture.invitation(2), &alternate_member_two_approval)
                .await
                .expect_err("approval exact-byte conflict");
            assert_eq!(conflict.code, FederationSetupHttpErrorCode::Conflict);
        }

        let member_three_approval = FederationSetupApproval::sign(
            &proposal,
            member_id(3),
            fixture.member_secrets.get(&member_id(3)).expect("secret"),
            fixture.url_policy,
        )
        .expect("member three approval");
        client
            .approve(fixture.invitation(3), &member_three_approval)
            .await
            .expect("member three approval accepted");
        let manifest = client
            .manifest(fixture.invitation(2))
            .await
            .expect("certified manifest");
        assert_eq!(manifest.approvals.len(), 3);
        let status = client
            .status(fixture.invitation(2))
            .await
            .expect("certified status");
        assert_eq!(
            status.coordinator_phase,
            FederationSetupCoordinatorPhase::Certified
        );
        assert_eq!(status.enrolled_members.len(), 3);
        assert_eq!(status.approved_members.len(), 3);
        assert_eq!(status.manifest_hash, Some(manifest.manifest_hash));

        fixture.stop().await;
    }

    #[tokio::test]
    async fn real_http_limits_expiry_storage_failure_and_no_store_errors() {
        let fixture = setup_http_fixture(FederationSetupCoordinatorServiceConfig::default()).await;
        let client = FederationSetupHttpClient::default();
        let mut expired = fixture.invitation(2).clone();
        expired.expires_at_unix = current_unix_time().expect("time");
        let expired_error = client
            .enroll(&expired, fixture.enrollment(2))
            .await
            .expect_err("expired invitation");
        assert_eq!(expired_error.code, FederationSetupHttpErrorCode::Expired);

        fixture.service.set_storage_failure(true);
        let storage_error = client
            .enroll(fixture.invitation(2), fixture.enrollment(2))
            .await
            .expect_err("injected storage failure");
        assert_eq!(
            storage_error.code,
            FederationSetupHttpErrorCode::StorageFailure
        );
        fixture.service.set_storage_failure(false);

        let nonce = FederationSetupCorrelationNonce::from_bytes([4; 32]).expect("nonce");
        let response = reqwest::Client::new()
            .post(setup_url(fixture.invitation(2), SETUP_ENROLLMENTS_PATH).expect("URL"))
            .header(
                header::AUTHORIZATION,
                format!(
                    "Bearer {}",
                    fixture
                        .invitation(2)
                        .to_invitation_code()
                        .expect("invitation")
                ),
            )
            .header(FEDERATION_SETUP_CORRELATION_HEADER, nonce.to_string())
            .header(header::CONTENT_TYPE, "application/json")
            .body(vec![b' '; FEDERATION_MAX_SETUP_ENROLLMENT_BYTES + 8 * 1024])
            .send()
            .await
            .expect("oversized request response");
        assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .expect("cache control"),
            "no-store"
        );

        let response = reqwest::Client::new()
            .get(setup_url(fixture.invitation(2), SETUP_STATUS_PATH).expect("URL"))
            .header(FEDERATION_SETUP_CORRELATION_HEADER, nonce.to_string())
            .send()
            .await
            .expect("invalid invitation response");
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .expect("cache control"),
            "no-store"
        );

        fixture.stop().await;
    }

    #[tokio::test]
    async fn timeout_retains_admission_until_owned_work_finishes_and_shutdown_waits() {
        let config = FederationSetupCoordinatorServiceConfig {
            request_deadline: Duration::from_millis(25),
            per_source_concurrency: 1,
            ..FederationSetupCoordinatorServiceConfig::default()
        };
        let fixture = setup_http_fixture(config).await;
        let coordinator_url = Url::parse(&fixture.invitation(2).coordinator_url.to_string())
            .expect("coordinator URL");
        let coordinator_address = format!(
            "{}:{}",
            coordinator_url.host_str().expect("coordinator host"),
            coordinator_url.port().expect("coordinator port")
        );
        let mut slow_client = tokio::net::TcpStream::connect(coordinator_address)
            .await
            .expect("slow client connection");
        let slow_request = format!(
            "POST {SETUP_ENROLLMENTS_PATH} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\n{FEDERATION_SETUP_CORRELATION_HEADER}: {}\r\nContent-Type: application/json\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n{{",
            fixture
                .invitation(2)
                .to_invitation_code()
                .expect("invitation"),
            FederationSetupCorrelationNonce::from_bytes([3; 32]).expect("nonce")
        );
        slow_client
            .write_all(slow_request.as_bytes())
            .await
            .expect("partial slow request");
        let mut slow_response = vec![0; SETUP_ERROR_RESPONSE_BYTES];
        let response_bytes =
            tokio::time::timeout(Duration::from_secs(1), slow_client.read(&mut slow_response))
                .await
                .expect("slow request was bounded")
                .expect("slow response read");
        let slow_response = String::from_utf8_lossy(&slow_response[..response_bytes]);
        assert!(slow_response.starts_with("HTTP/1.1 504"));
        assert!(slow_response
            .to_ascii_lowercase()
            .contains("cache-control: no-store"));

        fixture
            .service
            .set_operation_delay(Some(Duration::from_millis(150)))
            .await;
        let client = FederationSetupHttpClient {
            request_deadline: Duration::from_secs(1),
            retry_attempts: 1,
            ..FederationSetupHttpClient::default()
        };
        let timeout = client
            .enroll(fixture.invitation(2), fixture.enrollment(2))
            .await
            .expect_err("slow owned operation times out");
        assert_eq!(timeout.code, FederationSetupHttpErrorCode::Timeout);
        let saturated = client
            .enroll(fixture.invitation(2), fixture.enrollment(2))
            .await
            .expect_err("timed-out operation retains admission");
        assert_eq!(saturated.code, FederationSetupHttpErrorCode::RateLimited);

        let started = tokio::time::Instant::now();
        fixture.service.shutdown().await;
        assert!(started.elapsed() >= Duration::from_millis(75));
        assert_eq!(fixture.service.active_tasks.load(Ordering::Acquire), 0);
        let unavailable = client
            .enroll(fixture.invitation(2), fixture.enrollment(2))
            .await
            .expect_err("shutdown coordinator rejects new work");
        assert_eq!(unavailable.code, FederationSetupHttpErrorCode::Unavailable);

        let _ = fixture.shutdown.send(());
        fixture.server.await.expect("setup server task");
    }

    #[tokio::test]
    async fn concurrent_same_invitation_has_one_owner_and_exact_retry_succeeds() {
        let config = FederationSetupCoordinatorServiceConfig {
            per_invitation_concurrency: 1,
            ..FederationSetupCoordinatorServiceConfig::default()
        };
        let fixture = setup_http_fixture(config).await;
        fixture
            .service
            .set_operation_delay(Some(Duration::from_millis(100)))
            .await;
        let client = FederationSetupHttpClient {
            retry_attempts: 1,
            ..FederationSetupHttpClient::default()
        };
        let (first, second) = tokio::join!(
            client.enroll(fixture.invitation(2), fixture.enrollment(2)),
            client.enroll(fixture.invitation(2), fixture.enrollment(2)),
        );
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        let error = first
            .err()
            .or_else(|| second.err())
            .expect("one saturation");
        assert_eq!(error.code, FederationSetupHttpErrorCode::RateLimited);
        fixture.service.set_operation_delay(None).await;
        client
            .enroll(fixture.invitation(2), fixture.enrollment(2))
            .await
            .expect("exact retry after owner completes");
        fixture.stop().await;
    }

    #[tokio::test]
    async fn admission_maps_do_not_retain_inactive_untrusted_keys() {
        let fixture = setup_http_fixture(FederationSetupCoordinatorServiceConfig::default()).await;
        for marker in 1_u8..=64 {
            let source = IpAddr::from([198, 51, 100, marker]);
            let invitation_id = FederationSetupInvitationId::from_bytes([marker; 32]);
            let admission = fixture
                .service
                .try_admit(source, invitation_id)
                .await
                .expect("inactive admission key");
            drop(admission);
        }
        assert_eq!(fixture.service.source_admission.lock().await.len(), 1);
        assert_eq!(fixture.service.invitation_admission.lock().await.len(), 1);
        fixture.stop().await;
    }

    #[tokio::test]
    async fn setup_client_never_follows_redirects_with_invitation_credentials() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("redirect listener");
        let address = listener.local_addr().expect("redirect address");
        let redirected_requests = Arc::new(AtomicUsize::new(0));
        let captured = redirected_requests.clone();
        let router = Router::new()
            .route(
                SETUP_ENROLLMENTS_PATH,
                post(|| async { axum::response::Redirect::temporary("/captured") }),
            )
            .route(
                "/captured",
                post(move || {
                    let captured = captured.clone();
                    async move {
                        captured.fetch_add(1, Ordering::AcqRel);
                        StatusCode::OK
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("redirect server");
        });
        let fixture = setup_http_fixture(FederationSetupCoordinatorServiceConfig::default()).await;
        let mut invitation = fixture.invitation(2).clone();
        invitation.coordinator_url = mint_url(&format!("http://{address}"));
        let client = FederationSetupHttpClient {
            retry_attempts: 1,
            ..FederationSetupHttpClient::default()
        };
        client
            .enroll(&invitation, fixture.enrollment(2))
            .await
            .expect_err("redirect is rejected");
        assert_eq!(redirected_requests.load(Ordering::Acquire), 0);
        server.abort();
        let _ = server.await;
        fixture.stop().await;
    }

    async fn setup_http_fixture(
        config: FederationSetupCoordinatorServiceConfig,
    ) -> SetupHttpFixture {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("localhost setup listener");
        let address = listener.local_addr().expect("listener address");
        let coordinator_url = mint_url(&format!("http://{address}"));
        let now = current_unix_time().expect("time");
        let setup_id = FederationSetupId::from_bytes([42; 32]);
        let coordinator_secret = secret_key(60_000);
        let coordinator_identity =
            FederationSetupCoordinatorPublicKey::new(coordinator_secret.public_key())
                .expect("coordinator identity");
        let database = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("database"));
        let url_policy = FederationSetupUrlPolicy::AllowInsecureLocalhost;
        let member_secrets = (1..=3)
            .map(|member| (member_id(member), secret_key(1_000 + member)))
            .collect::<BTreeMap<_, _>>();
        let enrollments = (1..=3)
            .map(|member| {
                let member_id = member_id(member);
                let invitation_id = invitation_id(member);
                let enrollment = FederationSetupEnrollment::sign(
                    setup_id,
                    invitation_id,
                    member_id,
                    mint_url(&format!("http://127.0.0.1:{}", 30_000 + member)),
                    mint_url(&format!("http://127.0.0.1:{}", 31_000 + member)),
                    [u8::try_from(member).expect("member byte"); 32],
                    FederationSetupProtocolVersions::default(),
                    member_secrets.get(&member_id).expect("member secret"),
                    url_policy,
                )
                .expect("enrollment");
                (member_id, enrollment)
            })
            .collect::<BTreeMap<_, _>>();
        let invitations = (2..=3)
            .map(|member| {
                let member_id = member_id(member);
                let invitation = FederationSetupInvitation::new(
                    setup_id,
                    invitation_id(member),
                    member_id,
                    coordinator_url.clone(),
                    coordinator_identity,
                    now + FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS + 60,
                    FederationSetupInvitationCapability::from_bytes(
                        [0xa0 + u8::try_from(member).expect("member byte"); 32],
                    )
                    .expect("capability"),
                    now,
                    url_policy,
                )
                .expect("invitation");
                (member_id, invitation)
            })
            .collect::<BTreeMap<_, _>>();
        let initial = FederationSetupSession::new(
            setup_id,
            FederationSetupLocalRole::Coordinator {
                member_id: member_id(1),
            },
            now,
        )
        .expect("session");
        let enrolled = initial
            .with_participant_phase(FederationSetupParticipantPhase::Enrolled, now)
            .expect("creator enrolled phase");
        let mut transaction = Database::begin_transaction(&*database)
            .await
            .expect("fixture transaction");
        transaction
            .add_federation_setup_session(&enrolled)
            .await
            .expect("session");
        transaction
            .add_federation_setup_enrollment(enrollments.get(&member_id(1)).expect("creator"))
            .await
            .expect("creator enrollment");
        for invitation in invitations.values() {
            transaction
                .add_federation_setup_invitation(
                    &FederationSetupInvitationRecord::from_invitation(invitation, now)
                        .expect("invitation record"),
                )
                .await
                .expect("invitation");
        }
        transaction.commit().await.expect("commit fixture");
        let dynamic_database: DynMintDatabase = database.clone();
        let service = Arc::new(
            FederationSetupCoordinatorService::new(
                dynamic_database,
                setup_id,
                3,
                coordinator_secret.clone(),
                url_policy,
                config,
            )
            .expect("setup service"),
        );
        let router = create_federation_setup_router(service.clone());
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = shutdown_receiver.await;
            })
            .await
            .expect("setup server");
        });
        SetupHttpFixture {
            database,
            service,
            invitations,
            enrollments,
            member_secrets,
            coordinator_secret,
            url_policy,
            shutdown,
            server,
        }
    }

    async fn publish_proposal(fixture: &SetupHttpFixture) -> FederationSetupProposal {
        let enrollments = fixture
            .database
            .get_federation_setup_enrollments(fixture.service.setup_id)
            .await
            .expect("enrollments")
            .into_iter()
            .map(|enrollment| (enrollment.member_id, enrollment))
            .collect();
        let network = FederationSetupBitcoinNetwork::Regtest;
        let proposal = FederationSetupProposal::sign(
            FederationSetupProposalBody::new(
                fixture.service.setup_id,
                fixture.service.coordinator_identity,
                enrollments,
                ThresholdParams::new(3, 2, 3).expect("thresholds"),
                FederationSetupProtocolVersions::default(),
                network,
                FederationPaymentObservationPolicy::member_quorum(3),
                FederationCheckpointPolicy::default(),
                vec![FederationDkgKeysetSpec::new(
                    CurrencyUnit::Sat,
                    true,
                    0,
                    None,
                    [Amount::from(1), Amount::from(2)]
                        .into_iter()
                        .collect::<BTreeSet<_>>(),
                )
                .expect("keyset")],
                FederationSetupFrostPolicy::new(
                    1,
                    vec![FederationSetupFrostApplication::CdkOnchain {
                        application_key_epoch: 1,
                        confirmation_policy_version: 1,
                        minimum_confirmations: 1,
                        fee_policy_version: 1,
                    }],
                    network,
                )
                .expect("FROST policy"),
                fixture.url_policy,
            )
            .expect("proposal body"),
            &fixture.coordinator_secret,
            fixture.url_policy,
        )
        .expect("proposal");
        let session = fixture.service.session().await.expect("session");
        let published = session
            .with_coordinator_phase(
                FederationSetupCoordinatorPhase::ProposalPublished,
                current_unix_time().expect("time"),
            )
            .expect("published phase");
        let mut transaction = Database::begin_transaction(&*fixture.database)
            .await
            .expect("proposal transaction");
        transaction
            .publish_federation_setup_proposal(&session, &published, &proposal)
            .await
            .expect("publish proposal");
        transaction.commit().await.expect("commit proposal");
        proposal
    }

    fn member_id(value: u16) -> MemberId {
        MemberId::new(value).expect("member ID")
    }

    fn invitation_id(member: u16) -> FederationSetupInvitationId {
        FederationSetupInvitationId::from_bytes(
            [42_u8.wrapping_add(u8::try_from(member).expect("member byte")); 32],
        )
    }

    fn mint_url(value: &str) -> MintUrl {
        MintUrl::from_str(value).expect("mint URL")
    }

    fn secret_key(value: u16) -> SecretKey {
        let mut bytes = [0; 32];
        bytes[30..].copy_from_slice(&value.to_be_bytes());
        SecretKey::from_slice(&bytes).expect("secret key")
    }
}

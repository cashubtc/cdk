//! The setup coordinator: bounded admission and the durable effect of each
//! setup protocol request, independent of the transport that carries it.

use core::future::Future;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use cdk_common::database::mint::{Database, DynMintDatabase};
use cdk_common::federation::{
    FederationSetupApproval, FederationSetupCoordinatorPhase, FederationSetupCoordinatorPublicKey,
    FederationSetupCorrelationNonce, FederationSetupEnrollment,
    FederationSetupEnrollmentAcknowledgement, FederationSetupId, FederationSetupInvitation,
    FederationSetupInvitationId, FederationSetupInvitationRecord, FederationSetupInvitationState,
    FederationSetupManifest, FederationSetupProposal, FederationSetupResponseBody,
    FederationSetupResponseRouteKind, FederationSetupSession, FederationSetupSignedResponse,
    FederationSetupStatus, FederationSetupUrlPolicy, FEDERATION_MAX_SETUP_INVITATION_BYTES,
    FEDERATION_SETUP_PROTOCOL_VERSION,
};
use cdk_common::SecretKey;
use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore};

use super::error::{FederationSetupHttpError, FederationSetupHttpErrorCode};

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

/// The connection a setup request arrived on; admission is bounded per source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FederationSetupRequestSource {
    /// A TCP peer address.
    Ip(IpAddr),
    /// An authenticated Iroh endpoint.
    #[cfg(feature = "iroh")]
    Iroh(cdk_iroh::EndpointId),
}

/// The credentials every setup request carries.
#[derive(Debug, Clone)]
pub struct FederationSetupRequestAuthorization {
    /// The invitation presented as the bearer credential.
    pub invitation: FederationSetupInvitation,
    /// The caller's nonce, bound into the signed response.
    pub correlation_nonce: FederationSetupCorrelationNonce,
}

impl FederationSetupRequestAuthorization {
    /// Parse the `Authorization` and correlation header values of a request.
    pub fn from_headers(
        authorization: Option<&str>,
        correlation_nonce: Option<&str>,
    ) -> Result<Self, FederationSetupHttpError> {
        let authorization =
            authorization.ok_or_else(FederationSetupHttpError::invalid_invitation)?;
        if authorization.len() > FEDERATION_MAX_SETUP_INVITATION_BYTES + 7 {
            return Err(FederationSetupHttpError::invalid_invitation());
        }
        let invitation = authorization
            .strip_prefix("Bearer ")
            .ok_or_else(FederationSetupHttpError::invalid_invitation)?
            .parse()
            .map_err(|_| FederationSetupHttpError::invalid_invitation())?;
        let correlation_nonce = correlation_nonce
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
        Ok(Self {
            invitation,
            correlation_nonce,
        })
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
    source_admission: Mutex<BTreeMap<FederationSetupRequestSource, Weak<Semaphore>>>,
    invitation_admission: Mutex<BTreeMap<FederationSetupInvitationId, Weak<Semaphore>>>,
    approval_admission: Arc<Semaphore>,
    shutting_down: AtomicBool,
    active_tasks: AtomicUsize,
    idle: Notify,
    faults: Arc<dyn FederationSetupOperationFaults>,
}

/// Fault points in one coordinator operation. Production injects none; tests
/// delay operations or fail storage through their own implementation.
#[async_trait::async_trait]
#[doc(hidden)]
pub trait FederationSetupOperationFaults: Send + Sync {
    /// Run before the operation body; an error ends the operation.
    async fn before_operation(&self) -> Result<(), FederationSetupHttpError> {
        Ok(())
    }
}

/// The production coordinator: no injected faults.
struct NoSetupOperationFaults;

impl FederationSetupOperationFaults for NoSetupOperationFaults {}

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
            faults: Arc::new(NoSetupOperationFaults),
        })
    }

    /// Maximum wall time a caller waits for one owned operation.
    pub fn request_deadline(&self) -> Duration {
        self.config.request_deadline
    }

    /// The setup this coordinator serves.
    pub fn setup_id(&self) -> FederationSetupId {
        self.setup_id
    }

    /// The coordinator's signing identity.
    pub fn coordinator_identity(&self) -> FederationSetupCoordinatorPublicKey {
        self.coordinator_identity
    }

    /// Number of operations still running, including timed-out ones.
    #[cfg(any(test, feature = "test-utils"))]
    #[doc(hidden)]
    pub fn active_tasks(&self) -> usize {
        self.active_tasks.load(Ordering::Acquire)
    }

    /// Reject new work and wait until every detached timed-out operation finishes.
    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        loop {
            // Register before observing the count: the last task can finish
            // between that load and awaiting its notify_waiters notification.
            let idle = self.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.active_tasks.load(Ordering::Acquire) == 0 {
                break;
            }
            idle.await;
        }
    }

    /// Enroll the invitation's signed member record.
    pub async fn enroll_request(
        self: Arc<Self>,
        source: FederationSetupRequestSource,
        request: FederationSetupRequestAuthorization,
        enrollment: FederationSetupEnrollment,
    ) -> Result<
        FederationSetupSignedResponse<FederationSetupEnrollmentAcknowledgement>,
        FederationSetupHttpError,
    > {
        let FederationSetupRequestAuthorization {
            invitation,
            correlation_nonce,
        } = request;
        let invitation_id = invitation.invitation_id;
        self.run_owned(source, invitation_id, move |service| async move {
            let authenticated = service.authenticate(invitation, false).await?;
            let enrollment = service.enroll(authenticated, enrollment).await?;
            let acknowledgement = FederationSetupEnrollmentAcknowledgement::sign(
                &enrollment,
                correlation_nonce,
                &service.coordinator_secret_key,
            )
            .map_err(FederationSetupHttpError::from_federation)?;
            service.sign_response(
                FederationSetupResponseRouteKind::EnrollmentAck,
                correlation_nonce,
                acknowledgement,
            )
        })
        .await
    }

    /// The published proposal, for an enrolled member.
    pub async fn proposal_request(
        self: Arc<Self>,
        source: FederationSetupRequestSource,
        request: FederationSetupRequestAuthorization,
    ) -> Result<FederationSetupSignedResponse<FederationSetupProposal>, FederationSetupHttpError>
    {
        let FederationSetupRequestAuthorization {
            invitation,
            correlation_nonce,
        } = request;
        let invitation_id = invitation.invitation_id;
        self.run_owned(source, invitation_id, move |service| async move {
            service.authenticate(invitation, true).await?;
            let proposal = service.proposal().await?;
            service.sign_response(
                FederationSetupResponseRouteKind::Proposal,
                correlation_nonce,
                proposal,
            )
        })
        .await
    }

    /// Record an enrolled member's approval; the last approval certifies the manifest.
    pub async fn approval_request(
        self: Arc<Self>,
        source: FederationSetupRequestSource,
        request: FederationSetupRequestAuthorization,
        approval: FederationSetupApproval,
    ) -> Result<FederationSetupSignedResponse<FederationSetupApproval>, FederationSetupHttpError>
    {
        let FederationSetupRequestAuthorization {
            invitation,
            correlation_nonce,
        } = request;
        let invitation_id = invitation.invitation_id;
        self.run_owned(source, invitation_id, move |service| async move {
            let authenticated = service.authenticate(invitation, true).await?;
            let approval = service.approve(authenticated, approval).await?;
            service.sign_response(
                FederationSetupResponseRouteKind::ApprovalAck,
                correlation_nonce,
                approval,
            )
        })
        .await
    }

    /// The certified manifest, for an enrolled member.
    pub async fn manifest_request(
        self: Arc<Self>,
        source: FederationSetupRequestSource,
        request: FederationSetupRequestAuthorization,
    ) -> Result<FederationSetupSignedResponse<FederationSetupManifest>, FederationSetupHttpError>
    {
        let FederationSetupRequestAuthorization {
            invitation,
            correlation_nonce,
        } = request;
        let invitation_id = invitation.invitation_id;
        self.run_owned(source, invitation_id, move |service| async move {
            service.authenticate(invitation, true).await?;
            let manifest = service.manifest().await?;
            service.sign_response(
                FederationSetupResponseRouteKind::Manifest,
                correlation_nonce,
                manifest,
            )
        })
        .await
    }

    /// Aggregate setup progress, for an enrolled member.
    pub async fn status_request(
        self: Arc<Self>,
        source: FederationSetupRequestSource,
        request: FederationSetupRequestAuthorization,
    ) -> Result<FederationSetupSignedResponse<FederationSetupStatus>, FederationSetupHttpError>
    {
        let FederationSetupRequestAuthorization {
            invitation,
            correlation_nonce,
        } = request;
        let invitation_id = invitation.invitation_id;
        self.run_owned(source, invitation_id, move |service| async move {
            service.authenticate(invitation, true).await?;
            let status = service.status().await?;
            service.sign_response(
                FederationSetupResponseRouteKind::Status,
                correlation_nonce,
                status,
            )
        })
        .await
    }

    /// Read validated local coordinator progress without network authentication.
    pub async fn local_status(&self) -> Result<FederationSetupStatus, FederationSetupHttpError> {
        self.status().await
    }

    /// Replace the fault hooks run before every operation.
    #[cfg(any(test, feature = "test-utils"))]
    #[doc(hidden)]
    #[must_use]
    pub fn with_faults(mut self, faults: Arc<dyn FederationSetupOperationFaults>) -> Self {
        self.faults = faults;
        self
    }

    fn sign_response<T>(
        &self,
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
            &self.coordinator_secret_key,
        )
        .map_err(FederationSetupHttpError::from_federation)
    }

    async fn run_owned<T, F, Fut>(
        self: Arc<Self>,
        source: FederationSetupRequestSource,
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
            service.faults.before_operation().await?;
            operation(service).await
        });
        match tokio::time::timeout(self.config.request_deadline, &mut task).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(FederationSetupHttpError::storage()),
            Err(_) => Err(FederationSetupHttpError::timeout()),
        }
    }

    async fn try_admit(
        &self,
        source: FederationSetupRequestSource,
        invitation_id: FederationSetupInvitationId,
    ) -> Result<SetupAdmission, FederationSetupHttpError> {
        let global = self
            .global_admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| FederationSetupHttpError::rate_limited())?;
        let source_semaphore = {
            let mut sources = self.source_admission.lock().await;
            sources.retain(|_, semaphore| semaphore.strong_count() != 0);
            match sources.get(&source).and_then(Weak::upgrade) {
                Some(semaphore) => semaphore,
                None if sources.len() >= self.config.max_source_buckets => {
                    return Err(FederationSetupHttpError::rate_limited());
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
            .map_err(|_| FederationSetupHttpError::rate_limited())?;
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
            .map_err(|_| FederationSetupHttpError::rate_limited())?;
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
            .ok_or_else(FederationSetupHttpError::not_ready)?;
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
            .map_err(|_| FederationSetupHttpError::rate_limited())?;
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
            .ok_or_else(FederationSetupHttpError::not_ready)?;
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

    /// The coordinator's durable setup lifecycle.
    pub async fn session(&self) -> Result<FederationSetupSession, FederationSetupHttpError> {
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

fn current_unix_time() -> Result<u64, FederationSetupHttpError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| FederationSetupHttpError::storage())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn admission_maps_do_not_retain_inactive_untrusted_keys() {
        let database: DynMintDatabase =
            Arc::new(cdk_sqlite::mint::memory::empty().await.expect("database"));
        let service = FederationSetupCoordinatorService::new(
            database,
            FederationSetupId::from_bytes([42; 32]),
            3,
            SecretKey::from_slice(&[7; 32]).expect("coordinator secret"),
            FederationSetupUrlPolicy::AllowInsecureLocalhost,
            FederationSetupCoordinatorServiceConfig::default(),
        )
        .expect("setup service");
        for marker in 1_u8..=64 {
            let source = IpAddr::from([198, 51, 100, marker]);
            let invitation_id = FederationSetupInvitationId::from_bytes([marker; 32]);
            let admission = service
                .try_admit(FederationSetupRequestSource::Ip(source), invitation_id)
                .await
                .expect("inactive admission key");
            drop(admission);
        }
        assert_eq!(service.source_admission.lock().await.len(), 1);
        assert_eq!(service.invitation_admission.lock().await.len(), 1);
    }
}

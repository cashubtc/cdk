//! Restartable orchestration for dealer-free federation DKG over private REST.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use cdk::federation::{
    redact_federation_operator_message, FederationConfig, FederationDkgActivationRequest,
    FederationDkgCeremonyId, FederationDkgCeremonyState, FederationDkgCommitmentRequest,
    FederationDkgDriverPhase, FederationDkgDriverPlan, FederationDkgDriverProgress,
    FederationDkgDriverRecords, FederationDkgKeysetSpec, FederationDkgMemberKeysetShare,
    FederationDkgReadinessRequest, FederationDkgRevealRequest, FederationDkgSecretShareRequest,
    FederationDkgTranscriptSignatureRequest, FederationError,
    FederationFrostDkgConfirmationRequest, FederationFrostDkgContext, FederationFrostDkgLocalPhase,
    FederationFrostDkgLocalState, FederationFrostDkgRound1Request, FederationFrostDkgRound2Record,
    FederationFrostDkgRound2Request, FederationFrostRootPrivateConfig,
    FederationFrostRootPublicConfig, FederationFrostRootRecord, FederationSetupManifest,
    FederationSetupUrlPolicy, MemberId, ValidatedFederationDkgParticipantState,
};
use cdk_frost::rand_core::OsRng;
use cdk_frost::{
    advance_dkg, finish_dkg, start_dkg, CanonicalRootKeyPackage, DkgRound1Package, DkgRound1Secret,
    DkgRound2Secret, EncryptedSecret, SecretEncryptionKey,
};
use cdk_http_client::{HttpError, Transport};
use futures::future::join_all;
use futures::{stream, StreamExt};
use sha2::{Digest, Sha256};

use crate::MintState;

use super::peer_client::{FederationRestPeerClient, FederationRestPeerClientError};
use super::sync_config::FederationPeerSyncConfig;

const FEDERATION_DKG_DELIVERY_CONCURRENCY: usize = 4;

/// Confidentiality policy for dealer-free DKG secret-share delivery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FederationDkgSecretShareTransportPolicy {
    /// Require HTTPS or Iroh for every configured federation member endpoint.
    #[default]
    RequireSecureTransport,
    /// Permit plain HTTP only for explicit loopback development endpoints.
    AllowInsecureLocalhost,
}

/// Bounded retry policy for one cooperative DKG driver invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederationDkgDriverRunConfig {
    /// Maximum number of durable plan-and-deliver ticks.
    pub max_ticks: usize,
    /// Delay between incomplete ticks.
    pub retry_delay: Duration,
    /// Overall time budget for one ceremony driver run.
    ///
    /// DKG requires all configured roster members for the entire ceremony.
    /// When one or more members become unavailable mid-ceremony, the driver
    /// aborts once this budget elapses and reports the members it is still
    /// waiting on, instead of occupying the member with unbounded retry.
    /// Production deployments should keep this generous enough for slow
    /// networks; tests should choose small values.
    pub ceremony_timeout: Duration,
    /// Confidentiality policy applied before any ceremony message is sent.
    pub secret_share_transport: FederationDkgSecretShareTransportPolicy,
    /// Number of authenticated DKG requests one member may burst per receiver.
    pub member_request_burst: usize,
    /// Interval at which one authenticated DKG request token is restored.
    pub member_request_refill_interval: Duration,
}

impl Default for FederationDkgDriverRunConfig {
    fn default() -> Self {
        Self {
            max_ticks: 64,
            retry_delay: Duration::from_secs(1),
            ceremony_timeout: Duration::from_secs(600),
            secret_share_transport: FederationDkgSecretShareTransportPolicy::RequireSecureTransport,
            member_request_burst: 16,
            member_request_refill_interval: Duration::from_millis(250),
        }
    }
}

/// Complete operator input needed to start or resume one DKG ceremony.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationDkgAutoStartConfig {
    /// Certified setup authorization required by every DKG artifact.
    pub(crate) setup_authorization: cdk::federation::FederationSetupAuthorizationHash,
    /// Unique ceremony ID shared by every configured member.
    pub(crate) ceremony_id: FederationDkgCeremonyId,
    /// Application-neutral FROST root epoch produced alongside the BLS keyset.
    pub(crate) root_key_epoch: u64,
    /// Exact aggregate keyset policy every participant must commit to.
    pub(crate) keyset_spec: FederationDkgKeysetSpec,
    /// Bounded retry and confidential-transport policy.
    pub(crate) driver: FederationDkgDriverRunConfig,
}

impl FederationDkgAutoStartConfig {
    /// Derive all shared DKG inputs from one validated unanimous setup manifest.
    pub fn from_manifest(
        manifest: &FederationSetupManifest,
        url_policy: FederationSetupUrlPolicy,
        mut driver: FederationDkgDriverRunConfig,
    ) -> Result<Self, cdk::Error> {
        manifest.validate(url_policy)?;
        let keyset_spec = match manifest.keyset_specs() {
            [keyset_spec] => keyset_spec.clone(),
            keysets => {
                return Err(cdk::Error::Custom(format!(
                    "initial DKG handoff requires exactly one BLS keyset specification, found {}",
                    keysets.len()
                )))
            }
        };
        driver.secret_share_transport = match url_policy {
            FederationSetupUrlPolicy::RequireSecureTransport => {
                FederationDkgSecretShareTransportPolicy::RequireSecureTransport
            }
            FederationSetupUrlPolicy::AllowInsecureLocalhost => {
                FederationDkgSecretShareTransportPolicy::AllowInsecureLocalhost
            }
        };
        driver.validate()?;
        Ok(Self {
            setup_authorization: manifest.setup_authorization_hash()?,
            ceremony_id: manifest.ceremony_id()?,
            root_key_epoch: manifest.frost_policy().root_key_epoch,
            keyset_spec,
            driver,
        })
    }

    /// Return the manifest-derived setup authorization.
    #[must_use]
    pub const fn setup_authorization(&self) -> cdk::federation::FederationSetupAuthorizationHash {
        self.setup_authorization
    }

    /// Return the manifest-derived ceremony identifier.
    #[must_use]
    pub const fn ceremony_id(&self) -> FederationDkgCeremonyId {
        self.ceremony_id
    }

    /// Return the manifest-derived FROST root epoch.
    #[must_use]
    pub const fn root_key_epoch(&self) -> u64 {
        self.root_key_epoch
    }

    /// Return the exact manifest-derived BLS keyset policy.
    #[must_use]
    pub const fn keyset_spec(&self) -> &FederationDkgKeysetSpec {
        &self.keyset_spec
    }

    /// Return the machine-local bounded driver policy.
    #[must_use]
    pub const fn driver(&self) -> FederationDkgDriverRunConfig {
        self.driver
    }

    /// Validate setup-mode and transport requirements before spawning the driver.
    pub fn validate_for_public_config(
        &self,
        public_config: &FederationConfig,
    ) -> Result<(), cdk::Error> {
        public_config.validate()?;
        if self.setup_authorization != public_config.setup_authorization {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "DKG auto-start setup authorization does not match the public config"
                    .to_string(),
            }
            .into());
        }
        if !public_config.keysets.is_empty() || public_config.setup_transcript.is_some() {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "live DKG start requires a public setup config without finalized keysets or transcript"
                    .to_string(),
            }
            .into());
        }
        self.keyset_spec.validate_for_federation(public_config)?;
        if self.root_key_epoch == 0 {
            return Err(cdk::Error::Custom(
                "federation FROST root key epoch must be greater than zero".to_string(),
            ));
        }
        self.driver.validate()?;
        validate_federation_dkg_secret_share_transport(
            public_config,
            self.driver.secret_share_transport,
        )
    }
}

impl FederationDkgDriverRunConfig {
    fn validate(self) -> Result<(), cdk::Error> {
        if self.max_ticks == 0 {
            return Err(cdk::Error::Custom(
                "federation DKG driver max ticks must be greater than zero".to_string(),
            ));
        }
        if self.retry_delay.is_zero() {
            return Err(cdk::Error::Custom(
                "federation DKG driver retry delay must be greater than zero".to_string(),
            ));
        }
        if self.ceremony_timeout.is_zero() {
            return Err(cdk::Error::Custom(
                "federation DKG driver ceremony timeout must be greater than zero".to_string(),
            ));
        }
        if !(1..=64).contains(&self.member_request_burst) {
            return Err(cdk::Error::Custom(
                "federation DKG member request burst must be between 1 and 64".to_string(),
            ));
        }
        if self.member_request_refill_interval < Duration::from_millis(10) {
            return Err(cdk::Error::Custom(
                "federation DKG member request refill interval must be at least 10 milliseconds"
                    .to_string(),
            ));
        }

        Ok(())
    }
}

/// Redacted result of one durable DKG plan-and-deliver tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationDkgDriverTickOutcome {
    /// Phase justified by durable records before delivery.
    pub phase_before: FederationDkgDriverPhase,
    /// Phase justified after all attempted delivery and persistence work.
    pub phase_after: FederationDkgDriverPhase,
    /// Redacted durable progress after the tick.
    pub progress: FederationDkgDriverProgress,
    /// Number of authenticated private REST deliveries attempted.
    pub attempted_deliveries: usize,
    /// Number of deliveries acknowledged by their configured receiver.
    pub successful_deliveries: usize,
    /// Number of deliveries that can be retried safely.
    pub failed_deliveries: usize,
    /// Configured receivers with at least one failed delivery.
    pub failed_members: Vec<MemberId>,
}

impl FederationDkgDriverTickOutcome {
    /// Whether the durable local view contains a complete, verified ceremony.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.phase_after == FederationDkgDriverPhase::Complete && self.failed_deliveries == 0
    }
}

/// Redacted result of one bounded DKG driver invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationDkgDriverRunOutcome {
    /// Number of ticks executed by this invocation.
    pub ticks: usize,
    /// Total private REST deliveries attempted across all ticks.
    pub attempted_deliveries: usize,
    /// Total private REST deliveries acknowledged across all ticks.
    pub successful_deliveries: usize,
    /// Total private REST deliveries that failed across all ticks.
    pub failed_deliveries: usize,
    /// Last durable tick outcome.
    pub last_tick: FederationDkgDriverTickOutcome,
}

impl FederationDkgDriverRunOutcome {
    /// Whether the invocation reached a complete, verified ceremony.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.last_tick.is_complete()
    }
}

#[derive(Debug, Default)]
struct FederationDkgDeliveryTotals {
    attempted: usize,
    successful: usize,
    failed: usize,
}

impl FederationDkgDeliveryTotals {
    fn record(&mut self, tick: &FederationDkgDriverTickOutcome) {
        self.attempted = self.attempted.saturating_add(tick.attempted_deliveries);
        self.successful = self.successful.saturating_add(tick.successful_deliveries);
        self.failed = self.failed.saturating_add(tick.failed_deliveries);
    }
}

fn trace_federation_dkg_driver_tick(
    local_member: MemberId,
    ceremony_id: &FederationDkgCeremonyId,
    tick: usize,
    outcome: &FederationDkgDriverTickOutcome,
) {
    if !matches!(
        std::env::var("CDK_MINTD_FEDERATION_DKG_TRACE_TICKS").as_deref(),
        Ok("1")
    ) {
        return;
    }
    tracing::info!(
        %local_member,
        %ceremony_id,
        tick,
        phase_before = ?outcome.phase_before,
        phase_after = ?outcome.phase_after,
        attempted_deliveries = outcome.attempted_deliveries,
        successful_deliveries = outcome.successful_deliveries,
        failed_deliveries = outcome.failed_deliveries,
        "Federation DKG driver tick metrics"
    );
}

#[derive(Debug, Default)]
pub(crate) struct DeliverySummary {
    pub(crate) attempted: usize,
    pub(crate) successful: usize,
    pub(crate) failed: usize,
    pub(crate) failed_members: BTreeSet<MemberId>,
}

#[derive(Clone)]
struct FederationDkgPlanCache {
    records: FederationDkgDriverRecords,
    plan: FederationDkgDriverPlan,
}

#[derive(Clone)]
struct TrackedDkgBroadcast<T> {
    request: T,
    acknowledged_members: BTreeSet<MemberId>,
}

#[derive(Clone, Default)]
pub(crate) struct FederationDkgDeliveryTracker {
    commitment: Option<TrackedDkgBroadcast<FederationDkgCommitmentRequest>>,
    reveal: Option<TrackedDkgBroadcast<FederationDkgRevealRequest>>,
    secret_shares: BTreeMap<MemberId, FederationDkgSecretShareRequest>,
    transcript_signature: Option<TrackedDkgBroadcast<FederationDkgTranscriptSignatureRequest>>,
    activation: Option<TrackedDkgBroadcast<FederationDkgActivationRequest>>,
}

fn pending_broadcast_targets<T, I>(
    tracked: &mut Option<TrackedDkgBroadcast<T>>,
    request: &T,
    targets: I,
) -> Vec<MemberId>
where
    T: Clone + PartialEq,
    I: IntoIterator<Item = MemberId>,
{
    let request_changed = match tracked.as_ref() {
        Some(tracked) => tracked.request != *request,
        None => true,
    };
    if request_changed {
        *tracked = Some(TrackedDkgBroadcast {
            request: request.clone(),
            acknowledged_members: BTreeSet::new(),
        });
    }
    let acknowledged_members = &tracked
        .as_ref()
        .expect("tracked DKG broadcast must be initialized")
        .acknowledged_members;
    targets
        .into_iter()
        .filter(|member| !acknowledged_members.contains(member))
        .collect()
}

fn record_broadcast_acknowledgements<T>(
    tracked: &mut Option<TrackedDkgBroadcast<T>>,
    request: &T,
    acknowledged_members: &BTreeSet<MemberId>,
) where
    T: PartialEq,
{
    if let Some(tracked) = tracked.as_mut() {
        if tracked.request == *request {
            tracked
                .acknowledged_members
                .extend(acknowledged_members.iter().copied());
        }
    }
}

fn broadcast_acknowledged_by_all<T, I>(
    tracked: &Option<TrackedDkgBroadcast<T>>,
    request: &T,
    members: I,
) -> bool
where
    T: PartialEq,
    I: IntoIterator<Item = MemberId>,
{
    tracked.as_ref().is_some_and(|tracked| {
        tracked.request == *request
            && members
                .into_iter()
                .all(|member| tracked.acknowledged_members.contains(&member))
    })
}

fn secret_shares_acknowledged_by_all(
    tracked: &BTreeMap<MemberId, FederationDkgSecretShareRequest>,
    requests: &BTreeMap<MemberId, FederationDkgSecretShareRequest>,
) -> bool {
    !requests.is_empty()
        && requests
            .iter()
            .all(|(member, request)| tracked.get(member) == Some(request))
}

impl DeliverySummary {
    fn record(&mut self, member_id: MemberId, accepted: bool) {
        self.attempted = self.attempted.saturating_add(1);
        if accepted {
            self.successful = self.successful.saturating_add(1);
        } else {
            self.failed = self.failed.saturating_add(1);
            self.failed_members.insert(member_id);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FederationDkgDeliveryFailureKind {
    Timeout,
    Connection,
    OutboundAdmission,
    RemoteAdmission,
    RemoteNotReady,
    RemoteValidation,
    RemoteStorage,
    RemoteServer,
    HttpStatus,
    ResponseDecode,
    ResponseValidation,
    TransportConfiguration,
    Transport,
    Unknown,
}

impl FederationDkgDeliveryFailureKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connection => "connection",
            Self::OutboundAdmission => "outbound_admission",
            Self::RemoteAdmission => "remote_admission",
            Self::RemoteNotReady => "remote_not_ready",
            Self::RemoteValidation => "remote_validation",
            Self::RemoteStorage => "remote_storage",
            Self::RemoteServer => "remote_server",
            Self::HttpStatus => "http_status",
            Self::ResponseDecode => "response_decode",
            Self::ResponseValidation => "response_validation",
            Self::TransportConfiguration => "transport_configuration",
            Self::Transport => "transport",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FederationDkgDeliveryFailure {
    kind: FederationDkgDeliveryFailureKind,
    http_status: Option<u16>,
}

fn classify_federation_dkg_delivery_failure(err: &anyhow::Error) -> FederationDkgDeliveryFailure {
    if let Some(peer_error) = err.downcast_ref::<FederationRestPeerClientError>() {
        let kind = match peer_error {
            FederationRestPeerClientError::RequestTimeout { .. } => {
                FederationDkgDeliveryFailureKind::Timeout
            }
            FederationRestPeerClientError::OutboundAdmissionFull { .. }
            | FederationRestPeerClientError::OutboundAdmissionClosed { .. } => {
                FederationDkgDeliveryFailureKind::OutboundAdmission
            }
        };
        return FederationDkgDeliveryFailure {
            kind,
            http_status: None,
        };
    }
    if let Some(http_error) = err.downcast_ref::<HttpError>() {
        let (kind, http_status) = match http_error {
            HttpError::Status { status: 429, .. } => {
                (FederationDkgDeliveryFailureKind::RemoteAdmission, Some(429))
            }
            HttpError::Status { status: 425, .. } => {
                (FederationDkgDeliveryFailureKind::RemoteNotReady, Some(425))
            }
            HttpError::Status { status, .. } if (400..500).contains(status) => (
                FederationDkgDeliveryFailureKind::RemoteValidation,
                Some(*status),
            ),
            HttpError::Status { status: 503, .. } => {
                (FederationDkgDeliveryFailureKind::RemoteStorage, Some(503))
            }
            HttpError::Status { status, .. } if (500..600).contains(status) => (
                FederationDkgDeliveryFailureKind::RemoteServer,
                Some(*status),
            ),
            HttpError::Status { status, .. } => {
                (FederationDkgDeliveryFailureKind::HttpStatus, Some(*status))
            }
            HttpError::Connection(_) => (FederationDkgDeliveryFailureKind::Connection, None),
            HttpError::Timeout => (FederationDkgDeliveryFailureKind::Timeout, None),
            HttpError::Serialization(_) | HttpError::ResponseTooLarge { .. } => {
                (FederationDkgDeliveryFailureKind::ResponseDecode, None)
            }
            HttpError::Redirect(_) | HttpError::Proxy(_) | HttpError::Build(_) => (
                FederationDkgDeliveryFailureKind::TransportConfiguration,
                None,
            ),
            HttpError::Other(_) => (FederationDkgDeliveryFailureKind::Transport, None),
        };
        return FederationDkgDeliveryFailure { kind, http_status };
    }
    if err.downcast_ref::<FederationError>().is_some() {
        return FederationDkgDeliveryFailure {
            kind: FederationDkgDeliveryFailureKind::ResponseValidation,
            http_status: None,
        };
    }

    FederationDkgDeliveryFailure {
        kind: FederationDkgDeliveryFailureKind::Unknown,
        http_status: None,
    }
}

pub(crate) async fn federation_dkg_readiness_required(
    state: &MintState,
    ceremony_id: FederationDkgCeremonyId,
) -> Result<bool, cdk::Error> {
    let participant_states = state
        .mint
        .localstore()
        .get_federation_dkg_participant_states()
        .await?;
    Ok(participant_states
        .iter()
        .all(|participant| participant.ceremony_id() != ceremony_id))
}

pub(crate) async fn run_federation_dkg_readiness_round<T>(
    client: &FederationRestPeerClient<T>,
    public_config: &FederationConfig,
    request: &FederationDkgReadinessRequest,
    acknowledged_members: &mut BTreeSet<MemberId>,
) -> DeliverySummary
where
    T: Transport,
{
    let targets = public_config
        .members
        .keys()
        .copied()
        .filter(|member_id| !acknowledged_members.contains(member_id));
    let results = join_all(targets.map(|member_id| {
        let member = public_config
            .member(member_id)
            .expect("configured DKG readiness target must exist");
        async move {
            (
                member.member_id,
                client.submit_dkg_readiness(member, request).await,
            )
        }
    }))
    .await;
    let mut delivery = DeliverySummary::default();
    let acknowledged = record_delivery_results("readiness", results, &mut delivery);
    acknowledged_members.extend(acknowledged);
    delivery
}

/// Return the roster members whose artifacts are still missing for one DKG
/// phase, so a timed-out ceremony can name who it is waiting on.
async fn missing_federation_dkg_members_for_phase(
    state: &MintState,
    ceremony_id: FederationDkgCeremonyId,
    phase: FederationDkgDriverPhase,
) -> Result<Vec<MemberId>, cdk::Error> {
    let runtime_config = state.mint.federation_config().ok_or_else(|| {
        cdk::Error::Custom("federation mint is missing its runtime config".to_string())
    })?;
    let members: Vec<MemberId> = runtime_config
        .public_config()
        .members
        .keys()
        .copied()
        .collect();
    let local_member = runtime_config.member_id();
    let db = state.mint.localstore();
    let missing = match phase {
        FederationDkgDriverPhase::Commitment => {
            let records = db.get_federation_dkg_commitments().await?;
            members
                .into_iter()
                .filter(|member| {
                    !records.iter().any(|record| {
                        record.ceremony_id == ceremony_id && record.member_id == *member
                    })
                })
                .collect()
        }
        FederationDkgDriverPhase::Reveal => {
            let records = db.get_federation_dkg_reveals().await?;
            members
                .into_iter()
                .filter(|member| {
                    !records.iter().any(|record| {
                        record.ceremony_id == ceremony_id && record.member_id == *member
                    })
                })
                .collect()
        }
        FederationDkgDriverPhase::SecretShares => {
            let records = db.get_federation_dkg_secret_share_contributions().await?;
            members
                .into_iter()
                .filter(|member| {
                    !records.iter().any(|record| {
                        record.ceremony_id == ceremony_id
                            && record.sender == *member
                            && record.receiver == local_member
                    })
                })
                .collect()
        }
        FederationDkgDriverPhase::TranscriptSignature => {
            let records = db.get_federation_dkg_transcript_signatures().await?;
            members
                .into_iter()
                .filter(|member| !records.iter().any(|record| record.signer == *member))
                .collect()
        }
        FederationDkgDriverPhase::Activation => {
            let records = db.get_federation_dkg_activations().await?;
            members
                .into_iter()
                .filter(|member| {
                    !records
                        .iter()
                        .any(|record| record.ceremony_id == ceremony_id && record.signer == *member)
                })
                .collect()
        }
        FederationDkgDriverPhase::FrostRound1 => {
            let records = db.get_federation_frost_dkg_round1().await?;
            members
                .into_iter()
                .filter(|member| {
                    !records.iter().any(|record| {
                        record.context.ceremony_id == ceremony_id && record.sender == *member
                    })
                })
                .collect()
        }
        FederationDkgDriverPhase::FrostRound2 => {
            let records = db.get_federation_frost_dkg_round2().await?;
            members
                .into_iter()
                .filter(|member| {
                    !records.iter().any(|record| {
                        record.context.ceremony_id == ceremony_id
                            && record.sender == *member
                            && record.receiver == local_member
                    })
                })
                .collect()
        }
        FederationDkgDriverPhase::FrostConfirmation => {
            let records = db.get_federation_frost_dkg_confirmations().await?;
            members
                .into_iter()
                .filter(|member| {
                    !records.iter().any(|record| {
                        record.context.ceremony_id == ceremony_id && record.member_id == *member
                    })
                })
                .collect()
        }
        FederationDkgDriverPhase::LocalKeysetShare | FederationDkgDriverPhase::Complete => {
            Vec::new()
        }
    };
    Ok(missing)
}

pub(crate) async fn wait_for_federation_dkg_readiness<T>(
    client: &FederationRestPeerClient<T>,
    public_config: &FederationConfig,
    request: &FederationDkgReadinessRequest,
    driver_config: FederationDkgDriverRunConfig,
) -> Result<usize, cdk::Error>
where
    T: Transport,
{
    let mut acknowledged_members = BTreeSet::new();
    for attempt in 1..=driver_config.max_ticks {
        let _ = run_federation_dkg_readiness_round(
            client,
            public_config,
            request,
            &mut acknowledged_members,
        )
        .await;
        if acknowledged_members.len() == public_config.members.len() {
            return Ok(attempt);
        }
        if attempt < driver_config.max_ticks {
            tokio::time::sleep(driver_config.retry_delay).await;
        }
    }

    Err(cdk::Error::Custom(format!(
        "federation DKG readiness exhausted after {} attempts with {}/{} members ready; \
         DKG requires all configured roster members to be online and reachable for the \
         entire ceremony — recover by operator roster reconfiguration and a new ceremony \
         with a new ceremony id, not by indefinite retry",
        driver_config.max_ticks,
        acknowledged_members.len(),
        public_config.members.len()
    )))
}

/// Persist a new local participant state or reload the exact existing ceremony state.
///
/// Random secret material is generated only when no state for `ceremony_id` is
/// durable. A restart therefore resumes the original contributions instead of
/// creating a conflicting polynomial.
pub(crate) async fn prepare_federation_dkg_participant_state(
    state: &MintState,
    ceremony_id: FederationDkgCeremonyId,
    keyset_spec: FederationDkgKeysetSpec,
) -> Result<ValidatedFederationDkgParticipantState, cdk::Error> {
    let runtime_config = state.mint.federation_config().ok_or_else(|| {
        cdk::Error::Custom("federation mint is missing its runtime config".to_string())
    })?;
    let public_config = runtime_config.public_config().clone();
    let local_member = runtime_config.member_id();
    if !public_config.keysets.is_empty() || public_config.setup_transcript.is_some() {
        return Err(FederationError::DkgTranscriptMismatch {
            reason:
                "live DKG setup requires a public config without finalized keysets or transcript"
                    .to_string(),
        }
        .into());
    }

    let db = state.mint.localstore();
    let participant_states = db.get_federation_dkg_participant_states().await?;
    if let Some(existing) = participant_states
        .into_iter()
        .find(|participant| participant.ceremony_id() == ceremony_id)
    {
        let validation_config = public_config.clone();
        let participant = crate::run_federation_dkg_cpu_task(
            "federation DKG participant validation task failed",
            move || ValidatedFederationDkgParticipantState::new(&validation_config, existing),
        )
        .await?;
        if participant.participant_state().member_id() != local_member {
            return Err(FederationError::MemberIdMismatch {
                expected: local_member,
                found: participant.participant_state().member_id(),
            }
            .into());
        }
        if participant.participant_state().keyset_spec() != &keyset_spec {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: local_member,
                reason: "persisted participant keyset policy does not match requested ceremony"
                    .to_string(),
            }
            .into());
        }
        return Ok(participant);
    }

    let generation_config = public_config.clone();
    let participant = crate::run_federation_dkg_cpu_task(
        "federation DKG participant generation task failed",
        move || {
            ValidatedFederationDkgParticipantState::generate(
                &generation_config,
                ceremony_id,
                local_member,
                keyset_spec,
            )
        },
    )
    .await?;
    let mut tx = db.begin_transaction().await?;
    tx.add_federation_dkg_participant_state(participant.participant_state())
        .await?;
    tx.commit().await?;
    Ok(participant)
}

/// Run one idempotent DKG tick over authenticated private REST.
///
/// Every phase is delivered to all configured members, including self. Phase
/// batches remain ordered, while deliveries within one phase run concurrently.
/// Later plans rebroadcast earlier artifacts, allowing lagging members to catch
/// up without a separate acknowledgement journal.
async fn run_frost_dkg_tick<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    public_config: &FederationConfig,
    private_config: &cdk::federation::MemberConfigPrivate,
    ceremony_id: FederationDkgCeremonyId,
    root_key_epoch: u64,
) -> Result<(FederationDkgDriverPhase, DeliverySummary), cdk::Error>
where
    T: Transport,
{
    let context = FederationFrostDkgContext::new(public_config, ceremony_id, root_key_epoch)?;
    let frost_context = context.to_frost_context()?;
    let local_member = private_config.member_id;
    let frost_member = cdk_frost::MemberId::new(local_member.get())
        .map_err(|error| cdk::Error::Custom(error.to_string()))?;
    let storage_key = frost_storage_key(private_config, &context)?;
    let db = state.mint.localstore();
    let roots = db.get_federation_frost_root_records().await?;
    let local_root = roots
        .iter()
        .find(|record| record.context == context && record.member_id == local_member);
    let mut delivery = DeliverySummary::default();

    if let Some(root) = local_root {
        let public = root.validate(public_config)?;
        let confirmation = FederationFrostDkgConfirmationRequest {
            context: context.clone(),
            member_id: local_member,
            public_package: public.encode(),
        };
        let members = public_config.members.values().cloned().collect::<Vec<_>>();
        let results = stream::iter(members.into_iter().map(|member| {
            let confirmation = confirmation.clone();
            async move {
                (
                    member.member_id,
                    client
                        .submit_frost_dkg_confirmation(&member, &confirmation)
                        .await,
                )
            }
        }))
        .buffer_unordered(FEDERATION_DKG_DELIVERY_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        record_delivery_results("FROST confirmation", results, &mut delivery);
        let confirmations = db.get_federation_frost_dkg_confirmations().await?;
        let matching = confirmations
            .iter()
            .filter(|candidate| candidate.context == context)
            .collect::<Vec<_>>();
        for candidate in &matching {
            let candidate_public = candidate.validate(public_config, candidate.member_id)?;
            if candidate_public.encode() != public.encode() {
                return Err(FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "FROST public-root confirmation mismatch from member {}",
                        candidate.member_id
                    ),
                }
                .into());
            }
        }
        return Ok((
            if matching.len() == public_config.members.len() {
                FederationDkgDriverPhase::Complete
            } else {
                FederationDkgDriverPhase::FrostConfirmation
            },
            delivery,
        ));
    }

    let states = db.get_federation_frost_dkg_local_states().await?;
    let local_states = states
        .iter()
        .filter(|record| record.context == context && record.member_id == local_member)
        .collect::<Vec<_>>();
    let round2_state = local_states
        .iter()
        .find(|record| record.phase == FederationFrostDkgLocalPhase::Round2)
        .copied();
    let round1_state = local_states
        .iter()
        .find(|record| record.phase == FederationFrostDkgLocalPhase::Round1)
        .copied();

    if round1_state.is_none() && round2_state.is_none() {
        let (secret, package) = start_dkg(frost_context, frost_member, OsRng)
            .map_err(|error| cdk::Error::Custom(error.to_string()))?;
        let encrypted = secret
            .seal(&storage_key, &mut OsRng)
            .map_err(|error| cdk::Error::Custom(error.to_string()))?;
        state
            .federation
            .persist_frost_dkg_local_state(&FederationFrostDkgLocalState {
                context,
                member_id: local_member,
                phase: FederationFrostDkgLocalPhase::Round1,
                encrypted_state: encrypted.to_bytes(),
            })
            .await?;
        let _ = package;
        return Ok((FederationDkgDriverPhase::FrostRound1, delivery));
    }

    if let Some(state_record) = round1_state.filter(|_| round2_state.is_none()) {
        state_record.validate(public_config)?;
        let encrypted = EncryptedSecret::from_bytes(state_record.encrypted_state.clone())
            .map_err(|error| cdk::Error::Custom(error.to_string()))?;
        let secret = DkgRound1Secret::open(
            frost_context.clone(),
            frost_member,
            &storage_key,
            &encrypted,
        )
        .map_err(|error| cdk::Error::Custom(error.to_string()))?;
        let own_package = secret.public_package();
        let request = FederationFrostDkgRound1Request {
            context: context.clone(),
            sender: local_member,
            package: own_package.encode(),
        };
        let members = public_config.members.values().cloned().collect::<Vec<_>>();
        let results = stream::iter(members.into_iter().map(|member| {
            let request = request.clone();
            async move {
                (
                    member.member_id,
                    client.submit_frost_dkg_round1(&member, &request).await,
                )
            }
        }))
        .buffer_unordered(FEDERATION_DKG_DELIVERY_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        record_delivery_results("FROST round one", results, &mut delivery);
        if delivery.failed != 0 || delivery.successful != public_config.members.len() {
            return Ok((FederationDkgDriverPhase::FrostRound1, delivery));
        }
        let requests = db.get_federation_frost_dkg_round1().await?;
        let received = requests
            .iter()
            .filter(|candidate| candidate.context == context && candidate.sender != local_member)
            .map(|candidate| candidate.decode_package(public_config, candidate.sender))
            .collect::<cdk::federation::Result<Vec<_>>>()?;
        if received.len() + 1 != public_config.members.len() {
            return Ok((FederationDkgDriverPhase::FrostRound1, delivery));
        }
        let (round2_secret, outgoing) =
            advance_dkg(secret, received).map_err(|error| cdk::Error::Custom(error.to_string()))?;
        let encrypted = round2_secret
            .seal(&storage_key, &mut OsRng)
            .map_err(|error| cdk::Error::Custom(error.to_string()))?;
        let mut tx = db.begin_transaction().await?;
        tx.set_federation_frost_dkg_local_state(&FederationFrostDkgLocalState {
            context: context.clone(),
            member_id: local_member,
            phase: FederationFrostDkgLocalPhase::Round2,
            encrypted_state: encrypted.to_bytes(),
        })
        .await?;
        for (receiver, package) in outgoing {
            let encrypted = package
                .seal(&storage_key, &mut OsRng)
                .map_err(|error| cdk::Error::Custom(error.to_string()))?;
            tx.add_federation_frost_dkg_round2(&FederationFrostDkgRound2Record {
                context: context.clone(),
                sender: local_member,
                receiver: MemberId::new(receiver.get())?,
                encrypted_package: encrypted.to_bytes(),
            })
            .await?;
        }
        tx.commit().await?;
        return Ok((FederationDkgDriverPhase::FrostRound2, delivery));
    }

    let state_record = round2_state.ok_or_else(|| {
        cdk::Error::Custom("FROST DKG has no restartable round-two state".to_string())
    })?;
    state_record.validate(public_config)?;
    let records = db.get_federation_frost_dkg_round2().await?;
    let outgoing = records
        .into_iter()
        .filter(|candidate| candidate.context == context && candidate.sender == local_member)
        .map(|record| {
            let package = record.open(public_config, &storage_key)?;
            Ok(FederationFrostDkgRound2Request {
                context: record.context,
                sender: record.sender,
                receiver: record.receiver,
                package: package.encode(),
            })
        })
        .collect::<cdk::federation::Result<Vec<_>>>()?;
    let outgoing_count = outgoing.len();
    let results = stream::iter(outgoing.into_iter().map(|request| async move {
        let member = public_config
            .member(request.receiver)
            .expect("validated FROST receiver must be configured");
        (
            member.member_id,
            client.submit_frost_dkg_round2(member, &request).await,
        )
    }))
    .buffer_unordered(FEDERATION_DKG_DELIVERY_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;
    record_delivery_results("FROST round two", results, &mut delivery);
    if delivery.failed != 0 || delivery.successful != outgoing_count {
        return Ok((FederationDkgDriverPhase::FrostRound2, delivery));
    }
    let records = db.get_federation_frost_dkg_round2().await?;
    let incoming = records
        .iter()
        .filter(|candidate| {
            candidate.context == context
                && candidate.receiver == local_member
                && candidate.sender != local_member
        })
        .map(|candidate| candidate.open(public_config, &storage_key))
        .collect::<cdk::federation::Result<Vec<_>>>()?;
    if incoming.len() + 1 != public_config.members.len() {
        return Ok((FederationDkgDriverPhase::FrostRound2, delivery));
    }
    let round1 = db
        .get_federation_frost_dkg_round1()
        .await?
        .into_iter()
        .filter(|candidate| candidate.context == context && candidate.sender != local_member)
        .map(|candidate| candidate.decode_package(public_config, candidate.sender))
        .collect::<cdk::federation::Result<Vec<DkgRound1Package>>>()?;
    let encrypted = EncryptedSecret::from_bytes(state_record.encrypted_state.clone())
        .map_err(|error| cdk::Error::Custom(error.to_string()))?;
    let secret = DkgRound2Secret::open(frost_context, frost_member, &storage_key, &encrypted)
        .map_err(|error| cdk::Error::Custom(error.to_string()))?;
    let root = finish_dkg(secret, round1, incoming)
        .map_err(|error| cdk::Error::Custom(error.to_string()))?;
    let public = root.public().clone();
    let encrypted_root = root
        .seal(&storage_key, &mut OsRng)
        .map_err(|error| cdk::Error::Custom(error.to_string()))?;
    state
        .federation
        .persist_frost_root_record(&FederationFrostRootRecord {
            context,
            member_id: local_member,
            public_package: public.encode(),
            encrypted_root_share: encrypted_root.to_bytes(),
        })
        .await?;
    Ok((FederationDkgDriverPhase::FrostConfirmation, delivery))
}

pub(crate) fn frost_storage_key(
    private_config: &cdk::federation::MemberConfigPrivate,
    context: &FederationFrostDkgContext,
) -> Result<SecretEncryptionKey, cdk::Error> {
    let mut hasher = Sha256::new();
    hasher.update(b"cdk/federation/frost-sealed-storage/v1");
    hasher.update(private_config.identity_secret_key.to_secret_bytes());
    hasher.update(context.federation_id.to_bytes());
    hasher.update(context.ceremony_id.to_bytes());
    hasher.update(context.root_key_epoch.to_be_bytes());
    hasher.update(private_config.member_id.get().to_be_bytes());
    SecretEncryptionKey::new(hasher.finalize().into())
        .map_err(|error| cdk::Error::Custom(error.to_string()))
}

#[cfg(test)]
pub(crate) async fn run_federation_dkg_driver_tick<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    participant: &ValidatedFederationDkgParticipantState,
    secret_share_transport: FederationDkgSecretShareTransportPolicy,
    delivery_tracker: &mut FederationDkgDeliveryTracker,
) -> Result<FederationDkgDriverTickOutcome, cdk::Error>
where
    T: Transport,
{
    let mut plan_cache = None;
    run_federation_dkg_driver_tick_with_cache(
        state,
        client,
        participant,
        secret_share_transport,
        &mut plan_cache,
        delivery_tracker,
    )
    .await
}

async fn run_federation_dkg_driver_tick_with_cache<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    participant: &ValidatedFederationDkgParticipantState,
    secret_share_transport: FederationDkgSecretShareTransportPolicy,
    plan_cache: &mut Option<FederationDkgPlanCache>,
    delivery_tracker: &mut FederationDkgDeliveryTracker,
) -> Result<FederationDkgDriverTickOutcome, cdk::Error>
where
    T: Transport,
{
    let runtime_config = state.mint.federation_config().ok_or_else(|| {
        cdk::Error::Custom("federation mint is missing its runtime config".to_string())
    })?;
    let public_config = runtime_config.public_config().clone();
    let private_config = runtime_config.private_config().clone();
    validate_federation_dkg_secret_share_transport(&public_config, secret_share_transport)?;

    let ceremony_id = participant.participant_state().ceremony_id();
    let root_key_epoch = state
        .federation
        .dkg_auto_start_config()
        .ok_or_else(|| cdk::Error::Custom("FROST DKG configuration is missing".to_string()))?
        .root_key_epoch;
    let records = load_federation_dkg_driver_records(state, ceremony_id).await?;
    let plan = plan_federation_dkg_records(
        records,
        &public_config,
        &private_config,
        participant,
        plan_cache,
    )
    .await?;
    if let Some(finalized) = plan.finalized_member_config.as_ref() {
        state
            .remember_federation_dkg_finalized_member_config(finalized.clone())
            .await?;
    }
    let phase_before = plan.phase;
    let mut delivery = DeliverySummary::default();
    let (frost_phase, frost_delivery) = run_frost_dkg_tick(
        state,
        client,
        &public_config,
        &private_config,
        ceremony_id,
        root_key_epoch,
    )
    .await?;
    delivery.attempted = delivery.attempted.saturating_add(frost_delivery.attempted);
    delivery.successful = delivery
        .successful
        .saturating_add(frost_delivery.successful);
    delivery.failed = delivery.failed.saturating_add(frost_delivery.failed);
    delivery
        .failed_members
        .extend(frost_delivery.failed_members);
    let secret_shares_ready_before_tick =
        secret_shares_acknowledged_by_all(&delivery_tracker.secret_shares, &plan.secret_shares);
    let transcript_ready_before_tick = plan.transcript_signature.as_ref().is_some_and(|request| {
        broadcast_acknowledged_by_all(
            &delivery_tracker.transcript_signature,
            request,
            public_config.members.keys().copied(),
        )
    });

    if let Some(request) = plan.commitment.as_ref() {
        let targets = pending_broadcast_targets(
            &mut delivery_tracker.commitment,
            request,
            public_config.members.keys().copied(),
        );
        let results = stream::iter(targets.into_iter().map(|member_id| {
            let member = public_config
                .member(member_id)
                .expect("tracked DKG commitment receiver must be configured");
            async move {
                (
                    member.member_id,
                    client.submit_dkg_commitment(member, request).await,
                )
            }
        }))
        .buffer_unordered(FEDERATION_DKG_DELIVERY_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        let acknowledged = record_delivery_results("commitment", results, &mut delivery);
        record_broadcast_acknowledgements(&mut delivery_tracker.commitment, request, &acknowledged);
    }

    if let Some(request) = plan.reveal.as_ref() {
        let targets = pending_broadcast_targets(
            &mut delivery_tracker.reveal,
            request,
            public_config.members.keys().copied(),
        );
        let results = stream::iter(targets.into_iter().map(|member_id| {
            let member = public_config
                .member(member_id)
                .expect("tracked DKG reveal receiver must be configured");
            async move {
                (
                    member.member_id,
                    client.submit_dkg_reveal(member, request).await,
                )
            }
        }))
        .buffer_unordered(FEDERATION_DKG_DELIVERY_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        let acknowledged = record_delivery_results("reveal", results, &mut delivery);
        record_broadcast_acknowledgements(&mut delivery_tracker.reveal, request, &acknowledged);
    }

    if !plan.secret_shares.is_empty() {
        let targets = plan
            .secret_shares
            .iter()
            .filter(|(receiver, request)| {
                delivery_tracker.secret_shares.get(receiver) != Some(*request)
            })
            .map(|(receiver, _)| *receiver)
            .collect::<Vec<_>>();
        let deliveries = targets.into_iter().map(|receiver| {
            let member = public_config
                .member(receiver)
                .expect("validated DKG plan receiver must be configured");
            let request = plan
                .secret_shares
                .get(&receiver)
                .expect("validated DKG plan request must be configured")
                .clone();
            async move {
                (
                    member.member_id,
                    client.submit_dkg_secret_share(member, &request).await,
                )
            }
        });
        let results = stream::iter(deliveries)
            .buffer_unordered(FEDERATION_DKG_DELIVERY_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        let acknowledged = record_delivery_results("secret share", results, &mut delivery);
        for member_id in acknowledged {
            if let Some(request) = plan.secret_shares.get(&member_id) {
                delivery_tracker
                    .secret_shares
                    .insert(member_id, request.clone());
            }
        }
    }

    if let Some(keyset_share) = plan.derived_keyset_share.as_ref() {
        persist_federation_dkg_member_keyset_share(state, keyset_share).await?;
    }

    if let (true, Some(request)) = (
        secret_shares_ready_before_tick,
        plan.transcript_signature.as_ref(),
    ) {
        let targets = pending_broadcast_targets(
            &mut delivery_tracker.transcript_signature,
            request,
            public_config.members.keys().copied(),
        );
        let results = stream::iter(targets.into_iter().map(|member_id| {
            let member = public_config
                .member(member_id)
                .expect("tracked DKG transcript receiver must be configured");
            async move {
                (
                    member.member_id,
                    client
                        .submit_dkg_transcript_signature(member, request)
                        .await,
                )
            }
        }))
        .buffer_unordered(FEDERATION_DKG_DELIVERY_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        let acknowledged = record_delivery_results("transcript signature", results, &mut delivery);
        record_broadcast_acknowledgements(
            &mut delivery_tracker.transcript_signature,
            request,
            &acknowledged,
        );
    }

    if let (true, true, Some(request)) = (
        transcript_ready_before_tick,
        frost_phase == FederationDkgDriverPhase::Complete,
        plan.activation.as_ref(),
    ) {
        let targets = pending_broadcast_targets(
            &mut delivery_tracker.activation,
            request,
            public_config.members.keys().copied(),
        );
        let results = stream::iter(targets.into_iter().map(|member_id| {
            let member = public_config
                .member(member_id)
                .expect("tracked DKG activation receiver must be configured");
            async move {
                (
                    member.member_id,
                    client.submit_dkg_activation(member, request).await,
                )
            }
        }))
        .buffer_unordered(FEDERATION_DKG_DELIVERY_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        let acknowledged = record_delivery_results("activation", results, &mut delivery);
        record_broadcast_acknowledgements(&mut delivery_tracker.activation, request, &acknowledged);
    }

    let records = load_federation_dkg_driver_records(state, ceremony_id).await?;
    let next_plan = plan_federation_dkg_records(
        records,
        &public_config,
        &private_config,
        participant,
        plan_cache,
    )
    .await?;
    let phase_after = if next_plan.phase == FederationDkgDriverPhase::Complete
        && frost_phase != FederationDkgDriverPhase::Complete
    {
        frost_phase
    } else {
        next_plan.phase
    };
    Ok(FederationDkgDriverTickOutcome {
        phase_before,
        phase_after,
        progress: next_plan.progress,
        attempted_deliveries: delivery.attempted,
        successful_deliveries: delivery.successful,
        failed_deliveries: delivery.failed,
        failed_members: delivery.failed_members.into_iter().collect(),
    })
}

async fn plan_federation_dkg_records(
    records: FederationDkgDriverRecords,
    public_config: &FederationConfig,
    private_config: &cdk::federation::MemberConfigPrivate,
    participant: &ValidatedFederationDkgParticipantState,
    plan_cache: &mut Option<FederationDkgPlanCache>,
) -> Result<FederationDkgDriverPlan, cdk::Error> {
    if let Some(cached) = plan_cache.as_ref() {
        if cached.records == records {
            return Ok(cached.plan.clone());
        }
    }

    let records_for_plan = records.clone();
    let previous = plan_cache.clone();
    let public_config = public_config.clone();
    let private_config = private_config.clone();
    let participant = participant.clone();
    let plan =
        crate::run_federation_dkg_cpu_task("federation DKG planner task failed", move || {
            if let Some(cached) = previous.as_ref() {
                if let Some(plan) = records_for_plan.replan_activation_only_change(
                    &public_config,
                    &cached.records,
                    &cached.plan,
                )? {
                    return Ok(plan);
                }
            }
            records_for_plan.plan_with_validated_participant(
                &public_config,
                &private_config,
                &participant,
            )
        })
        .await?;
    *plan_cache = Some(FederationDkgPlanCache {
        records,
        plan: plan.clone(),
    });
    Ok(plan)
}

/// Run a bounded number of idempotent DKG ticks with the default HTTP transport.
pub(crate) async fn run_federation_dkg_driver(
    state: &MintState,
    ceremony_id: FederationDkgCeremonyId,
    keyset_spec: FederationDkgKeysetSpec,
    sync_config: FederationPeerSyncConfig,
    driver_config: FederationDkgDriverRunConfig,
) -> Result<FederationDkgDriverRunOutcome, cdk::Error> {
    driver_config.validate()?;
    let runtime_config = state.mint.federation_config().ok_or_else(|| {
        cdk::Error::Custom("federation mint is missing its runtime config".to_string())
    })?;
    let public_config = runtime_config.public_config().clone();
    let private_config = runtime_config.private_config().clone();
    let local_member = private_config.member_id;
    validate_federation_dkg_secret_share_transport(
        &public_config,
        driver_config.secret_share_transport,
    )?;
    let client = FederationRestPeerClient::new_with_validated_config(
        state.federation.transport.clone(),
        &public_config,
        private_config,
        sync_config,
    )
    .map_err(|err| cdk::Error::Custom(err.to_string()))?;
    if federation_dkg_readiness_required(state, ceremony_id).await? {
        let readiness = FederationDkgReadinessRequest::new(
            public_config.federation_id,
            public_config.setup_authorization,
            ceremony_id,
            keyset_spec.clone(),
        );
        let readiness_attempts =
            wait_for_federation_dkg_readiness(&client, &public_config, &readiness, driver_config)
                .await?;
        tracing::info!(
            member_id = %local_member,
            %ceremony_id,
            readiness_attempts,
            "Federation DKG all-member readiness barrier completed"
        );
    }
    let participant =
        prepare_federation_dkg_participant_state(state, ceremony_id, keyset_spec).await?;

    run_federation_dkg_driver_loop_with_client(
        state,
        &client,
        &participant,
        ceremony_id,
        driver_config,
    )
    .await
}

/// Run the bounded DKG tick loop with an injected authenticated client.
///
/// DKG requires all configured roster members for the entire ceremony: the
/// loop stops after `max_ticks` or `ceremony_timeout`, and ending without
/// completion aborts with an error naming the members it is still waiting
/// on plus the roster-reconfiguration remedy.
pub(crate) async fn run_federation_dkg_driver_loop_with_client<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    participant: &ValidatedFederationDkgParticipantState,
    ceremony_id: FederationDkgCeremonyId,
    driver_config: FederationDkgDriverRunConfig,
) -> Result<FederationDkgDriverRunOutcome, cdk::Error>
where
    T: Transport,
{
    let runtime_config = state.mint.federation_config().ok_or_else(|| {
        cdk::Error::Custom("federation mint is missing its runtime config".to_string())
    })?;
    let local_member = runtime_config.member_id();
    let mut plan_cache = None;
    let mut delivery_tracker = FederationDkgDeliveryTracker::default();
    let ceremony_started_at = tokio::time::Instant::now();
    let mut last_tick = run_federation_dkg_driver_tick_with_cache(
        state,
        client,
        participant,
        driver_config.secret_share_transport,
        &mut plan_cache,
        &mut delivery_tracker,
    )
    .await?;
    let mut ticks = 1;
    let mut delivery_totals = FederationDkgDeliveryTotals::default();
    delivery_totals.record(&last_tick);
    trace_federation_dkg_driver_tick(local_member, &ceremony_id, ticks, &last_tick);
    while !last_tick.is_complete()
        && ticks < driver_config.max_ticks
        && ceremony_started_at.elapsed() < driver_config.ceremony_timeout
    {
        let stop = state.federation.tasks.stop_token();
        tokio::select! {
            _ = stop.cancelled() => return Err(cdk::Error::Custom("federation DKG shutdown requested".to_string())),
            _ = tokio::time::sleep(driver_config.retry_delay) => {}
        }

        last_tick = run_federation_dkg_driver_tick_with_cache(
            state,
            client,
            participant,
            driver_config.secret_share_transport,
            &mut plan_cache,
            &mut delivery_tracker,
        )
        .await?;
        ticks = ticks.saturating_add(1);
        delivery_totals.record(&last_tick);
        trace_federation_dkg_driver_tick(local_member, &ceremony_id, ticks, &last_tick);
    }

    if last_tick.is_complete() {
        complete_federation_dkg_ceremony(state, ceremony_id).await?;
        return Ok(FederationDkgDriverRunOutcome {
            ticks,
            attempted_deliveries: delivery_totals.attempted,
            successful_deliveries: delivery_totals.successful,
            failed_deliveries: delivery_totals.failed,
            last_tick,
        });
    }

    // DKG requires all configured roster members for the entire ceremony.
    // A bounded run that ends without completion must name the members the
    // ceremony is still waiting on so the operator can reconfigure the
    // roster instead of retrying forever.
    let missing_members =
        missing_federation_dkg_members_for_phase(state, ceremony_id, last_tick.phase_after).await?;
    let elapsed = ceremony_started_at.elapsed();
    let missing_display = missing_members
        .iter()
        .map(MemberId::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let abort_reason = if missing_members.is_empty() {
        "no outstanding member artifacts (delivery in flight)".to_string()
    } else {
        format!("still waiting on member(s): {missing_display}")
    };
    let abort_message = format!(
        "federation DKG ceremony {ceremony_id} aborted after {:.1}s in phase {:?} without \
         completion ({abort_reason}); DKG requires all configured roster members for the \
         entire ceremony — recover by operator roster reconfiguration and a new ceremony \
         with a new ceremony id, not by indefinite retry",
        elapsed.as_secs_f64(),
        last_tick.phase_after,
    );
    tracing::error!(
        member_id = %local_member,
        %ceremony_id,
        phase = ?last_tick.phase_after,
        missing_members = %missing_display,
        ticks,
        "Federation DKG ceremony aborted: {}",
        abort_message
    );
    Err(cdk::Error::Custom(abort_message))
}

pub(crate) fn spawn_federation_dkg_driver(
    state: MintState,
    sync_config: FederationPeerSyncConfig,
    config: FederationDkgAutoStartConfig,
) {
    let local_member = state
        .mint
        .federation_config()
        .expect("validated DKG auto-start has a federation runtime config")
        .member_id();
    let tasks = state.federation.tasks.clone();
    let failures = tasks.clone();
    tasks.spawn("DKG", false, async move {
        match run_federation_dkg_driver(
            &state,
            config.ceremony_id,
            config.keyset_spec,
            sync_config,
            config.driver,
        )
        .await
        {
            Ok(outcome) if outcome.is_complete() => {
                tracing::info!(
                    %local_member,
                    ceremony_id = %config.ceremony_id,
                    ticks = outcome.ticks,
                    attempted_deliveries = outcome.attempted_deliveries,
                    successful_deliveries = outcome.successful_deliveries,
                    failed_deliveries = outcome.failed_deliveries,
                    "Federation DKG ceremony completed; restart this member to activate the finalized keyset"
                );
            }
            Ok(outcome) => {
                tracing::warn!(
                    %local_member,
                    ceremony_id = %config.ceremony_id,
                    ticks = outcome.ticks,
                    attempted_deliveries = outcome.attempted_deliveries,
                    successful_deliveries = outcome.successful_deliveries,
                    failed_deliveries = outcome.failed_deliveries,
                    "Federation DKG driver exhausted its bounded run before completion"
                );
                failures.fail("federation DKG driver did not complete".to_string());
            }
            Err(err) => {
                tracing::error!(
                    %local_member,
                    ceremony_id = %config.ceremony_id,
                    "Federation DKG driver stopped: {}",
                    redact_federation_operator_message(&err.to_string())
                );
                failures.fail("federation DKG driver failed; inspect the ceremony status".to_string());
            }
        }
    });
}

fn validate_federation_dkg_secret_share_transport(
    public_config: &FederationConfig,
    policy: FederationDkgSecretShareTransportPolicy,
) -> Result<(), cdk::Error> {
    for member in public_config.members.values() {
        let url = url::Url::parse(&member.federation_api_url.to_string())
            .map_err(|err| cdk::Error::Custom(err.to_string()))?;
        let accepted = match url.scheme() {
            "https" | "iroh" => true,
            "http" if policy == FederationDkgSecretShareTransportPolicy::AllowInsecureLocalhost => {
                is_loopback_url(&url)
            }
            _ => false,
        };
        if !accepted {
            return Err(cdk::Error::Custom(format!(
                "federation DKG secret-share transport for member {} requires HTTPS or Iroh; plain HTTP is allowed only for explicit loopback development mode",
                member.member_id
            )));
        }
    }

    Ok(())
}

fn is_loopback_url(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

async fn load_federation_dkg_driver_records(
    state: &MintState,
    ceremony_id: FederationDkgCeremonyId,
) -> Result<FederationDkgDriverRecords, cdk::Error> {
    let db = state.mint.localstore();
    let mut commitments = db.get_federation_dkg_commitments().await?;
    commitments.retain(|commitment| commitment.ceremony_id == ceremony_id);
    let mut reveals = db.get_federation_dkg_reveals().await?;
    reveals.retain(|reveal| reveal.ceremony_id == ceremony_id);
    let mut secret_share_contributions = db.get_federation_dkg_secret_share_contributions().await?;
    secret_share_contributions.retain(|contribution| contribution.ceremony_id == ceremony_id);
    let mut keyset_shares = db.get_federation_dkg_member_keyset_shares().await?;
    keyset_shares.retain(|share| share.public_ceremony.ceremony_id() == ceremony_id);
    let mut activations = db.get_federation_dkg_activations().await?;
    activations.retain(|activation| activation.ceremony_id == ceremony_id);
    Ok(FederationDkgDriverRecords {
        commitments,
        reveals,
        secret_share_contributions,
        keyset_shares,
        transcript_signatures: db.get_federation_dkg_transcript_signatures().await?,
        activations,
    })
}

async fn persist_federation_dkg_member_keyset_share(
    state: &MintState,
    keyset_share: &FederationDkgMemberKeysetShare,
) -> Result<(), cdk::Error> {
    let db = state.mint.localstore();
    let mut tx = db.begin_transaction().await?;
    tx.add_federation_dkg_member_keyset_share(keyset_share)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn complete_federation_dkg_ceremony(
    state: &MintState,
    ceremony_id: FederationDkgCeremonyId,
) -> Result<(), cdk::Error> {
    let db = state.mint.localstore();
    let runtime_config = state.mint.federation_config().ok_or_else(|| {
        cdk::Error::Custom("federation mint is missing its runtime config".to_string())
    })?;
    let public_config = runtime_config.public_config();
    let local_member = runtime_config.member_id();
    let root_key_epoch = state
        .federation
        .dkg_auto_start_config()
        .filter(|config| config.ceremony_id == ceremony_id)
        .ok_or_else(|| {
            cdk::Error::Custom(
                "dual DKG completion requires matching FROST ceremony configuration".to_string(),
            )
        })?
        .root_key_epoch;
    let frost_context = FederationFrostDkgContext::new(public_config, ceremony_id, root_key_epoch)?;
    let frost_root = db
        .get_federation_frost_root_records()
        .await?
        .into_iter()
        .find(|record| record.context == frost_context && record.member_id == local_member)
        .ok_or_else(|| {
            cdk::Error::Custom(
                "dual DKG completion requires the local sealed FROST root share".to_string(),
            )
        })?;
    let frost_public_package = frost_root.validate(public_config)?;
    let storage_key = frost_storage_key(runtime_config.private_config(), &frost_context)?;
    let encrypted_root = EncryptedSecret::from_bytes(frost_root.encrypted_root_share.clone())
        .map_err(|error| cdk::Error::Custom(error.to_string()))?;
    CanonicalRootKeyPackage::open(
        cdk_frost::MemberId::new(local_member.get())
            .map_err(|error| cdk::Error::Custom(error.to_string()))?,
        frost_public_package.clone(),
        &storage_key,
        &encrypted_root,
    )
    .map_err(|error| cdk::Error::Custom(error.to_string()))?;
    let frost_public = frost_public_package.encode();
    let confirmations = db
        .get_federation_frost_dkg_confirmations()
        .await?
        .into_iter()
        .filter(|confirmation| confirmation.context == frost_context)
        .collect::<Vec<_>>();
    let mut confirmed_members = BTreeSet::new();
    for confirmation in confirmations {
        let public = confirmation.validate(public_config, confirmation.member_id)?;
        if public.encode() != frost_public || !confirmed_members.insert(confirmation.member_id) {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: format!(
                    "conflicting or duplicate FROST activation confirmation from member {}",
                    confirmation.member_id
                ),
            }
            .into());
        }
    }
    if confirmed_members != public_config.members.keys().copied().collect() {
        return Err(FederationError::DkgTranscriptMismatch {
            reason: "dual DKG completion requires matching FROST confirmation from every member"
                .to_string(),
        }
        .into());
    }
    let bls_results = db
        .get_federation_dkg_member_keyset_shares()
        .await?
        .into_iter()
        .filter(|share| {
            share.member_id == local_member && share.public_ceremony.ceremony_id() == ceremony_id
        })
        .collect::<Vec<_>>();
    if bls_results.is_empty() {
        return Err(FederationError::DkgTranscriptMismatch {
            reason: "dual DKG completion requires a local BLS keyset result".to_string(),
        }
        .into());
    }
    for bls in bls_results {
        cdk::federation::FederationDualDkgResult {
            bls,
            frost: frost_root.clone(),
        }
        .validate(public_config)?;
    }
    let root_public = FederationFrostRootPublicConfig::from_dkg_record(public_config, &frost_root)?;
    let root_private =
        FederationFrostRootPrivateConfig::from_dkg_record(&root_public, &frost_root)?;
    let ceremonies = db.get_federation_dkg_ceremonies().await?;
    let ceremony = ceremonies
        .iter()
        .find(|record| record.ceremony_id == ceremony_id);
    let mut tx = db.begin_transaction().await?;
    tx.add_federation_frost_root_public_config(&root_public)
        .await?;
    tx.add_federation_frost_root_private_config(&root_private)
        .await?;
    let Some(ceremony) = ceremony else {
        tx.commit().await?;
        return Ok(());
    };
    match ceremony.state {
        FederationDkgCeremonyState::Complete | FederationDkgCeremonyState::Active => {
            if ceremony.state == FederationDkgCeremonyState::Active {
                let completed = ceremony.completed()?;
                tx.transition_federation_dkg_ceremony(ceremony, &completed)
                    .await?;
            }
            tx.commit().await?;
            Ok(())
        }
        FederationDkgCeremonyState::Aborted => Err(FederationError::DkgTranscriptMismatch {
            reason: format!("aborted DKG ceremony {ceremony_id} cannot complete"),
        }
        .into()),
    }
}

fn record_delivery_results<R>(
    phase: &'static str,
    results: Vec<(MemberId, anyhow::Result<R>)>,
    summary: &mut DeliverySummary,
) -> BTreeSet<MemberId> {
    let mut acknowledged_members = BTreeSet::new();
    for (member_id, result) in results {
        match result {
            Ok(_) => {
                summary.record(member_id, true);
                acknowledged_members.insert(member_id);
            }
            Err(err) => {
                summary.record(member_id, false);
                let failure = classify_federation_dkg_delivery_failure(&err);
                match failure.http_status {
                    Some(http_status) => tracing::warn!(
                        %member_id,
                        phase,
                        failure_kind = failure.kind.as_str(),
                        http_status,
                        "Federation DKG delivery failed and will be retried"
                    ),
                    None => tracing::warn!(
                        %member_id,
                        phase,
                        failure_kind = failure.kind.as_str(),
                        "Federation DKG delivery failed and will be retried"
                    ),
                }
            }
        }
    }
    acknowledged_members
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::str::FromStr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use cdk::federation::{MemberConfigPublic, ThresholdParams};
    use cdk::mint_url::MintUrl;
    use cdk::nuts::{CurrencyUnit, SecretKey};
    use cdk::Amount;
    use tower::ServiceExt;

    use super::*;

    fn auto_start_config(
        federation_api_url: &str,
        policy: FederationDkgSecretShareTransportPolicy,
    ) -> (FederationConfig, FederationDkgAutoStartConfig) {
        let member_id = MemberId::new(1).expect("member ID");
        let identity = SecretKey::from_slice(&[1; 32]).expect("identity key");
        let member = MemberConfigPublic {
            member_id,
            public_mint_url: MintUrl::from_str("https://mint.example.com").expect("mint URL"),
            federation_api_url: MintUrl::from_str(federation_api_url).expect("federation API URL"),
            identity_public_key: identity.public_key(),
        };
        let peer_id = MemberId::new(2).expect("peer ID");
        let peer_identity = SecretKey::from_slice(&[2; 32]).expect("peer identity key");
        let peer = MemberConfigPublic {
            member_id: peer_id,
            public_mint_url: MintUrl::from_str("https://peer-mint.example.com")
                .expect("peer mint URL"),
            federation_api_url: MintUrl::from_str("https://peer-fed.example.com")
                .expect("peer federation API URL"),
            identity_public_key: peer_identity.public_key(),
        };
        let public_config = FederationConfig::new(
            cdk_common::federation::FederationSetupAuthorizationHash::from_bytes([1; 32]),
            ThresholdParams::with_bft_consensus(2, 1).expect("thresholds"),
            BTreeMap::from([(member_id, member), (peer_id, peer)]),
            Vec::new(),
        )
        .expect("setup config");
        let config = FederationDkgAutoStartConfig {
            setup_authorization: public_config.setup_authorization,
            ceremony_id: FederationDkgCeremonyId::from_bytes([7; 32]),
            root_key_epoch: 1,
            keyset_spec: FederationDkgKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeSet::from([Amount::from(1)]),
            )
            .expect("keyset spec"),
            driver: FederationDkgDriverRunConfig {
                secret_share_transport: policy,
                ..FederationDkgDriverRunConfig::default()
            },
        };
        (public_config, config)
    }

    fn cpu_responsiveness_fixture() -> (
        FederationConfig,
        FederationDkgCeremonyId,
        FederationDkgKeysetSpec,
        MemberId,
    ) {
        let members = (1..=5_u16)
            .map(|value| {
                let member_id = MemberId::new(value).expect("member ID");
                let byte = u8::try_from(value).expect("test member byte");
                let identity = SecretKey::from_slice(&[byte; 32]).expect("identity key");
                let member = MemberConfigPublic {
                    member_id,
                    public_mint_url: MintUrl::from_str(&format!(
                        "https://mint-{value}.example.invalid"
                    ))
                    .expect("mint URL"),
                    federation_api_url: MintUrl::from_str(&format!(
                        "https://fed-{value}.example.invalid"
                    ))
                    .expect("federation URL"),
                    identity_public_key: identity.public_key(),
                };
                (member_id, member)
            })
            .collect();
        let public_config = FederationConfig::new(
            cdk_common::federation::FederationSetupAuthorizationHash::from_bytes([1; 32]),
            ThresholdParams::with_bft_consensus(5, 3).expect("thresholds"),
            members,
            Vec::new(),
        )
        .expect("setup config");
        let ceremony_id = FederationDkgCeremonyId::from_bytes([19; 32]);
        let keyset_spec = FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            BTreeSet::from([Amount::from(1)]),
        )
        .expect("keyset spec");
        (
            public_config,
            ceremony_id,
            keyset_spec,
            MemberId::new(1).expect("member ID"),
        )
    }

    async fn measure_generation_probe(
        use_cpu_worker: bool,
    ) -> (Duration, Duration, ValidatedFederationDkgParticipantState) {
        let (public_config, ceremony_id, keyset_spec, member_id) = cpu_responsiveness_fixture();
        let generation_config = public_config.clone();
        let generation = async move {
            let started = Instant::now();
            let participant = if use_cpu_worker {
                crate::run_federation_dkg_cpu_task("test DKG generation failed", move || {
                    ValidatedFederationDkgParticipantState::generate(
                        &generation_config,
                        ceremony_id,
                        member_id,
                        keyset_spec,
                    )
                })
                .await
                .expect("worker generation")
            } else {
                ValidatedFederationDkgParticipantState::generate(
                    &generation_config,
                    ceremony_id,
                    member_id,
                    keyset_spec,
                )
                .expect("direct generation")
            };
            (started.elapsed(), participant)
        };
        let probe_started = Instant::now();
        let probe = async move {
            tokio::task::yield_now().await;
            let response = Router::new()
                .route("/probe", get(|| async { StatusCode::NO_CONTENT }))
                .oneshot(
                    Request::builder()
                        .uri("/probe")
                        .body(axum::body::Body::empty())
                        .expect("probe request"),
                )
                .await
                .expect("probe response");
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            probe_started.elapsed()
        };
        let ((generation_elapsed, participant), probe_elapsed) = tokio::join!(generation, probe);
        participant
            .participant_state()
            .validate(&public_config)
            .expect("generated participant validates");
        (generation_elapsed, probe_elapsed, participant)
    }

    #[test]
    fn run_config_rejects_unbounded_values() {
        assert!(FederationDkgDriverRunConfig {
            max_ticks: 0,
            retry_delay: Duration::from_secs(1),
            ..FederationDkgDriverRunConfig::default()
        }
        .validate()
        .is_err());
        assert!(FederationDkgDriverRunConfig {
            max_ticks: 1,
            retry_delay: Duration::ZERO,
            ..FederationDkgDriverRunConfig::default()
        }
        .validate()
        .is_err());
        assert!(FederationDkgDriverRunConfig {
            member_request_burst: 0,
            ..FederationDkgDriverRunConfig::default()
        }
        .validate()
        .is_err());
        assert!(FederationDkgDriverRunConfig {
            member_request_burst: 65,
            ..FederationDkgDriverRunConfig::default()
        }
        .validate()
        .is_err());
        assert!(FederationDkgDriverRunConfig {
            member_request_refill_interval: Duration::from_millis(9),
            ..FederationDkgDriverRunConfig::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn dkg_delivery_failure_classification_is_structured_and_redacted() {
        let member_id = MemberId::new(1).expect("member ID");
        let timeout = anyhow::Error::new(FederationRestPeerClientError::RequestTimeout {
            member_id,
            operation: "test operation".to_string(),
            timeout: Duration::from_secs(1),
        });
        assert_eq!(
            classify_federation_dkg_delivery_failure(&timeout).kind,
            FederationDkgDeliveryFailureKind::Timeout
        );

        let outbound = anyhow::Error::new(FederationRestPeerClientError::OutboundAdmissionFull {
            member_id,
            operation: "test operation".to_string(),
        });
        assert_eq!(
            classify_federation_dkg_delivery_failure(&outbound).kind,
            FederationDkgDeliveryFailureKind::OutboundAdmission
        );

        let cases = [
            (
                HttpError::Redirect("private redirect target".to_string()),
                FederationDkgDeliveryFailureKind::TransportConfiguration,
                None,
            ),
            (
                HttpError::Connection("private connection detail".to_string()),
                FederationDkgDeliveryFailureKind::Connection,
                None,
            ),
            (
                HttpError::Timeout,
                FederationDkgDeliveryFailureKind::Timeout,
                None,
            ),
            (
                HttpError::Status {
                    status: 429,
                    message: "private admission detail".to_string(),
                },
                FederationDkgDeliveryFailureKind::RemoteAdmission,
                Some(429),
            ),
            (
                HttpError::Status {
                    status: 425,
                    message: "private readiness detail".to_string(),
                },
                FederationDkgDeliveryFailureKind::RemoteNotReady,
                Some(425),
            ),
            (
                HttpError::Status {
                    status: 400,
                    message: "private validation detail".to_string(),
                },
                FederationDkgDeliveryFailureKind::RemoteValidation,
                Some(400),
            ),
            (
                HttpError::Status {
                    status: 500,
                    message: "private internal detail".to_string(),
                },
                FederationDkgDeliveryFailureKind::RemoteServer,
                Some(500),
            ),
            (
                HttpError::Status {
                    status: 503,
                    message: "private storage path and payload".to_string(),
                },
                FederationDkgDeliveryFailureKind::RemoteStorage,
                Some(503),
            ),
            (
                HttpError::Serialization("private response body".to_string()),
                FederationDkgDeliveryFailureKind::ResponseDecode,
                None,
            ),
            (
                HttpError::Proxy("private proxy address".to_string()),
                FederationDkgDeliveryFailureKind::TransportConfiguration,
                None,
            ),
            (
                HttpError::Other("private transport detail".to_string()),
                FederationDkgDeliveryFailureKind::Transport,
                None,
            ),
        ];
        for (error, expected_kind, expected_status) in cases {
            let failure = classify_federation_dkg_delivery_failure(&anyhow::Error::new(error));
            assert_eq!(failure.kind, expected_kind);
            assert_eq!(failure.http_status, expected_status);
            assert!(!format!("{failure:?}").contains("private"));
        }

        let validation = anyhow::Error::new(FederationError::DkgTranscriptMismatch {
            reason: "private response detail".to_string(),
        });
        let failure = classify_federation_dkg_delivery_failure(&validation);
        assert_eq!(
            failure.kind,
            FederationDkgDeliveryFailureKind::ResponseValidation
        );
        assert!(!format!("{failure:?}").contains("private"));

        let unknown = anyhow::anyhow!("private unknown detail");
        let failure = classify_federation_dkg_delivery_failure(&unknown);
        assert_eq!(failure.kind, FederationDkgDeliveryFailureKind::Unknown);
        assert!(!format!("{failure:?}").contains("private"));
    }

    #[test]
    fn secret_share_transport_requires_secure_scheme_or_explicit_loopback() {
        let https = url::Url::parse("https://member.example.com").expect("HTTPS URL");
        let loopback_v4 = url::Url::parse("http://127.0.0.1:8080").expect("IPv4 URL");
        let loopback_v6 = url::Url::parse("http://[::1]:8080").expect("IPv6 URL");
        let localhost = url::Url::parse("http://localhost:8080").expect("localhost URL");
        let remote_http = url::Url::parse("http://member.example.com").expect("remote HTTP URL");

        assert!(!is_loopback_url(&https));
        assert!(is_loopback_url(&loopback_v4));
        assert!(is_loopback_url(&loopback_v6));
        assert!(is_loopback_url(&localhost));
        assert!(!is_loopback_url(&remote_http));
    }

    #[test]
    fn auto_start_transport_policy_fails_closed_before_ceremony() {
        let (https, require_https) = auto_start_config(
            "https://member.example.com",
            FederationDkgSecretShareTransportPolicy::RequireSecureTransport,
        );
        require_https
            .validate_for_public_config(&https)
            .expect("HTTPS is accepted");

        let (iroh, require_secure) = auto_start_config(
            "iroh://member-endpoint",
            FederationDkgSecretShareTransportPolicy::RequireSecureTransport,
        );
        require_secure
            .validate_for_public_config(&iroh)
            .expect("Iroh is accepted");

        let (loopback, require_https) = auto_start_config(
            "http://127.0.0.1:8080",
            FederationDkgSecretShareTransportPolicy::RequireSecureTransport,
        );
        assert!(require_https.validate_for_public_config(&loopback).is_err());
        let (_, allow_loopback) = auto_start_config(
            "http://127.0.0.1:8080",
            FederationDkgSecretShareTransportPolicy::AllowInsecureLocalhost,
        );
        allow_loopback
            .validate_for_public_config(&loopback)
            .expect("explicit loopback development policy is accepted");

        let (remote_http, allow_loopback) = auto_start_config(
            "http://member.example.com",
            FederationDkgSecretShareTransportPolicy::AllowInsecureLocalhost,
        );
        assert!(allow_loopback
            .validate_for_public_config(&remote_http)
            .is_err());
    }

    #[test]
    fn delivery_tracker_retries_unacknowledged_and_resets_for_changed_payload() {
        let first = MemberId::new(1).expect("first member");
        let second = MemberId::new(2).expect("second member");
        let mut tracked = None;

        assert_eq!(
            pending_broadcast_targets(&mut tracked, &11_u64, [first, second]),
            vec![first, second]
        );
        record_broadcast_acknowledgements(&mut tracked, &11, &BTreeSet::from([first]));
        assert!(!broadcast_acknowledged_by_all(
            &tracked,
            &11,
            [first, second]
        ));
        assert_eq!(
            pending_broadcast_targets(&mut tracked, &11, [first, second]),
            vec![second]
        );
        record_broadcast_acknowledgements(&mut tracked, &11, &BTreeSet::from([second]));
        assert!(broadcast_acknowledged_by_all(
            &tracked,
            &11,
            [first, second]
        ));
        assert_eq!(
            pending_broadcast_targets(&mut tracked, &12, [first, second]),
            vec![first, second]
        );
        assert!(!broadcast_acknowledged_by_all(
            &tracked,
            &12,
            [first, second]
        ));
    }

    #[test]
    fn delivery_totals_accumulate_every_tick_with_saturating_counts() {
        let progress = FederationDkgDriverProgress {
            member_count: 3,
            commitment_count: 0,
            reveal_count: 0,
            secret_share_contribution_count: 0,
            local_keyset_share_ready: false,
            transcript_signature_count: 0,
            activation_count: 0,
        };
        let first = FederationDkgDriverTickOutcome {
            phase_before: FederationDkgDriverPhase::Commitment,
            phase_after: FederationDkgDriverPhase::Commitment,
            progress,
            attempted_deliveries: 3,
            successful_deliveries: 2,
            failed_deliveries: 1,
            failed_members: vec![MemberId::new(3).expect("member")],
        };
        let second = FederationDkgDriverTickOutcome {
            phase_before: FederationDkgDriverPhase::Commitment,
            phase_after: FederationDkgDriverPhase::Reveal,
            progress,
            attempted_deliveries: 1,
            successful_deliveries: 1,
            failed_deliveries: 0,
            failed_members: Vec::new(),
        };

        let mut totals = FederationDkgDeliveryTotals::default();
        totals.record(&first);
        totals.record(&second);
        assert_eq!(totals.attempted, 4);
        assert_eq!(totals.successful, 3);
        assert_eq!(totals.failed, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bounded_dkg_cpu_worker_keeps_axum_probe_responsive() {
        let mut direct_generations = Vec::new();
        let mut direct_probes = Vec::new();
        let mut worker_generations = Vec::new();
        let mut worker_probes = Vec::new();
        for _ in 0..5 {
            let (direct_generation, direct_probe, _) = measure_generation_probe(false).await;
            let (worker_generation, worker_probe, _) = measure_generation_probe(true).await;
            direct_generations.push(direct_generation);
            direct_probes.push(direct_probe);
            worker_generations.push(worker_generation);
            worker_probes.push(worker_probe);
        }
        direct_generations.sort_unstable();
        direct_probes.sort_unstable();
        worker_generations.sort_unstable();
        worker_probes.sort_unstable();
        let direct_generation = direct_generations[2];
        let direct_probe = direct_probes[2];
        let _worker_generation = worker_generations[2];
        let worker_probe = worker_probes[2];

        assert!(direct_probe >= direct_generation / 2);
        assert!(worker_probe < Duration::from_millis(100));
        assert!(worker_probe.saturating_mul(5) < direct_probe);
    }

    #[tokio::test]
    async fn cancelled_dkg_waiter_does_not_release_running_cpu_permit() {
        let admission = Box::leak(Box::new(tokio::sync::Semaphore::new(1)));
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first_started_wait = first_started.notified();
        let first_started_task = Arc::clone(&first_started);
        let (release_first, wait_for_release) = std::sync::mpsc::channel();
        let first = tokio::spawn(crate::run_federation_dkg_cpu_task_with_admission(
            admission,
            "first test DKG task failed",
            move || {
                first_started_task.notify_one();
                wait_for_release.recv().expect("release first CPU task");
                Ok::<_, cdk::Error>(())
            },
        ));
        first_started_wait.await;
        first.abort();

        let second_started = Arc::new(AtomicBool::new(false));
        let second_started_task = Arc::clone(&second_started);
        let second = tokio::spawn(crate::run_federation_dkg_cpu_task_with_admission(
            admission,
            "second test DKG task failed",
            move || {
                second_started_task.store(true, Ordering::SeqCst);
                Ok::<_, cdk::Error>(())
            },
        ));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!second_started.load(Ordering::SeqCst));

        release_first.send(()).expect("release first CPU task");
        tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .expect("second task receives CPU permit")
            .expect("second task joins")
            .expect("second task succeeds");
    }
}

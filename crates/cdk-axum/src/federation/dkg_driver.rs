//! Restartable orchestration for dealer-free federation DKG over private REST.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use cdk::federation::{
    redact_federation_operator_message, FederationConfig, FederationDkgCeremonyId,
    FederationDkgCommitmentRequest, FederationDkgDriverPhase, FederationDkgDriverPlan,
    FederationDkgDriverProgress, FederationDkgDriverRecords, FederationDkgKeysetSpec,
    FederationDkgMemberKeysetShare, FederationDkgRevealRequest, FederationDkgSecretShareRequest,
    FederationDkgTranscriptSignatureRequest, FederationError, MemberId,
    ValidatedFederationDkgParticipantState,
};
use cdk_http_client::{Async, Transport};
use futures::future::join_all;

use crate::MintState;

use super::peer_client::FederationRestPeerClient;
use super::sync_config::FederationPeerSyncConfig;

/// Confidentiality policy for dealer-free DKG secret-share delivery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FederationDkgSecretShareTransportPolicy {
    /// Require HTTPS for every configured federation member endpoint.
    #[default]
    RequireHttps,
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
    /// Confidentiality policy applied before any ceremony message is sent.
    pub secret_share_transport: FederationDkgSecretShareTransportPolicy,
}

impl Default for FederationDkgDriverRunConfig {
    fn default() -> Self {
        Self {
            max_ticks: 64,
            retry_delay: Duration::from_secs(1),
            secret_share_transport: FederationDkgSecretShareTransportPolicy::RequireHttps,
        }
    }
}

/// Complete operator input needed to start or resume one DKG ceremony.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationDkgAutoStartConfig {
    /// Unique ceremony ID shared by every configured member.
    pub ceremony_id: FederationDkgCeremonyId,
    /// Exact aggregate keyset policy every participant must commit to.
    pub keyset_spec: FederationDkgKeysetSpec,
    /// Bounded retry and confidential-transport policy.
    pub driver: FederationDkgDriverRunConfig,
}

impl FederationDkgAutoStartConfig {
    /// Validate setup-mode and transport requirements before spawning the driver.
    pub fn validate_for_public_config(
        &self,
        public_config: &FederationConfig,
    ) -> Result<(), cdk::Error> {
        public_config.validate()?;
        if !public_config.keysets.is_empty() || public_config.setup_transcript.is_some() {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "live DKG start requires a public setup config without finalized keysets or transcript"
                    .to_string(),
            }
            .into());
        }
        self.keyset_spec.validate()?;
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
struct DeliverySummary {
    attempted: usize,
    successful: usize,
    failed: usize,
    failed_members: BTreeSet<MemberId>,
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

    let records = load_federation_dkg_driver_records(state).await?;
    let plan = plan_federation_dkg_records(
        records,
        &public_config,
        &private_config,
        participant,
        plan_cache,
    )
    .await?;
    let phase_before = plan.phase;
    let mut delivery = DeliverySummary::default();

    if let Some(request) = plan.commitment.as_ref() {
        let targets = pending_broadcast_targets(
            &mut delivery_tracker.commitment,
            request,
            public_config.members.keys().copied(),
        );
        let results = join_all(targets.into_iter().map(|member_id| {
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
        let results = join_all(targets.into_iter().map(|member_id| {
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
        .await;
        let acknowledged = record_delivery_results("reveal", results, &mut delivery);
        record_broadcast_acknowledgements(&mut delivery_tracker.reveal, request, &acknowledged);
    }

    if !plan.secret_shares.is_empty() {
        let deliveries = plan
            .secret_shares
            .iter()
            .filter(|(receiver, request)| {
                delivery_tracker.secret_shares.get(receiver) != Some(*request)
            })
            .map(|(receiver, request)| {
                let member = public_config
                    .member(*receiver)
                    .expect("validated DKG plan receiver must be configured");
                async move {
                    (
                        member.member_id,
                        client.submit_dkg_secret_share(member, request).await,
                    )
                }
            });
        let results = join_all(deliveries).await;
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

    if let Some(request) = plan.transcript_signature.as_ref() {
        let targets = pending_broadcast_targets(
            &mut delivery_tracker.transcript_signature,
            request,
            public_config.members.keys().copied(),
        );
        let results = join_all(targets.into_iter().map(|member_id| {
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
        .await;
        let acknowledged = record_delivery_results("transcript signature", results, &mut delivery);
        record_broadcast_acknowledgements(
            &mut delivery_tracker.transcript_signature,
            request,
            &acknowledged,
        );
    }

    let records = load_federation_dkg_driver_records(state).await?;
    let next_plan = plan_federation_dkg_records(
        records,
        &public_config,
        &private_config,
        participant,
        plan_cache,
    )
    .await?;
    Ok(FederationDkgDriverTickOutcome {
        phase_before,
        phase_after: next_plan.phase,
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
    let public_config = public_config.clone();
    let private_config = private_config.clone();
    let participant = participant.clone();
    let plan =
        crate::run_federation_dkg_cpu_task("federation DKG planner task failed", move || {
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
pub async fn run_federation_dkg_driver(
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
    validate_federation_dkg_secret_share_transport(
        &public_config,
        driver_config.secret_share_transport,
    )?;
    let participant =
        prepare_federation_dkg_participant_state(state, ceremony_id, keyset_spec).await?;
    let client = FederationRestPeerClient::new_with_config(
        Async::default(),
        &public_config,
        private_config,
        sync_config,
    )
    .map_err(|err| cdk::Error::Custom(err.to_string()))?;

    let mut plan_cache = None;
    let mut delivery_tracker = FederationDkgDeliveryTracker::default();
    let mut last_tick = run_federation_dkg_driver_tick_with_cache(
        state,
        &client,
        &participant,
        driver_config.secret_share_transport,
        &mut plan_cache,
        &mut delivery_tracker,
    )
    .await?;
    let mut ticks = 1;
    while !last_tick.is_complete() && ticks < driver_config.max_ticks {
        tokio::time::sleep(driver_config.retry_delay).await;
        last_tick = run_federation_dkg_driver_tick_with_cache(
            state,
            &client,
            &participant,
            driver_config.secret_share_transport,
            &mut plan_cache,
            &mut delivery_tracker,
        )
        .await?;
        ticks = ticks.saturating_add(1);
    }

    Ok(FederationDkgDriverRunOutcome { ticks, last_tick })
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
    tokio::spawn(async move {
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
                    "Federation DKG ceremony completed; restart this member to activate the finalized keyset"
                );
            }
            Ok(outcome) => {
                tracing::warn!(
                    %local_member,
                    ceremony_id = %config.ceremony_id,
                    ticks = outcome.ticks,
                    failed_deliveries = outcome.last_tick.failed_deliveries,
                    "Federation DKG driver exhausted its bounded run before completion"
                );
            }
            Err(err) => {
                tracing::error!(
                    %local_member,
                    ceremony_id = %config.ceremony_id,
                    "Federation DKG driver stopped: {}",
                    redact_federation_operator_message(&err.to_string())
                );
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
            "https" => true,
            "http" if policy == FederationDkgSecretShareTransportPolicy::AllowInsecureLocalhost => {
                is_loopback_url(&url)
            }
            _ => false,
        };
        if !accepted {
            return Err(cdk::Error::Custom(format!(
                "federation DKG secret-share transport for member {} requires HTTPS; plain HTTP is allowed only for explicit loopback development mode",
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
) -> Result<FederationDkgDriverRecords, cdk::Error> {
    let db = state.mint.localstore();
    Ok(FederationDkgDriverRecords {
        commitments: db.get_federation_dkg_commitments().await?,
        reveals: db.get_federation_dkg_reveals().await?,
        secret_share_contributions: db.get_federation_dkg_secret_share_contributions().await?,
        keyset_shares: db.get_federation_dkg_member_keyset_shares().await?,
        transcript_signatures: db.get_federation_dkg_transcript_signatures().await?,
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
            Err(_) => {
                summary.record(member_id, false);
                tracing::warn!(
                    %member_id,
                    phase,
                    "Federation DKG delivery failed and will be retried"
                );
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
        let public_config = FederationConfig::new(
            ThresholdParams::with_bft_consensus(1, 1).expect("thresholds"),
            BTreeMap::from([(member_id, member)]),
            Vec::new(),
        )
        .expect("setup config");
        let config = FederationDkgAutoStartConfig {
            ceremony_id: FederationDkgCeremonyId::from_bytes([7; 32]),
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
    fn run_config_rejects_zero_values() {
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
    }

    #[test]
    fn secret_share_transport_requires_https_or_explicit_loopback() {
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
            FederationDkgSecretShareTransportPolicy::RequireHttps,
        );
        require_https
            .validate_for_public_config(&https)
            .expect("HTTPS is accepted");

        let (loopback, require_https) = auto_start_config(
            "http://127.0.0.1:8080",
            FederationDkgSecretShareTransportPolicy::RequireHttps,
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
        assert_eq!(
            pending_broadcast_targets(&mut tracked, &11, [first, second]),
            vec![second]
        );
        assert_eq!(
            pending_broadcast_targets(&mut tracked, &12, [first, second]),
            vec![first, second]
        );
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

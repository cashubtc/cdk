//! Restartable orchestration for dealer-free federation DKG over private REST.

use std::collections::BTreeSet;
use std::time::Duration;

use cdk::federation::{
    FederationDkgCeremonyId, FederationDkgDriverPhase, FederationDkgDriverProgress,
    FederationDkgDriverRecords, FederationDkgKeysetSpec, FederationDkgMemberKeysetShare,
    FederationDkgParticipantState, FederationError, MemberId,
};
use cdk_http_client::{Async, Transport};
use futures::future::join_all;

use crate::MintState;

use super::peer_client::FederationRestPeerClient;
use super::sync_config::FederationPeerSyncConfig;

/// Bounded retry policy for one cooperative DKG driver invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederationDkgDriverRunConfig {
    /// Maximum number of durable plan-and-deliver ticks.
    pub max_ticks: usize,
    /// Delay between incomplete ticks.
    pub retry_delay: Duration,
}

impl Default for FederationDkgDriverRunConfig {
    fn default() -> Self {
        Self {
            max_ticks: 64,
            retry_delay: Duration::from_secs(1),
        }
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
) -> Result<FederationDkgParticipantState, cdk::Error> {
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
        existing.validate(&public_config)?;
        if existing.member_id() != local_member {
            return Err(FederationError::MemberIdMismatch {
                expected: local_member,
                found: existing.member_id(),
            }
            .into());
        }
        if existing.keyset_spec() != &keyset_spec {
            return Err(FederationError::DkgCommitmentMismatch {
                member_id: local_member,
                reason: "persisted participant keyset policy does not match requested ceremony"
                    .to_string(),
            }
            .into());
        }
        return Ok(existing);
    }

    let participant = FederationDkgParticipantState::generate(
        &public_config,
        ceremony_id,
        local_member,
        keyset_spec,
    )?;
    let mut tx = db.begin_transaction().await?;
    tx.add_federation_dkg_participant_state(&participant)
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
pub(crate) async fn run_federation_dkg_driver_tick<T>(
    state: &MintState,
    client: &FederationRestPeerClient<T>,
    participant: &FederationDkgParticipantState,
) -> Result<FederationDkgDriverTickOutcome, cdk::Error>
where
    T: Transport,
{
    let runtime_config = state.mint.federation_config().ok_or_else(|| {
        cdk::Error::Custom("federation mint is missing its runtime config".to_string())
    })?;
    let public_config = runtime_config.public_config().clone();
    let private_config = runtime_config.private_config().clone();
    participant.validate(&public_config)?;
    if participant.member_id() != private_config.member_id {
        return Err(FederationError::MemberIdMismatch {
            expected: private_config.member_id,
            found: participant.member_id(),
        }
        .into());
    }

    let records = load_federation_dkg_driver_records(state).await?;
    let plan = records.plan(&public_config, &private_config, participant)?;
    let phase_before = plan.phase;
    let mut delivery = DeliverySummary::default();

    if let Some(request) = plan.commitment.as_ref() {
        let results = join_all(public_config.members.values().map(|member| async move {
            (
                member.member_id,
                client.submit_dkg_commitment(member, request).await,
            )
        }))
        .await;
        record_delivery_results("commitment", results, &mut delivery);
    }

    if let Some(request) = plan.reveal.as_ref() {
        let results = join_all(public_config.members.values().map(|member| async move {
            (
                member.member_id,
                client.submit_dkg_reveal(member, request).await,
            )
        }))
        .await;
        record_delivery_results("reveal", results, &mut delivery);
    }

    if !plan.secret_shares.is_empty() {
        let deliveries = plan.secret_shares.iter().map(|(receiver, request)| {
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
        record_delivery_results("secret share", results, &mut delivery);
    }

    if let Some(keyset_share) = plan.derived_keyset_share.as_ref() {
        persist_federation_dkg_member_keyset_share(state, keyset_share).await?;
    }

    if let Some(request) = plan.transcript_signature.as_ref() {
        let results = join_all(public_config.members.values().map(|member| async move {
            (
                member.member_id,
                client
                    .submit_dkg_transcript_signature(member, request)
                    .await,
            )
        }))
        .await;
        record_delivery_results("transcript signature", results, &mut delivery);
    }

    let records = load_federation_dkg_driver_records(state).await?;
    let next_plan = records.plan(&public_config, &private_config, participant)?;
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

/// Run a bounded number of idempotent DKG ticks with the default HTTP transport.
pub async fn run_federation_dkg_driver(
    state: &MintState,
    ceremony_id: FederationDkgCeremonyId,
    keyset_spec: FederationDkgKeysetSpec,
    sync_config: FederationPeerSyncConfig,
    driver_config: FederationDkgDriverRunConfig,
) -> Result<FederationDkgDriverRunOutcome, cdk::Error> {
    driver_config.validate()?;
    let participant =
        prepare_federation_dkg_participant_state(state, ceremony_id, keyset_spec).await?;
    let runtime_config = state.mint.federation_config().ok_or_else(|| {
        cdk::Error::Custom("federation mint is missing its runtime config".to_string())
    })?;
    let client = FederationRestPeerClient::new_with_config(
        Async::default(),
        runtime_config.public_config(),
        runtime_config.private_config().clone(),
        sync_config,
    )
    .map_err(|err| cdk::Error::Custom(err.to_string()))?;

    let mut last_tick = run_federation_dkg_driver_tick(state, &client, &participant).await?;
    let mut ticks = 1;
    while !last_tick.is_complete() && ticks < driver_config.max_ticks {
        tokio::time::sleep(driver_config.retry_delay).await;
        last_tick = run_federation_dkg_driver_tick(state, &client, &participant).await?;
        ticks = ticks.saturating_add(1);
    }

    Ok(FederationDkgDriverRunOutcome { ticks, last_tick })
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
) {
    for (member_id, result) in results {
        match result {
            Ok(_) => summary.record(member_id, true),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_config_rejects_zero_values() {
        assert!(FederationDkgDriverRunConfig {
            max_ticks: 0,
            retry_delay: Duration::from_secs(1),
        }
        .validate()
        .is_err());
        assert!(FederationDkgDriverRunConfig {
            max_ticks: 1,
            retry_delay: Duration::ZERO,
        }
        .validate()
        .is_err());
    }
}

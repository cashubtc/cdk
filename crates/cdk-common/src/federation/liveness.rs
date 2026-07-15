//! Deterministic proposer and signing-coordinator election for federated wallets.

use bitcoin::hashes::{sha256, Hash, HashEngine};
use serde::{Deserialize, Serialize};

use super::{FederationError, MemberId, Result};

const WALLET_LIVENESS_STATUS_VERSION: u16 = 1;
/// Fixed wall-clock lease used by the first CDK on-chain signing scheduler.
pub const FEDERATION_WALLET_SIGNING_ATTEMPT_LEASE_MILLIS: u64 = 30_000;
const PROPOSER_OFFSET_DOMAIN: &[u8] = b"cdk/federation/wallet-proposer-offset/v1";
const COORDINATOR_OFFSET_DOMAIN: &[u8] = b"cdk/federation/wallet-coordinator-offset/v1";
const SIGNER_SET_OFFSET_DOMAIN: &[u8] = b"cdk/federation/wallet-signer-set-offset/v1";

/// Deterministic coordinator and signer set for one monotonic signing attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationWalletSigningPlan {
    /// Monotonic one-based attempt number.
    pub attempt: u32,
    /// Member with the first opportunity to aggregate this attempt.
    pub coordinator: MemberId,
    /// Exact threshold-sized signer set in canonical member order.
    pub signer_set: Vec<MemberId>,
}

/// Redacted most-recent activity of the local wallet liveness worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FederationWalletLivenessActivity {
    /// No pending proposal or signing work exists.
    #[default]
    Idle,
    /// Work exists but another deterministic lease owner acts first.
    WaitingForLease,
    /// The local member is constructing or submitting a proposal.
    Proposing,
    /// The local member is coordinating or aggregating a FROST attempt.
    Signing,
    /// A bounded local retry delay follows an unavailable peer or backend.
    Backoff,
}

/// Bounded operator status for proposer and signing-coordinator liveness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationWalletLivenessStatus {
    /// Status schema version.
    pub version: u16,
    /// Pending deterministic payment batches observed in the last round.
    pub pending_batches: u64,
    /// Accepted transactions awaiting threshold signatures in the last round.
    pub awaiting_signatures: u64,
    /// Proposal submissions attempted by this process.
    pub proposal_attempts: u64,
    /// FROST coordination or aggregation runs attempted by this process.
    pub signing_attempts: u64,
    /// Saturating unavailable-signer or transport failure count.
    pub signer_failures: u64,
    /// Current redacted worker activity.
    pub activity: FederationWalletLivenessActivity,
}

impl Default for FederationWalletLivenessStatus {
    fn default() -> Self {
        Self {
            version: WALLET_LIVENESS_STATUS_VERSION,
            pending_batches: 0,
            awaiting_signatures: 0,
            proposal_attempts: 0,
            signing_attempts: 0,
            signer_failures: 0,
            activity: FederationWalletLivenessActivity::Idle,
        }
    }
}

/// Domain-separated deterministic member schedule for one batch or proposal ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationWalletLivenessSchedule {
    seed: [u8; 32],
    members: Vec<MemberId>,
    threshold: usize,
}

impl FederationWalletLivenessSchedule {
    /// Construct a schedule from a canonical batch/proposal identifier and roster.
    pub fn new(
        seed: [u8; 32],
        members: impl IntoIterator<Item = MemberId>,
        threshold: u16,
    ) -> Result<Self> {
        let mut members = members.into_iter().collect::<Vec<_>>();
        members.sort_unstable();
        members.dedup();
        let threshold = usize::from(threshold);
        if seed == [0; 32] || members.is_empty() || threshold == 0 || threshold > members.len() {
            return Err(invalid("seed, roster, or threshold is invalid"));
        }
        Ok(Self {
            seed,
            members,
            threshold,
        })
    }

    /// Select the sole honest proposer for a one-based local lease attempt.
    pub fn proposer(&self, attempt: u32) -> Result<MemberId> {
        let attempt = checked_attempt(attempt)?;
        let offset = domain_offset(PROPOSER_OFFSET_DOMAIN, self.seed);
        let index = add_mod(offset, attempt, self.members.len() as u128);
        Ok(self.members[index as usize])
    }

    /// Select a coordinator and threshold subset for a one-based signing attempt.
    ///
    /// Signer subsets walk the complete lexicographic combination space from a
    /// domain-separated offset. Thus any threshold-sized online subset is
    /// eventually selected without allocating the full combination set.
    pub fn signing_plan(&self, attempt: u32) -> Result<FederationWalletSigningPlan> {
        let attempt_index = checked_attempt(attempt)?;
        let coordinator_offset = domain_offset(COORDINATOR_OFFSET_DOMAIN, self.seed);
        let coordinator_index = add_mod(
            coordinator_offset,
            attempt_index,
            self.members.len() as u128,
        );
        let combination_count = binomial(self.members.len(), self.threshold);
        let signer_offset = domain_offset(SIGNER_SET_OFFSET_DOMAIN, self.seed);
        let rank = add_mod(signer_offset, attempt_index, combination_count);
        let signer_set = unrank_combination(self.members.as_slice(), self.threshold, rank)?;
        Ok(FederationWalletSigningPlan {
            attempt,
            coordinator: self.members[coordinator_index as usize],
            signer_set,
        })
    }

    /// Number of unique threshold signer subsets before the schedule repeats.
    #[must_use]
    pub fn signer_set_count(&self) -> u128 {
        binomial(self.members.len(), self.threshold)
    }
}

/// Return the restart-stable active signing attempt for Unix epoch milliseconds.
#[must_use]
pub fn federation_wallet_signing_attempt_at(now_epoch_millis: u128) -> u32 {
    let attempt = now_epoch_millis / u128::from(FEDERATION_WALLET_SIGNING_ATTEMPT_LEASE_MILLIS) + 1;
    u32::try_from(attempt).unwrap_or(u32::MAX)
}

/// Accept the current lease and one delayed immediately preceding lease.
///
/// A future attempt is never accepted, which prevents an authenticated member
/// from pinning the durable nonce high-water mark arbitrarily far ahead.
#[must_use]
pub fn federation_wallet_signing_attempt_is_current(attempt: u32, now_epoch_millis: u128) -> bool {
    let current = federation_wallet_signing_attempt_at(now_epoch_millis);
    attempt == current || attempt.checked_add(1) == Some(current)
}

fn checked_attempt(attempt: u32) -> Result<u128> {
    if attempt == 0 {
        return Err(invalid("liveness attempt must be non-zero"));
    }
    Ok(u128::from(attempt - 1))
}

fn domain_offset(domain: &[u8], seed: [u8; 32]) -> u128 {
    let mut engine = sha256::Hash::engine();
    engine.input(domain);
    engine.input(&seed);
    let digest = sha256::Hash::from_engine(engine).to_byte_array();
    let mut prefix = [0_u8; 16];
    prefix.copy_from_slice(&digest[..16]);
    u128::from_be_bytes(prefix)
}

fn add_mod(offset: u128, attempt_index: u128, modulus: u128) -> u128 {
    (offset % modulus + attempt_index % modulus) % modulus
}

fn binomial(n: usize, k: usize) -> u128 {
    let k = k.min(n.saturating_sub(k));
    (0..k).fold(1_u128, |value, index| {
        value
            .saturating_mul((n - index) as u128)
            .checked_div((index + 1) as u128)
            .unwrap_or(u128::MAX)
    })
}

fn unrank_combination(
    members: &[MemberId],
    threshold: usize,
    mut rank: u128,
) -> Result<Vec<MemberId>> {
    let mut selected = Vec::with_capacity(threshold);
    let mut candidate = 0_usize;
    for position in 0..threshold {
        let remaining = threshold - position - 1;
        let final_candidate = members.len() - (remaining + 1);
        loop {
            if candidate > final_candidate {
                return Err(invalid("signer-set combination rank is out of range"));
            }
            let suffix_count = binomial(members.len() - candidate - 1, remaining);
            if rank < suffix_count {
                selected.push(members[candidate]);
                candidate += 1;
                break;
            }
            rank -= suffix_count;
            candidate += 1;
        }
    }
    Ok(selected)
}

fn invalid(reason: impl Into<String>) -> FederationError {
    FederationError::WalletLivenessInvalid {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn members(count: u16) -> Vec<MemberId> {
        (1..=count)
            .map(|member| MemberId::new(member).expect("member"))
            .collect()
    }

    #[test]
    fn proposer_leases_rotate_every_member_deterministically() {
        let schedule = FederationWalletLivenessSchedule::new([7; 32], members(5), 3)
            .expect("liveness schedule");
        let first_cycle = (1..=5)
            .map(|attempt| schedule.proposer(attempt).expect("proposer"))
            .collect::<BTreeSet<_>>();
        assert_eq!(first_cycle.len(), 5);
        assert_eq!(
            schedule.proposer(1).expect("first proposer"),
            schedule.proposer(6).expect("wrapped proposer")
        );
        assert!(schedule.proposer(0).is_err());
    }

    #[test]
    fn signing_attempts_cover_every_threshold_subset_and_rotate_coordinators() {
        let schedule = FederationWalletLivenessSchedule::new([8; 32], members(5), 3)
            .expect("liveness schedule");
        assert_eq!(schedule.signer_set_count(), 10);
        let plans = (1..=10)
            .map(|attempt| schedule.signing_plan(attempt).expect("signing plan"))
            .collect::<Vec<_>>();
        let subsets = plans
            .iter()
            .map(|plan| plan.signer_set.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(subsets.len(), 10);
        assert!(subsets.contains(&vec![members(5)[0], members(5)[2], members(5)[4]]));
        assert_eq!(
            plans
                .iter()
                .map(|plan| plan.coordinator)
                .collect::<BTreeSet<_>>()
                .len(),
            5
        );
        assert_eq!(
            schedule.signing_plan(1).expect("first").signer_set,
            schedule.signing_plan(11).expect("wrapped").signer_set
        );
    }

    #[test]
    fn signing_partition_requires_threshold_and_eventually_schedules_online_subset() {
        let roster = members(5);
        let online = BTreeSet::from([roster[0], roster[2], roster[4]]);
        let below_threshold = BTreeSet::from([roster[0], roster[2]]);
        let schedule =
            FederationWalletLivenessSchedule::new([9; 32], roster, 3).expect("liveness schedule");
        assert!(!(1..=10).any(|attempt| {
            schedule
                .signing_plan(attempt)
                .expect("signing plan")
                .signer_set
                .iter()
                .all(|member| below_threshold.contains(member))
        }));
        assert!((1..=10).any(|attempt| {
            schedule
                .signing_plan(attempt)
                .expect("signing plan")
                .signer_set
                .iter()
                .all(|member| online.contains(member))
        }));
    }

    #[test]
    fn signing_lease_rejects_future_and_delayed_stale_attempts() {
        let current = federation_wallet_signing_attempt_at(90_001);
        assert_eq!(current, 4);
        assert!(federation_wallet_signing_attempt_is_current(4, 90_001));
        assert!(federation_wallet_signing_attempt_is_current(3, 90_001));
        assert!(!federation_wallet_signing_attempt_is_current(2, 90_001));
        assert!(!federation_wallet_signing_attempt_is_current(5, 90_001));
    }

    #[test]
    #[ignore = "explicit wallet-liveness CPU microbenchmark"]
    fn deterministic_schedule_active_cpu_microbenchmark() {
        let schedule = FederationWalletLivenessSchedule::new([10; 32], members(7), 5)
            .expect("liveness schedule");
        let started = std::time::Instant::now();
        let mut checksum = 0_u64;
        for attempt in 1..=100_000 {
            let plan = schedule.signing_plan(attempt).expect("signing plan");
            checksum = checksum
                .saturating_add(u64::from(plan.coordinator.get()))
                .saturating_add(plan.signer_set.len().try_into().unwrap_or(u64::MAX));
        }
        let elapsed = started.elapsed();
        assert!(checksum > 0);
        assert!(elapsed < std::time::Duration::from_secs(10));
        eprintln!(
            "100000 deterministic signing schedules: {} microseconds",
            elapsed.as_micros()
        );
    }
}

use std::collections::BTreeMap;
use std::sync::Arc;

use cdk::federation::MemberId;
use cdk::mint::QuoteId;
use cdk_common::database::DynMintDatabase;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PRIMARY_NAMESPACE: &str = "cdk_federation";
const SECONDARY_NAMESPACE: &str = "payment_observer";
const SCHEDULE_KEY: &str = "mint_quote_backoff_v1";
const SCHEDULE_VERSION: u8 = 1;

const BASE_DELAY_MS: u64 = 5_000;
const MAX_DELAY_MS: u64 = 5 * 60 * 1_000;
const REQUEST_COOLDOWN_MS: u64 = 5_000;
const ATTEMPT_LEASE_MS: u64 = 15_000;
const JITTER_PERCENT: u64 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MintQuoteObservationTrigger {
    Scheduled,
    Requested,
    Notified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MintQuoteObservationPermit {
    quote: String,
    generation: u64,
}

#[derive(Debug, Clone, Default)]
pub(super) struct MintQuoteObservationSchedule {
    mutation_lock: Arc<tokio::sync::Mutex<()>>,
}

impl MintQuoteObservationSchedule {
    pub(super) async fn candidates(
        &self,
        db: &DynMintDatabase,
        quote_ids: &[QuoteId],
        trigger: MintQuoteObservationTrigger,
        now_ms: u64,
    ) -> Result<Vec<QuoteId>, cdk::Error> {
        let schedules = load_schedules(db).await?;
        Ok(quote_ids
            .iter()
            .filter(|quote_id| {
                schedules
                    .quotes
                    .get(&quote_id.to_string())
                    .is_none_or(|schedule| schedule.is_due(trigger, now_ms))
            })
            .cloned()
            .collect())
    }

    pub(super) async fn claim(
        &self,
        db: &DynMintDatabase,
        quote_id: &QuoteId,
        trigger: MintQuoteObservationTrigger,
        now_ms: u64,
    ) -> Result<Option<MintQuoteObservationPermit>, cdk::Error> {
        let _guard = self.mutation_lock.lock().await;
        let mut schedules = load_schedules(db).await?;
        let quote = quote_id.to_string();
        let schedule = schedules.quotes.entry(quote.clone()).or_default();
        if !schedule.is_due(trigger, now_ms) {
            return Ok(None);
        }

        schedule.generation = schedule.generation.saturating_add(1);
        schedule.last_attempt_at_ms = now_ms;
        schedule.lease_until_ms = now_ms.saturating_add(ATTEMPT_LEASE_MS);
        schedule.next_attempt_at_ms = schedule.lease_until_ms;
        let generation = schedule.generation;
        store_schedules(db, &schedules).await?;

        Ok(Some(MintQuoteObservationPermit { quote, generation }))
    }

    pub(super) async fn finish(
        &self,
        db: &DynMintDatabase,
        permit: MintQuoteObservationPermit,
        member_id: MemberId,
        made_progress: bool,
        now_ms: u64,
    ) -> Result<(), cdk::Error> {
        let _guard = self.mutation_lock.lock().await;
        let mut schedules = load_schedules(db).await?;
        let Some(schedule) = schedules.quotes.get_mut(&permit.quote) else {
            return Ok(());
        };
        if schedule.generation != permit.generation {
            return Ok(());
        }

        schedule.lease_until_ms = 0;
        if made_progress {
            schedule.consecutive_no_progress = 0;
        } else {
            schedule.consecutive_no_progress = schedule.consecutive_no_progress.saturating_add(1);
        }
        let nominal_delay = nominal_delay_ms(schedule.consecutive_no_progress);
        let delay = jittered_delay_ms(
            nominal_delay,
            &permit.quote,
            member_id,
            schedule.consecutive_no_progress,
        );
        schedule.next_attempt_at_ms = now_ms.saturating_add(delay);
        store_schedules(db, &schedules).await
    }

    pub(super) async fn remove(
        &self,
        db: &DynMintDatabase,
        quote_id: &QuoteId,
    ) -> Result<(), cdk::Error> {
        let _guard = self.mutation_lock.lock().await;
        let mut schedules = load_schedules(db).await?;
        if schedules.quotes.remove(&quote_id.to_string()).is_some() {
            store_schedules(db, &schedules).await?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedMintQuoteSchedules {
    version: u8,
    quotes: BTreeMap<String, PersistedMintQuoteSchedule>,
}

impl Default for PersistedMintQuoteSchedules {
    fn default() -> Self {
        Self {
            version: SCHEDULE_VERSION,
            quotes: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedMintQuoteSchedule {
    consecutive_no_progress: u32,
    last_attempt_at_ms: u64,
    next_attempt_at_ms: u64,
    lease_until_ms: u64,
    generation: u64,
}

impl PersistedMintQuoteSchedule {
    fn is_due(&self, trigger: MintQuoteObservationTrigger, now_ms: u64) -> bool {
        if now_ms < self.lease_until_ms {
            return false;
        }

        match trigger {
            MintQuoteObservationTrigger::Scheduled => now_ms >= self.next_attempt_at_ms,
            MintQuoteObservationTrigger::Requested => {
                now_ms.saturating_sub(self.last_attempt_at_ms) >= REQUEST_COOLDOWN_MS
            }
            MintQuoteObservationTrigger::Notified => true,
        }
    }
}

async fn load_schedules(db: &DynMintDatabase) -> Result<PersistedMintQuoteSchedules, cdk::Error> {
    let Some(bytes) = db
        .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, SCHEDULE_KEY)
        .await?
    else {
        return Ok(PersistedMintQuoteSchedules::default());
    };
    match serde_json::from_slice::<PersistedMintQuoteSchedules>(&bytes) {
        Ok(schedules) if schedules.version == SCHEDULE_VERSION => Ok(schedules),
        Ok(schedules) => {
            tracing::warn!(
                stored_version = schedules.version,
                expected_version = SCHEDULE_VERSION,
                "Ignoring unsupported federation mint quote backoff state"
            );
            Ok(PersistedMintQuoteSchedules::default())
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "Ignoring invalid federation mint quote backoff state"
            );
            Ok(PersistedMintQuoteSchedules::default())
        }
    }
}

async fn store_schedules(
    db: &DynMintDatabase,
    schedules: &PersistedMintQuoteSchedules,
) -> Result<(), cdk::Error> {
    let bytes = serde_json::to_vec(schedules)?;
    let mut tx = db.begin_transaction().await?;
    tx.kv_write(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, SCHEDULE_KEY, &bytes)
        .await?;
    tx.commit().await?;
    Ok(())
}

fn nominal_delay_ms(consecutive_no_progress: u32) -> u64 {
    let exponent = consecutive_no_progress.saturating_sub(1).min(63);
    BASE_DELAY_MS
        .saturating_mul(1u64.checked_shl(exponent).unwrap_or(u64::MAX))
        .min(MAX_DELAY_MS)
}

fn jittered_delay_ms(
    nominal_delay_ms: u64,
    quote: &str,
    member_id: MemberId,
    consecutive_no_progress: u32,
) -> u64 {
    let spread = nominal_delay_ms.saturating_mul(JITTER_PERCENT) / 100;
    if spread == 0 {
        return nominal_delay_ms;
    }

    let mut hasher = Sha256::new();
    hasher.update(quote.as_bytes());
    hasher.update(member_id.get().to_be_bytes());
    hasher.update(consecutive_no_progress.to_be_bytes());
    let digest = hasher.finalize();
    let sample = u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 digest always contains eight bytes"),
    );
    let width = spread.saturating_mul(2).saturating_add(1);
    nominal_delay_ms
        .saturating_sub(spread)
        .saturating_add(sample % width)
        .min(MAX_DELAY_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member_id() -> MemberId {
        MemberId::new(1).expect("valid member id")
    }

    #[test]
    fn no_progress_delay_doubles_and_caps() {
        assert_eq!(nominal_delay_ms(0), BASE_DELAY_MS);
        assert_eq!(nominal_delay_ms(1), BASE_DELAY_MS);
        assert_eq!(nominal_delay_ms(2), BASE_DELAY_MS * 2);
        assert_eq!(nominal_delay_ms(3), BASE_DELAY_MS * 4);
        assert_eq!(nominal_delay_ms(20), MAX_DELAY_MS);
        assert_eq!(nominal_delay_ms(u32::MAX), MAX_DELAY_MS);
    }

    #[test]
    fn deterministic_jitter_stays_within_twenty_percent() {
        let nominal = 40_000;
        let delay = jittered_delay_ms(nominal, "quote", member_id(), 4);
        assert!((32_000..=48_000).contains(&delay));
        assert_eq!(delay, jittered_delay_ms(nominal, "quote", member_id(), 4));
    }

    #[test]
    fn request_can_wake_backed_off_quote_without_resetting_attempts() {
        let schedule = PersistedMintQuoteSchedule {
            consecutive_no_progress: 7,
            last_attempt_at_ms: 1_000,
            next_attempt_at_ms: 300_000,
            lease_until_ms: 0,
            generation: 2,
        };

        assert!(!schedule.is_due(MintQuoteObservationTrigger::Scheduled, 6_000));
        assert!(!schedule.is_due(MintQuoteObservationTrigger::Requested, 5_999));
        assert!(schedule.is_due(MintQuoteObservationTrigger::Requested, 6_000));
        assert_eq!(schedule.consecutive_no_progress, 7);
    }

    #[test]
    fn active_attempt_blocks_every_trigger_until_lease_expires() {
        let schedule = PersistedMintQuoteSchedule {
            lease_until_ms: 10_000,
            ..Default::default()
        };

        for trigger in [
            MintQuoteObservationTrigger::Scheduled,
            MintQuoteObservationTrigger::Requested,
            MintQuoteObservationTrigger::Notified,
        ] {
            assert!(!schedule.is_due(trigger, 9_999));
            assert!(schedule.is_due(trigger, 10_000));
        }
    }

    #[tokio::test]
    async fn schedule_survives_a_new_scheduler_instance() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let quote_id = QuoteId::new();
        let first = MintQuoteObservationSchedule::default();
        let permit = first
            .claim(
                &db,
                &quote_id,
                MintQuoteObservationTrigger::Scheduled,
                1_000,
            )
            .await
            .expect("claim schedule")
            .expect("first attempt is due");
        first
            .finish(&db, permit, member_id(), false, 2_000)
            .await
            .expect("finish schedule");

        let persisted = load_schedules(&db).await.expect("load schedule");
        let quote = persisted
            .quotes
            .get(&quote_id.to_string())
            .expect("persisted quote schedule");
        assert_eq!(quote.consecutive_no_progress, 1);
        assert_eq!(quote.last_attempt_at_ms, 1_000);
        assert_eq!(quote.lease_until_ms, 0);
        assert!((6_000..=8_000).contains(&quote.next_attempt_at_ms));

        let restarted = MintQuoteObservationSchedule::default();
        assert!(restarted
            .candidates(
                &db,
                std::slice::from_ref(&quote_id),
                MintQuoteObservationTrigger::Scheduled,
                5_999,
            )
            .await
            .expect("scheduled candidates")
            .is_empty());
        assert_eq!(
            restarted
                .candidates(
                    &db,
                    std::slice::from_ref(&quote_id),
                    MintQuoteObservationTrigger::Requested,
                    6_000,
                )
                .await
                .expect("requested candidates"),
            vec![quote_id]
        );
    }
}

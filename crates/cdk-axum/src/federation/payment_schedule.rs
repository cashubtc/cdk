use std::collections::BTreeMap;
use std::sync::Arc;

use cdk::federation::MemberId;
use cdk::mint::QuoteId;
use cdk_common::database::DynMintDatabase;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PRIMARY_NAMESPACE: &str = "cdk_federation";
const SECONDARY_NAMESPACE: &str = "payment_observer";
const SCHEDULE_KEY: &str = "payment_quote_backoff_v2";
const LEGACY_MINT_SCHEDULE_KEY: &str = "mint_quote_backoff_v1";
const SCHEDULE_VERSION: u8 = 2;

const BASE_DELAY_MS: u64 = 5_000;
const MAX_DELAY_MS: u64 = 5 * 60 * 1_000;
const MINT_REQUEST_COOLDOWN_MS: u64 = 5_000;
const ATTEMPT_LEASE_MS: u64 = 15_000;
const FEDERATION_REPAIR_DELAY_MS: u64 = 60_000;
const JITTER_PERCENT: u64 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum FederationPaymentQuoteKind {
    Mint,
    Melt,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct FederationPaymentQuote {
    pub(super) kind: FederationPaymentQuoteKind,
    pub(super) id: QuoteId,
}

impl FederationPaymentQuote {
    pub(super) fn mint(id: QuoteId) -> Self {
        Self {
            kind: FederationPaymentQuoteKind::Mint,
            id,
        }
    }

    pub(super) fn melt(id: QuoteId) -> Self {
        Self {
            kind: FederationPaymentQuoteKind::Melt,
            id,
        }
    }

    fn schedule_key(&self) -> String {
        let prefix = match self.kind {
            FederationPaymentQuoteKind::Mint => "mint",
            FederationPaymentQuoteKind::Melt => "melt",
        };
        format!("{prefix}:{}", self.id)
    }

    fn request_cooldown_ms(&self) -> u64 {
        match self.kind {
            FederationPaymentQuoteKind::Mint => MINT_REQUEST_COOLDOWN_MS,
            // Federated melts were request-driven before they shared this
            // scheduler. Preserve that public GET/POST behavior while the
            // active-attempt lease still coalesces concurrent requests.
            FederationPaymentQuoteKind::Melt => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FederationPaymentAdvancement {
    Progressed,
    AwaitingExternalEvidence,
    AwaitingFederationEvidence,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FederationPaymentObservationTrigger {
    Scheduled,
    Requested,
    BackendNotified,
    OperationAccepted(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FederationPaymentObservationPermit {
    quote: String,
    generation: u64,
}

#[derive(Debug, Clone)]
pub(super) struct FederationPaymentObservationSchedule {
    mutation_lock: Arc<tokio::sync::Mutex<()>>,
    changes: tokio::sync::watch::Sender<u64>,
}

impl Default for FederationPaymentObservationSchedule {
    fn default() -> Self {
        let (changes, _) = tokio::sync::watch::channel(0);
        Self {
            mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
            changes,
        }
    }
}

impl FederationPaymentObservationSchedule {
    pub(super) fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub(super) async fn candidates(
        &self,
        db: &DynMintDatabase,
        quotes: &[FederationPaymentQuote],
        trigger: FederationPaymentObservationTrigger,
        now_ms: u64,
    ) -> Result<Vec<FederationPaymentQuote>, cdk::Error> {
        let schedules = load_schedules(db).await?;
        Ok(quotes
            .iter()
            .filter(|quote| {
                schedules
                    .quotes
                    .get(&quote.schedule_key())
                    .is_none_or(|schedule| {
                        schedule.is_due(trigger, quote.request_cooldown_ms(), now_ms)
                    })
            })
            .cloned()
            .collect())
    }

    pub(super) async fn claim(
        &self,
        db: &DynMintDatabase,
        quote: &FederationPaymentQuote,
        trigger: FederationPaymentObservationTrigger,
        now_ms: u64,
    ) -> Result<Option<FederationPaymentObservationPermit>, cdk::Error> {
        let _guard = self.mutation_lock.lock().await;
        let mut schedules = load_schedules(db).await?;
        let request_cooldown_ms = quote.request_cooldown_ms();
        let quote = quote.schedule_key();
        let schedule = schedules.quotes.entry(quote.clone()).or_default();
        if !schedule.is_due(trigger, request_cooldown_ms, now_ms) {
            return Ok(None);
        }

        schedule.generation = schedule.generation.saturating_add(1);
        schedule.last_attempt_at_ms = now_ms;
        schedule.lease_until_ms = now_ms.saturating_add(ATTEMPT_LEASE_MS);
        schedule.next_attempt_at_ms = schedule.lease_until_ms;
        let generation = schedule.generation;
        store_schedules(db, &schedules).await?;
        self.notify_changed();

        Ok(Some(FederationPaymentObservationPermit {
            quote,
            generation,
        }))
    }

    pub(super) async fn finish(
        &self,
        db: &DynMintDatabase,
        permit: FederationPaymentObservationPermit,
        member_id: MemberId,
        advancement: FederationPaymentAdvancement,
        made_progress: bool,
        last_processed_operation_index: Option<u64>,
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

        if advancement == FederationPaymentAdvancement::Complete {
            schedules.quotes.remove(&permit.quote);
            store_schedules(db, &schedules).await?;
            self.notify_changed();
            return Ok(());
        }

        schedule.lease_until_ms = 0;
        if let Some(index) = last_processed_operation_index {
            schedule.last_processed_operation_index = Some(
                schedule
                    .last_processed_operation_index
                    .map_or(index, |current| current.max(index)),
            );
        }
        match advancement {
            FederationPaymentAdvancement::Progressed => {
                schedule.wait = PersistedPaymentWait::Ready;
                schedule.consecutive_no_progress = 0;
                schedule.next_attempt_at_ms = now_ms;
            }
            FederationPaymentAdvancement::AwaitingExternalEvidence => {
                schedule.wait = PersistedPaymentWait::External;
                schedule.consecutive_no_progress = if made_progress {
                    1
                } else {
                    schedule.consecutive_no_progress.saturating_add(1).max(1)
                };
                let nominal_delay = nominal_delay_ms(schedule.consecutive_no_progress);
                let delay = jittered_delay_ms(
                    nominal_delay,
                    &permit.quote,
                    member_id,
                    schedule.consecutive_no_progress,
                );
                schedule.next_attempt_at_ms = now_ms.saturating_add(delay);
            }
            FederationPaymentAdvancement::AwaitingFederationEvidence => {
                schedule.wait = PersistedPaymentWait::Federation;
                schedule.next_attempt_at_ms = now_ms.saturating_add(FEDERATION_REPAIR_DELAY_MS);
            }
            FederationPaymentAdvancement::Complete => unreachable!("handled above"),
        }
        store_schedules(db, &schedules).await?;
        self.notify_changed();
        Ok(())
    }

    pub(super) async fn next_attempt_at_ms(
        &self,
        db: &DynMintDatabase,
    ) -> Result<Option<u64>, cdk::Error> {
        Ok(load_schedules(db)
            .await?
            .quotes
            .values()
            .map(|schedule| schedule.next_attempt_at_ms.max(schedule.lease_until_ms))
            .min())
    }

    pub(super) async fn active_lease_until_ms(
        &self,
        db: &DynMintDatabase,
        quotes: &[FederationPaymentQuote],
        now_ms: u64,
    ) -> Result<Option<u64>, cdk::Error> {
        let schedules = load_schedules(db).await?;
        Ok(quotes
            .iter()
            .filter_map(|quote| schedules.quotes.get(&quote.schedule_key()))
            .map(|schedule| schedule.lease_until_ms)
            .filter(|lease_until_ms| *lease_until_ms > now_ms)
            .min())
    }

    pub(super) async fn remove(
        &self,
        db: &DynMintDatabase,
        quote: &FederationPaymentQuote,
    ) -> Result<(), cdk::Error> {
        let _guard = self.mutation_lock.lock().await;
        let mut schedules = load_schedules(db).await?;
        if schedules.quotes.remove(&quote.schedule_key()).is_some() {
            store_schedules(db, &schedules).await?;
            self.notify_changed();
        }
        Ok(())
    }

    fn notify_changed(&self) {
        self.changes.send_modify(|generation| {
            *generation = generation.saturating_add(1);
        });
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedPaymentSchedules {
    version: u8,
    quotes: BTreeMap<String, PersistedPaymentSchedule>,
}

impl Default for PersistedPaymentSchedules {
    fn default() -> Self {
        Self {
            version: SCHEDULE_VERSION,
            quotes: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PersistedPaymentWait {
    #[default]
    Ready,
    External,
    Federation,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedPaymentSchedule {
    consecutive_no_progress: u32,
    last_attempt_at_ms: u64,
    next_attempt_at_ms: u64,
    lease_until_ms: u64,
    generation: u64,
    #[serde(default)]
    wait: PersistedPaymentWait,
    #[serde(default)]
    last_processed_operation_index: Option<u64>,
}

impl PersistedPaymentSchedule {
    fn is_due(
        &self,
        trigger: FederationPaymentObservationTrigger,
        request_cooldown_ms: u64,
        now_ms: u64,
    ) -> bool {
        if now_ms < self.lease_until_ms {
            return false;
        }

        match trigger {
            FederationPaymentObservationTrigger::Scheduled => now_ms >= self.next_attempt_at_ms,
            FederationPaymentObservationTrigger::Requested => {
                now_ms.saturating_sub(self.last_attempt_at_ms) >= request_cooldown_ms
            }
            FederationPaymentObservationTrigger::BackendNotified => true,
            FederationPaymentObservationTrigger::OperationAccepted(index) => self
                .last_processed_operation_index
                .is_none_or(|processed| index > processed),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct LegacyMintSchedules {
    quotes: BTreeMap<String, PersistedPaymentSchedule>,
}

async fn load_schedules(db: &DynMintDatabase) -> Result<PersistedPaymentSchedules, cdk::Error> {
    if let Some(bytes) = db
        .kv_read(PRIMARY_NAMESPACE, SECONDARY_NAMESPACE, SCHEDULE_KEY)
        .await?
    {
        return match serde_json::from_slice::<PersistedPaymentSchedules>(&bytes) {
            Ok(schedules) if schedules.version == SCHEDULE_VERSION => Ok(schedules),
            Ok(schedules) => {
                tracing::warn!(
                    stored_version = schedules.version,
                    expected_version = SCHEDULE_VERSION,
                    "Ignoring unsupported federation payment quote backoff state"
                );
                Ok(PersistedPaymentSchedules::default())
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "Ignoring invalid federation payment quote backoff state"
                );
                Ok(PersistedPaymentSchedules::default())
            }
        };
    }

    let Some(bytes) = db
        .kv_read(
            PRIMARY_NAMESPACE,
            SECONDARY_NAMESPACE,
            LEGACY_MINT_SCHEDULE_KEY,
        )
        .await?
    else {
        return Ok(PersistedPaymentSchedules::default());
    };
    let Ok(legacy) = serde_json::from_slice::<LegacyMintSchedules>(&bytes) else {
        return Ok(PersistedPaymentSchedules::default());
    };
    Ok(PersistedPaymentSchedules {
        version: SCHEDULE_VERSION,
        quotes: legacy
            .quotes
            .into_iter()
            .map(|(quote, schedule)| (format!("mint:{quote}"), schedule))
            .collect(),
    })
}

async fn store_schedules(
    db: &DynMintDatabase,
    schedules: &PersistedPaymentSchedules,
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
    fn request_wakes_backed_off_quote_without_resetting_attempts() {
        let schedule = PersistedPaymentSchedule {
            consecutive_no_progress: 7,
            last_attempt_at_ms: 1_000,
            next_attempt_at_ms: 300_000,
            lease_until_ms: 0,
            generation: 2,
            wait: PersistedPaymentWait::External,
            last_processed_operation_index: None,
        };

        assert!(!schedule.is_due(FederationPaymentObservationTrigger::Scheduled, 0, 6_000));
        assert!(!schedule.is_due(
            FederationPaymentObservationTrigger::Requested,
            MINT_REQUEST_COOLDOWN_MS,
            5_999,
        ));
        assert!(schedule.is_due(
            FederationPaymentObservationTrigger::Requested,
            MINT_REQUEST_COOLDOWN_MS,
            6_000,
        ));
        assert_eq!(schedule.consecutive_no_progress, 7);
    }

    #[test]
    fn request_policy_preserves_mint_cooldown_and_request_driven_melts() {
        let mint = FederationPaymentQuote::mint(QuoteId::new());
        let melt = FederationPaymentQuote::melt(QuoteId::new());
        let schedule = PersistedPaymentSchedule {
            last_attempt_at_ms: 1_000,
            ..Default::default()
        };

        assert!(!schedule.is_due(
            FederationPaymentObservationTrigger::Requested,
            mint.request_cooldown_ms(),
            1_001,
        ));
        assert!(schedule.is_due(
            FederationPaymentObservationTrigger::Requested,
            melt.request_cooldown_ms(),
            1_001,
        ));
    }

    #[test]
    fn active_attempt_blocks_every_trigger_until_lease_expires() {
        let schedule = PersistedPaymentSchedule {
            lease_until_ms: 10_000,
            ..Default::default()
        };

        for trigger in [
            FederationPaymentObservationTrigger::Scheduled,
            FederationPaymentObservationTrigger::Requested,
            FederationPaymentObservationTrigger::BackendNotified,
        ] {
            assert!(!schedule.is_due(trigger, 0, 9_999));
            assert!(schedule.is_due(trigger, 0, 10_000));
        }
    }

    #[test]
    fn only_a_newer_operation_index_wakes_a_persisted_quote() {
        let schedule = PersistedPaymentSchedule {
            next_attempt_at_ms: 300_000,
            last_processed_operation_index: Some(41),
            wait: PersistedPaymentWait::Federation,
            ..Default::default()
        };

        assert!(!schedule.is_due(
            FederationPaymentObservationTrigger::OperationAccepted(40),
            0,
            1_000,
        ));
        assert!(!schedule.is_due(
            FederationPaymentObservationTrigger::OperationAccepted(41),
            0,
            1_000,
        ));
        assert!(schedule.is_due(
            FederationPaymentObservationTrigger::OperationAccepted(42),
            0,
            1_000,
        ));
        assert_eq!(schedule.consecutive_no_progress, 0);
    }

    #[tokio::test]
    async fn concurrent_wakes_share_one_persisted_attempt_lease() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let quote = FederationPaymentQuote::mint(QuoteId::new());
        let schedule = FederationPaymentObservationSchedule::default();
        let (first, second) = tokio::join!(
            schedule.claim(
                &db,
                &quote,
                FederationPaymentObservationTrigger::BackendNotified,
                1_000,
            ),
            schedule.claim(
                &db,
                &quote,
                FederationPaymentObservationTrigger::OperationAccepted(1),
                1_000,
            ),
        );

        let permits = [first.expect("first claim"), second.expect("second claim")]
            .into_iter()
            .filter(Option::is_some)
            .count();
        assert_eq!(permits, 1);
    }

    #[tokio::test]
    async fn mint_and_melt_schedules_survive_restart_and_keep_separate_deadlines() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let mint = FederationPaymentQuote::mint(QuoteId::new());
        let melt = FederationPaymentQuote::melt(QuoteId::new());
        let first = FederationPaymentObservationSchedule::default();

        for quote in [&mint, &melt] {
            let permit = first
                .claim(
                    &db,
                    quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    1_000,
                )
                .await
                .expect("claim schedule")
                .expect("first attempt is due");
            first
                .finish(
                    &db,
                    permit,
                    member_id(),
                    FederationPaymentAdvancement::AwaitingExternalEvidence,
                    false,
                    Some(10),
                    2_000,
                )
                .await
                .expect("finish schedule");
        }

        let persisted = load_schedules(&db).await.expect("load schedule");
        assert_eq!(persisted.quotes.len(), 2);
        assert!(persisted.quotes.contains_key(&mint.schedule_key()));
        assert!(persisted.quotes.contains_key(&melt.schedule_key()));

        let restarted = FederationPaymentObservationSchedule::default();
        assert!(restarted
            .candidates(
                &db,
                &[mint.clone(), melt.clone()],
                FederationPaymentObservationTrigger::Scheduled,
                5_999,
            )
            .await
            .expect("scheduled candidates")
            .is_empty());
        assert_eq!(
            restarted
                .candidates(
                    &db,
                    &[mint, melt],
                    FederationPaymentObservationTrigger::Requested,
                    6_000,
                )
                .await
                .expect("requested candidates")
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn legacy_mint_schedule_migrates_into_the_shared_namespace() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let quote = QuoteId::new();
        let mut quotes = BTreeMap::new();
        quotes.insert(
            quote.to_string(),
            serde_json::json!({
                "consecutive_no_progress": 4,
                "last_attempt_at_ms": 1_000,
                "next_attempt_at_ms": 41_000,
                "lease_until_ms": 0,
                "generation": 2
            }),
        );
        let legacy = serde_json::json!({
            "version": 1,
            "quotes": quotes
        });
        let bytes = serde_json::to_vec(&legacy).expect("legacy schedule JSON");
        let mut tx = db.begin_transaction().await.expect("begin transaction");
        tx.kv_write(
            PRIMARY_NAMESPACE,
            SECONDARY_NAMESPACE,
            LEGACY_MINT_SCHEDULE_KEY,
            &bytes,
        )
        .await
        .expect("write legacy schedule");
        tx.commit().await.expect("commit legacy schedule");

        let schedules = load_schedules(&db).await.expect("load migrated schedule");
        let migrated = schedules
            .quotes
            .get(&FederationPaymentQuote::mint(quote).schedule_key())
            .expect("migrated mint schedule");
        assert_eq!(migrated.consecutive_no_progress, 4);
        assert_eq!(migrated.next_attempt_at_ms, 41_000);
        assert_eq!(migrated.wait, PersistedPaymentWait::Ready);
    }

    #[tokio::test]
    async fn federation_wait_uses_slow_repair_without_changing_external_counter() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let quote = FederationPaymentQuote::mint(QuoteId::new());
        let schedule = FederationPaymentObservationSchedule::default();
        let permit = schedule
            .claim(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                1_000,
            )
            .await
            .expect("claim")
            .expect("permit");
        schedule
            .finish(
                &db,
                permit,
                member_id(),
                FederationPaymentAdvancement::AwaitingFederationEvidence,
                false,
                Some(10),
                2_000,
            )
            .await
            .expect("finish");

        let persisted = load_schedules(&db).await.expect("load");
        let persisted = persisted
            .quotes
            .get(&quote.schedule_key())
            .expect("quote schedule");
        assert_eq!(persisted.wait, PersistedPaymentWait::Federation);
        assert_eq!(persisted.consecutive_no_progress, 0);
        assert_eq!(
            persisted.next_attempt_at_ms,
            2_000 + FEDERATION_REPAIR_DELAY_MS
        );
    }

    #[tokio::test]
    async fn complete_removes_the_exact_quote_schedule() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let completed = FederationPaymentQuote::mint(QuoteId::new());
        let pending = FederationPaymentQuote::melt(QuoteId::new());
        let schedule = FederationPaymentObservationSchedule::default();

        for quote in [&completed, &pending] {
            let permit = schedule
                .claim(
                    &db,
                    quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    1_000,
                )
                .await
                .expect("claim")
                .expect("permit");
            schedule
                .finish(
                    &db,
                    permit,
                    member_id(),
                    if quote == &completed {
                        FederationPaymentAdvancement::Complete
                    } else {
                        FederationPaymentAdvancement::AwaitingExternalEvidence
                    },
                    false,
                    Some(10),
                    2_000,
                )
                .await
                .expect("finish");
        }

        let persisted = load_schedules(&db).await.expect("load");
        assert!(!persisted.quotes.contains_key(&completed.schedule_key()));
        assert!(persisted.quotes.contains_key(&pending.schedule_key()));
    }
}

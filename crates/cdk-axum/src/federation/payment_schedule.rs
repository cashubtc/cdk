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
const SCHEDULE_VERSION: u8 = 4;

const BASE_DELAY_MS: u64 = 5_000;
const MAX_DELAY_MS: u64 = 5 * 60 * 1_000;
const MINT_REQUEST_COOLDOWN_MS: u64 = 5_000;
const ATTEMPT_LEASE_MS: u64 = 15_000;
const FEDERATION_REPAIR_DELAY_MS: u64 = 60_000;
const OBSERVER_FALLBACK_DELAY_MS: u64 = 6_000;
const BACKEND_NOTIFICATION_RECONNECT_DELAY_MS: u64 = 100;
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
    #[cfg_attr(not(feature = "federation-bark"), allow(dead_code))]
    AwaitingBackendNotification,
    AwaitingFederationEvidence,
    #[cfg_attr(not(feature = "federation-bark"), allow(dead_code))]
    AwaitingObserverFallback,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FederationPaymentObservationTrigger {
    Scheduled,
    Requested,
    BackendNotified,
    OperationAccepted(u64),
}

impl FederationPaymentObservationTrigger {
    pub(super) const fn wait_for_backend_update(self) -> bool {
        !matches!(self, Self::BackendNotified)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FederationPaymentObservationPermit {
    quote: String,
    generation: u64,
    observer_fallback_round: u32,
    previous_wait: PersistedPaymentWait,
    previous_next_attempt_at_ms: u64,
}

impl FederationPaymentObservationPermit {
    pub(super) fn observer_fallback_round(&self) -> u32 {
        self.observer_fallback_round
    }
}

/// Progress from one bounded attempt, used to update its durable retry schedule.
#[derive(Debug, Clone, Copy)]
pub(super) struct FederationPaymentObservationProgress {
    pub(super) advancement: FederationPaymentAdvancement,
    pub(super) made_progress: bool,
    pub(super) last_processed_operation_index: Option<u64>,
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

    pub(super) async fn scan_cursor(
        &self,
        db: &DynMintDatabase,
        kind: FederationPaymentQuoteKind,
    ) -> Result<Option<QuoteId>, cdk::Error> {
        let schedules = load_schedules(db).await?;
        Ok(match kind {
            FederationPaymentQuoteKind::Mint => schedules.mint_scan_after,
            FederationPaymentQuoteKind::Melt => schedules.melt_scan_after,
        })
    }

    pub(super) async fn set_scan_cursor(
        &self,
        db: &DynMintDatabase,
        kind: FederationPaymentQuoteKind,
        cursor: Option<QuoteId>,
    ) -> Result<(), cdk::Error> {
        let _guard = self.mutation_lock.lock().await;
        let mut schedules = load_schedules(db).await?;
        let stored = match kind {
            FederationPaymentQuoteKind::Mint => &mut schedules.mint_scan_after,
            FederationPaymentQuoteKind::Melt => &mut schedules.melt_scan_after,
        };
        if *stored != cursor {
            *stored = cursor;
            store_schedules(db, &schedules).await?;
        }
        Ok(())
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

        let previous_wait = schedule.wait;
        let previous_next_attempt_at_ms = schedule.next_attempt_at_ms;
        match schedule.wait {
            PersistedPaymentWait::ObserverFallback if now_ms >= schedule.next_attempt_at_ms => {
                schedule.observer_fallback_round =
                    schedule.observer_fallback_round.saturating_add(1);
            }
            PersistedPaymentWait::BackendNotification
                if schedule.next_observer_fallback_at_ms > 0
                    && now_ms >= schedule.next_observer_fallback_at_ms =>
            {
                schedule.observer_fallback_round =
                    schedule.observer_fallback_round.saturating_add(1);
                schedule.next_observer_fallback_at_ms =
                    now_ms.saturating_add(OBSERVER_FALLBACK_DELAY_MS);
            }
            _ => {}
        }
        schedule.generation = schedule.generation.saturating_add(1);
        schedule.last_attempt_at_ms = now_ms;
        schedule.lease_until_ms = now_ms.saturating_add(ATTEMPT_LEASE_MS);
        schedule.next_attempt_at_ms = schedule.lease_until_ms;
        let generation = schedule.generation;
        let observer_fallback_round = schedule.observer_fallback_round;
        store_schedules(db, &schedules).await?;
        self.notify_changed();

        Ok(Some(FederationPaymentObservationPermit {
            quote,
            generation,
            observer_fallback_round,
            previous_wait,
            previous_next_attempt_at_ms,
        }))
    }

    pub(super) async fn finish(
        &self,
        db: &DynMintDatabase,
        permit: FederationPaymentObservationPermit,
        member_id: MemberId,
        progress: FederationPaymentObservationProgress,
        now_ms: u64,
    ) -> Result<(), cdk::Error> {
        let FederationPaymentObservationProgress {
            advancement,
            made_progress,
            last_processed_operation_index,
        } = progress;
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
        if made_progress {
            schedule.observer_fallback_round = 0;
            schedule.next_observer_fallback_at_ms = 0;
        }
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
                schedule.observer_fallback_round = 0;
                schedule.next_observer_fallback_at_ms = 0;
                schedule.next_attempt_at_ms = now_ms;
            }
            FederationPaymentAdvancement::AwaitingExternalEvidence => {
                schedule.wait = PersistedPaymentWait::External;
                schedule.next_observer_fallback_at_ms = 0;
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
            FederationPaymentAdvancement::AwaitingBackendNotification => {
                schedule.wait = PersistedPaymentWait::BackendNotification;
                schedule.consecutive_no_progress = 0;
                if schedule.next_observer_fallback_at_ms == 0 {
                    schedule.next_observer_fallback_at_ms =
                        now_ms.saturating_add(OBSERVER_FALLBACK_DELAY_MS);
                }
                schedule.next_attempt_at_ms =
                    now_ms.saturating_add(BACKEND_NOTIFICATION_RECONNECT_DELAY_MS);
            }
            FederationPaymentAdvancement::AwaitingFederationEvidence => {
                schedule.wait = PersistedPaymentWait::Federation;
                schedule.observer_fallback_round = 0;
                schedule.next_observer_fallback_at_ms = 0;
                schedule.next_attempt_at_ms = now_ms.saturating_add(FEDERATION_REPAIR_DELAY_MS);
            }
            FederationPaymentAdvancement::AwaitingObserverFallback => {
                schedule.wait = PersistedPaymentWait::ObserverFallback;
                schedule.next_observer_fallback_at_ms = 0;
                schedule.consecutive_no_progress = 0;
                schedule.next_attempt_at_ms = if !made_progress
                    && permit.previous_wait == PersistedPaymentWait::ObserverFallback
                    && permit.previous_next_attempt_at_ms > now_ms
                {
                    permit.previous_next_attempt_at_ms
                } else {
                    now_ms.saturating_add(OBSERVER_FALLBACK_DELAY_MS)
                };
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
    #[serde(default)]
    mint_scan_after: Option<QuoteId>,
    #[serde(default)]
    melt_scan_after: Option<QuoteId>,
}

impl Default for PersistedPaymentSchedules {
    fn default() -> Self {
        Self {
            version: SCHEDULE_VERSION,
            quotes: BTreeMap::new(),
            mint_scan_after: None,
            melt_scan_after: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PersistedPaymentWait {
    #[default]
    Ready,
    External,
    BackendNotification,
    Federation,
    ObserverFallback,
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
    #[serde(default)]
    observer_fallback_round: u32,
    #[serde(default)]
    next_observer_fallback_at_ms: u64,
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

    Ok(PersistedPaymentSchedules::default())
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
            observer_fallback_round: 0,
            next_observer_fallback_at_ms: 0,
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
    async fn backend_notification_wait_reconnects_without_external_backoff() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let quote = FederationPaymentQuote::melt(QuoteId::new());
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
                FederationPaymentObservationProgress {
                    advancement: FederationPaymentAdvancement::AwaitingBackendNotification,
                    made_progress: false,
                    last_processed_operation_index: Some(1),
                },
                2_000,
            )
            .await
            .expect("finish notification wait");

        let persisted = load_schedules(&db).await.expect("load schedule");
        let persisted = persisted
            .quotes
            .get(&quote.schedule_key())
            .expect("quote schedule");
        assert_eq!(persisted.wait, PersistedPaymentWait::BackendNotification);
        assert_eq!(persisted.next_attempt_at_ms, 2_100);
        assert_eq!(persisted.next_observer_fallback_at_ms, 8_000);

        let fallback = schedule
            .claim(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                8_000,
            )
            .await
            .expect("claim fallback")
            .expect("fallback permit");
        assert_eq!(fallback.observer_fallback_round(), 1);
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
                    FederationPaymentObservationProgress {
                        advancement: FederationPaymentAdvancement::AwaitingExternalEvidence,
                        made_progress: false,
                        last_processed_operation_index: Some(10),
                    },
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
    async fn payment_scan_cursors_survive_restart_and_remain_independent() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let mint_cursor = QuoteId::from(uuid::Uuid::from_u128(10));
        let melt_cursor = QuoteId::from(uuid::Uuid::from_u128(20));
        let first = FederationPaymentObservationSchedule::default();
        first
            .set_scan_cursor(
                &db,
                FederationPaymentQuoteKind::Mint,
                Some(mint_cursor.clone()),
            )
            .await
            .expect("persist mint cursor");
        first
            .set_scan_cursor(
                &db,
                FederationPaymentQuoteKind::Melt,
                Some(melt_cursor.clone()),
            )
            .await
            .expect("persist melt cursor");

        let restarted = FederationPaymentObservationSchedule::default();
        assert_eq!(
            restarted
                .scan_cursor(&db, FederationPaymentQuoteKind::Mint)
                .await
                .expect("load mint cursor"),
            Some(mint_cursor)
        );
        assert_eq!(
            restarted
                .scan_cursor(&db, FederationPaymentQuoteKind::Melt)
                .await
                .expect("load melt cursor"),
            Some(melt_cursor.clone())
        );

        restarted
            .set_scan_cursor(&db, FederationPaymentQuoteKind::Mint, None)
            .await
            .expect("wrap mint cursor");
        assert_eq!(
            restarted
                .scan_cursor(&db, FederationPaymentQuoteKind::Mint)
                .await
                .expect("load wrapped mint cursor"),
            None
        );
        assert_eq!(
            restarted
                .scan_cursor(&db, FederationPaymentQuoteKind::Melt)
                .await
                .expect("load retained melt cursor"),
            Some(melt_cursor)
        );
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
                FederationPaymentObservationProgress {
                    advancement: FederationPaymentAdvancement::AwaitingFederationEvidence,
                    made_progress: false,
                    last_processed_operation_index: Some(10),
                },
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
    async fn observer_fallback_expands_only_after_its_persisted_deadline() {
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
        assert_eq!(permit.observer_fallback_round(), 0);
        schedule
            .finish(
                &db,
                permit,
                member_id(),
                FederationPaymentObservationProgress {
                    advancement: FederationPaymentAdvancement::AwaitingObserverFallback,
                    made_progress: false,
                    last_processed_operation_index: Some(10),
                },
                2_000,
            )
            .await
            .expect("finish initial fallback");

        let restarted = FederationPaymentObservationSchedule::default();
        let early = restarted
            .claim(
                &db,
                &quote,
                FederationPaymentObservationTrigger::OperationAccepted(11),
                3_000,
            )
            .await
            .expect("early wake")
            .expect("operation wake permit");
        assert_eq!(early.observer_fallback_round(), 0);
        restarted
            .finish(
                &db,
                early,
                member_id(),
                FederationPaymentObservationProgress {
                    advancement: FederationPaymentAdvancement::AwaitingObserverFallback,
                    made_progress: false,
                    last_processed_operation_index: Some(11),
                },
                3_100,
            )
            .await
            .expect("restore original fallback deadline");

        let persisted = load_schedules(&db).await.expect("load fallback");
        let persisted = persisted
            .quotes
            .get(&quote.schedule_key())
            .expect("quote schedule");
        assert_eq!(persisted.wait, PersistedPaymentWait::ObserverFallback);
        assert_eq!(persisted.observer_fallback_round, 0);
        assert_eq!(persisted.next_attempt_at_ms, 8_000);
        assert!(restarted
            .claim(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                7_999,
            )
            .await
            .expect("not yet due")
            .is_none());

        let expanded = restarted
            .claim(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                8_000,
            )
            .await
            .expect("due fallback")
            .expect("expanded permit");
        assert_eq!(expanded.observer_fallback_round(), 1);
        restarted
            .finish(
                &db,
                expanded,
                member_id(),
                FederationPaymentObservationProgress {
                    advancement: FederationPaymentAdvancement::AwaitingObserverFallback,
                    made_progress: false,
                    last_processed_operation_index: Some(11),
                },
                8_100,
            )
            .await
            .expect("finish expanded fallback");
        let persisted = load_schedules(&db).await.expect("load expanded fallback");
        let persisted = persisted
            .quotes
            .get(&quote.schedule_key())
            .expect("quote schedule");
        assert_eq!(persisted.observer_fallback_round, 1);
        assert_eq!(persisted.next_attempt_at_ms, 14_100);
    }

    #[tokio::test]
    async fn progress_resets_observer_fallback_round() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let quote = FederationPaymentQuote::melt(QuoteId::new());
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
                FederationPaymentObservationProgress {
                    advancement: FederationPaymentAdvancement::AwaitingObserverFallback,
                    made_progress: false,
                    last_processed_operation_index: None,
                },
                2_000,
            )
            .await
            .expect("finish fallback");
        let expanded = schedule
            .claim(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                8_000,
            )
            .await
            .expect("expanded claim")
            .expect("expanded permit");
        assert_eq!(expanded.observer_fallback_round(), 1);
        schedule
            .finish(
                &db,
                expanded,
                member_id(),
                FederationPaymentObservationProgress {
                    advancement: FederationPaymentAdvancement::Progressed,
                    made_progress: true,
                    last_processed_operation_index: Some(12),
                },
                8_100,
            )
            .await
            .expect("finish progress");

        let persisted = load_schedules(&db).await.expect("load reset schedule");
        let persisted = persisted
            .quotes
            .get(&quote.schedule_key())
            .expect("quote schedule");
        assert_eq!(persisted.wait, PersistedPaymentWait::Ready);
        assert_eq!(persisted.observer_fallback_round, 0);
    }

    #[tokio::test]
    async fn unsupported_schedule_versions_rebuild_the_local_cache() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        for version in [1, 2, 3, SCHEDULE_VERSION + 1] {
            let schedules = PersistedPaymentSchedules {
                version,
                quotes: BTreeMap::from([(
                    "mint:obsolete".to_string(),
                    PersistedPaymentSchedule {
                        consecutive_no_progress: 4,
                        next_attempt_at_ms: 41_000,
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            };
            store_schedules(&db, &schedules).await.expect("store cache");
            let rebuilt = load_schedules(&db).await.expect("rebuild cache");
            assert_eq!(rebuilt.version, SCHEDULE_VERSION);
            assert!(rebuilt.quotes.is_empty());
        }
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
                    FederationPaymentObservationProgress {
                        advancement: if quote == &completed {
                            FederationPaymentAdvancement::Complete
                        } else {
                            FederationPaymentAdvancement::AwaitingExternalEvidence
                        },
                        made_progress: false,
                        last_processed_operation_index: Some(10),
                    },
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

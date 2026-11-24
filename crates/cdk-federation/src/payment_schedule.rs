use std::collections::HashMap;
use std::sync::Arc;

pub use cdk::federation::FederationPaymentAdvancement;
use cdk::federation::MemberId;
use cdk::mint::QuoteId;
use cdk_common::database::DynMintDatabase;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PRIMARY_NAMESPACE: &str = "cdk_federation";
/// Scan cursors, one key per quote kind.
const CURSOR_NAMESPACE: &str = "payment_observer";
/// One schedule per quote, keyed by [`FederationPaymentQuote::storage_key`].
const QUOTE_NAMESPACE: &str = "payment_quote_schedule_v5";
const MINT_SCAN_CURSOR_KEY: &str = "mint_scan_after";
const MELT_SCAN_CURSOR_KEY: &str = "melt_scan_after";

const BASE_DELAY_MS: u64 = 5_000;
const MAX_DELAY_MS: u64 = 5 * 60 * 1_000;
const MINT_REQUEST_COOLDOWN_MS: u64 = 5_000;
const ATTEMPT_LEASE_MS: u64 = 15_000;
const FEDERATION_REPAIR_DELAY_MS: u64 = 60_000;
const OBSERVER_FALLBACK_DELAY_MS: u64 = 6_000;
const BACKEND_NOTIFICATION_RECONNECT_DELAY_MS: u64 = 100;
const JITTER_PERCENT: u64 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FederationPaymentQuoteKind {
    Mint,
    Melt,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FederationPaymentQuote {
    pub kind: FederationPaymentQuoteKind,
    pub id: QuoteId,
}

impl FederationPaymentQuote {
    pub fn mint(id: QuoteId) -> Self {
        Self {
            kind: FederationPaymentQuoteKind::Mint,
            id,
        }
    }

    pub fn melt(id: QuoteId) -> Self {
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

    /// KV key for this quote's schedule. Quote IDs may contain characters the
    /// KV store rejects, so the key carries a digest of the schedule key.
    fn storage_key(&self) -> String {
        storage_key(&self.schedule_key())
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
pub enum FederationPaymentObservationTrigger {
    Scheduled,
    Requested,
    BackendNotified,
    OperationAccepted(u64),
}

impl FederationPaymentObservationTrigger {
    pub const fn wait_for_backend_update(self) -> bool {
        !matches!(self, Self::BackendNotified)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationPaymentObservationPermit {
    /// Schedule key; also the jitter seed.
    quote: String,
    generation: u64,
    observer_fallback_round: u32,
    previous_wait: PersistedPaymentWait,
    previous_next_attempt_at_ms: u64,
}

impl FederationPaymentObservationPermit {
    pub fn observer_fallback_round(&self) -> u32 {
        self.observer_fallback_round
    }
}

/// Progress from one bounded attempt, used to update its durable retry schedule.
#[derive(Debug, Clone, Copy)]
pub struct FederationPaymentObservationProgress {
    pub advancement: FederationPaymentAdvancement,
    pub made_progress: bool,
    pub last_processed_operation_index: Option<u64>,
}

/// Durable retry schedule for payment observation, one stored entry per quote.
///
/// Each mutation reads and writes only its own quote's entry under that
/// quote's lock, so quotes never wait on each other.
#[derive(Debug, Clone)]
pub struct FederationPaymentObservationSchedule {
    locks: KeyedLocks,
    changes: tokio::sync::watch::Sender<u64>,
}

impl Default for FederationPaymentObservationSchedule {
    fn default() -> Self {
        let (changes, _) = tokio::sync::watch::channel(0);
        Self {
            locks: KeyedLocks::default(),
            changes,
        }
    }
}

impl FederationPaymentObservationSchedule {
    pub fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub async fn candidates(
        &self,
        db: &DynMintDatabase,
        quotes: &[FederationPaymentQuote],
        trigger: FederationPaymentObservationTrigger,
        now_ms: u64,
    ) -> Result<Vec<FederationPaymentQuote>, cdk::Error> {
        let mut due = Vec::with_capacity(quotes.len());
        for quote in quotes {
            let is_due = load_schedule(db, &quote.storage_key())
                .await?
                .is_none_or(|schedule| {
                    schedule.is_due(trigger, quote.request_cooldown_ms(), now_ms)
                });
            if is_due {
                due.push(quote.clone());
            }
        }
        Ok(due)
    }

    pub async fn scan_cursor(
        &self,
        db: &DynMintDatabase,
        kind: FederationPaymentQuoteKind,
    ) -> Result<Option<QuoteId>, cdk::Error> {
        load_scan_cursor(db, scan_cursor_key(kind)).await
    }

    pub async fn set_scan_cursor(
        &self,
        db: &DynMintDatabase,
        kind: FederationPaymentQuoteKind,
        cursor: Option<QuoteId>,
    ) -> Result<(), cdk::Error> {
        let key = scan_cursor_key(kind);
        let _guard = self.locks.lock(key).await;
        if load_scan_cursor(db, key).await? != cursor {
            store_scan_cursor(db, key, cursor.as_ref()).await?;
        }
        Ok(())
    }

    pub async fn claim(
        &self,
        db: &DynMintDatabase,
        quote: &FederationPaymentQuote,
        trigger: FederationPaymentObservationTrigger,
        now_ms: u64,
    ) -> Result<Option<FederationPaymentObservationPermit>, cdk::Error> {
        let storage_key = quote.storage_key();
        let _guard = self.locks.lock(&storage_key).await;
        let mut schedule = load_schedule(db, &storage_key).await?.unwrap_or_default();
        let request_cooldown_ms = quote.request_cooldown_ms();
        let quote = quote.schedule_key();
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
        store_schedule(db, &storage_key, &schedule).await?;
        self.notify_changed();

        Ok(Some(FederationPaymentObservationPermit {
            quote,
            generation,
            observer_fallback_round,
            previous_wait,
            previous_next_attempt_at_ms,
        }))
    }

    pub async fn finish(
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
        let storage_key = storage_key(&permit.quote);
        let _guard = self.locks.lock(&storage_key).await;
        let Some(mut schedule) = load_schedule(db, &storage_key).await? else {
            return Ok(());
        };
        if schedule.generation != permit.generation {
            return Ok(());
        }

        if advancement == FederationPaymentAdvancement::Complete {
            remove_schedule(db, &storage_key).await?;
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
        store_schedule(db, &storage_key, &schedule).await?;
        self.notify_changed();
        Ok(())
    }

    pub async fn next_attempt_at_ms(
        &self,
        db: &DynMintDatabase,
    ) -> Result<Option<u64>, cdk::Error> {
        let mut next = None::<u64>;
        for key in db.kv_list(PRIMARY_NAMESPACE, QUOTE_NAMESPACE).await? {
            if let Some(schedule) = load_schedule(db, &key).await? {
                let at = schedule.next_attempt_at_ms.max(schedule.lease_until_ms);
                next = Some(next.map_or(at, |next| next.min(at)));
            }
        }
        Ok(next)
    }

    pub async fn active_lease_until_ms(
        &self,
        db: &DynMintDatabase,
        quotes: &[FederationPaymentQuote],
        now_ms: u64,
    ) -> Result<Option<u64>, cdk::Error> {
        let mut earliest = None::<u64>;
        for quote in quotes {
            if let Some(schedule) = load_schedule(db, &quote.storage_key()).await? {
                if schedule.lease_until_ms > now_ms {
                    earliest = Some(earliest.map_or(schedule.lease_until_ms, |earliest| {
                        earliest.min(schedule.lease_until_ms)
                    }));
                }
            }
        }
        Ok(earliest)
    }

    pub async fn remove(
        &self,
        db: &DynMintDatabase,
        quote: &FederationPaymentQuote,
    ) -> Result<(), cdk::Error> {
        let storage_key = quote.storage_key();
        let _guard = self.locks.lock(&storage_key).await;
        if load_schedule(db, &storage_key).await?.is_some() {
            remove_schedule(db, &storage_key).await?;
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

fn storage_key(schedule_key: &str) -> String {
    let (prefix, _) = schedule_key
        .split_once(':')
        .unwrap_or(("quote", schedule_key));
    let digest = Sha256::digest(schedule_key.as_bytes());
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{prefix}_{hex}")
}

const fn scan_cursor_key(kind: FederationPaymentQuoteKind) -> &'static str {
    match kind {
        FederationPaymentQuoteKind::Mint => MINT_SCAN_CURSOR_KEY,
        FederationPaymentQuoteKind::Melt => MELT_SCAN_CURSOR_KEY,
    }
}

/// Load one quote's schedule. The schedule is a local cache of retry timing,
/// so an entry that no longer decodes is dropped and rebuilt.
async fn load_schedule(
    db: &DynMintDatabase,
    key: &str,
) -> Result<Option<PersistedPaymentSchedule>, cdk::Error> {
    let Some(bytes) = db.kv_read(PRIMARY_NAMESPACE, QUOTE_NAMESPACE, key).await? else {
        return Ok(None);
    };
    match serde_json::from_slice(&bytes) {
        Ok(schedule) => Ok(Some(schedule)),
        Err(error) => {
            tracing::warn!(
                error = %error,
                "Ignoring invalid federation payment quote schedule"
            );
            Ok(None)
        }
    }
}

async fn store_schedule(
    db: &DynMintDatabase,
    key: &str,
    schedule: &PersistedPaymentSchedule,
) -> Result<(), cdk::Error> {
    let bytes = serde_json::to_vec(schedule)?;
    let mut tx = db.begin_transaction().await?;
    tx.kv_write(PRIMARY_NAMESPACE, QUOTE_NAMESPACE, key, &bytes)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn remove_schedule(db: &DynMintDatabase, key: &str) -> Result<(), cdk::Error> {
    let mut tx = db.begin_transaction().await?;
    tx.kv_remove(PRIMARY_NAMESPACE, QUOTE_NAMESPACE, key)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn load_scan_cursor(db: &DynMintDatabase, key: &str) -> Result<Option<QuoteId>, cdk::Error> {
    let Some(bytes) = db.kv_read(PRIMARY_NAMESPACE, CURSOR_NAMESPACE, key).await? else {
        return Ok(None);
    };
    Ok(serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        tracing::warn!(error = %error, "Ignoring invalid federation payment scan cursor");
        None
    }))
}

async fn store_scan_cursor(
    db: &DynMintDatabase,
    key: &str,
    cursor: Option<&QuoteId>,
) -> Result<(), cdk::Error> {
    let bytes = serde_json::to_vec(&cursor)?;
    let mut tx = db.begin_transaction().await?;
    tx.kv_write(PRIMARY_NAMESPACE, CURSOR_NAMESPACE, key, &bytes)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// One async lock per key, created on first use and dropped when unused.
#[derive(Debug, Clone, Default)]
struct KeyedLocks(Arc<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>);

impl KeyedLocks {
    async fn lock(&self, key: &str) -> KeyedLockGuard {
        let mutex = {
            let mut locks = self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Arc::clone(locks.entry(key.to_owned()).or_default())
        };
        let guard = Arc::clone(&mutex).lock_owned().await;
        KeyedLockGuard {
            locks: self.clone(),
            key: key.to_owned(),
            mutex,
            guard: Some(guard),
        }
    }
}

struct KeyedLockGuard {
    locks: KeyedLocks,
    key: String,
    mutex: Arc<tokio::sync::Mutex<()>>,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl Drop for KeyedLockGuard {
    fn drop(&mut self) {
        self.guard.take();
        let mut locks = self
            .locks
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Only the map and this guard still hold the lock: nobody is waiting.
        if Arc::strong_count(&self.mutex) == 2 {
            locks.remove(&self.key);
        }
    }
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
    use std::str::FromStr;

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

        let persisted = load_schedule(&db, &quote.storage_key())
            .await
            .expect("load schedule")
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

        let stored = db
            .kv_list(PRIMARY_NAMESPACE, QUOTE_NAMESPACE)
            .await
            .expect("list schedules");
        assert_eq!(stored.len(), 2);
        assert!(stored.contains(&mint.storage_key()));
        assert!(stored.contains(&melt.storage_key()));

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

        let persisted = load_schedule(&db, &quote.storage_key())
            .await
            .expect("load")
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

        let persisted = load_schedule(&db, &quote.storage_key())
            .await
            .expect("load fallback")
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
        let persisted = load_schedule(&db, &quote.storage_key())
            .await
            .expect("load expanded fallback")
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

        let persisted = load_schedule(&db, &quote.storage_key())
            .await
            .expect("load reset schedule")
            .expect("quote schedule");
        assert_eq!(persisted.wait, PersistedPaymentWait::Ready);
        assert_eq!(persisted.observer_fallback_round, 0);
    }

    #[tokio::test]
    async fn undecodable_schedules_rebuild_the_local_cache() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let quote = FederationPaymentQuote::mint(QuoteId::new());
        let mut tx = db.begin_transaction().await.expect("transaction");
        tx.kv_write(
            PRIMARY_NAMESPACE,
            QUOTE_NAMESPACE,
            &quote.storage_key(),
            b"{\"obsolete\":true",
        )
        .await
        .expect("write corrupt cache");
        tx.commit().await.expect("commit");

        assert!(load_schedule(&db, &quote.storage_key())
            .await
            .expect("rebuild cache")
            .is_none());
        let schedule = FederationPaymentObservationSchedule::default();
        assert!(schedule
            .claim(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                1_000,
            )
            .await
            .expect("claim")
            .is_some());
    }

    #[tokio::test]
    async fn storage_keys_are_valid_kv_keys_for_any_quote_id() {
        for id in [
            QuoteId::new(),
            QuoteId::from_str("YWJjZA==").expect("base64 quote id"),
        ] {
            for quote in [
                FederationPaymentQuote::mint(id.clone()),
                FederationPaymentQuote::melt(id.clone()),
            ] {
                cdk_common::database::validate_kvstore_string(&quote.storage_key())
                    .expect("storage key is a valid KV key");
            }
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

        assert!(load_schedule(&db, &completed.storage_key())
            .await
            .expect("load")
            .is_none());
        assert!(load_schedule(&db, &pending.storage_key())
            .await
            .expect("load")
            .is_some());
    }
}

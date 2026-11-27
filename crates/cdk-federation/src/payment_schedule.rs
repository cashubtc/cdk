use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::str::FromStr;
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

const BASE_DELAY_MS: u64 = 1_000;
const MAX_DELAY_MS: u64 = 5 * 60 * 1_000;
const MINT_REQUEST_COOLDOWN_MS: u64 = 1_000;
const ATTEMPT_LEASE_MS: u64 = 15_000;
const FEDERATION_REPAIR_DELAY_MS: u64 = 60_000;
const OBSERVER_FALLBACK_DELAY_MS: u64 = 2_000;
const BACKEND_NOTIFICATION_RECONNECT_DELAY_MS: u64 = 100;
const JITTER_PERCENT: u64 = 20;
const HEAP_COMPACTION_MIN_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationPaymentQuoteKind {
    Mint,
    Melt,
}

impl FederationPaymentQuoteKind {
    /// Stable lowercase name for logs and schedule keys.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mint => "mint",
            Self::Melt => "melt",
        }
    }
}

/// Bounded execution lane for a scheduled payment observation task.
///
/// Isolating unpaid incoming invoice waits from active incoming claims,
/// outgoing melts, and direct `MintPayment` probes prevents long-polling
/// unpaid invoices from starving settlement or causing busy-spins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationPaymentScheduleLane {
    /// Unpaid Bark incoming mint quotes awaiting external payment.
    IncomingWait,
    /// Bark incoming mint quotes with payment activity or active claim work.
    IncomingClaim,
    /// Bark outgoing melt quotes.
    OutgoingMelt,
    /// Generic `MintPayment` / non-Bark quotes driven directly.
    Direct,
}

impl FederationPaymentScheduleLane {
    /// Every execution lane in stable order.
    pub const ALL: [Self; 4] = [
        Self::IncomingWait,
        Self::IncomingClaim,
        Self::OutgoingMelt,
        Self::Direct,
    ];

    /// Fixed array index for per-lane heaps and saturation flags.
    pub const fn index(self) -> usize {
        match self {
            Self::IncomingWait => 0,
            Self::IncomingClaim => 1,
            Self::OutgoingMelt => 2,
            Self::Direct => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
        format!("{}:{}", self.kind.as_str(), self.id)
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
/// quote's lock, so quotes never wait on each other. An in-memory per-lane
/// priority queue (`BinaryHeap`) mirrors the durable entries after a one-time
/// hydration from the database so computing the next wakeup deadline and
/// selecting due quotes are O(1)/O(log N) without scanning the KV store.
#[derive(Debug, Clone)]
pub struct FederationPaymentObservationSchedule {
    locks: KeyedLocks,
    changes: tokio::sync::watch::Sender<u64>,
    hydrate_lock: Arc<tokio::sync::Mutex<()>>,
    index: Arc<std::sync::Mutex<ScheduleIndex>>,
}

impl Default for FederationPaymentObservationSchedule {
    fn default() -> Self {
        let (changes, _) = tokio::sync::watch::channel(0);
        Self {
            locks: KeyedLocks::default(),
            changes,
            hydrate_lock: Arc::new(tokio::sync::Mutex::new(())),
            index: Arc::new(std::sync::Mutex::new(ScheduleIndex::default())),
        }
    }
}

impl FederationPaymentObservationSchedule {
    pub fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// Hydrate the in-memory priority queue from persisted KV entries once.
    ///
    /// Safe to call repeatedly or concurrently with `claim`, `finish`, and
    /// `remove`: any entry already mutated in memory before hydration finishes
    /// keeps its fresher state.
    pub async fn hydrate(&self, db: &DynMintDatabase) -> Result<(), cdk::Error> {
        if self.lock_index().hydrated {
            return Ok(());
        }
        let _hydrate_guard = self.hydrate_lock.lock().await;
        if self.lock_index().hydrated {
            return Ok(());
        }

        let keys = db.kv_list(PRIMARY_NAMESPACE, QUOTE_NAMESPACE).await?;
        let mut loaded = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(schedule) = load_schedule(db, &key).await? {
                loaded.push((key, schedule));
            }
        }
        self.lock_index().apply_hydrated(loaded);
        Ok(())
    }

    /// Whether a quote has already recorded payment or claim activity.
    pub async fn has_payment_activity(
        &self,
        db: &DynMintDatabase,
        quote: &FederationPaymentQuote,
    ) -> Result<bool, cdk::Error> {
        self.hydrate(db).await?;
        let storage_key = quote.storage_key();
        let index = self.lock_index();
        Ok(index
            .entries
            .get(&storage_key)
            .is_some_and(|entry| entry.schedule.has_active_claim_progress()))
    }

    pub async fn candidates(
        &self,
        db: &DynMintDatabase,
        quotes: &[FederationPaymentQuote],
        trigger: FederationPaymentObservationTrigger,
        now_ms: u64,
    ) -> Result<Vec<FederationPaymentQuote>, cdk::Error> {
        self.hydrate(db).await?;
        let index = self.lock_index();
        let mut due = Vec::with_capacity(quotes.len());
        for quote in quotes {
            let is_due = index.entries.get(&quote.storage_key()).is_none_or(|entry| {
                entry
                    .schedule
                    .is_due(trigger, quote.request_cooldown_ms(), now_ms)
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
        let lane = default_lane_for_quote(quote, trigger);
        self.claim_in_lane(db, quote, trigger, lane, now_ms).await
    }

    /// Claim an attempt lease for `quote` in `lane`.
    ///
    /// If the quote currently holds an active lease and a streaming backend
    /// notification or newer operation arrives, records a pending immediate
    /// recheck on the durable schedule so the wakeup is never lost once the
    /// active attempt finishes.
    pub async fn claim_in_lane(
        &self,
        db: &DynMintDatabase,
        quote: &FederationPaymentQuote,
        trigger: FederationPaymentObservationTrigger,
        lane: FederationPaymentScheduleLane,
        now_ms: u64,
    ) -> Result<Option<FederationPaymentObservationPermit>, cdk::Error> {
        let storage_key = quote.storage_key();
        let _guard = self.locks.lock(&storage_key).await;
        let mut schedule = load_schedule(db, &storage_key).await?.unwrap_or_default();
        let request_cooldown_ms = quote.request_cooldown_ms();
        let quote_key = quote.schedule_key();
        if !schedule.is_due(trigger, request_cooldown_ms, now_ms) {
            if now_ms < schedule.lease_until_ms
                && record_deferred_wake(&mut schedule, quote, trigger, lane)
            {
                store_schedule(db, &storage_key, &schedule).await?;
                self.lock_index()
                    .upsert(storage_key, Some(quote.clone()), schedule);
            }
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
        schedule.bind_quote_and_lane(quote, lane, trigger);
        schedule.pending_backend_notification = false;
        schedule.pending_operation_index = None;
        schedule.generation = schedule.generation.saturating_add(1);
        schedule.last_attempt_at_ms = now_ms;
        schedule.lease_until_ms = now_ms.saturating_add(ATTEMPT_LEASE_MS);
        schedule.next_attempt_at_ms = schedule.lease_until_ms;
        let generation = schedule.generation;
        let observer_fallback_round = schedule.observer_fallback_round;
        store_schedule(db, &storage_key, &schedule).await?;
        self.lock_index()
            .upsert(storage_key, Some(quote.clone()), schedule);
        self.notify_changed();

        Ok(Some(FederationPaymentObservationPermit {
            quote: quote_key,
            generation,
            observer_fallback_round,
            previous_wait,
            previous_next_attempt_at_ms,
        }))
    }

    /// Record that a quote is due immediately without leasing it.
    ///
    /// Used when an event-driven wake (`BackendNotified` or `OperationAccepted`)
    /// encounters a saturated task lane: pulling `next_attempt_at_ms` forward to
    /// `now_ms` ensures the quote is picked up first as soon as a permit opens
    /// in its lane, without violating the unleased-when-full invariant.
    pub async fn mark_due_now(
        &self,
        db: &DynMintDatabase,
        quote: &FederationPaymentQuote,
        lane: FederationPaymentScheduleLane,
        trigger: FederationPaymentObservationTrigger,
        now_ms: u64,
    ) -> Result<(), cdk::Error> {
        let storage_key = quote.storage_key();
        let _guard = self.locks.lock(&storage_key).await;
        let mut schedule = load_schedule(db, &storage_key).await?.unwrap_or_default();
        schedule.bind_quote_and_lane(quote, lane, trigger);
        if now_ms < schedule.lease_until_ms {
            record_deferred_wake(&mut schedule, quote, trigger, lane);
        } else {
            if trigger == FederationPaymentObservationTrigger::BackendNotified {
                schedule.wait = PersistedPaymentWait::Ready;
                schedule.next_attempt_at_ms = now_ms;
            } else if schedule.wait != PersistedPaymentWait::ObserverFallback {
                schedule.next_attempt_at_ms = schedule.next_attempt_at_ms.min(now_ms);
            }
        }
        store_schedule(db, &storage_key, &schedule).await?;
        self.lock_index()
            .upsert(storage_key, Some(quote.clone()), schedule);
        self.notify_changed();
        Ok(())
    }

    /// Reconcile a quote's indexed and persisted lane without leasing it or
    /// changing its scheduled wakeup time.
    ///
    /// Prevents a busy-spin when a due quote indexed in an available lane
    /// (for example a pre-upgrade KV row) classifies into a currently
    /// saturated lane and hits `CapacityReached`.
    pub async fn reconcile_lane(
        &self,
        db: &DynMintDatabase,
        quote: &FederationPaymentQuote,
        lane: FederationPaymentScheduleLane,
    ) -> Result<(), cdk::Error> {
        self.hydrate(db).await?;
        let storage_key = quote.storage_key();
        let needs_reconcile = self
            .lock_index()
            .entries
            .get(&storage_key)
            .is_some_and(|entry| entry.lane != lane);
        if !needs_reconcile {
            return Ok(());
        }
        let _guard = self.locks.lock(&storage_key).await;
        let Some(mut schedule) = load_schedule(db, &storage_key).await? else {
            self.lock_index().remove(&storage_key);
            return Ok(());
        };
        schedule.bind_quote_and_lane(quote, lane, FederationPaymentObservationTrigger::Scheduled);
        store_schedule(db, &storage_key, &schedule).await?;
        self.lock_index()
            .upsert(storage_key, Some(quote.clone()), schedule);
        self.notify_changed();
        Ok(())
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
            self.lock_index().remove(&storage_key);
            return Ok(());
        };
        if schedule.generation != permit.generation {
            return Ok(());
        }

        if advancement == FederationPaymentAdvancement::Complete {
            remove_schedule(db, &storage_key).await?;
            self.lock_index().remove(&storage_key);
            self.notify_changed();
            return Ok(());
        }

        schedule.lease_until_ms = 0;
        if made_progress || advancement == FederationPaymentAdvancement::Progressed {
            schedule.observer_fallback_round = 0;
            schedule.next_observer_fallback_at_ms = 0;
            schedule.has_payment_activity = true;
            if schedule.lane == Some(FederationPaymentScheduleLane::IncomingWait) {
                schedule.lane = Some(FederationPaymentScheduleLane::IncomingClaim);
            }
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

        // Honor any streaming backend notification or newer accepted operation
        // that arrived while this attempt held the quote's lease.
        if schedule.pending_backend_notification {
            schedule.pending_backend_notification = false;
            schedule.pending_operation_index = None;
            schedule.wait = PersistedPaymentWait::Ready;
            schedule.next_attempt_at_ms = now_ms;
        } else if let Some(pending_index) = schedule.pending_operation_index.take() {
            let unseen = schedule
                .last_processed_operation_index
                .is_none_or(|processed| pending_index > processed);
            if unseen && advancement != FederationPaymentAdvancement::AwaitingObserverFallback {
                schedule.next_attempt_at_ms = now_ms;
            }
        }

        let quote = schedule.payment_quote();
        store_schedule(db, &storage_key, &schedule).await?;
        self.lock_index().upsert(storage_key, quote, schedule);
        self.notify_changed();
        Ok(())
    }

    pub async fn next_attempt_at_ms(
        &self,
        db: &DynMintDatabase,
    ) -> Result<Option<u64>, cdk::Error> {
        self.next_attempt_at_ms_for_lanes(db, &FederationPaymentScheduleLane::ALL)
            .await
    }

    /// Return the earliest scheduled wakeup across `lanes` in amortized O(1).
    pub async fn next_attempt_at_ms_for_lanes(
        &self,
        db: &DynMintDatabase,
        lanes: &[FederationPaymentScheduleLane],
    ) -> Result<Option<u64>, cdk::Error> {
        if lanes.is_empty() {
            return Ok(None);
        }
        self.hydrate(db).await?;
        Ok(self.lock_index().next_attempt_at_ms_for_lanes(lanes))
    }

    /// Return up to `limit` quotes due at or before `now_ms` across `lanes`,
    /// ordered by earliest deadline first (FIFO on equal deadlines).
    pub async fn due_quotes_for_lanes(
        &self,
        db: &DynMintDatabase,
        lanes: &[FederationPaymentScheduleLane],
        now_ms: u64,
        limit: usize,
    ) -> Result<Vec<FederationPaymentQuote>, cdk::Error> {
        if lanes.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        self.hydrate(db).await?;
        Ok(self.lock_index().due_quotes_for_lanes(lanes, now_ms, limit))
    }

    pub async fn active_lease_until_ms(
        &self,
        db: &DynMintDatabase,
        quotes: &[FederationPaymentQuote],
        now_ms: u64,
    ) -> Result<Option<u64>, cdk::Error> {
        self.hydrate(db).await?;
        let index = self.lock_index();
        let mut earliest = None::<u64>;
        for quote in quotes {
            if let Some(entry) = index.entries.get(&quote.storage_key()) {
                if entry.schedule.lease_until_ms > now_ms {
                    earliest = Some(earliest.map_or(entry.schedule.lease_until_ms, |earliest| {
                        earliest.min(entry.schedule.lease_until_ms)
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
        let existed_in_db = load_schedule(db, &storage_key).await?.is_some();
        if existed_in_db {
            remove_schedule(db, &storage_key).await?;
        }
        let existed_in_index = self.lock_index().remove(&storage_key);
        if existed_in_db || existed_in_index {
            self.notify_changed();
        }
        Ok(())
    }

    fn lock_index(&self) -> std::sync::MutexGuard<'_, ScheduleIndex> {
        self.index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn notify_changed(&self) {
        self.changes.send_modify(|generation| {
            *generation = generation.saturating_add(1);
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HeapEntry {
    deadline_ms: u64,
    version: u64,
    storage_key: String,
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.deadline_ms
            .cmp(&other.deadline_ms)
            .then_with(|| self.version.cmp(&other.version))
            .then_with(|| self.storage_key.cmp(&other.storage_key))
    }
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone)]
struct IndexedScheduleEntry {
    version: u64,
    lane: FederationPaymentScheduleLane,
    quote: Option<FederationPaymentQuote>,
    schedule: PersistedPaymentSchedule,
}

#[derive(Debug, Default)]
struct ScheduleIndex {
    hydrated: bool,
    next_version: u64,
    entries: HashMap<String, IndexedScheduleEntry>,
    heaps: [BinaryHeap<Reverse<HeapEntry>>; 4],
    /// Storage keys mutated or removed before initial DB hydration finished.
    /// Hydration skips these keys so stale DB reads never overwrite fresher
    /// in-memory state.
    touched_before_hydration: HashSet<String>,
}

impl ScheduleIndex {
    fn upsert(
        &mut self,
        storage_key: String,
        quote: Option<FederationPaymentQuote>,
        schedule: PersistedPaymentSchedule,
    ) {
        if !self.hydrated {
            self.touched_before_hydration.insert(storage_key.clone());
        }
        let existing_quote = self
            .entries
            .get(&storage_key)
            .and_then(|entry| entry.quote.clone());
        let resolved_quote = quote
            .or_else(|| schedule.payment_quote())
            .or(existing_quote);
        let fallback_kind = resolved_quote
            .as_ref()
            .map(|quote| quote.kind)
            .or_else(|| kind_from_storage_key(&storage_key));
        let lane = schedule.effective_lane(fallback_kind);
        let deadline_ms = schedule.deadline_ms();
        self.next_version = self.next_version.saturating_add(1);
        let version = self.next_version;

        self.entries.insert(
            storage_key.clone(),
            IndexedScheduleEntry {
                version,
                lane,
                quote: resolved_quote,
                schedule,
            },
        );
        self.heaps[lane.index()].push(Reverse(HeapEntry {
            deadline_ms,
            version,
            storage_key,
        }));
        self.maybe_compact_lane(lane);
    }

    fn remove(&mut self, storage_key: &str) -> bool {
        if !self.hydrated {
            self.touched_before_hydration.insert(storage_key.to_owned());
        }
        self.entries.remove(storage_key).is_some()
    }

    fn apply_hydrated(&mut self, loaded: Vec<(String, PersistedPaymentSchedule)>) {
        if self.hydrated {
            return;
        }
        for (storage_key, schedule) in loaded {
            if self.touched_before_hydration.contains(&storage_key) {
                continue;
            }
            let quote = schedule.payment_quote();
            let fallback_kind = quote
                .as_ref()
                .map(|quote| quote.kind)
                .or_else(|| kind_from_storage_key(&storage_key));
            let lane = schedule.effective_lane(fallback_kind);
            let deadline_ms = schedule.deadline_ms();
            self.next_version = self.next_version.saturating_add(1);
            let version = self.next_version;

            self.entries.insert(
                storage_key.clone(),
                IndexedScheduleEntry {
                    version,
                    lane,
                    quote,
                    schedule,
                },
            );
            self.heaps[lane.index()].push(Reverse(HeapEntry {
                deadline_ms,
                version,
                storage_key,
            }));
        }
        self.touched_before_hydration.clear();
        self.hydrated = true;
    }

    fn peek_valid_top(&mut self, lane: FederationPaymentScheduleLane) -> Option<&HeapEntry> {
        let heap = &mut self.heaps[lane.index()];
        while let Some(Reverse(top)) = heap.peek() {
            let valid = self
                .entries
                .get(&top.storage_key)
                .is_some_and(|entry| entry.version == top.version && entry.lane == lane);
            if valid {
                break;
            }
            heap.pop();
        }
        self.heaps[lane.index()].peek().map(|entry| &entry.0)
    }

    fn next_attempt_at_ms_for_lanes(
        &mut self,
        lanes: &[FederationPaymentScheduleLane],
    ) -> Option<u64> {
        let mut earliest = None::<u64>;
        for &lane in lanes {
            if let Some(top) = self.peek_valid_top(lane) {
                earliest =
                    Some(earliest.map_or(top.deadline_ms, |current| current.min(top.deadline_ms)));
            }
        }
        earliest
    }

    fn due_quotes_for_lanes(
        &mut self,
        lanes: &[FederationPaymentScheduleLane],
        now_ms: u64,
        limit: usize,
    ) -> Vec<FederationPaymentQuote> {
        let mut due = Vec::with_capacity(limit.min(self.entries.len()));
        let mut popped = Vec::new();
        let max_pops = limit.saturating_mul(2).max(HEAP_COMPACTION_MIN_LEN);

        while due.len() < limit && popped.len() < max_pops {
            let mut best = None::<(FederationPaymentScheduleLane, u64, u64)>;
            for &lane in lanes {
                if let Some(top) = self.peek_valid_top(lane) {
                    if top.deadline_ms <= now_ms {
                        let candidate = (lane, top.deadline_ms, top.version);
                        if best.is_none_or(|(_, best_deadline, best_version)| {
                            (top.deadline_ms, top.version) < (best_deadline, best_version)
                        }) {
                            best = Some(candidate);
                        }
                    }
                }
            }
            let Some((best_lane, _, _)) = best else {
                break;
            };
            let Some(Reverse(entry)) = self.heaps[best_lane.index()].pop() else {
                break;
            };
            if let Some(quote) = self
                .entries
                .get(&entry.storage_key)
                .and_then(|indexed| indexed.quote.clone())
            {
                due.push(quote);
                popped.push((best_lane, entry));
            } else {
                // Legacy KV entries without stored quote metadata trigger one
                // fallback DB scan; dropping them from the in-memory index
                // prevents an orphaned pre-upgrade row from spinning the loop.
                self.entries.remove(&entry.storage_key);
            }
        }

        for (lane, entry) in popped {
            self.heaps[lane.index()].push(Reverse(entry));
        }

        due
    }

    fn maybe_compact_lane(&mut self, lane: FederationPaymentScheduleLane) {
        let threshold = self
            .entries
            .len()
            .saturating_mul(4)
            .max(HEAP_COMPACTION_MIN_LEN);
        if self.heaps[lane.index()].len() <= threshold {
            return;
        }
        let mut rebuilt = BinaryHeap::new();
        for (storage_key, entry) in &self.entries {
            if entry.lane == lane {
                rebuilt.push(Reverse(HeapEntry {
                    deadline_ms: entry.schedule.deadline_ms(),
                    version: entry.version,
                    storage_key: storage_key.clone(),
                }));
            }
        }
        self.heaps[lane.index()] = rebuilt;
    }
}

fn default_lane_for_quote(
    quote: &FederationPaymentQuote,
    trigger: FederationPaymentObservationTrigger,
) -> FederationPaymentScheduleLane {
    match quote.kind {
        FederationPaymentQuoteKind::Melt => FederationPaymentScheduleLane::OutgoingMelt,
        FederationPaymentQuoteKind::Mint => match trigger {
            FederationPaymentObservationTrigger::BackendNotified => {
                FederationPaymentScheduleLane::IncomingClaim
            }
            FederationPaymentObservationTrigger::Scheduled
            | FederationPaymentObservationTrigger::Requested
            | FederationPaymentObservationTrigger::OperationAccepted(_) => {
                FederationPaymentScheduleLane::IncomingWait
            }
        },
    }
}

fn record_deferred_wake(
    schedule: &mut PersistedPaymentSchedule,
    quote: &FederationPaymentQuote,
    trigger: FederationPaymentObservationTrigger,
    lane: FederationPaymentScheduleLane,
) -> bool {
    match trigger {
        FederationPaymentObservationTrigger::BackendNotified => {
            schedule.bind_quote_and_lane(quote, lane, trigger);
            schedule.pending_backend_notification = true;
            true
        }
        FederationPaymentObservationTrigger::OperationAccepted(index)
            if schedule
                .last_processed_operation_index
                .is_none_or(|processed| index > processed) =>
        {
            schedule.bind_quote_and_lane(quote, lane, trigger);
            schedule.pending_operation_index = Some(
                schedule
                    .pending_operation_index
                    .map_or(index, |current| current.max(index)),
            );
            true
        }
        FederationPaymentObservationTrigger::Scheduled
        | FederationPaymentObservationTrigger::Requested
        | FederationPaymentObservationTrigger::OperationAccepted(_) => false,
    }
}

fn kind_from_storage_key(storage_key: &str) -> Option<FederationPaymentQuoteKind> {
    if storage_key.starts_with("mint_") {
        Some(FederationPaymentQuoteKind::Mint)
    } else if storage_key.starts_with("melt_") {
        Some(FederationPaymentQuoteKind::Melt)
    } else {
        None
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
    #[serde(default)]
    quote_kind: Option<FederationPaymentQuoteKind>,
    #[serde(default)]
    quote_id: Option<String>,
    #[serde(default)]
    lane: Option<FederationPaymentScheduleLane>,
    #[serde(default)]
    has_payment_activity: bool,
    #[serde(default)]
    pending_backend_notification: bool,
    #[serde(default)]
    pending_operation_index: Option<u64>,
}

impl PersistedPaymentSchedule {
    fn deadline_ms(&self) -> u64 {
        self.next_attempt_at_ms.max(self.lease_until_ms)
    }

    fn payment_quote(&self) -> Option<FederationPaymentQuote> {
        let kind = self.quote_kind?;
        let id = QuoteId::from_str(self.quote_id.as_deref()?).ok()?;
        Some(FederationPaymentQuote { kind, id })
    }

    fn has_active_claim_progress(&self) -> bool {
        self.has_payment_activity
            || (self.generation > 0
                && self.lease_until_ms == 0
                && self.wait == PersistedPaymentWait::Ready)
    }

    fn bind_quote_and_lane(
        &mut self,
        quote: &FederationPaymentQuote,
        lane: FederationPaymentScheduleLane,
        trigger: FederationPaymentObservationTrigger,
    ) {
        self.quote_kind = Some(quote.kind);
        self.quote_id = Some(quote.id.to_string());
        if lane == FederationPaymentScheduleLane::IncomingClaim
            || trigger == FederationPaymentObservationTrigger::BackendNotified
        {
            self.has_payment_activity = true;
        }
        self.lane = Some(match lane {
            FederationPaymentScheduleLane::Direct => FederationPaymentScheduleLane::Direct,
            FederationPaymentScheduleLane::OutgoingMelt => {
                FederationPaymentScheduleLane::OutgoingMelt
            }
            FederationPaymentScheduleLane::IncomingClaim => {
                FederationPaymentScheduleLane::IncomingClaim
            }
            FederationPaymentScheduleLane::IncomingWait => {
                if self.has_active_claim_progress()
                    || self.lane == Some(FederationPaymentScheduleLane::IncomingClaim)
                {
                    FederationPaymentScheduleLane::IncomingClaim
                } else {
                    FederationPaymentScheduleLane::IncomingWait
                }
            }
        });
    }

    fn effective_lane(
        &self,
        fallback_kind: Option<FederationPaymentQuoteKind>,
    ) -> FederationPaymentScheduleLane {
        if self.lane == Some(FederationPaymentScheduleLane::Direct) {
            return FederationPaymentScheduleLane::Direct;
        }
        if self.quote_kind.or(fallback_kind) == Some(FederationPaymentQuoteKind::Melt) {
            return FederationPaymentScheduleLane::OutgoingMelt;
        }
        if self.has_active_claim_progress()
            || self.lane == Some(FederationPaymentScheduleLane::IncomingClaim)
        {
            return FederationPaymentScheduleLane::IncomingClaim;
        }
        self.lane
            .unwrap_or(FederationPaymentScheduleLane::IncomingWait)
    }

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
            ..Default::default()
        };

        assert!(!schedule.is_due(
            FederationPaymentObservationTrigger::Scheduled,
            0,
            1_000 + MINT_REQUEST_COOLDOWN_MS,
        ));
        assert!(!schedule.is_due(
            FederationPaymentObservationTrigger::Requested,
            MINT_REQUEST_COOLDOWN_MS,
            1_000 + MINT_REQUEST_COOLDOWN_MS - 1,
        ));
        assert!(schedule.is_due(
            FederationPaymentObservationTrigger::Requested,
            MINT_REQUEST_COOLDOWN_MS,
            1_000 + MINT_REQUEST_COOLDOWN_MS,
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
        assert_eq!(
            persisted.next_attempt_at_ms,
            2_000 + BACKEND_NOTIFICATION_RECONNECT_DELAY_MS
        );
        let fallback_at = 2_000 + OBSERVER_FALLBACK_DELAY_MS;
        assert_eq!(persisted.next_observer_fallback_at_ms, fallback_at);

        let fallback = schedule
            .claim(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                fallback_at,
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

        let min_jittered_delay = BASE_DELAY_MS.saturating_sub(BASE_DELAY_MS * JITTER_PERCENT / 100);
        let restarted = FederationPaymentObservationSchedule::default();
        assert!(restarted
            .candidates(
                &db,
                &[mint.clone(), melt.clone()],
                FederationPaymentObservationTrigger::Scheduled,
                2_000 + min_jittered_delay - 1,
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
                    1_000 + MINT_REQUEST_COOLDOWN_MS,
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
        let initial_deadline = 2_000 + OBSERVER_FALLBACK_DELAY_MS;
        assert_eq!(persisted.wait, PersistedPaymentWait::ObserverFallback);
        assert_eq!(persisted.observer_fallback_round, 0);
        assert_eq!(persisted.next_attempt_at_ms, initial_deadline);
        assert!(restarted
            .claim(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                initial_deadline - 1,
            )
            .await
            .expect("not yet due")
            .is_none());

        let expanded = restarted
            .claim(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                initial_deadline,
            )
            .await
            .expect("due fallback")
            .expect("expanded permit");
        assert_eq!(expanded.observer_fallback_round(), 1);
        let expanded_finish_at = initial_deadline + 100;
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
                expanded_finish_at,
            )
            .await
            .expect("finish expanded fallback");
        let persisted = load_schedule(&db, &quote.storage_key())
            .await
            .expect("load expanded fallback")
            .expect("quote schedule");
        assert_eq!(persisted.observer_fallback_round, 1);
        assert_eq!(
            persisted.next_attempt_at_ms,
            expanded_finish_at + OBSERVER_FALLBACK_DELAY_MS
        );
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
                2_000 + OBSERVER_FALLBACK_DELAY_MS,
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
                2_000 + OBSERVER_FALLBACK_DELAY_MS + 100,
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

    #[tokio::test]
    async fn priority_queue_orders_due_quotes_and_filters_by_lane() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let schedule = FederationPaymentObservationSchedule::default();
        let unpaid_mint = FederationPaymentQuote::mint(QuoteId::new());
        let claim_mint = FederationPaymentQuote::mint(QuoteId::new());
        let melt = FederationPaymentQuote::melt(QuoteId::new());
        let direct = FederationPaymentQuote::mint(QuoteId::new());

        // Schedule unpaid_mint at t=1_100 (IncomingWait), melt at t=1_300 (OutgoingMelt),
        // claim_mint at t=1_200 (IncomingClaim), direct at t=1_400 (Direct).
        for (quote, lane, finish_at, advancement) in [
            (
                &unpaid_mint,
                FederationPaymentScheduleLane::IncomingWait,
                1_000_u64,
                FederationPaymentAdvancement::AwaitingBackendNotification,
            ),
            (
                &claim_mint,
                FederationPaymentScheduleLane::IncomingClaim,
                1_200_u64,
                FederationPaymentAdvancement::Progressed,
            ),
            (
                &melt,
                FederationPaymentScheduleLane::OutgoingMelt,
                1_200_u64,
                FederationPaymentAdvancement::AwaitingBackendNotification,
            ),
            (
                &direct,
                FederationPaymentScheduleLane::Direct,
                1_400_u64,
                FederationPaymentAdvancement::Progressed,
            ),
        ] {
            let permit = schedule
                .claim_in_lane(
                    &db,
                    quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    lane,
                    500,
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
                        advancement,
                        made_progress: advancement == FederationPaymentAdvancement::Progressed,
                        last_processed_operation_index: None,
                    },
                    finish_at,
                )
                .await
                .expect("finish");
        }

        // Global earliest is unpaid_mint at 1_100.
        assert_eq!(
            schedule.next_attempt_at_ms(&db).await.expect("next"),
            Some(1_100)
        );

        // Excluding saturated IncomingWait skips unpaid_mint and returns claim_mint at 1_200.
        let non_wait_lanes = [
            FederationPaymentScheduleLane::IncomingClaim,
            FederationPaymentScheduleLane::OutgoingMelt,
            FederationPaymentScheduleLane::Direct,
        ];
        assert_eq!(
            schedule
                .next_attempt_at_ms_for_lanes(&db, &non_wait_lanes)
                .await
                .expect("next non-wait"),
            Some(1_200)
        );
        assert_eq!(
            schedule
                .next_attempt_at_ms_for_lanes(&db, &[FederationPaymentScheduleLane::OutgoingMelt])
                .await
                .expect("next melt"),
            Some(1_300)
        );
        assert_eq!(
            schedule
                .due_quotes_for_lanes(&db, &FederationPaymentScheduleLane::ALL, 1_350, 10)
                .await
                .expect("due quotes"),
            vec![unpaid_mint.clone(), claim_mint.clone(), melt.clone()]
        );
        assert_eq!(
            schedule
                .due_quotes_for_lanes(&db, &non_wait_lanes, 1_500, 10)
                .await
                .expect("due non-wait quotes"),
            vec![claim_mint, melt, direct]
        );
    }

    #[tokio::test]
    async fn priority_queue_handles_lazy_invalidation_and_compaction() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let schedule = FederationPaymentObservationSchedule::default();
        let churned = FederationPaymentQuote::mint(QuoteId::new());
        let steady = FederationPaymentQuote::melt(QuoteId::new());

        let steady_permit = schedule
            .claim_in_lane(
                &db,
                &steady,
                FederationPaymentObservationTrigger::Scheduled,
                FederationPaymentScheduleLane::OutgoingMelt,
                1_000,
            )
            .await
            .expect("claim steady")
            .expect("steady permit");
        schedule
            .finish(
                &db,
                steady_permit,
                member_id(),
                FederationPaymentObservationProgress {
                    advancement: FederationPaymentAdvancement::AwaitingFederationEvidence,
                    made_progress: false,
                    last_processed_operation_index: None,
                },
                1_000,
            )
            .await
            .expect("finish steady");

        // Repeatedly reschedule one quote to create >100 stale heap entries.
        let mut now_ms = 2_000_u64;
        for _ in 0..80 {
            let permit = schedule
                .claim_in_lane(
                    &db,
                    &churned,
                    FederationPaymentObservationTrigger::BackendNotified,
                    FederationPaymentScheduleLane::IncomingClaim,
                    now_ms,
                )
                .await
                .expect("claim churned")
                .expect("churned permit");
            now_ms += 10;
            schedule
                .finish(
                    &db,
                    permit,
                    member_id(),
                    FederationPaymentObservationProgress {
                        advancement: FederationPaymentAdvancement::AwaitingBackendNotification,
                        made_progress: false,
                        last_processed_operation_index: None,
                    },
                    now_ms,
                )
                .await
                .expect("finish churned");
            now_ms += 200;
        }

        // Compaction must keep heap size bounded despite 160 updates on 1 quote.
        let claim_heap_len =
            schedule.lock_index().heaps[FederationPaymentScheduleLane::IncomingClaim.index()].len();
        assert!(
            claim_heap_len <= HEAP_COMPACTION_MIN_LEN,
            "stale heap entries must be compacted (len={claim_heap_len})"
        );

        let expected_churned_deadline = (now_ms - 200) + BACKEND_NOTIFICATION_RECONNECT_DELAY_MS;
        assert_eq!(
            schedule.next_attempt_at_ms(&db).await.expect("next"),
            Some(expected_churned_deadline)
        );

        // Removing the churned quote lazily invalidates its remaining entry.
        schedule.remove(&db, &churned).await.expect("remove");
        assert_eq!(
            schedule
                .next_attempt_at_ms(&db)
                .await
                .expect("next after remove"),
            Some(1_000 + FEDERATION_REPAIR_DELAY_MS)
        );
        assert!(schedule
            .due_quotes_for_lanes(&db, &FederationPaymentScheduleLane::ALL, now_ms, 10)
            .await
            .expect("due after remove")
            .is_empty());
    }

    #[tokio::test]
    async fn priority_queue_hydrates_after_restart_without_clobbering_fresher_mutations() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let first = FederationPaymentObservationSchedule::default();
        let persisted_quote = FederationPaymentQuote::mint(QuoteId::new());
        let mutated_quote = FederationPaymentQuote::melt(QuoteId::new());

        for (quote, lane, finish_at) in [
            (
                &persisted_quote,
                FederationPaymentScheduleLane::IncomingWait,
                2_000_u64,
            ),
            (
                &mutated_quote,
                FederationPaymentScheduleLane::OutgoingMelt,
                1_500_u64,
            ),
        ] {
            let permit = first
                .claim_in_lane(
                    &db,
                    quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    lane,
                    1_000,
                )
                .await
                .expect("claim")
                .expect("permit");
            first
                .finish(
                    &db,
                    permit,
                    member_id(),
                    FederationPaymentObservationProgress {
                        advancement: FederationPaymentAdvancement::AwaitingBackendNotification,
                        made_progress: false,
                        last_processed_operation_index: None,
                    },
                    finish_at,
                )
                .await
                .expect("finish");
        }

        // Simulate restart: remove mutated_quote before hydration runs, then
        // hydrate via `next_attempt_at_ms`.
        let restarted = FederationPaymentObservationSchedule::default();
        restarted
            .remove(&db, &mutated_quote)
            .await
            .expect("remove before hydration");

        assert_eq!(
            restarted
                .next_attempt_at_ms(&db)
                .await
                .expect("hydrated next"),
            Some(2_000 + BACKEND_NOTIFICATION_RECONNECT_DELAY_MS)
        );
        assert_eq!(
            restarted
                .due_quotes_for_lanes(&db, &FederationPaymentScheduleLane::ALL, 2_500, 10)
                .await
                .expect("hydrated due"),
            vec![persisted_quote]
        );
    }

    #[tokio::test]
    async fn backend_notification_during_active_lease_schedules_immediate_recheck() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let schedule = FederationPaymentObservationSchedule::default();
        let quote = FederationPaymentQuote::mint(QuoteId::new());

        let permit = schedule
            .claim_in_lane(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                FederationPaymentScheduleLane::Direct,
                1_000,
            )
            .await
            .expect("claim")
            .expect("permit");

        // A streaming payment notification arrives while the quote is leased.
        assert!(schedule
            .claim_in_lane(
                &db,
                &quote,
                FederationPaymentObservationTrigger::BackendNotified,
                FederationPaymentScheduleLane::Direct,
                1_050,
            )
            .await
            .expect("notified while leased")
            .is_none());

        // The in-flight poll finishes with no progress (normally ~1s backoff),
        // but must schedule an immediate recheck instead of losing the event.
        schedule
            .finish(
                &db,
                permit,
                member_id(),
                FederationPaymentObservationProgress {
                    advancement: FederationPaymentAdvancement::AwaitingExternalEvidence,
                    made_progress: false,
                    last_processed_operation_index: None,
                },
                1_100,
            )
            .await
            .expect("finish");

        assert_eq!(
            schedule.next_attempt_at_ms(&db).await.expect("next"),
            Some(1_100)
        );
        assert_eq!(
            schedule
                .due_quotes_for_lanes(&db, &[FederationPaymentScheduleLane::Direct], 1_100, 10)
                .await
                .expect("due immediately"),
            vec![quote]
        );
    }

    #[tokio::test]
    async fn mark_due_now_keeps_quote_unleased_and_promotes_claim_lane() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let schedule = FederationPaymentObservationSchedule::default();
        let quote = FederationPaymentQuote::mint(QuoteId::new());

        let permit = schedule
            .claim_in_lane(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                FederationPaymentScheduleLane::IncomingWait,
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
                    last_processed_operation_index: None,
                },
                1_500,
            )
            .await
            .expect("finish");

        schedule
            .mark_due_now(
                &db,
                &quote,
                FederationPaymentScheduleLane::IncomingClaim,
                FederationPaymentObservationTrigger::BackendNotified,
                2_000,
            )
            .await
            .expect("mark due now");

        // Moved out of IncomingWait into IncomingClaim, due at 2_000, unleased.
        assert_eq!(
            schedule
                .next_attempt_at_ms_for_lanes(&db, &[FederationPaymentScheduleLane::IncomingWait])
                .await
                .expect("wait lane"),
            None
        );
        assert_eq!(
            schedule
                .next_attempt_at_ms_for_lanes(&db, &[FederationPaymentScheduleLane::IncomingClaim])
                .await
                .expect("claim lane"),
            Some(2_000)
        );
        assert!(schedule
            .active_lease_until_ms(&db, std::slice::from_ref(&quote), 2_000)
            .await
            .expect("lease")
            .is_none());
    }

    #[tokio::test]
    async fn fresh_incoming_wait_quote_stays_in_wait_lane_during_first_lease_and_non_priority_wake()
    {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let schedule = FederationPaymentObservationSchedule::default();
        let quote = FederationPaymentQuote::mint(QuoteId::new());

        let permit = schedule
            .claim_in_lane(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                FederationPaymentScheduleLane::IncomingWait,
                1_000,
            )
            .await
            .expect("claim fresh wait quote")
            .expect("permit");

        // While the first lease is active, wait is still default Ready, but
        // lease_until_ms > 0 so the unpaid quote must NOT self-promote to IncomingClaim.
        assert!(!schedule
            .has_payment_activity(&db, &quote)
            .await
            .expect("activity check"));
        assert_eq!(
            schedule
                .next_attempt_at_ms_for_lanes(&db, &[FederationPaymentScheduleLane::IncomingClaim])
                .await
                .expect("claim lane"),
            None
        );
        assert_eq!(
            schedule
                .next_attempt_at_ms_for_lanes(&db, &[FederationPaymentScheduleLane::IncomingWait])
                .await
                .expect("wait lane"),
            Some(1_000 + ATTEMPT_LEASE_MS)
        );

        // A non-priority OperationAccepted wake (e.g. MintQuote right after ReceivePrepared)
        // arriving while the first lease is active must keep the quote in IncomingWait.
        assert!(schedule
            .claim_in_lane(
                &db,
                &quote,
                FederationPaymentObservationTrigger::OperationAccepted(5),
                FederationPaymentScheduleLane::IncomingWait,
                1_050,
            )
            .await
            .expect("wake during first lease")
            .is_none());
        assert!(!schedule
            .has_payment_activity(&db, &quote)
            .await
            .expect("still unpaid"));
        assert_eq!(
            schedule
                .next_attempt_at_ms_for_lanes(&db, &[FederationPaymentScheduleLane::IncomingClaim])
                .await
                .expect("claim lane after wake"),
            None
        );

        schedule
            .finish(
                &db,
                permit,
                member_id(),
                FederationPaymentObservationProgress {
                    advancement: FederationPaymentAdvancement::AwaitingBackendNotification,
                    made_progress: false,
                    last_processed_operation_index: Some(5),
                },
                1_200,
            )
            .await
            .expect("finish unpaid attempt");
        assert!(!schedule
            .has_payment_activity(&db, &quote)
            .await
            .expect("unpaid after finish"));
        assert_eq!(
            schedule
                .next_attempt_at_ms_for_lanes(&db, &[FederationPaymentScheduleLane::IncomingClaim])
                .await
                .expect("claim lane after finish"),
            None
        );
        assert_eq!(
            schedule
                .next_attempt_at_ms_for_lanes(&db, &[FederationPaymentScheduleLane::IncomingWait])
                .await
                .expect("wait lane after finish"),
            Some(1_200 + BACKEND_NOTIFICATION_RECONNECT_DELAY_MS)
        );
    }
}

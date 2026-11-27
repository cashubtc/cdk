//! Generic durable session state machine shared by every custody protocol.
//!
//! Ordinary FROST nonces, virtual MuSig2 nonces, and threshold-DH requests all
//! follow the same participant-local lifecycle: activate an attempt for an
//! authorization slot, reserve one record for that attempt, advance it by exact
//! compare-and-swap, and finally compact terminal detail into a permanent
//! replay tombstone. [`SessionRecord`] captures what differs between the record
//! kinds, and [`MemorySessionTable`] / [`crate::session_redb::RedbSessionTable`]
//! implement the six store operations once, with the same rules in both
//! backends. Every operation either succeeds completely or leaves the table
//! unchanged.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Mutex;

use crate::types::MAX_SIGNING_BATCH_SIZE;
use crate::{
    Error, NonceBurnReason, NonceLifecycle, NonceRecord, NonceReservation, NonceTransitionChain,
    SigningSessionContext, SigningSessionId, SigningSlotId,
};

/// Every store failure whose error depends on the record kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    /// A lower attempt than the durable high-water mark was activated.
    StaleAttempt,
    /// The current attempt was re-activated with another session.
    SignerSetChanged,
    /// An active slot points at a record that does not exist.
    ActiveRecordMissing,
    /// The reserved session exists only as compacted replay evidence.
    SessionCompacted,
    /// The record's attempt is not the durable high-water mark.
    NotActivated,
    /// The record's authorization slot already owns an active record.
    SlotClaimed,
    /// The record key is already reserved or tombstoned.
    KeyClaimed,
    /// A load by key found only a compacted tombstone.
    KeyCompacted,
    /// A compare-and-swap target does not exist.
    RecordDisappeared,
    /// A compare-and-swap target differs from the expected revision.
    ChangedConcurrently,
    /// The terminal order references a record that does not exist.
    TerminalRecordMissing,
    /// The terminal order references a record that is not terminal.
    TerminalOrderNonterminal,
}

/// One kind of durable participant session record.
///
/// Every hook encodes a real difference between the record kinds; the store
/// operations themselves live in the generic tables.
pub(crate) trait SessionRecord: fmt::Debug + Clone + PartialEq + Sized {
    /// Unique record identifier (the nonce, or the session for keyless kinds).
    type Key: fmt::Debug + Copy + Ord;
    /// Compact replay evidence retained in memory after compaction.
    type Tombstone: fmt::Debug + Clone + PartialEq;
    /// Public result of a reservation.
    type Reservation;

    /// Whether the record key is the session identifier itself.
    ///
    /// Such kinds keep no durable session index, report a compacted key on a
    /// load by session, and need no separate key-uniqueness check.
    const KEYED_BY_SESSION: bool;

    /// Unique key of this record.
    fn key(&self) -> Self::Key;

    /// Canonical encoding of a key used by durable tables.
    fn key_bytes(key: Self::Key) -> [u8; 32];

    /// Fully bound session context.
    fn context(&self) -> &SigningSessionContext;

    /// Whether the record permanently forbids further use.
    fn is_terminal(&self) -> bool;

    /// Compact replay evidence for a terminal record.
    fn tombstone(&self) -> Self::Tombstone;

    /// Next revision burned because a newer attempt replaced it.
    fn burned_for_replacement(&self) -> Result<Self, Error>;

    /// Validate a context before activating its attempt.
    fn validate_activation(context: &SigningSessionContext) -> Result<(), Error>;

    /// Validate a record before its first reservation.
    fn validate_new(&self) -> Result<(), Error>;

    /// Validate one exact compare-and-swap replacement.
    fn validate_transition(expected: &Self, replacement: &Self) -> Result<(), Error>;

    /// Reservation result for a newly inserted record.
    fn inserted() -> Self::Reservation;

    /// Reservation result returning an existing record for retry handling.
    fn existing(record: Self) -> Self::Reservation;

    /// Error reported for one failure.
    fn error(failure: Failure) -> Error;
}

/// Check the durable attempt high-water mark for a slot.
///
/// Returns `Ok(true)` when the exact attempt is already active, `Ok(false)` when
/// the slot has no attempt yet or the context names a higher one, and an error
/// for stale or conflicting attempts.
pub(crate) fn check_attempt<R>(
    current: Option<(u32, SigningSessionId)>,
    context: &SigningSessionContext,
) -> Result<bool, Error>
where
    R: SessionRecord,
{
    match current {
        Some((attempt, _)) if attempt > context.attempt() => Err(R::error(Failure::StaleAttempt)),
        Some((attempt, session_id)) if attempt == context.attempt() => {
            if session_id != context.session_id() {
                return Err(R::error(Failure::SignerSetChanged));
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// In-memory state for one record kind.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MemorySessionTable<R>
where
    R: SessionRecord,
{
    records: BTreeMap<R::Key, R>,
    sessions: BTreeMap<SigningSessionId, R::Key>,
    active_slots: BTreeMap<SigningSlotId, R::Key>,
    latest_attempts: BTreeMap<SigningSlotId, (u32, SigningSessionId)>,
    tombstones: BTreeMap<R::Key, R::Tombstone>,
    terminal_order: Vec<R::Key>,
}

impl<R> Default for MemorySessionTable<R>
where
    R: SessionRecord,
{
    fn default() -> Self {
        Self {
            records: BTreeMap::new(),
            sessions: BTreeMap::new(),
            active_slots: BTreeMap::new(),
            latest_attempts: BTreeMap::new(),
            tombstones: BTreeMap::new(),
            terminal_order: Vec::new(),
        }
    }
}

impl<R> MemorySessionTable<R>
where
    R: SessionRecord,
{
    /// Retained tombstone for one compacted key.
    pub(crate) fn tombstone(&self, key: R::Key) -> Option<R::Tombstone> {
        self.tombstones.get(&key).cloned()
    }

    fn activate_attempt(&mut self, context: &SigningSessionContext) -> Result<(), Error> {
        let slot_id = context.slot_id();
        let current = self.latest_attempts.get(&slot_id).copied();
        if check_attempt::<R>(current, context)? {
            return Ok(());
        }

        // Every fallible step runs before the first mutation.
        let burned = match self.active_slots.get(&slot_id).copied() {
            Some(key) => {
                let existing = self
                    .records
                    .get(&key)
                    .ok_or_else(|| R::error(Failure::ActiveRecordMissing))?;
                let burned = existing.burned_for_replacement()?;
                R::validate_transition(existing, &burned)?;
                Some((key, burned))
            }
            None => None,
        };
        if let Some((key, burned)) = burned {
            self.records.insert(key, burned);
            self.active_slots.remove(&slot_id);
            self.terminal_order.push(key);
        }
        self.latest_attempts
            .insert(slot_id, (context.attempt(), context.session_id()));
        Ok(())
    }

    fn reserve(&mut self, record: R) -> Result<R::Reservation, Error> {
        let session_id = record.context().session_id();
        let slot_id = record.context().slot_id();
        let key = record.key();
        if R::KEYED_BY_SESSION {
            if let Some(existing) = self.records.get(&key) {
                return Ok(R::existing(existing.clone()));
            }
            if self.tombstones.contains_key(&key) {
                return Err(R::error(Failure::SessionCompacted));
            }
        } else if let Some(existing_key) = self.sessions.get(&session_id) {
            return match self.records.get(existing_key) {
                Some(existing) => Ok(R::existing(existing.clone())),
                None => Err(R::error(Failure::SessionCompacted)),
            };
        }
        if self.latest_attempts.get(&slot_id) != Some(&(record.context().attempt(), session_id)) {
            return Err(R::error(Failure::NotActivated));
        }
        if self.active_slots.contains_key(&slot_id) {
            return Err(R::error(Failure::SlotClaimed));
        }
        if !R::KEYED_BY_SESSION
            && (self.records.contains_key(&key) || self.tombstones.contains_key(&key))
        {
            return Err(R::error(Failure::KeyClaimed));
        }
        // Session-keyed kinds keep this index in memory only, to map a session
        // back to its typed key; it is never consulted for their uniqueness.
        self.sessions.insert(session_id, key);
        self.active_slots.insert(slot_id, key);
        self.records.insert(key, record);
        Ok(R::inserted())
    }

    fn load_key(&self, key: R::Key) -> Result<Option<R>, Error> {
        if let Some(record) = self.records.get(&key) {
            return Ok(Some(record.clone()));
        }
        if self.tombstones.contains_key(&key) {
            return Err(R::error(Failure::KeyCompacted));
        }
        Ok(None)
    }

    fn load_session(&self, session_id: SigningSessionId) -> Result<Option<R>, Error> {
        let Some(key) = self.sessions.get(&session_id) else {
            return Ok(None);
        };
        if R::KEYED_BY_SESSION {
            return self.load_key(*key);
        }
        Ok(self.records.get(key).cloned())
    }

    fn compare_and_swap(&mut self, expected: &R, replacement: R) -> Result<(), Error> {
        let key = expected.key();
        let current = self
            .records
            .get(&key)
            .ok_or_else(|| R::error(Failure::RecordDisappeared))?;
        if current != expected {
            return Err(R::error(Failure::ChangedConcurrently));
        }
        if !expected.is_terminal() && replacement.is_terminal() {
            self.active_slots.remove(&expected.context().slot_id());
            self.terminal_order.push(key);
        }
        self.records.insert(key, replacement);
        Ok(())
    }

    fn nonterminal_records(&self) -> Vec<R> {
        self.records
            .values()
            .filter(|record| !record.is_terminal())
            .cloned()
            .collect()
    }

    fn compact_terminal(&mut self, retain_detailed: usize) -> Result<usize, Error> {
        let compact_count = self.terminal_order.len().saturating_sub(retain_detailed);
        // Every fallible step runs before the first mutation.
        let tombstones = self
            .terminal_order
            .iter()
            .take(compact_count)
            .map(|key| {
                let record = self
                    .records
                    .get(key)
                    .ok_or_else(|| R::error(Failure::TerminalRecordMissing))?;
                if !record.is_terminal() {
                    return Err(R::error(Failure::TerminalOrderNonterminal));
                }
                Ok((*key, record.tombstone()))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        self.terminal_order.drain(..compact_count);
        for (key, tombstone) in tombstones {
            self.records.remove(&key);
            self.tombstones.insert(key, tombstone);
        }
        Ok(compact_count)
    }
}

/// One record kind's in-memory table behind a shared store mutex.
pub(crate) struct MemoryStoreTable<'a, S, R>
where
    R: SessionRecord,
{
    state: &'a Mutex<S>,
    poisoned: &'static str,
    table: fn(&mut S) -> &mut MemorySessionTable<R>,
}

impl<'a, S, R> MemoryStoreTable<'a, S, R>
where
    R: SessionRecord,
{
    /// Select one table inside a store mutex.
    pub(crate) fn new(
        state: &'a Mutex<S>,
        poisoned: &'static str,
        table: fn(&mut S) -> &mut MemorySessionTable<R>,
    ) -> Self {
        Self {
            state,
            poisoned,
            table,
        }
    }

    /// Run `operation` on the table while holding the store lock.
    pub(crate) fn with_table<T>(
        &self,
        operation: impl FnOnce(&mut MemorySessionTable<R>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::NoncePersistence(self.poisoned.to_string()))?;
        operation((self.table)(&mut state))
    }

    /// Advance the attempt high-water mark, burning an older active record.
    pub(crate) fn activate_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        R::validate_activation(context)?;
        self.with_table(|table| table.activate_attempt(context))
    }

    /// Reserve a new record, or return the exact existing session.
    pub(crate) fn reserve(&self, record: R) -> Result<R::Reservation, Error> {
        record.validate_new()?;
        self.with_table(|table| table.reserve(record))
    }

    /// Load a detailed record by key, rejecting compacted keys.
    pub(crate) fn load_key(&self, key: R::Key) -> Result<Option<R>, Error> {
        self.with_table(|table| table.load_key(key))
    }

    /// Load a batch of detailed records by key in one lock acquisition.
    pub(crate) fn load_keys_batch(
        &self,
        keys: &[R::Key],
    ) -> Result<Vec<Result<Option<R>, Error>>, Error> {
        self.with_table(|table| Ok(keys.iter().map(|key| table.load_key(*key)).collect()))
    }

    /// Load a detailed record by session.
    pub(crate) fn load_session(&self, session_id: SigningSessionId) -> Result<Option<R>, Error> {
        self.with_table(|table| table.load_session(session_id))
    }

    /// Replace one exact record revision.
    pub(crate) fn compare_and_swap(&self, expected: &R, replacement: R) -> Result<(), Error> {
        R::validate_transition(expected, &replacement)?;
        self.with_table(|table| table.compare_and_swap(expected, replacement))
    }

    /// Every nonterminal detailed record.
    pub(crate) fn nonterminal_records(&self) -> Result<Vec<R>, Error> {
        self.with_table(|table| Ok(table.nonterminal_records()))
    }

    /// Compact old terminal detail into replay tombstones.
    pub(crate) fn compact_terminal(&self, retain_detailed: usize) -> Result<usize, Error> {
        self.with_table(|table| table.compact_terminal(retain_detailed))
    }
}

impl<S> MemoryStoreTable<'_, S, NonceRecord> {
    /// Atomically activate, reserve, and publish commitments for a batch of
    /// freshly generated nonce records.
    pub(crate) fn commit_nonce_batch(
        &self,
        records: Vec<NonceRecord>,
    ) -> Result<Vec<NonceReservation>, Error> {
        validate_nonce_commit_batch(&records)?;
        self.with_table(|table| {
            let mut staged = table.clone();
            let mut reservations = Vec::with_capacity(records.len());
            for generated in records {
                staged.activate_attempt(generated.context())?;
                match staged.reserve(generated.clone())? {
                    NonceReservation::Existing(existing) => {
                        validate_retryable_existing_nonce(&existing, &generated)?;
                        reservations.push(NonceReservation::Existing(existing));
                    }
                    NonceReservation::Inserted => {
                        let reserved =
                            generated.transition(NonceLifecycle::Reserved, true, None)?;
                        NonceRecord::validate_transition(&generated, &reserved)?;
                        staged.compare_and_swap(&generated, reserved.clone())?;
                        let sent =
                            reserved.transition(NonceLifecycle::CommitmentSent, true, None)?;
                        NonceRecord::validate_transition(&reserved, &sent)?;
                        staged.compare_and_swap(&reserved, sent)?;
                        reservations.push(NonceReservation::Inserted);
                    }
                }
            }
            *table = staged;
            Ok(reservations)
        })
    }

    /// Atomically apply a batch of validated nonce transition chains.
    pub(crate) fn apply_nonce_batch_transitions(
        &self,
        chains: Vec<NonceTransitionChain>,
    ) -> Result<(), Error> {
        validate_nonce_transition_batch(&chains)?;
        self.with_table(|table| {
            let mut staged = table.clone();
            for chain in chains {
                let expected = chain.expected();
                let replacement = chain.final_record();
                let key = expected.key();
                let current = staged
                    .records
                    .get(&key)
                    .ok_or_else(|| NonceRecord::error(Failure::RecordDisappeared))?;
                if current != expected {
                    return Err(NonceRecord::error(Failure::ChangedConcurrently));
                }
                if !expected.is_terminal() && replacement.is_terminal() {
                    staged.active_slots.remove(&expected.context().slot_id());
                    staged.terminal_order.push(key);
                }
                staged.records.insert(key, replacement.clone());
            }
            *table = staged;
            Ok(())
        })
    }

    /// Atomically burn every nonterminal record for a batch of sessions.
    pub(crate) fn burn_nonce_sessions_batch(
        &self,
        sessions: &[SigningSessionId],
        reason: NonceBurnReason,
    ) -> Result<(), Error> {
        validate_session_burn_batch(sessions)?;
        self.with_table(|table| {
            let mut staged = table.clone();
            for session_id in sessions {
                let Some(record) = staged.load_session(*session_id)? else {
                    continue;
                };
                if record.is_terminal() {
                    continue;
                }
                let burned = record.transition(NonceLifecycle::Burned { reason }, false, None)?;
                NonceRecord::validate_transition(&record, &burned)?;
                staged.compare_and_swap(&record, burned)?;
            }
            *table = staged;
            Ok(())
        })
    }
}

pub(crate) fn validate_nonce_commit_batch(records: &[NonceRecord]) -> Result<(), Error> {
    if records.is_empty() {
        return Err(Error::InvalidSigningSession(
            "signing batch cannot be empty".to_string(),
        ));
    }
    if records.len() > MAX_SIGNING_BATCH_SIZE {
        return Err(Error::OversizedInput {
            kind: "signing batch",
            max: MAX_SIGNING_BATCH_SIZE,
        });
    }
    let mut sessions = BTreeSet::new();
    let mut slots = BTreeSet::new();
    let mut nonces = BTreeSet::new();
    for record in records {
        NonceRecord::validate_activation(record.context())?;
        record.validate_new()?;
        if !sessions.insert(record.context().session_id())
            || !slots.insert(record.context().slot_id())
            || !nonces.insert(record.nonce_id())
        {
            return Err(Error::InvalidSigningSession(
                "duplicate signing session, slot, or nonce in batch".to_string(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_retryable_existing_nonce(
    existing: &NonceRecord,
    generated: &NonceRecord,
) -> Result<(), Error> {
    if existing.context() != generated.context()
        || existing.derivation() != generated.derivation()
        || !matches!(
            existing.lifecycle(),
            NonceLifecycle::CommitmentSent | NonceLifecycle::Consumed { .. }
        )
    {
        return Err(Error::InvalidCommitment(
            "signing session already exists in a non-retryable state".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_nonce_transition_batch(
    chains: &[NonceTransitionChain],
) -> Result<(), Error> {
    if chains.is_empty() {
        return Err(Error::InvalidSigningSession(
            "nonce transition batch cannot be empty".to_string(),
        ));
    }
    if chains.len() > MAX_SIGNING_BATCH_SIZE {
        return Err(Error::OversizedInput {
            kind: "nonce transition batch",
            max: MAX_SIGNING_BATCH_SIZE,
        });
    }
    let mut nonces = BTreeSet::new();
    for chain in chains {
        if !nonces.insert(chain.expected().nonce_id()) {
            return Err(Error::InvalidSigningSession(
                "duplicate nonce in transition batch".to_string(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_session_burn_batch(sessions: &[SigningSessionId]) -> Result<(), Error> {
    if sessions.is_empty() {
        return Err(Error::InvalidSigningSession(
            "session burn batch cannot be empty".to_string(),
        ));
    }
    if sessions.len() > MAX_SIGNING_BATCH_SIZE {
        return Err(Error::OversizedInput {
            kind: "session burn batch",
            max: MAX_SIGNING_BATCH_SIZE,
        });
    }
    let mut seen = BTreeSet::new();
    for session_id in sessions {
        if !seen.insert(*session_id) {
            return Err(Error::InvalidSigningSession(
                "duplicate session in burn batch".to_string(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ApplicationDerivationVersion, ApplicationId, ApplicationKeyEpoch, ApplicationRegistration,
        CustodyProtocol, MemberId, NonceBurnReason, RootKeyEpoch, SigningAuthorizationScope,
        SigningDerivation, ThresholdDhLifecycle, ThresholdDhRecord,
    };

    fn dh_context(authorization_reference: u8, attempt: u32) -> SigningSessionContext {
        let registration = ApplicationRegistration::new(
            ApplicationId::from_label("org.cashubtc.cdk.session-table-test").expect("app id"),
            ApplicationKeyEpoch::new(1).expect("key epoch"),
            ApplicationDerivationVersion::new(1).expect("version"),
            [7; 32],
        )
        .expect("registration");
        let scope = SigningAuthorizationScope::new(
            [0xf0; 32],
            RootKeyEpoch::new(1).expect("root epoch"),
            registration,
            SigningDerivation::application_key().binding(),
            [authorization_reference; 32],
            [9; 32],
        )
        .expect("scope");
        SigningSessionContext::new_for_protocol(
            scope,
            attempt,
            [1, 2].map(|member| MemberId::new(member).expect("member")),
            CustodyProtocol::ThresholdDh,
        )
        .expect("context")
    }

    fn burned_dh_record(context: SigningSessionContext) -> ThresholdDhRecord {
        ThresholdDhRecord::pending(context, SigningDerivation::application_key(), [3; 32])
            .transition(
                ThresholdDhLifecycle::Burned {
                    reason: NonceBurnReason::Aborted,
                },
                None,
            )
            .expect("burned record")
    }

    #[test]
    fn failed_memory_activation_leaves_table_unchanged() {
        let older = dh_context(1, 0);
        let newer = dh_context(1, 1);
        let slot = older.slot_id();

        // Active slot whose record is missing.
        let mut table = MemorySessionTable::<ThresholdDhRecord>::default();
        table.latest_attempts.insert(slot, (0, older.session_id()));
        table.active_slots.insert(slot, older.session_id());
        let before = table.clone();
        assert!(matches!(
            table.activate_attempt(&newer),
            Err(Error::NoncePersistence(_))
        ));
        assert_eq!(table, before);

        // Active slot whose record cannot be burned again.
        table
            .records
            .insert(older.session_id(), burned_dh_record(older.clone()));
        let before = table.clone();
        assert!(matches!(
            table.activate_attempt(&newer),
            Err(Error::NoncePersistence(_))
        ));
        assert_eq!(table, before);
    }

    #[test]
    fn failed_memory_compaction_leaves_table_unchanged() {
        let first = dh_context(1, 0);
        let second = dh_context(2, 0);

        // The second terminal entry has no record.
        let mut table = MemorySessionTable::<ThresholdDhRecord>::default();
        table
            .records
            .insert(first.session_id(), burned_dh_record(first.clone()));
        table.terminal_order.push(first.session_id());
        table.terminal_order.push(second.session_id());
        let before = table.clone();
        assert!(matches!(
            table.compact_terminal(0),
            Err(Error::NoncePersistence(_))
        ));
        assert_eq!(table, before);

        // The second terminal entry is not terminal.
        table.records.insert(
            second.session_id(),
            ThresholdDhRecord::pending(
                second.clone(),
                SigningDerivation::application_key(),
                [4; 32],
            ),
        );
        let before = table.clone();
        assert!(matches!(
            table.compact_terminal(0),
            Err(Error::NoncePersistence(_))
        ));
        assert_eq!(table, before);

        // A valid order still compacts.
        table.terminal_order.pop();
        assert_eq!(table.compact_terminal(0).expect("compaction"), 1);
        assert!(table.tombstone(first.session_id()).is_some());
        assert!(table.terminal_order.is_empty());
    }
}

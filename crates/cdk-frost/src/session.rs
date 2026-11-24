//! Generic durable session state machine shared by every custody protocol.
//!
//! Ordinary FROST nonces, virtual MuSig2 nonces, and threshold-DH requests all
//! follow the same participant-local lifecycle: activate an attempt for an
//! authorization slot, reserve one record for that attempt, advance it by exact
//! compare-and-swap, and finally compact terminal detail into a permanent
//! replay tombstone. [`SessionRecord`] captures what differs between the record
//! kinds, and [`MemorySessionTable`] / [`crate::session_redb::RedbSessionTable`]
//! implement the six store operations once.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use crate::{Error, SigningSessionContext, SigningSessionId, SigningSlotId};

/// Storage backend reporting a failure.
///
/// The record kinds historically worded a few failures differently in memory
/// and in redb; the backend lets [`SessionRecord::error`] keep that wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Backend {
    /// In-memory tables.
    Memory,
    /// Durable redb tables.
    Redb,
}

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
    type Tombstone: fmt::Debug + Clone;
    /// Public result of a reservation.
    type Reservation;

    /// Validate new records before taking the store lock or write transaction.
    ///
    /// When false, the new record is validated after the activation and slot
    /// checks, inside the lock or transaction.
    const VALIDATE_NEW_FIRST: bool;

    /// Release the in-memory active slot before loading the record it owns.
    ///
    /// When true, an in-memory activation that fails while burning the older
    /// record has already released that slot.
    const MEMORY_RELEASES_SLOT_BEFORE_BURN: bool;

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

    /// Error reported for one failure by one backend.
    fn error(failure: Failure, backend: Backend) -> Error;
}

/// Check the durable attempt high-water mark for a slot.
///
/// Returns `Ok(true)` when the exact attempt is already active, `Ok(false)` when
/// the slot has no attempt yet or the context names a higher one, and an error
/// for stale or conflicting attempts.
pub(crate) fn check_attempt<R>(
    current: Option<(u32, SigningSessionId)>,
    context: &SigningSessionContext,
    backend: Backend,
) -> Result<bool, Error>
where
    R: SessionRecord,
{
    match current {
        Some((attempt, _)) if attempt > context.attempt() => {
            Err(R::error(Failure::StaleAttempt, backend))
        }
        Some((attempt, session_id)) if attempt == context.attempt() => {
            if session_id != context.session_id() {
                return Err(R::error(Failure::SignerSetChanged, backend));
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// In-memory state for one record kind.
#[derive(Debug)]
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
        if check_attempt::<R>(current, context, Backend::Memory)? {
            return Ok(());
        }

        let active = if R::MEMORY_RELEASES_SLOT_BEFORE_BURN {
            self.active_slots.remove(&slot_id)
        } else {
            self.active_slots.get(&slot_id).copied()
        };
        if let Some(key) = active {
            let existing = self
                .records
                .get(&key)
                .cloned()
                .ok_or_else(|| R::error(Failure::ActiveRecordMissing, Backend::Memory))?;
            let burned = existing.burned_for_replacement()?;
            R::validate_transition(&existing, &burned)?;
            self.records.insert(key, burned);
            if !R::MEMORY_RELEASES_SLOT_BEFORE_BURN {
                self.active_slots.remove(&slot_id);
            }
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
                return Err(R::error(Failure::SessionCompacted, Backend::Memory));
            }
        } else if let Some(existing_key) = self.sessions.get(&session_id) {
            return match self.records.get(existing_key) {
                Some(existing) => Ok(R::existing(existing.clone())),
                None => Err(R::error(Failure::SessionCompacted, Backend::Memory)),
            };
        }
        if self.latest_attempts.get(&slot_id) != Some(&(record.context().attempt(), session_id)) {
            return Err(R::error(Failure::NotActivated, Backend::Memory));
        }
        if self.active_slots.contains_key(&slot_id) {
            return Err(R::error(Failure::SlotClaimed, Backend::Memory));
        }
        if !R::VALIDATE_NEW_FIRST {
            record.validate_new()?;
        }
        if !R::KEYED_BY_SESSION
            && (self.records.contains_key(&key) || self.tombstones.contains_key(&key))
        {
            return Err(R::error(Failure::KeyClaimed, Backend::Memory));
        }
        // Session-keyed kinds keep this index in memory only, to map a session
        // back to its typed key; it is never consulted for their uniqueness.
        self.sessions.insert(session_id, key);
        self.active_slots.insert(slot_id, key);
        self.records.insert(key, record);
        Ok(R::inserted())
    }

    fn load_key(&self, key: R::Key) -> Result<Option<R>, Error> {
        if self.tombstones.contains_key(&key) {
            return Err(R::error(Failure::KeyCompacted, Backend::Memory));
        }
        Ok(self.records.get(&key).cloned())
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
            .ok_or_else(|| R::error(Failure::RecordDisappeared, Backend::Memory))?;
        if current != expected {
            return Err(R::error(Failure::ChangedConcurrently, Backend::Memory));
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
        let keys = self
            .terminal_order
            .drain(..compact_count)
            .collect::<Vec<_>>();
        for key in &keys {
            let record = self
                .records
                .remove(key)
                .ok_or_else(|| R::error(Failure::TerminalRecordMissing, Backend::Memory))?;
            self.tombstones.insert(*key, record.tombstone());
        }
        Ok(keys.len())
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
        if R::VALIDATE_NEW_FIRST {
            record.validate_new()?;
        }
        self.with_table(|table| table.reserve(record))
    }

    /// Load a detailed record by key, rejecting compacted keys.
    pub(crate) fn load_key(&self, key: R::Key) -> Result<Option<R>, Error> {
        self.with_table(|table| table.load_key(key))
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

//! Generic redb tables for the durable session state machine.
//!
//! Every operation runs in one redb transaction, so each uniqueness check is
//! made in the same transaction as the write it guards.

use std::fmt;
use std::marker::PhantomData;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

use crate::session::{check_attempt, Failure, SessionRecord};
use crate::{Error, SigningSessionContext, SigningSessionId};

type BytesTable = TableDefinition<'static, &'static [u8], &'static [u8]>;

/// On-disk layout of one record kind.
///
/// Table and metadata names are part of the durable format and must never
/// change.
#[derive(Clone, Copy)]
pub(crate) struct RedbTables {
    /// Encoded records by key.
    pub(crate) records: BytesTable,
    /// Session to record key, or `None` when records are keyed by session.
    pub(crate) sessions: Option<BytesTable>,
    /// Authorization slot to the record key owning it.
    pub(crate) active_slots: BytesTable,
    /// Authorization slot to its encoded attempt high-water mark.
    pub(crate) latest_attempts: BytesTable,
    /// Encoded replay tombstones by key.
    pub(crate) tombstones: BytesTable,
    /// Terminal sequence number to record key, oldest first.
    pub(crate) terminal_order: TableDefinition<'static, u64, &'static [u8]>,
    /// Metadata table holding the next terminal sequence number.
    pub(crate) meta: TableDefinition<'static, &'static str, u64>,
    /// Metadata key holding the next terminal sequence number.
    pub(crate) next_terminal_sequence: &'static str,
    /// Error when attempt metadata has the wrong length.
    pub(crate) invalid_attempt_length: &'static str,
    /// Error when the terminal sequence metadata is missing.
    pub(crate) missing_sequence: &'static str,
    /// Error when the terminal sequence overflows.
    pub(crate) sequence_overflow: &'static str,
}

/// Record kind with a durable redb layout.
pub(crate) trait RedbSessionRecord: SessionRecord {
    /// On-disk layout of this record kind.
    const TABLES: RedbTables;

    /// Encode the complete record.
    fn encode(&self) -> Result<Vec<u8>, Error>;

    /// Decode and validate one encoded record.
    fn decode(bytes: &[u8]) -> Result<Self, Error>;

    /// Encode the replay tombstone for this terminal record.
    fn encode_tombstone(&self) -> Result<Vec<u8>, Error>;
}

/// Create any missing tables of one record kind and its sequence metadata.
pub(crate) fn initialize_tables(
    transaction: &redb::WriteTransaction,
    tables: &RedbTables,
) -> Result<(), Error> {
    let _ = transaction
        .open_table(tables.records)
        .map_err(persistence)?;
    if let Some(sessions) = tables.sessions {
        let _ = transaction.open_table(sessions).map_err(persistence)?;
    }
    let _ = transaction
        .open_table(tables.active_slots)
        .map_err(persistence)?;
    let _ = transaction
        .open_table(tables.latest_attempts)
        .map_err(persistence)?;
    let _ = transaction
        .open_table(tables.tombstones)
        .map_err(persistence)?;
    let _ = transaction
        .open_table(tables.terminal_order)
        .map_err(persistence)?;
    let mut meta = transaction.open_table(tables.meta).map_err(persistence)?;
    if meta
        .get(tables.next_terminal_sequence)
        .map_err(persistence)?
        .is_none()
    {
        meta.insert(tables.next_terminal_sequence, 0)
            .map_err(persistence)?;
    }
    Ok(())
}

/// One record kind's tables in a redb database.
pub(crate) struct RedbSessionTable<'a, R> {
    database: &'a Database,
    record: PhantomData<R>,
}

impl<'a, R> RedbSessionTable<'a, R>
where
    R: RedbSessionRecord,
{
    /// Select one record kind's tables.
    pub(crate) fn new(database: &'a Database) -> Self {
        const {
            assert!(
                R::TABLES.sessions.is_none() == R::KEYED_BY_SESSION,
                "exactly the kinds not keyed by session need a session index table",
            );
        }
        Self {
            database,
            record: PhantomData,
        }
    }

    /// Advance the attempt high-water mark, burning an older active record.
    pub(crate) fn activate_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        R::validate_activation(context)?;
        let tables = R::TABLES;
        let transaction = self.database.begin_write().map_err(persistence)?;
        let slot_key = context.slot_id().to_bytes();
        let current = read_attempt(&transaction, &tables, &slot_key)?;
        if check_attempt::<R>(current, context)? {
            return Ok(());
        }

        if let Some(active_key) = read_bytes(&transaction, tables.active_slots, &slot_key)? {
            let existing = read_record::<R>(&transaction, &active_key)?
                .ok_or_else(|| R::error(Failure::ActiveRecordMissing))?;
            let burned = existing.burned_for_replacement()?;
            R::validate_transition(&existing, &burned)?;
            let bytes = burned.encode()?;
            {
                let mut records = transaction
                    .open_table(tables.records)
                    .map_err(persistence)?;
                records
                    .insert(active_key.as_slice(), bytes.as_slice())
                    .map_err(persistence)?;
                let mut slots = transaction
                    .open_table(tables.active_slots)
                    .map_err(persistence)?;
                slots.remove(slot_key.as_slice()).map_err(persistence)?;
            }
            append_terminal(&transaction, &tables, &active_key)?;
        }
        let encoded = encode_attempt(context.attempt(), context.session_id());
        {
            let mut attempts = transaction
                .open_table(tables.latest_attempts)
                .map_err(persistence)?;
            attempts
                .insert(slot_key.as_slice(), encoded.as_slice())
                .map_err(persistence)?;
        }
        transaction.commit().map_err(persistence)
    }

    /// Reserve a new record, or return the exact existing session.
    pub(crate) fn reserve(&self, record: R) -> Result<R::Reservation, Error> {
        record.validate_new()?;
        let tables = R::TABLES;
        let transaction = self.database.begin_write().map_err(persistence)?;
        let session_key = record.context().session_id().to_bytes();
        let slot_key = record.context().slot_id().to_bytes();
        let record_key = R::key_bytes(record.key());
        match tables.sessions {
            Some(sessions) => {
                if let Some(existing_key) = read_bytes(&transaction, sessions, &session_key)? {
                    return match read_record::<R>(&transaction, &existing_key)? {
                        Some(existing) => Ok(R::existing(existing)),
                        None => Err(R::error(Failure::SessionCompacted)),
                    };
                }
            }
            None => {
                if let Some(existing) = read_record::<R>(&transaction, &record_key)? {
                    return Ok(R::existing(existing));
                }
                if contains(&transaction, tables.tombstones, &record_key)? {
                    return Err(R::error(Failure::SessionCompacted));
                }
            }
        }
        if read_attempt(&transaction, &tables, &slot_key)?
            != Some((record.context().attempt(), record.context().session_id()))
        {
            return Err(R::error(Failure::NotActivated));
        }
        if contains(&transaction, tables.active_slots, &slot_key)? {
            return Err(R::error(Failure::SlotClaimed));
        }
        if !R::KEYED_BY_SESSION
            && (contains(&transaction, tables.records, &record_key)?
                || contains(&transaction, tables.tombstones, &record_key)?)
        {
            return Err(R::error(Failure::KeyClaimed));
        }
        let bytes = record.encode()?;
        {
            let mut records = transaction
                .open_table(tables.records)
                .map_err(persistence)?;
            records
                .insert(record_key.as_slice(), bytes.as_slice())
                .map_err(persistence)?;
            if let Some(sessions) = tables.sessions {
                let mut sessions = transaction.open_table(sessions).map_err(persistence)?;
                sessions
                    .insert(session_key.as_slice(), record_key.as_slice())
                    .map_err(persistence)?;
            }
            let mut slots = transaction
                .open_table(tables.active_slots)
                .map_err(persistence)?;
            slots
                .insert(slot_key.as_slice(), record_key.as_slice())
                .map_err(persistence)?;
        }
        transaction.commit().map_err(persistence)?;
        Ok(R::inserted())
    }

    /// Load a detailed record by key, rejecting compacted keys.
    pub(crate) fn load_key(&self, key: R::Key) -> Result<Option<R>, Error> {
        let transaction = self.database.begin_read().map_err(persistence)?;
        self.load_key_bytes(&transaction, &R::key_bytes(key))
    }

    /// Load a detailed record by session.
    pub(crate) fn load_session(&self, session_id: SigningSessionId) -> Result<Option<R>, Error> {
        let transaction = self.database.begin_read().map_err(persistence)?;
        let session_key = session_id.to_bytes();
        let Some(sessions) = R::TABLES.sessions else {
            return self.load_key_bytes(&transaction, &session_key);
        };
        let record_key = {
            let sessions = transaction.open_table(sessions).map_err(persistence)?;
            sessions
                .get(session_key.as_slice())
                .map_err(persistence)?
                .map(|value| value.value().to_vec())
        };
        let Some(record_key) = record_key else {
            return Ok(None);
        };
        let records = transaction
            .open_table(R::TABLES.records)
            .map_err(persistence)?;
        records
            .get(record_key.as_slice())
            .map_err(persistence)?
            .map(|value| R::decode(value.value()))
            .transpose()
    }

    /// Replace one exact record revision.
    pub(crate) fn compare_and_swap(&self, expected: &R, replacement: R) -> Result<(), Error> {
        R::validate_transition(expected, &replacement)?;
        let tables = R::TABLES;
        let transaction = self.database.begin_write().map_err(persistence)?;
        let record_key = R::key_bytes(expected.key());
        let current = read_record::<R>(&transaction, &record_key)?
            .ok_or_else(|| R::error(Failure::RecordDisappeared))?;
        if current != *expected {
            return Err(R::error(Failure::ChangedConcurrently));
        }
        let bytes = replacement.encode()?;
        {
            let mut records = transaction
                .open_table(tables.records)
                .map_err(persistence)?;
            records
                .insert(record_key.as_slice(), bytes.as_slice())
                .map_err(persistence)?;
        }
        if !expected.is_terminal() && replacement.is_terminal() {
            let slot_key = expected.context().slot_id().to_bytes();
            {
                let mut slots = transaction
                    .open_table(tables.active_slots)
                    .map_err(persistence)?;
                slots.remove(slot_key.as_slice()).map_err(persistence)?;
            }
            append_terminal(&transaction, &tables, &record_key)?;
        }
        transaction.commit().map_err(persistence)
    }

    /// Every nonterminal detailed record.
    pub(crate) fn nonterminal_records(&self) -> Result<Vec<R>, Error> {
        let transaction = self.database.begin_read().map_err(persistence)?;
        let records = transaction
            .open_table(R::TABLES.records)
            .map_err(persistence)?;
        let mut output = Vec::new();
        for item in records.iter().map_err(persistence)? {
            let (_, value) = item.map_err(persistence)?;
            let record = R::decode(value.value())?;
            if !record.is_terminal() {
                output.push(record);
            }
        }
        Ok(output)
    }

    /// Compact old terminal detail into replay tombstones, oldest first.
    pub(crate) fn compact_terminal(&self, retain_detailed: usize) -> Result<usize, Error> {
        let tables = R::TABLES;
        let transaction = self.database.begin_write().map_err(persistence)?;
        let terminal = {
            let order = transaction
                .open_table(tables.terminal_order)
                .map_err(persistence)?;
            let mut entries = Vec::new();
            for item in order.iter().map_err(persistence)? {
                let (sequence, key) = item.map_err(persistence)?;
                entries.push((sequence.value(), key.value().to_vec()));
            }
            let compact = entries.len().saturating_sub(retain_detailed);
            entries.into_iter().take(compact).collect::<Vec<_>>()
        };
        for (sequence, record_key) in &terminal {
            let record = read_record::<R>(&transaction, record_key)?
                .ok_or_else(|| R::error(Failure::TerminalRecordMissing))?;
            if !record.is_terminal() {
                return Err(R::error(Failure::TerminalOrderNonterminal));
            }
            let tombstone = record.encode_tombstone()?;
            {
                let mut records = transaction
                    .open_table(tables.records)
                    .map_err(persistence)?;
                records.remove(record_key.as_slice()).map_err(persistence)?;
                let mut tombstones = transaction
                    .open_table(tables.tombstones)
                    .map_err(persistence)?;
                tombstones
                    .insert(record_key.as_slice(), tombstone.as_slice())
                    .map_err(persistence)?;
                let mut order = transaction
                    .open_table(tables.terminal_order)
                    .map_err(persistence)?;
                order.remove(*sequence).map_err(persistence)?;
            }
        }
        transaction.commit().map_err(persistence)?;
        Ok(terminal.len())
    }

    fn load_key_bytes(
        &self,
        transaction: &redb::ReadTransaction,
        key: &[u8],
    ) -> Result<Option<R>, Error> {
        let record = {
            let records = transaction
                .open_table(R::TABLES.records)
                .map_err(persistence)?;
            let record = records
                .get(key)
                .map_err(persistence)?
                .map(|value| R::decode(value.value()))
                .transpose()?;
            record
        };
        if record.is_some() {
            return Ok(record);
        }
        let compacted = {
            let tombstones = transaction
                .open_table(R::TABLES.tombstones)
                .map_err(persistence)?;
            tombstones.get(key).map_err(persistence)?.is_some()
        };
        if compacted {
            return Err(R::error(Failure::KeyCompacted));
        }
        Ok(None)
    }
}

fn read_record<R>(transaction: &redb::WriteTransaction, key: &[u8]) -> Result<Option<R>, Error>
where
    R: RedbSessionRecord,
{
    let table = transaction
        .open_table(R::TABLES.records)
        .map_err(persistence)?;
    let record = table
        .get(key)
        .map_err(persistence)?
        .map(|value| R::decode(value.value()))
        .transpose()?;
    Ok(record)
}

fn read_bytes(
    transaction: &redb::WriteTransaction,
    definition: BytesTable,
    key: &[u8],
) -> Result<Option<Vec<u8>>, Error> {
    let table = transaction.open_table(definition).map_err(persistence)?;
    let value = table
        .get(key)
        .map_err(persistence)?
        .map(|value| value.value().to_vec());
    Ok(value)
}

fn contains(
    transaction: &redb::WriteTransaction,
    definition: BytesTable,
    key: &[u8],
) -> Result<bool, Error> {
    let table = transaction.open_table(definition).map_err(persistence)?;
    let exists = table.get(key).map_err(persistence)?.is_some();
    Ok(exists)
}

fn read_attempt(
    transaction: &redb::WriteTransaction,
    tables: &RedbTables,
    slot_key: &[u8; 32],
) -> Result<Option<(u32, SigningSessionId)>, Error> {
    let table = transaction
        .open_table(tables.latest_attempts)
        .map_err(persistence)?;
    let attempt = table
        .get(slot_key.as_slice())
        .map_err(persistence)?
        .map(|value| decode_attempt(value.value(), tables))
        .transpose()?;
    Ok(attempt)
}

fn encode_attempt(attempt: u32, session_id: SigningSessionId) -> [u8; 36] {
    let mut encoded = [0_u8; 36];
    encoded[..4].copy_from_slice(&attempt.to_be_bytes());
    encoded[4..].copy_from_slice(&session_id.to_bytes());
    encoded
}

fn decode_attempt(encoded: &[u8], tables: &RedbTables) -> Result<(u32, SigningSessionId), Error> {
    if encoded.len() != 36 {
        return Err(Error::NoncePersistence(
            tables.invalid_attempt_length.to_string(),
        ));
    }
    let attempt = u32::from_be_bytes(
        encoded[..4]
            .try_into()
            .expect("validated attempt prefix length"),
    );
    let session_id = SigningSessionId::from_bytes(
        encoded[4..]
            .try_into()
            .expect("validated session identifier length"),
    )?;
    Ok((attempt, session_id))
}

fn append_terminal(
    transaction: &redb::WriteTransaction,
    tables: &RedbTables,
    record_key: &[u8],
) -> Result<(), Error> {
    let sequence = next_terminal_sequence(transaction, tables)?;
    let mut order = transaction
        .open_table(tables.terminal_order)
        .map_err(persistence)?;
    order.insert(sequence, record_key).map_err(persistence)?;
    Ok(())
}

fn next_terminal_sequence(
    transaction: &redb::WriteTransaction,
    tables: &RedbTables,
) -> Result<u64, Error> {
    let mut meta = transaction.open_table(tables.meta).map_err(persistence)?;
    let current = meta
        .get(tables.next_terminal_sequence)
        .map_err(persistence)?
        .ok_or_else(|| Error::NoncePersistence(tables.missing_sequence.to_string()))?
        .value();
    let next = current
        .checked_add(1)
        .ok_or_else(|| Error::NoncePersistence(tables.sequence_overflow.to_string()))?;
    meta.insert(tables.next_terminal_sequence, next)
        .map_err(persistence)?;
    Ok(current)
}

/// Map a redb or encoding failure to a nonce persistence error.
pub(crate) fn persistence(error: impl fmt::Display) -> Error {
    Error::NoncePersistence(error.to_string())
}

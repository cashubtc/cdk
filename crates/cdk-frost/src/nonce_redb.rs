//! Durable redb implementation of the participant-local nonce store.
//!
//! The store lives in its own file, never in the mint's SQL database, and must
//! never be restored from a backup: a restored copy would offer consumed nonces
//! again, and a nonce used for two different messages reveals the key share.
//! If the file is lost or suspect, delete it; the member generates fresh nonces.
//! See the crate README.

use std::fmt;
use std::path::Path;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use redb::{Database, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::session_redb::{
    initialize_tables, persistence, RedbSessionRecord, RedbSessionTable, RedbTables,
};
use crate::{
    DurableNonceStore, Error, NonceBurnReason, NonceId, NonceLifecycle, NonceRecord,
    NonceReservation, NonceTransitionChain, SigningSessionContext, SigningSessionId,
};

const RECORDS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("frost_nonce_records_v1");
const SESSIONS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("frost_nonce_sessions_v1");
const ACTIVE_SLOTS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("frost_nonce_active_slots_v1");
const LATEST_ATTEMPTS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("frost_nonce_latest_attempts_v1");
const TOMBSTONES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("frost_nonce_tombstones_v1");
const TERMINAL_ORDER: TableDefinition<u64, &[u8]> =
    TableDefinition::new("frost_nonce_terminal_order_v1");
const META: TableDefinition<&str, u64> = TableDefinition::new("frost_nonce_meta_v1");
const NEXT_TERMINAL_SEQUENCE: &str = "next_terminal_sequence";

/// Transactional participant-local redb nonce store.
#[derive(Clone)]
pub struct RedbNonceStore {
    pub(crate) database: Arc<Database>,
    #[cfg(test)]
    write_transactions: Arc<AtomicUsize>,
}

impl fmt::Debug for RedbNonceStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedbNonceStore")
            .field("database", &"<private local nonce database>")
            .finish()
    }
}

impl RedbNonceStore {
    /// Open or create a private local nonce database and initialize its schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let database = Database::create(path).map_err(persistence)?;
        let store = Self {
            database: Arc::new(database),
            #[cfg(test)]
            write_transactions: Arc::new(AtomicUsize::new(0)),
        };
        store.initialize()?;
        crate::protocol_redb::initialize_database(&store.database)?;
        Ok(store)
    }

    #[cfg(test)]
    pub(crate) fn write_tx_count(&self) -> usize {
        self.write_transactions.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn reset_write_tx_count(&self) {
        self.write_transactions.store(0, Ordering::SeqCst);
    }

    fn initialize(&self) -> Result<(), Error> {
        let transaction = self.database.begin_write().map_err(persistence)?;
        initialize_tables(&transaction, &NonceRecord::TABLES)?;
        transaction.commit().map_err(persistence)
    }

    pub(crate) fn table<R>(&self) -> RedbSessionTable<'_, R>
    where
        R: RedbSessionRecord,
    {
        let table = RedbSessionTable::new(&self.database);
        #[cfg(test)]
        let table = table.with_write_counter(&self.write_transactions);
        table
    }
}

impl DurableNonceStore for RedbNonceStore {
    fn activate_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        self.table::<NonceRecord>().activate_attempt(context)
    }

    fn reserve(&self, record: NonceRecord) -> Result<NonceReservation, Error> {
        self.table::<NonceRecord>().reserve(record)
    }

    fn commit_batch(&self, records: Vec<NonceRecord>) -> Result<Vec<NonceReservation>, Error> {
        self.table::<NonceRecord>().commit_nonce_batch(records)
    }

    fn load_nonce(&self, nonce_id: NonceId) -> Result<Option<NonceRecord>, Error> {
        self.table::<NonceRecord>().load_key(nonce_id)
    }

    fn load_nonces_batch(
        &self,
        nonce_ids: &[NonceId],
    ) -> Result<Vec<Result<Option<NonceRecord>, Error>>, Error> {
        self.table::<NonceRecord>().load_keys_batch(nonce_ids)
    }

    fn load_session(&self, session_id: SigningSessionId) -> Result<Option<NonceRecord>, Error> {
        self.table::<NonceRecord>().load_session(session_id)
    }

    fn compare_and_swap(
        &self,
        expected: &NonceRecord,
        replacement: NonceRecord,
    ) -> Result<(), Error> {
        self.table::<NonceRecord>()
            .compare_and_swap(expected, replacement)
    }

    fn apply_batch_transitions(&self, chains: Vec<NonceTransitionChain>) -> Result<(), Error> {
        self.table::<NonceRecord>()
            .apply_nonce_batch_transitions(chains)
    }

    fn burn_sessions_batch(
        &self,
        sessions: &[SigningSessionId],
        reason: NonceBurnReason,
    ) -> Result<(), Error> {
        self.table::<NonceRecord>()
            .burn_nonce_sessions_batch(sessions, reason)
    }

    fn nonterminal_records(&self) -> Result<Vec<NonceRecord>, Error> {
        self.table::<NonceRecord>().nonterminal_records()
    }

    fn compact_terminal(&self, retain_detailed: usize) -> Result<usize, Error> {
        self.table::<NonceRecord>()
            .compact_terminal(retain_detailed)
    }
}

impl RedbSessionRecord for NonceRecord {
    const TABLES: RedbTables = RedbTables {
        records: RECORDS,
        sessions: Some(SESSIONS),
        active_slots: ACTIVE_SLOTS,
        latest_attempts: LATEST_ATTEMPTS,
        tombstones: TOMBSTONES,
        terminal_order: TERMINAL_ORDER,
        meta: META,
        next_terminal_sequence: NEXT_TERMINAL_SEQUENCE,
        invalid_attempt_length: "signing attempt metadata has an invalid length",
        missing_sequence: "nonce sequence metadata is missing",
        sequence_overflow: "terminal nonce sequence overflow",
    };

    fn encode(&self) -> Result<Vec<u8>, Error> {
        self.to_bytes()
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_bytes(bytes)
    }

    fn encode_tombstone(&self) -> Result<Vec<u8>, Error> {
        let tombstone = NonceTombstoneWire {
            session_id: self.context().session_id().to_bytes(),
            slot_id: self.context().slot_id().to_bytes(),
            nonce_id: self.nonce_id().to_bytes(),
            lifecycle: self.lifecycle().clone(),
        };
        serde_json::to_vec(&tombstone).map_err(|error| Error::NoncePersistence(error.to_string()))
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NonceTombstoneWire {
    session_id: [u8; 32],
    slot_id: [u8; 32],
    nonce_id: [u8; 32],
    lifecycle: NonceLifecycle,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DurableProtocolStore;

    #[test]
    fn debug_does_not_expose_database_path() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("private-nonces.redb");
        let store = RedbNonceStore::open(&path).expect("redb nonce store");
        let debug = format!("{store:?}");
        assert!(debug.contains("<private local nonce database>"));
        assert!(!debug.contains(path.to_string_lossy().as_ref()));
    }

    #[test]
    fn opening_ordinary_only_schema_adds_empty_protocol_tables() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("legacy-ordinary-nonces.redb");
        {
            let database = Database::create(&path).expect("legacy database");
            let transaction = database.begin_write().expect("legacy transaction");
            {
                let _ = transaction.open_table(RECORDS).expect("records");
                let _ = transaction.open_table(SESSIONS).expect("sessions");
                let _ = transaction.open_table(ACTIVE_SLOTS).expect("active slots");
                let _ = transaction
                    .open_table(LATEST_ATTEMPTS)
                    .expect("latest attempts");
                let _ = transaction.open_table(TOMBSTONES).expect("tombstones");
                let _ = transaction
                    .open_table(TERMINAL_ORDER)
                    .expect("terminal order");
                let mut meta = transaction.open_table(META).expect("metadata");
                meta.insert(NEXT_TERMINAL_SEQUENCE, 0)
                    .expect("terminal sequence");
            }
            transaction.commit().expect("legacy commit");
        }

        let store = RedbNonceStore::open(&path).expect("upgrade-compatible open");
        assert!(store
            .nonterminal_records()
            .expect("ordinary records")
            .is_empty());
        assert!(store
            .nonterminal_virtual_musig_records()
            .expect("virtual MuSig2 records")
            .is_empty());
        assert!(store
            .nonterminal_threshold_dh_records()
            .expect("threshold-DH records")
            .is_empty());
    }
}

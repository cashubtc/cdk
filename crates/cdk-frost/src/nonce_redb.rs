//! Durable redb implementation of the participant-local nonce store.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::nonce::{validate_new_nonce_record, validate_nonce_transition};
use crate::{
    DurableNonceStore, Error, NonceBurnReason, NonceId, NonceLifecycle, NonceRecord,
    NonceReservation, SigningSessionContext, SigningSessionId,
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
        };
        store.initialize()?;
        crate::protocol_redb::initialize_database(&store.database)?;
        Ok(store)
    }

    fn initialize(&self) -> Result<(), Error> {
        let transaction = self.database.begin_write().map_err(persistence)?;
        {
            let _ = transaction.open_table(RECORDS).map_err(persistence)?;
            let _ = transaction.open_table(SESSIONS).map_err(persistence)?;
            let _ = transaction.open_table(ACTIVE_SLOTS).map_err(persistence)?;
            let _ = transaction
                .open_table(LATEST_ATTEMPTS)
                .map_err(persistence)?;
            let _ = transaction.open_table(TOMBSTONES).map_err(persistence)?;
            let _ = transaction
                .open_table(TERMINAL_ORDER)
                .map_err(persistence)?;
            let mut meta = transaction.open_table(META).map_err(persistence)?;
            if meta
                .get(NEXT_TERMINAL_SEQUENCE)
                .map_err(persistence)?
                .is_none()
            {
                meta.insert(NEXT_TERMINAL_SEQUENCE, 0)
                    .map_err(persistence)?;
            }
        }
        transaction.commit().map_err(persistence)
    }
}

impl DurableNonceStore for RedbNonceStore {
    fn activate_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        let transaction = self.database.begin_write().map_err(persistence)?;
        let slot_key = context.slot_id().to_bytes();
        let current = {
            let attempts = transaction
                .open_table(LATEST_ATTEMPTS)
                .map_err(persistence)?;
            let value = attempts
                .get(slot_key.as_slice())
                .map_err(persistence)?
                .map(|value| decode_attempt(value.value()))
                .transpose()?;
            value
        };
        match current {
            Some((attempt, _)) if attempt > context.attempt() => {
                return Err(Error::InvalidSigningSession(
                    "signing attempt is stale".to_string(),
                ));
            }
            Some((attempt, session_id)) if attempt == context.attempt() => {
                if session_id != context.session_id() {
                    return Err(Error::InvalidSigningSession(
                        "signer set changed within an active attempt".to_string(),
                    ));
                }
                return Ok(());
            }
            _ => {}
        }

        let active_nonce = {
            let slots = transaction.open_table(ACTIVE_SLOTS).map_err(persistence)?;
            let value = slots
                .get(slot_key.as_slice())
                .map_err(persistence)?
                .map(|value| value.value().to_vec());
            value
        };
        if let Some(active_nonce) = active_nonce {
            let existing = {
                let records = transaction.open_table(RECORDS).map_err(persistence)?;
                let bytes = records
                    .get(active_nonce.as_slice())
                    .map_err(persistence)?
                    .map(|value| value.value().to_vec())
                    .ok_or_else(|| {
                        Error::NoncePersistence(
                            "active signing slot has no nonce record".to_string(),
                        )
                    })?;
                NonceRecord::from_bytes(&bytes)?
            };
            let burned = existing.transition(
                NonceLifecycle::Burned {
                    reason: NonceBurnReason::ReplacedAttempt,
                },
                false,
                None,
            )?;
            validate_nonce_transition(&existing, &burned)?;
            let burned_bytes = burned.to_bytes()?;
            {
                let mut records = transaction.open_table(RECORDS).map_err(persistence)?;
                records
                    .insert(active_nonce.as_slice(), burned_bytes.as_slice())
                    .map_err(persistence)?;
                let mut slots = transaction.open_table(ACTIVE_SLOTS).map_err(persistence)?;
                slots.remove(slot_key.as_slice()).map_err(persistence)?;
            }
            let sequence = next_terminal_sequence(&transaction)?;
            let mut order = transaction
                .open_table(TERMINAL_ORDER)
                .map_err(persistence)?;
            order
                .insert(sequence, active_nonce.as_slice())
                .map_err(persistence)?;
        }
        let encoded = encode_attempt(context.attempt(), context.session_id());
        let mut attempts = transaction
            .open_table(LATEST_ATTEMPTS)
            .map_err(persistence)?;
        attempts
            .insert(slot_key.as_slice(), encoded.as_slice())
            .map_err(persistence)?;
        drop(attempts);
        transaction.commit().map_err(persistence)
    }

    fn reserve(&self, record: NonceRecord) -> Result<NonceReservation, Error> {
        let transaction = self.database.begin_write().map_err(persistence)?;
        let session_key = record.context().session_id().to_bytes();
        let slot_key = record.context().slot_id().to_bytes();
        let nonce_key = record.nonce_id().to_bytes();
        let existing_nonce = {
            let sessions = transaction.open_table(SESSIONS).map_err(persistence)?;
            let value = sessions
                .get(session_key.as_slice())
                .map_err(persistence)?
                .map(|value| value.value().to_vec());
            value
        };
        if let Some(existing_nonce) = existing_nonce {
            let existing_bytes = {
                let records = transaction.open_table(RECORDS).map_err(persistence)?;
                let value = records
                    .get(existing_nonce.as_slice())
                    .map_err(persistence)?
                    .map(|value| value.value().to_vec());
                value
            };
            return match existing_bytes {
                Some(bytes) => Ok(NonceReservation::Existing(Box::new(
                    NonceRecord::from_bytes(&bytes)?,
                ))),
                None => Err(Error::InvalidCommitment(
                    "signing session is terminal and cannot be retried".to_string(),
                )),
            };
        }
        let activated = {
            let attempts = transaction
                .open_table(LATEST_ATTEMPTS)
                .map_err(persistence)?;
            let value = attempts
                .get(slot_key.as_slice())
                .map_err(persistence)?
                .map(|value| decode_attempt(value.value()))
                .transpose()?;
            value
        };
        if activated != Some((record.context().attempt(), record.context().session_id())) {
            return Err(Error::InvalidSigningSession(
                "signing attempt was not durably activated".to_string(),
            ));
        }
        let slot_claimed = {
            let slots = transaction.open_table(ACTIVE_SLOTS).map_err(persistence)?;
            let claimed = slots
                .get(slot_key.as_slice())
                .map_err(persistence)?
                .is_some();
            claimed
        };
        if slot_claimed {
            return Err(Error::InvalidSigningSession(
                "authorization slot already has an active signing attempt".to_string(),
            ));
        }
        validate_new_nonce_record(&record)?;
        let nonce_exists = {
            let records = transaction.open_table(RECORDS).map_err(persistence)?;
            let exists = records
                .get(nonce_key.as_slice())
                .map_err(persistence)?
                .is_some();
            exists
        } || {
            let tombstones = transaction.open_table(TOMBSTONES).map_err(persistence)?;
            let exists = tombstones
                .get(nonce_key.as_slice())
                .map_err(persistence)?
                .is_some();
            exists
        };
        if nonce_exists {
            return Err(Error::InvalidCommitment(
                "nonce identifier was already reserved".to_string(),
            ));
        }
        let bytes = record.to_bytes()?;
        {
            let mut records = transaction.open_table(RECORDS).map_err(persistence)?;
            records
                .insert(nonce_key.as_slice(), bytes.as_slice())
                .map_err(persistence)?;
            let mut sessions = transaction.open_table(SESSIONS).map_err(persistence)?;
            sessions
                .insert(session_key.as_slice(), nonce_key.as_slice())
                .map_err(persistence)?;
            let mut slots = transaction.open_table(ACTIVE_SLOTS).map_err(persistence)?;
            slots
                .insert(slot_key.as_slice(), nonce_key.as_slice())
                .map_err(persistence)?;
        }
        transaction.commit().map_err(persistence)?;
        Ok(NonceReservation::Inserted)
    }

    fn load_nonce(&self, nonce_id: NonceId) -> Result<Option<NonceRecord>, Error> {
        let transaction = self.database.begin_read().map_err(persistence)?;
        let key = nonce_id.to_bytes();
        let bytes = {
            let records = transaction.open_table(RECORDS).map_err(persistence)?;
            records
                .get(key.as_slice())
                .map_err(persistence)?
                .map(|value| value.value().to_vec())
        };
        if let Some(bytes) = bytes {
            return Ok(Some(NonceRecord::from_bytes(&bytes)?));
        }
        let compacted = {
            let tombstones = transaction.open_table(TOMBSTONES).map_err(persistence)?;
            tombstones
                .get(key.as_slice())
                .map_err(persistence)?
                .is_some()
        };
        if compacted {
            return Err(Error::InvalidCommitment(
                "nonce is terminal and its retry metadata was compacted".to_string(),
            ));
        }
        Ok(None)
    }

    fn load_session(
        &self,
        session_id: crate::SigningSessionId,
    ) -> Result<Option<NonceRecord>, Error> {
        let transaction = self.database.begin_read().map_err(persistence)?;
        let session_key = session_id.to_bytes();
        let nonce_id = {
            let sessions = transaction.open_table(SESSIONS).map_err(persistence)?;
            sessions
                .get(session_key.as_slice())
                .map_err(persistence)?
                .map(|value| value.value().to_vec())
        };
        let Some(nonce_id) = nonce_id else {
            return Ok(None);
        };
        let bytes = {
            let records = transaction.open_table(RECORDS).map_err(persistence)?;
            records
                .get(nonce_id.as_slice())
                .map_err(persistence)?
                .map(|value| value.value().to_vec())
        };
        bytes
            .map(|bytes| NonceRecord::from_bytes(&bytes))
            .transpose()
    }

    fn compare_and_swap(
        &self,
        expected: &NonceRecord,
        replacement: NonceRecord,
    ) -> Result<(), Error> {
        validate_nonce_transition(expected, &replacement)?;
        let transaction = self.database.begin_write().map_err(persistence)?;
        let nonce_key = expected.nonce_id().to_bytes();
        let current_bytes = {
            let records = transaction.open_table(RECORDS).map_err(persistence)?;
            let bytes = records
                .get(nonce_key.as_slice())
                .map_err(persistence)?
                .map(|value| value.value().to_vec())
                .ok_or_else(|| {
                    Error::NoncePersistence("nonce record disappeared during update".to_string())
                })?;
            bytes
        };
        if NonceRecord::from_bytes(&current_bytes)? != *expected {
            return Err(Error::NoncePersistence(
                "nonce record changed concurrently".to_string(),
            ));
        }
        let replacement_bytes = replacement.to_bytes()?;
        {
            let mut records = transaction.open_table(RECORDS).map_err(persistence)?;
            records
                .insert(nonce_key.as_slice(), replacement_bytes.as_slice())
                .map_err(persistence)?;
        }
        if !expected.lifecycle().is_terminal() && replacement.lifecycle().is_terminal() {
            let slot_key = expected.context().slot_id().to_bytes();
            {
                let mut slots = transaction.open_table(ACTIVE_SLOTS).map_err(persistence)?;
                slots.remove(slot_key.as_slice()).map_err(persistence)?;
            }
            let sequence = next_terminal_sequence(&transaction)?;
            let mut order = transaction
                .open_table(TERMINAL_ORDER)
                .map_err(persistence)?;
            order
                .insert(sequence, nonce_key.as_slice())
                .map_err(persistence)?;
        }
        transaction.commit().map_err(persistence)
    }

    fn nonterminal_records(&self) -> Result<Vec<NonceRecord>, Error> {
        let transaction = self.database.begin_read().map_err(persistence)?;
        let records = transaction.open_table(RECORDS).map_err(persistence)?;
        let mut output = Vec::new();
        for item in records.iter().map_err(persistence)? {
            let (_, bytes) = item.map_err(persistence)?;
            let record = NonceRecord::from_bytes(bytes.value())?;
            if !record.lifecycle().is_terminal() {
                output.push(record);
            }
        }
        Ok(output)
    }

    fn compact_terminal(&self, retain_detailed: usize) -> Result<usize, Error> {
        let transaction = self.database.begin_write().map_err(persistence)?;
        let terminal = {
            let order = transaction
                .open_table(TERMINAL_ORDER)
                .map_err(persistence)?;
            let mut entries = Vec::new();
            for item in order.iter().map_err(persistence)? {
                let (sequence, nonce_id) = item.map_err(persistence)?;
                entries.push((sequence.value(), nonce_id.value().to_vec()));
            }
            let compact = entries.len().saturating_sub(retain_detailed);
            entries.into_iter().take(compact).collect::<Vec<_>>()
        };
        for (sequence, nonce_bytes) in &terminal {
            let record_bytes = {
                let records = transaction.open_table(RECORDS).map_err(persistence)?;
                let bytes = records
                    .get(nonce_bytes.as_slice())
                    .map_err(persistence)?
                    .map(|value| value.value().to_vec())
                    .ok_or_else(|| {
                        Error::NoncePersistence("terminal nonce record is missing".to_string())
                    })?;
                bytes
            };
            let record = NonceRecord::from_bytes(&record_bytes)?;
            if !record.lifecycle().is_terminal() {
                return Err(Error::NoncePersistence(
                    "terminal order references a nonterminal nonce".to_string(),
                ));
            }
            let tombstone = NonceTombstoneWire {
                session_id: record.context().session_id().to_bytes(),
                slot_id: record.context().slot_id().to_bytes(),
                nonce_id: record.nonce_id().to_bytes(),
                lifecycle: record.lifecycle().clone(),
            };
            let tombstone = serde_json::to_vec(&tombstone)
                .map_err(|error| Error::NoncePersistence(error.to_string()))?;
            {
                let mut records = transaction.open_table(RECORDS).map_err(persistence)?;
                records
                    .remove(nonce_bytes.as_slice())
                    .map_err(persistence)?;
                let mut tombstones = transaction.open_table(TOMBSTONES).map_err(persistence)?;
                tombstones
                    .insert(nonce_bytes.as_slice(), tombstone.as_slice())
                    .map_err(persistence)?;
                let mut order = transaction
                    .open_table(TERMINAL_ORDER)
                    .map_err(persistence)?;
                order.remove(*sequence).map_err(persistence)?;
            }
        }
        transaction.commit().map_err(persistence)?;
        Ok(terminal.len())
    }
}

fn encode_attempt(attempt: u32, session_id: SigningSessionId) -> [u8; 36] {
    let mut encoded = [0_u8; 36];
    encoded[..4].copy_from_slice(&attempt.to_be_bytes());
    encoded[4..].copy_from_slice(&session_id.to_bytes());
    encoded
}

fn decode_attempt(encoded: &[u8]) -> Result<(u32, SigningSessionId), Error> {
    if encoded.len() != 36 {
        return Err(Error::NoncePersistence(
            "signing attempt metadata has an invalid length".to_string(),
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

fn next_terminal_sequence(transaction: &redb::WriteTransaction) -> Result<u64, Error> {
    let mut meta = transaction.open_table(META).map_err(persistence)?;
    let current = meta
        .get(NEXT_TERMINAL_SEQUENCE)
        .map_err(persistence)?
        .ok_or_else(|| Error::NoncePersistence("nonce sequence metadata is missing".to_string()))?
        .value();
    let next = current
        .checked_add(1)
        .ok_or_else(|| Error::NoncePersistence("terminal nonce sequence overflow".to_string()))?;
    meta.insert(NEXT_TERMINAL_SEQUENCE, next)
        .map_err(persistence)?;
    Ok(current)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NonceTombstoneWire {
    session_id: [u8; 32],
    slot_id: [u8; 32],
    nonce_id: [u8; 32],
    lifecycle: NonceLifecycle,
}

fn persistence(error: impl fmt::Display) -> Error {
    Error::NoncePersistence(error.to_string())
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

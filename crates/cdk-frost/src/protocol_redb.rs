//! Protocol-specific redb tables and transactional state transitions.

use std::fmt;
use std::sync::Arc;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::nonce::CustodyProtocol;
use crate::protocol::{
    validate_dh_transition, validate_new_dh_record, validate_new_virtual_record,
    validate_virtual_transition,
};
use crate::{
    DurableProtocolStore, Error, NonceBurnReason, RedbNonceStore, SigningSessionContext,
    SigningSessionId, ThresholdDhLifecycle, ThresholdDhRecord, ThresholdDhReservation,
    VirtualMusigLifecycle, VirtualMusigNonceId, VirtualMusigRecord, VirtualMusigReservation,
};

const MUSIG_RECORDS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("frost_virtual_musig2_records_v1");
const MUSIG_SESSIONS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("frost_virtual_musig2_sessions_v1");
const MUSIG_ACTIVE_SLOTS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("frost_virtual_musig2_active_slots_v1");
const MUSIG_LATEST_ATTEMPTS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("frost_virtual_musig2_latest_attempts_v1");
const MUSIG_TOMBSTONES: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("frost_virtual_musig2_tombstones_v1");
const MUSIG_TERMINAL_ORDER: TableDefinition<u64, &[u8]> =
    TableDefinition::new("frost_virtual_musig2_terminal_order_v1");

const DH_RECORDS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("frost_threshold_dh_records_v1");
const DH_ACTIVE_SLOTS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("frost_threshold_dh_active_slots_v1");
const DH_LATEST_ATTEMPTS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("frost_threshold_dh_latest_attempts_v1");
const DH_TOMBSTONES: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("frost_threshold_dh_tombstones_v1");
const DH_TERMINAL_ORDER: TableDefinition<u64, &[u8]> =
    TableDefinition::new("frost_threshold_dh_terminal_order_v1");

const PROTOCOL_META: TableDefinition<&str, u64> = TableDefinition::new("frost_protocol_meta_v1");
const NEXT_MUSIG_TERMINAL_SEQUENCE: &str = "next_virtual_musig2_terminal_sequence";
const NEXT_DH_TERMINAL_SEQUENCE: &str = "next_threshold_dh_terminal_sequence";

pub(crate) fn initialize_database(database: &Arc<Database>) -> Result<(), Error> {
    let transaction = database.begin_write().map_err(persistence)?;
    {
        let _ = transaction.open_table(MUSIG_RECORDS).map_err(persistence)?;
        let _ = transaction
            .open_table(MUSIG_SESSIONS)
            .map_err(persistence)?;
        let _ = transaction
            .open_table(MUSIG_ACTIVE_SLOTS)
            .map_err(persistence)?;
        let _ = transaction
            .open_table(MUSIG_LATEST_ATTEMPTS)
            .map_err(persistence)?;
        let _ = transaction
            .open_table(MUSIG_TOMBSTONES)
            .map_err(persistence)?;
        let _ = transaction
            .open_table(MUSIG_TERMINAL_ORDER)
            .map_err(persistence)?;
        let _ = transaction.open_table(DH_RECORDS).map_err(persistence)?;
        let _ = transaction
            .open_table(DH_ACTIVE_SLOTS)
            .map_err(persistence)?;
        let _ = transaction
            .open_table(DH_LATEST_ATTEMPTS)
            .map_err(persistence)?;
        let _ = transaction.open_table(DH_TOMBSTONES).map_err(persistence)?;
        let _ = transaction
            .open_table(DH_TERMINAL_ORDER)
            .map_err(persistence)?;
        let mut meta = transaction.open_table(PROTOCOL_META).map_err(persistence)?;
        for key in [NEXT_MUSIG_TERMINAL_SEQUENCE, NEXT_DH_TERMINAL_SEQUENCE] {
            if meta.get(key).map_err(persistence)?.is_none() {
                meta.insert(key, 0).map_err(persistence)?;
            }
        }
    }
    transaction.commit().map_err(persistence)
}

impl DurableProtocolStore for RedbNonceStore {
    fn activate_virtual_musig_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        require_protocol(context, CustodyProtocol::VirtualMusig2)?;
        let transaction = self.database.begin_write().map_err(persistence)?;
        let slot_key = context.slot_id().to_bytes();
        let current = read_attempt(&transaction, MUSIG_LATEST_ATTEMPTS, &slot_key)?;
        match current {
            Some((attempt, _)) if attempt > context.attempt() => {
                return Err(Error::InvalidSigningSession(
                    "virtual MuSig2 attempt is stale".to_string(),
                ));
            }
            Some((attempt, session)) if attempt == context.attempt() => {
                if session != context.session_id() {
                    return Err(Error::InvalidSigningSession(
                        "signer set changed within a virtual MuSig2 attempt".to_string(),
                    ));
                }
                return Ok(());
            }
            _ => {}
        }
        let active_nonce = {
            let slots = transaction
                .open_table(MUSIG_ACTIVE_SLOTS)
                .map_err(persistence)?;
            let value = slots
                .get(slot_key.as_slice())
                .map_err(persistence)?
                .map(|value| value.value().to_vec());
            value
        };
        if let Some(active_nonce) = active_nonce {
            let existing = read_musig_record(&transaction, &active_nonce)?.ok_or_else(|| {
                Error::NoncePersistence("active virtual MuSig2 record is missing".to_string())
            })?;
            let burned = existing.transition(
                VirtualMusigLifecycle::Burned {
                    reason: NonceBurnReason::ReplacedAttempt,
                },
                false,
                None,
            )?;
            validate_virtual_transition(&existing, &burned)?;
            let bytes = burned.to_bytes()?;
            {
                let mut records = transaction.open_table(MUSIG_RECORDS).map_err(persistence)?;
                records
                    .insert(active_nonce.as_slice(), bytes.as_slice())
                    .map_err(persistence)?;
                let mut slots = transaction
                    .open_table(MUSIG_ACTIVE_SLOTS)
                    .map_err(persistence)?;
                slots.remove(slot_key.as_slice()).map_err(persistence)?;
            }
            append_terminal(
                &transaction,
                MUSIG_TERMINAL_ORDER,
                NEXT_MUSIG_TERMINAL_SEQUENCE,
                &active_nonce,
            )?;
        }
        write_attempt(
            &transaction,
            MUSIG_LATEST_ATTEMPTS,
            &slot_key,
            context.attempt(),
            context.session_id(),
        )?;
        transaction.commit().map_err(persistence)
    }

    fn reserve_virtual_musig(
        &self,
        record: VirtualMusigRecord,
    ) -> Result<VirtualMusigReservation, Error> {
        validate_new_virtual_record(&record)?;
        let transaction = self.database.begin_write().map_err(persistence)?;
        let session_key = record.context().session_id().to_bytes();
        let slot_key = record.context().slot_id().to_bytes();
        let nonce_key = record.nonce_id().to_bytes();
        let existing_nonce = {
            let sessions = transaction
                .open_table(MUSIG_SESSIONS)
                .map_err(persistence)?;
            let value = sessions
                .get(session_key.as_slice())
                .map_err(persistence)?
                .map(|value| value.value().to_vec());
            value
        };
        if let Some(existing_nonce) = existing_nonce {
            return match read_musig_record(&transaction, &existing_nonce)? {
                Some(existing) => Ok(VirtualMusigReservation::Existing(Box::new(existing))),
                None => Err(Error::InvalidCommitment(
                    "virtual MuSig2 session is terminal and compacted".to_string(),
                )),
            };
        }
        if read_attempt(&transaction, MUSIG_LATEST_ATTEMPTS, &slot_key)?
            != Some((record.context().attempt(), record.context().session_id()))
        {
            return Err(Error::InvalidSigningSession(
                "virtual MuSig2 attempt was not durably activated".to_string(),
            ));
        }
        let slot_claimed = {
            let slots = transaction
                .open_table(MUSIG_ACTIVE_SLOTS)
                .map_err(persistence)?;
            let claimed = slots
                .get(slot_key.as_slice())
                .map_err(persistence)?
                .is_some();
            claimed
        };
        let nonce_exists = read_musig_record(&transaction, &nonce_key)?.is_some() || {
            let tombstones = transaction
                .open_table(MUSIG_TOMBSTONES)
                .map_err(persistence)?;
            let exists = tombstones
                .get(nonce_key.as_slice())
                .map_err(persistence)?
                .is_some();
            exists
        };
        if slot_claimed || nonce_exists {
            return Err(Error::InvalidSigningSession(
                "virtual MuSig2 slot or nonce is already claimed".to_string(),
            ));
        }
        let bytes = record.to_bytes()?;
        {
            let mut records = transaction.open_table(MUSIG_RECORDS).map_err(persistence)?;
            records
                .insert(nonce_key.as_slice(), bytes.as_slice())
                .map_err(persistence)?;
            let mut sessions = transaction
                .open_table(MUSIG_SESSIONS)
                .map_err(persistence)?;
            sessions
                .insert(session_key.as_slice(), nonce_key.as_slice())
                .map_err(persistence)?;
            let mut slots = transaction
                .open_table(MUSIG_ACTIVE_SLOTS)
                .map_err(persistence)?;
            slots
                .insert(slot_key.as_slice(), nonce_key.as_slice())
                .map_err(persistence)?;
        }
        transaction.commit().map_err(persistence)?;
        Ok(VirtualMusigReservation::Inserted)
    }

    fn load_virtual_musig_nonce(
        &self,
        nonce_id: VirtualMusigNonceId,
    ) -> Result<Option<VirtualMusigRecord>, Error> {
        let transaction = self.database.begin_read().map_err(persistence)?;
        let key = nonce_id.to_bytes();
        if let Some(record) = read_musig_record_read(&transaction, &key)? {
            return Ok(Some(record));
        }
        let compacted = {
            let tombstones = transaction
                .open_table(MUSIG_TOMBSTONES)
                .map_err(persistence)?;
            tombstones
                .get(key.as_slice())
                .map_err(persistence)?
                .is_some()
        };
        if compacted {
            return Err(Error::InvalidCommitment(
                "virtual MuSig2 nonce is terminal and compacted".to_string(),
            ));
        }
        Ok(None)
    }

    fn load_virtual_musig_session(
        &self,
        session_id: SigningSessionId,
    ) -> Result<Option<VirtualMusigRecord>, Error> {
        let transaction = self.database.begin_read().map_err(persistence)?;
        let session_key = session_id.to_bytes();
        let nonce = {
            let sessions = transaction
                .open_table(MUSIG_SESSIONS)
                .map_err(persistence)?;
            sessions
                .get(session_key.as_slice())
                .map_err(persistence)?
                .map(|value| value.value().to_vec())
        };
        let Some(nonce) = nonce else {
            return Ok(None);
        };
        read_musig_record_read(&transaction, &nonce)
    }

    fn compare_and_swap_virtual_musig(
        &self,
        expected: &VirtualMusigRecord,
        replacement: VirtualMusigRecord,
    ) -> Result<(), Error> {
        validate_virtual_transition(expected, &replacement)?;
        let transaction = self.database.begin_write().map_err(persistence)?;
        let nonce_key = expected.nonce_id().to_bytes();
        let current = read_musig_record(&transaction, &nonce_key)?.ok_or_else(|| {
            Error::NoncePersistence("virtual MuSig2 record disappeared".to_string())
        })?;
        if current != *expected {
            return Err(Error::NoncePersistence(
                "virtual MuSig2 record changed concurrently".to_string(),
            ));
        }
        let bytes = replacement.to_bytes()?;
        {
            let mut records = transaction.open_table(MUSIG_RECORDS).map_err(persistence)?;
            records
                .insert(nonce_key.as_slice(), bytes.as_slice())
                .map_err(persistence)?;
        }
        if !expected.lifecycle().is_terminal() && replacement.lifecycle().is_terminal() {
            let slot_key = expected.context().slot_id().to_bytes();
            {
                let mut slots = transaction
                    .open_table(MUSIG_ACTIVE_SLOTS)
                    .map_err(persistence)?;
                slots.remove(slot_key.as_slice()).map_err(persistence)?;
            }
            append_terminal(
                &transaction,
                MUSIG_TERMINAL_ORDER,
                NEXT_MUSIG_TERMINAL_SEQUENCE,
                &nonce_key,
            )?;
        }
        transaction.commit().map_err(persistence)
    }

    fn nonterminal_virtual_musig_records(&self) -> Result<Vec<VirtualMusigRecord>, Error> {
        let transaction = self.database.begin_read().map_err(persistence)?;
        let records = transaction.open_table(MUSIG_RECORDS).map_err(persistence)?;
        let mut output = Vec::new();
        for item in records.iter().map_err(persistence)? {
            let (_, value) = item.map_err(persistence)?;
            let record = VirtualMusigRecord::from_bytes(value.value())?;
            if !record.lifecycle().is_terminal() {
                output.push(record);
            }
        }
        Ok(output)
    }

    fn compact_virtual_musig(&self, retain_detailed: usize) -> Result<usize, Error> {
        let transaction = self.database.begin_write().map_err(persistence)?;
        let terminal = terminal_entries(&transaction, MUSIG_TERMINAL_ORDER, retain_detailed)?;
        for (sequence, nonce_key) in &terminal {
            let record = read_musig_record(&transaction, nonce_key)?.ok_or_else(|| {
                Error::NoncePersistence("terminal virtual MuSig2 record is missing".to_string())
            })?;
            if !record.lifecycle().is_terminal() {
                return Err(Error::NoncePersistence(
                    "virtual MuSig2 terminal order references active state".to_string(),
                ));
            }
            let tombstone = serde_json::to_vec(&MusigTombstoneWire {
                session_id: record.context().session_id().to_bytes(),
                slot_id: record.context().slot_id().to_bytes(),
                nonce_id: record.nonce_id().to_bytes(),
                lifecycle: record.lifecycle().clone(),
            })
            .map_err(persistence)?;
            {
                let mut records = transaction.open_table(MUSIG_RECORDS).map_err(persistence)?;
                records.remove(nonce_key.as_slice()).map_err(persistence)?;
                let mut tombstones = transaction
                    .open_table(MUSIG_TOMBSTONES)
                    .map_err(persistence)?;
                tombstones
                    .insert(nonce_key.as_slice(), tombstone.as_slice())
                    .map_err(persistence)?;
                let mut order = transaction
                    .open_table(MUSIG_TERMINAL_ORDER)
                    .map_err(persistence)?;
                order.remove(*sequence).map_err(persistence)?;
            }
        }
        transaction.commit().map_err(persistence)?;
        Ok(terminal.len())
    }

    fn activate_threshold_dh_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        require_protocol(context, CustodyProtocol::ThresholdDh)?;
        let transaction = self.database.begin_write().map_err(persistence)?;
        let slot_key = context.slot_id().to_bytes();
        let current = read_attempt(&transaction, DH_LATEST_ATTEMPTS, &slot_key)?;
        match current {
            Some((attempt, _)) if attempt > context.attempt() => {
                return Err(Error::InvalidSigningSession(
                    "threshold-DH attempt is stale".to_string(),
                ));
            }
            Some((attempt, session)) if attempt == context.attempt() => {
                if session != context.session_id() {
                    return Err(Error::InvalidSigningSession(
                        "signer set changed within a threshold-DH attempt".to_string(),
                    ));
                }
                return Ok(());
            }
            _ => {}
        }
        let active_session = {
            let slots = transaction
                .open_table(DH_ACTIVE_SLOTS)
                .map_err(persistence)?;
            let value = slots
                .get(slot_key.as_slice())
                .map_err(persistence)?
                .map(|value| value.value().to_vec());
            value
        };
        if let Some(active_session) = active_session {
            let existing = read_dh_record(&transaction, &active_session)?.ok_or_else(|| {
                Error::NoncePersistence("active threshold-DH record is missing".to_string())
            })?;
            let burned = existing.transition(
                ThresholdDhLifecycle::Burned {
                    reason: NonceBurnReason::ReplacedAttempt,
                },
                None,
            )?;
            validate_dh_transition(&existing, &burned)?;
            let bytes = burned.to_bytes()?;
            {
                let mut records = transaction.open_table(DH_RECORDS).map_err(persistence)?;
                records
                    .insert(active_session.as_slice(), bytes.as_slice())
                    .map_err(persistence)?;
                let mut slots = transaction
                    .open_table(DH_ACTIVE_SLOTS)
                    .map_err(persistence)?;
                slots.remove(slot_key.as_slice()).map_err(persistence)?;
            }
            append_terminal(
                &transaction,
                DH_TERMINAL_ORDER,
                NEXT_DH_TERMINAL_SEQUENCE,
                &active_session,
            )?;
        }
        write_attempt(
            &transaction,
            DH_LATEST_ATTEMPTS,
            &slot_key,
            context.attempt(),
            context.session_id(),
        )?;
        transaction.commit().map_err(persistence)
    }

    fn reserve_threshold_dh(
        &self,
        record: ThresholdDhRecord,
    ) -> Result<ThresholdDhReservation, Error> {
        validate_new_dh_record(&record)?;
        let transaction = self.database.begin_write().map_err(persistence)?;
        let session_key = record.context().session_id().to_bytes();
        let slot_key = record.context().slot_id().to_bytes();
        if let Some(existing) = read_dh_record(&transaction, &session_key)? {
            return Ok(ThresholdDhReservation::Existing(Box::new(existing)));
        }
        let compacted = {
            let tombstones = transaction.open_table(DH_TOMBSTONES).map_err(persistence)?;
            let exists = tombstones
                .get(session_key.as_slice())
                .map_err(persistence)?
                .is_some();
            exists
        };
        if compacted {
            return Err(Error::InvalidSigningSession(
                "threshold-DH session is terminal and compacted".to_string(),
            ));
        }
        if read_attempt(&transaction, DH_LATEST_ATTEMPTS, &slot_key)?
            != Some((record.context().attempt(), record.context().session_id()))
        {
            return Err(Error::InvalidSigningSession(
                "threshold-DH attempt was not durably activated".to_string(),
            ));
        }
        let slot_claimed = {
            let slots = transaction
                .open_table(DH_ACTIVE_SLOTS)
                .map_err(persistence)?;
            let claimed = slots
                .get(slot_key.as_slice())
                .map_err(persistence)?
                .is_some();
            claimed
        };
        if slot_claimed {
            return Err(Error::InvalidSigningSession(
                "threshold-DH slot is already claimed".to_string(),
            ));
        }
        let bytes = record.to_bytes()?;
        {
            let mut records = transaction.open_table(DH_RECORDS).map_err(persistence)?;
            records
                .insert(session_key.as_slice(), bytes.as_slice())
                .map_err(persistence)?;
            let mut slots = transaction
                .open_table(DH_ACTIVE_SLOTS)
                .map_err(persistence)?;
            slots
                .insert(slot_key.as_slice(), session_key.as_slice())
                .map_err(persistence)?;
        }
        transaction.commit().map_err(persistence)?;
        Ok(ThresholdDhReservation::Inserted)
    }

    fn load_threshold_dh_session(
        &self,
        session_id: SigningSessionId,
    ) -> Result<Option<ThresholdDhRecord>, Error> {
        let transaction = self.database.begin_read().map_err(persistence)?;
        let key = session_id.to_bytes();
        if let Some(record) = read_dh_record_read(&transaction, &key)? {
            return Ok(Some(record));
        }
        let compacted = {
            let tombstones = transaction.open_table(DH_TOMBSTONES).map_err(persistence)?;
            tombstones
                .get(key.as_slice())
                .map_err(persistence)?
                .is_some()
        };
        if compacted {
            return Err(Error::InvalidSigningSession(
                "threshold-DH session is terminal and compacted".to_string(),
            ));
        }
        Ok(None)
    }

    fn compare_and_swap_threshold_dh(
        &self,
        expected: &ThresholdDhRecord,
        replacement: ThresholdDhRecord,
    ) -> Result<(), Error> {
        validate_dh_transition(expected, &replacement)?;
        let transaction = self.database.begin_write().map_err(persistence)?;
        let session_key = expected.context().session_id().to_bytes();
        let current = read_dh_record(&transaction, &session_key)?.ok_or_else(|| {
            Error::NoncePersistence("threshold-DH record disappeared".to_string())
        })?;
        if current != *expected {
            return Err(Error::NoncePersistence(
                "threshold-DH record changed concurrently".to_string(),
            ));
        }
        let bytes = replacement.to_bytes()?;
        {
            let mut records = transaction.open_table(DH_RECORDS).map_err(persistence)?;
            records
                .insert(session_key.as_slice(), bytes.as_slice())
                .map_err(persistence)?;
        }
        if !expected.lifecycle().is_terminal() && replacement.lifecycle().is_terminal() {
            let slot_key = expected.context().slot_id().to_bytes();
            {
                let mut slots = transaction
                    .open_table(DH_ACTIVE_SLOTS)
                    .map_err(persistence)?;
                slots.remove(slot_key.as_slice()).map_err(persistence)?;
            }
            append_terminal(
                &transaction,
                DH_TERMINAL_ORDER,
                NEXT_DH_TERMINAL_SEQUENCE,
                &session_key,
            )?;
        }
        transaction.commit().map_err(persistence)
    }

    fn nonterminal_threshold_dh_records(&self) -> Result<Vec<ThresholdDhRecord>, Error> {
        let transaction = self.database.begin_read().map_err(persistence)?;
        let records = transaction.open_table(DH_RECORDS).map_err(persistence)?;
        let mut output = Vec::new();
        for item in records.iter().map_err(persistence)? {
            let (_, value) = item.map_err(persistence)?;
            let record = ThresholdDhRecord::from_bytes(value.value())?;
            if !record.lifecycle().is_terminal() {
                output.push(record);
            }
        }
        Ok(output)
    }

    fn compact_threshold_dh(&self, retain_detailed: usize) -> Result<usize, Error> {
        let transaction = self.database.begin_write().map_err(persistence)?;
        let terminal = terminal_entries(&transaction, DH_TERMINAL_ORDER, retain_detailed)?;
        for (sequence, session_key) in &terminal {
            let record = read_dh_record(&transaction, session_key)?.ok_or_else(|| {
                Error::NoncePersistence("terminal threshold-DH record is missing".to_string())
            })?;
            if !record.lifecycle().is_terminal() {
                return Err(Error::NoncePersistence(
                    "threshold-DH terminal order references active state".to_string(),
                ));
            }
            let tombstone = serde_json::to_vec(&DhTombstoneWire {
                session_id: record.context().session_id().to_bytes(),
                slot_id: record.context().slot_id().to_bytes(),
                request_binding: record.request_binding(),
                lifecycle: record.lifecycle().clone(),
            })
            .map_err(persistence)?;
            {
                let mut records = transaction.open_table(DH_RECORDS).map_err(persistence)?;
                records
                    .remove(session_key.as_slice())
                    .map_err(persistence)?;
                let mut tombstones = transaction.open_table(DH_TOMBSTONES).map_err(persistence)?;
                tombstones
                    .insert(session_key.as_slice(), tombstone.as_slice())
                    .map_err(persistence)?;
                let mut order = transaction
                    .open_table(DH_TERMINAL_ORDER)
                    .map_err(persistence)?;
                order.remove(*sequence).map_err(persistence)?;
            }
        }
        transaction.commit().map_err(persistence)?;
        Ok(terminal.len())
    }
}

fn read_musig_record(
    transaction: &redb::WriteTransaction,
    key: &[u8],
) -> Result<Option<VirtualMusigRecord>, Error> {
    let table = transaction.open_table(MUSIG_RECORDS).map_err(persistence)?;
    let record = table
        .get(key)
        .map_err(persistence)?
        .map(|value| VirtualMusigRecord::from_bytes(value.value()))
        .transpose()?;
    Ok(record)
}

fn read_musig_record_read(
    transaction: &redb::ReadTransaction,
    key: &[u8],
) -> Result<Option<VirtualMusigRecord>, Error> {
    let table = transaction.open_table(MUSIG_RECORDS).map_err(persistence)?;
    let record = table
        .get(key)
        .map_err(persistence)?
        .map(|value| VirtualMusigRecord::from_bytes(value.value()))
        .transpose()?;
    Ok(record)
}

fn read_dh_record(
    transaction: &redb::WriteTransaction,
    key: &[u8],
) -> Result<Option<ThresholdDhRecord>, Error> {
    let table = transaction.open_table(DH_RECORDS).map_err(persistence)?;
    let record = table
        .get(key)
        .map_err(persistence)?
        .map(|value| ThresholdDhRecord::from_bytes(value.value()))
        .transpose()?;
    Ok(record)
}

fn read_dh_record_read(
    transaction: &redb::ReadTransaction,
    key: &[u8],
) -> Result<Option<ThresholdDhRecord>, Error> {
    let table = transaction.open_table(DH_RECORDS).map_err(persistence)?;
    let record = table
        .get(key)
        .map_err(persistence)?
        .map(|value| ThresholdDhRecord::from_bytes(value.value()))
        .transpose()?;
    Ok(record)
}

fn read_attempt(
    transaction: &redb::WriteTransaction,
    definition: TableDefinition<&[u8], &[u8]>,
    slot_key: &[u8; 32],
) -> Result<Option<(u32, SigningSessionId)>, Error> {
    let table = transaction.open_table(definition).map_err(persistence)?;
    let attempt = table
        .get(slot_key.as_slice())
        .map_err(persistence)?
        .map(|value| decode_attempt(value.value()))
        .transpose()?;
    Ok(attempt)
}

fn write_attempt(
    transaction: &redb::WriteTransaction,
    definition: TableDefinition<&[u8], &[u8]>,
    slot_key: &[u8; 32],
    attempt: u32,
    session_id: SigningSessionId,
) -> Result<(), Error> {
    let encoded = encode_attempt(attempt, session_id);
    let mut table = transaction.open_table(definition).map_err(persistence)?;
    table
        .insert(slot_key.as_slice(), encoded.as_slice())
        .map_err(persistence)?;
    Ok(())
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
            "protocol attempt metadata has an invalid length".to_string(),
        ));
    }
    let attempt = u32::from_be_bytes(
        encoded[..4]
            .try_into()
            .expect("validated protocol attempt prefix length"),
    );
    let session = SigningSessionId::from_bytes(
        encoded[4..]
            .try_into()
            .expect("validated protocol session identifier length"),
    )?;
    Ok((attempt, session))
}

fn append_terminal(
    transaction: &redb::WriteTransaction,
    definition: TableDefinition<u64, &[u8]>,
    sequence_key: &str,
    record_key: &[u8],
) -> Result<(), Error> {
    let sequence = next_terminal_sequence(transaction, sequence_key)?;
    let mut order = transaction.open_table(definition).map_err(persistence)?;
    order.insert(sequence, record_key).map_err(persistence)?;
    Ok(())
}

fn next_terminal_sequence(transaction: &redb::WriteTransaction, key: &str) -> Result<u64, Error> {
    let mut meta = transaction.open_table(PROTOCOL_META).map_err(persistence)?;
    let current = meta
        .get(key)
        .map_err(persistence)?
        .ok_or_else(|| Error::NoncePersistence("protocol sequence is missing".to_string()))?
        .value();
    let next = current.checked_add(1).ok_or_else(|| {
        Error::NoncePersistence("protocol terminal sequence overflow".to_string())
    })?;
    meta.insert(key, next).map_err(persistence)?;
    Ok(current)
}

fn terminal_entries(
    transaction: &redb::WriteTransaction,
    definition: TableDefinition<u64, &[u8]>,
    retain_detailed: usize,
) -> Result<Vec<(u64, Vec<u8>)>, Error> {
    let order = transaction.open_table(definition).map_err(persistence)?;
    let mut entries = Vec::new();
    for item in order.iter().map_err(persistence)? {
        let (sequence, key) = item.map_err(persistence)?;
        entries.push((sequence.value(), key.value().to_vec()));
    }
    let compact = entries.len().saturating_sub(retain_detailed);
    Ok(entries.into_iter().take(compact).collect())
}

fn require_protocol(
    context: &SigningSessionContext,
    expected: CustodyProtocol,
) -> Result<(), Error> {
    if context.protocol() != expected {
        return Err(Error::InvalidSigningSession(
            "session uses another custody protocol namespace".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MusigTombstoneWire {
    session_id: [u8; 32],
    slot_id: [u8; 32],
    nonce_id: [u8; 32],
    lifecycle: VirtualMusigLifecycle,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DhTombstoneWire {
    session_id: [u8; 32],
    slot_id: [u8; 32],
    request_binding: [u8; 32],
    lifecycle: ThresholdDhLifecycle,
}

fn persistence(error: impl fmt::Display) -> Error {
    Error::NoncePersistence(error.to_string())
}

//! Protocol-specific redb tables and transactional state transitions.

use std::sync::Arc;

use redb::{Database, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::session_redb::{initialize_tables, persistence, RedbSessionRecord, RedbTables};
use crate::{
    DurableProtocolStore, Error, RedbNonceStore, SigningSessionContext, SigningSessionId,
    ThresholdDhLifecycle, ThresholdDhRecord, ThresholdDhReservation, VirtualMusigLifecycle,
    VirtualMusigNonceId, VirtualMusigRecord, VirtualMusigReservation,
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

const INVALID_ATTEMPT_LENGTH: &str = "protocol attempt metadata has an invalid length";
const MISSING_SEQUENCE: &str = "protocol sequence is missing";
const SEQUENCE_OVERFLOW: &str = "protocol terminal sequence overflow";

pub(crate) fn initialize_database(database: &Arc<Database>) -> Result<(), Error> {
    let transaction = database.begin_write().map_err(persistence)?;
    initialize_tables(&transaction, &VirtualMusigRecord::TABLES)?;
    initialize_tables(&transaction, &ThresholdDhRecord::TABLES)?;
    transaction.commit().map_err(persistence)
}

impl DurableProtocolStore for RedbNonceStore {
    fn activate_virtual_musig_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        self.table::<VirtualMusigRecord>().activate_attempt(context)
    }

    fn reserve_virtual_musig(
        &self,
        record: VirtualMusigRecord,
    ) -> Result<VirtualMusigReservation, Error> {
        self.table::<VirtualMusigRecord>().reserve(record)
    }

    fn load_virtual_musig_nonce(
        &self,
        nonce_id: VirtualMusigNonceId,
    ) -> Result<Option<VirtualMusigRecord>, Error> {
        self.table::<VirtualMusigRecord>().load_key(nonce_id)
    }

    fn load_virtual_musig_session(
        &self,
        session_id: SigningSessionId,
    ) -> Result<Option<VirtualMusigRecord>, Error> {
        self.table::<VirtualMusigRecord>().load_session(session_id)
    }

    fn compare_and_swap_virtual_musig(
        &self,
        expected: &VirtualMusigRecord,
        replacement: VirtualMusigRecord,
    ) -> Result<(), Error> {
        self.table::<VirtualMusigRecord>()
            .compare_and_swap(expected, replacement)
    }

    fn nonterminal_virtual_musig_records(&self) -> Result<Vec<VirtualMusigRecord>, Error> {
        self.table::<VirtualMusigRecord>().nonterminal_records()
    }

    fn compact_virtual_musig(&self, retain_detailed: usize) -> Result<usize, Error> {
        self.table::<VirtualMusigRecord>()
            .compact_terminal(retain_detailed)
    }

    fn activate_threshold_dh_attempt(&self, context: &SigningSessionContext) -> Result<(), Error> {
        self.table::<ThresholdDhRecord>().activate_attempt(context)
    }

    fn reserve_threshold_dh(
        &self,
        record: ThresholdDhRecord,
    ) -> Result<ThresholdDhReservation, Error> {
        self.table::<ThresholdDhRecord>().reserve(record)
    }

    fn load_threshold_dh_session(
        &self,
        session_id: SigningSessionId,
    ) -> Result<Option<ThresholdDhRecord>, Error> {
        self.table::<ThresholdDhRecord>().load_session(session_id)
    }

    fn compare_and_swap_threshold_dh(
        &self,
        expected: &ThresholdDhRecord,
        replacement: ThresholdDhRecord,
    ) -> Result<(), Error> {
        self.table::<ThresholdDhRecord>()
            .compare_and_swap(expected, replacement)
    }

    fn nonterminal_threshold_dh_records(&self) -> Result<Vec<ThresholdDhRecord>, Error> {
        self.table::<ThresholdDhRecord>().nonterminal_records()
    }

    fn compact_threshold_dh(&self, retain_detailed: usize) -> Result<usize, Error> {
        self.table::<ThresholdDhRecord>()
            .compact_terminal(retain_detailed)
    }
}

impl RedbSessionRecord for VirtualMusigRecord {
    const TABLES: RedbTables = RedbTables {
        records: MUSIG_RECORDS,
        sessions: Some(MUSIG_SESSIONS),
        active_slots: MUSIG_ACTIVE_SLOTS,
        latest_attempts: MUSIG_LATEST_ATTEMPTS,
        tombstones: MUSIG_TOMBSTONES,
        terminal_order: MUSIG_TERMINAL_ORDER,
        meta: PROTOCOL_META,
        next_terminal_sequence: NEXT_MUSIG_TERMINAL_SEQUENCE,
        invalid_attempt_length: INVALID_ATTEMPT_LENGTH,
        missing_sequence: MISSING_SEQUENCE,
        sequence_overflow: SEQUENCE_OVERFLOW,
    };
    const REDB_COMBINED_CLAIM_CHECK: bool = true;

    fn encode(&self) -> Result<Vec<u8>, Error> {
        self.to_bytes()
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_bytes(bytes)
    }

    fn encode_tombstone(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec(&MusigTombstoneWire {
            session_id: self.context().session_id().to_bytes(),
            slot_id: self.context().slot_id().to_bytes(),
            nonce_id: self.nonce_id().to_bytes(),
            lifecycle: self.lifecycle().clone(),
        })
        .map_err(persistence)
    }
}

impl RedbSessionRecord for ThresholdDhRecord {
    const TABLES: RedbTables = RedbTables {
        records: DH_RECORDS,
        sessions: None,
        active_slots: DH_ACTIVE_SLOTS,
        latest_attempts: DH_LATEST_ATTEMPTS,
        tombstones: DH_TOMBSTONES,
        terminal_order: DH_TERMINAL_ORDER,
        meta: PROTOCOL_META,
        next_terminal_sequence: NEXT_DH_TERMINAL_SEQUENCE,
        invalid_attempt_length: INVALID_ATTEMPT_LENGTH,
        missing_sequence: MISSING_SEQUENCE,
        sequence_overflow: SEQUENCE_OVERFLOW,
    };
    const REDB_COMBINED_CLAIM_CHECK: bool = false;

    fn encode(&self) -> Result<Vec<u8>, Error> {
        self.to_bytes()
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Self::from_bytes(bytes)
    }

    fn encode_tombstone(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec(&DhTombstoneWire {
            session_id: self.context().session_id().to_bytes(),
            slot_id: self.context().slot_id().to_bytes(),
            request_binding: self.request_binding(),
            lifecycle: self.lifecycle().clone(),
        })
        .map_err(persistence)
    }
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

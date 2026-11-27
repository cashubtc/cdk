//! Row storage for consensus state projections.
//!
//! Each projection is stored as one key-value row per entity (intent,
//! transaction, UTXO, allocation, reservation) in its own namespace. Hot-path
//! apply and validation operations perform O(1) point reads and writes against
//! entity rows and compact auxiliary index namespaces (`*_idx`), while cold
//! paths (state transfer, history retention, whole-state diagnostics) load the
//! full entity namespace.

use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use bitcoin::hashes::{sha256, Hash, HashEngine};
use bitcoin::OutPoint;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::canonical::canonical_json_bytes;
use super::{FederationError, FederationMaterializedStateRow, Result};
use crate::database;
use crate::database::mint::{
    DynMintDatabase, DynMintTransaction, KVStoreDatabase, KVStoreTransaction,
};
use crate::error::Error;

/// Primary key-value namespace shared by every consensus state table.
pub const FEDERATION_STATE_PRIMARY_NAMESPACE: &str = "federation";

/// Sentinel key stored inside a projection's auxiliary index namespace once
/// its index rows are synchronized with its authoritative entity rows.
pub(crate) const FEDERATION_STATE_INDEX_INIT_KEY: &str = "_init";

const FEDERATION_STATE_INDEX_INIT_VALUE: &[u8] = b"1";

/// Encoded rows of one projection, keyed by row key.
pub type FederationStateRows = BTreeMap<String, Vec<u8>>;

/// A consensus state projection stored as one row per entity.
///
/// Terminal entities record the journal index of the operation that made
/// them terminal. Operations never read them again: [`Self::retire_terminal_rows`]
/// removes them, or reduces them to a permanent guard, before any operation
/// loads the projection. The stored rows stay until history retention deletes
/// them below the retention floor ([`Self::retire_rows_below`]), so a member
/// that deleted them and one that did not load the same projection.
pub trait FederationStateTable: Sized {
    /// Secondary key-value namespace holding this projection's rows.
    const SECONDARY_NAMESPACE: &'static str;

    /// Optional secondary key-value namespace holding compact O(1) lookup
    /// indexes derived from this projection's rows.
    ///
    /// Index rows live in their own secondary namespace so checkpoint state
    /// digests and state transfers cover only the authoritative entity rows in
    /// [`Self::SECONDARY_NAMESPACE`], while member resets and SQL transactions
    /// still wipe or roll them back atomically.
    const INDEX_SECONDARY_NAMESPACE: Option<&'static str> = None;

    /// Validate the projection and encode one row per entity.
    fn to_rows(&self) -> Result<FederationStateRows>;

    /// Decode every row and validate the rebuilt projection. Every row key must
    /// be the key its content would be stored under.
    fn from_rows(rows: &FederationStateRows) -> Result<Self>;

    /// Rebuild the projection's auxiliary index rows from its stored entity
    /// rows (terminal entities included).
    ///
    /// Called only on cold paths: whole-state replacement, state-transfer
    /// installation, history retention pruning, and one-time lazy backfill of
    /// pre-existing databases.
    fn build_index_rows(_stored_rows: &FederationStateRows) -> Result<FederationStateRows> {
        Ok(FederationStateRows::new())
    }

    /// The rows operations read: every terminal entity is removed, or replaced
    /// by the guard that outlives it. Rows already reduced to a guard pass
    /// through unchanged, so the result does not depend on what was deleted.
    fn retire_terminal_rows(rows: FederationStateRows) -> Result<FederationStateRows> {
        Ok(rows)
    }

    /// The stored rows with at most `limit` terminal entities whose terminal
    /// index is below `floor` removed or reduced to their guard (all of them
    /// when `limit` is `None`).
    ///
    /// The materialized-state digest of a checkpoint covers the rows this
    /// returns for that checkpoint's floor; history retention writes them.
    fn retire_rows_below(
        rows: FederationStateRows,
        _floor: u64,
        _limit: Option<usize>,
    ) -> Result<FederationStateRows> {
        Ok(rows)
    }

    /// State-digest rows: one per stored row, carrying its exact stored bytes,
    /// so the signed digest covers everything a state transfer installs.
    fn materialized_rows(&self) -> Result<Vec<FederationMaterializedStateRow>> {
        self.to_rows()?
            .into_iter()
            .map(|(key, value)| {
                let record =
                    String::from_utf8(value).map_err(|_| FederationError::StateRowsInvalid {
                        reason: "stored row is not UTF-8 JSON",
                    })?;
                Ok(
                    FederationMaterializedStateRow::new(Self::SECONDARY_NAMESPACE, key)
                        .with_field("record", record),
                )
            })
            .collect()
    }
}

/// Synchronize a projection's auxiliary O(1) index namespace with its stored
/// entity rows inside `transaction`.
///
/// Cold path only: used by whole-state replacement, state-transfer
/// installation, history retention pruning, and one-time lazy backfill.
pub async fn sync_federation_state_indexes<S>(
    transaction: &mut DynMintTransaction,
    stored_rows: &FederationStateRows,
) -> std::result::Result<(), Error>
where
    S: FederationStateTable,
{
    sync_federation_state_indexes_kv::<S, _>(&mut **transaction, stored_rows).await
}

pub(crate) async fn sync_federation_state_indexes_kv<S, T>(
    transaction: &mut T,
    stored_rows: &FederationStateRows,
) -> std::result::Result<(), Error>
where
    S: FederationStateTable,
    T: KVStoreTransaction<database::Error> + ?Sized + Send,
{
    let Some(index_namespace) = S::INDEX_SECONDARY_NAMESPACE else {
        return Ok(());
    };
    let desired = S::build_index_rows(stored_rows)?;
    let existing: FederationStateRows = transaction
        .kv_entries(FEDERATION_STATE_PRIMARY_NAMESPACE, index_namespace)
        .await?
        .into_iter()
        .collect();
    for (key, value) in &desired {
        if existing.get(key) != Some(value) {
            transaction
                .kv_write(
                    FEDERATION_STATE_PRIMARY_NAMESPACE,
                    index_namespace,
                    key,
                    value,
                )
                .await?;
        }
    }
    if existing
        .get(FEDERATION_STATE_INDEX_INIT_KEY)
        .map(Vec::as_slice)
        != Some(FEDERATION_STATE_INDEX_INIT_VALUE)
    {
        transaction
            .kv_write(
                FEDERATION_STATE_PRIMARY_NAMESPACE,
                index_namespace,
                FEDERATION_STATE_INDEX_INIT_KEY,
                FEDERATION_STATE_INDEX_INIT_VALUE,
            )
            .await?;
    }
    for key in existing.keys() {
        if key != FEDERATION_STATE_INDEX_INIT_KEY && !desired.contains_key(key) {
            transaction
                .kv_remove(FEDERATION_STATE_PRIMARY_NAMESPACE, index_namespace, key)
                .await?;
        }
    }
    Ok(())
}

/// Ensure a projection's auxiliary index namespace is initialized inside
/// `transaction`.
///
/// On an already-initialized database this performs a single O(1) point read
/// of the sentinel key. On an empty or pre-index database it backfills the
/// index once inside the current transaction so rolled-back transactions never
/// leak index state.
pub(crate) async fn ensure_federation_state_index_initialized_kv<S, T>(
    transaction: &mut T,
) -> std::result::Result<(), Error>
where
    S: FederationStateTable,
    T: KVStoreTransaction<database::Error> + ?Sized + Send,
{
    let Some(index_namespace) = S::INDEX_SECONDARY_NAMESPACE else {
        return Ok(());
    };
    if transaction
        .kv_read(
            FEDERATION_STATE_PRIMARY_NAMESPACE,
            index_namespace,
            FEDERATION_STATE_INDEX_INIT_KEY,
        )
        .await?
        .is_some()
    {
        return Ok(());
    }
    let stored = load_federation_state_rows_kv::<S, _>(transaction).await?;
    sync_federation_state_indexes_kv::<S, _>(transaction, &stored).await
}

/// Return whether a projection's auxiliary index namespace has been initialized.
pub(crate) async fn is_federation_state_index_initialized_kv<S, D>(
    db: &D,
) -> std::result::Result<bool, Error>
where
    S: FederationStateTable,
    D: KVStoreDatabase<Err = database::Error> + ?Sized + Sync,
{
    let Some(index_namespace) = S::INDEX_SECONDARY_NAMESPACE else {
        return Ok(false);
    };
    Ok(db
        .kv_read(
            FEDERATION_STATE_PRIMARY_NAMESPACE,
            index_namespace,
            FEDERATION_STATE_INDEX_INIT_KEY,
        )
        .await?
        .is_some())
}

/// Read and validate a whole projection outside a database transaction, as
/// operations see it (terminal entities retired).
pub async fn read_federation_state<S>(db: &DynMintDatabase) -> std::result::Result<S, Error>
where
    S: FederationStateTable,
{
    read_federation_state_kv(db.as_ref()).await
}

pub(crate) async fn read_federation_state_kv<S, D>(db: &D) -> std::result::Result<S, Error>
where
    S: FederationStateTable,
    D: KVStoreDatabase<Err = database::Error> + ?Sized + Sync,
{
    let rows = read_federation_state_rows_kv::<S, _>(db).await?;
    Ok(S::from_rows(&S::retire_terminal_rows(rows)?)?)
}

/// Read every stored row of a projection, terminal entities included.
pub async fn read_federation_state_rows<S>(
    db: &DynMintDatabase,
) -> std::result::Result<FederationStateRows, Error>
where
    S: FederationStateTable,
{
    read_federation_state_rows_kv::<S, _>(db.as_ref()).await
}

pub(crate) async fn read_federation_state_rows_kv<S, D>(
    db: &D,
) -> std::result::Result<FederationStateRows, Error>
where
    S: FederationStateTable,
    D: KVStoreDatabase<Err = database::Error> + ?Sized + Sync,
{
    Ok(db
        .kv_entries(FEDERATION_STATE_PRIMARY_NAMESPACE, S::SECONDARY_NAMESPACE)
        .await?
        .into_iter()
        .collect())
}

/// Read every stored row of a projection inside a transaction, terminal
/// entities included.
pub async fn load_federation_state_rows<S>(
    transaction: &mut DynMintTransaction,
) -> std::result::Result<FederationStateRows, Error>
where
    S: FederationStateTable,
{
    load_federation_state_rows_kv::<S, _>(&mut **transaction).await
}

pub(crate) async fn load_federation_state_rows_kv<S, T>(
    transaction: &mut T,
) -> std::result::Result<FederationStateRows, Error>
where
    S: FederationStateTable,
    T: KVStoreTransaction<database::Error> + ?Sized + Send,
{
    Ok(transaction
        .kv_entries(FEDERATION_STATE_PRIMARY_NAMESPACE, S::SECONDARY_NAMESPACE)
        .await?
        .into_iter()
        .collect())
}

/// Materialized-state digest rows of a projection's stored rows, for a
/// checkpoint whose retention floor is `floor`.
///
/// Terminal entities below the floor are excluded (or appear as their
/// guard) whether or not this member already deleted them.
pub fn federation_state_rows_at_floor<S>(
    rows: FederationStateRows,
    floor: u64,
) -> Result<Vec<FederationMaterializedStateRow>>
where
    S: FederationStateTable,
{
    S::from_rows(&S::retire_rows_below(rows, floor, None)?)?.materialized_rows()
}

/// Delete at most `limit` terminal entities below `floor` from a stored
/// projection inside `transaction`, writing the guards that replace them.
///
/// Returns how many rows changed.
pub async fn retire_federation_state_below<S>(
    transaction: &mut DynMintTransaction,
    floor: u64,
    limit: usize,
) -> std::result::Result<u64, Error>
where
    S: FederationStateTable,
{
    retire_federation_state_below_kv::<S, _>(&mut **transaction, floor, limit).await
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn retire_federation_state_below_kv<S, T>(
    transaction: &mut T,
    floor: u64,
    limit: usize,
) -> std::result::Result<u64, Error>
where
    S: FederationStateTable,
    T: KVStoreTransaction<database::Error> + ?Sized + Send,
{
    let stored = load_federation_state_rows_kv::<S, _>(transaction).await?;
    let retired = S::retire_rows_below(stored.clone(), floor, Some(limit))?;
    // The retained rows must still form a valid projection.
    S::from_rows(&retired)?;
    let mut changed = 0_u64;
    for (key, value) in &retired {
        if stored.get(key) != Some(value) {
            transaction
                .kv_write(
                    FEDERATION_STATE_PRIMARY_NAMESPACE,
                    S::SECONDARY_NAMESPACE,
                    key,
                    value,
                )
                .await?;
            changed = changed.saturating_add(1);
        }
    }
    for key in stored.keys() {
        if !retired.contains_key(key) {
            transaction
                .kv_remove(
                    FEDERATION_STATE_PRIMARY_NAMESPACE,
                    S::SECONDARY_NAMESPACE,
                    key,
                )
                .await?;
            changed = changed.saturating_add(1);
        }
    }
    if changed > 0 && S::INDEX_SECONDARY_NAMESPACE.is_some() {
        sync_federation_state_indexes_kv::<S, _>(transaction, &retired).await?;
    }
    Ok(changed)
}

/// A projection loaded inside a database transaction, together with the rows
/// it was loaded from so [`Self::save`] writes only what changed.
///
/// Cold-path helper: loads every row of the projection in one query. Hot-path
/// apply and validation routines use the projection's O(1) targeted point
/// operations instead.
#[derive(Debug)]
pub struct FederationStoredState<S> {
    state: S,
    stored: FederationStateRows,
}

impl<S> FederationStoredState<S>
where
    S: FederationStateTable,
{
    /// Read and validate the projection as operations see it.
    ///
    /// Terminal entities are retired first. Their stored rows are left
    /// alone: [`Self::save`] compares against the retired rows, so it neither
    /// deletes a terminal entity nor writes its guard unless an operation
    /// changes them.
    pub async fn load(transaction: &mut DynMintTransaction) -> std::result::Result<Self, Error> {
        let stored = S::retire_terminal_rows(load_federation_state_rows::<S>(transaction).await?)?;
        let state = S::from_rows(&stored)?;
        Ok(Self { state, stored })
    }

    /// Write the changed rows and remove the rows that no longer exist.
    pub async fn save(
        self,
        transaction: &mut DynMintTransaction,
    ) -> std::result::Result<(), Error> {
        let rows = self.state.to_rows()?;
        for (key, value) in &rows {
            if self.stored.get(key) != Some(value) {
                transaction
                    .kv_write(
                        FEDERATION_STATE_PRIMARY_NAMESPACE,
                        S::SECONDARY_NAMESPACE,
                        key,
                        value,
                    )
                    .await?;
            }
        }
        for key in self.stored.keys() {
            if !rows.contains_key(key) {
                transaction
                    .kv_remove(
                        FEDERATION_STATE_PRIMARY_NAMESPACE,
                        S::SECONDARY_NAMESPACE,
                        key,
                    )
                    .await?;
            }
        }
        if S::INDEX_SECONDARY_NAMESPACE.is_some() {
            let stored_rows = load_federation_state_rows::<S>(transaction).await?;
            sync_federation_state_indexes::<S>(transaction, &stored_rows).await?;
        }
        Ok(())
    }

    /// The loaded projection.
    pub fn into_inner(self) -> S {
        self.state
    }
}

impl<S> Deref for FederationStoredState<S> {
    type Target = S;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl<S> DerefMut for FederationStoredState<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

/// Replace a whole stored projection with `state`.
pub async fn write_federation_state<S>(
    transaction: &mut DynMintTransaction,
    state: S,
) -> std::result::Result<(), Error>
where
    S: FederationStateTable,
{
    let mut stored = FederationStoredState::<S>::load(transaction).await?;
    stored.state = state;
    stored.save(transaction).await
}

/// Row key for an entity identified by an outpoint.
#[must_use]
pub fn outpoint_row_key(prefix: &str, outpoint: &OutPoint) -> String {
    format!("{prefix}_{}_{}", outpoint.txid, outpoint.vout)
}

/// Row key for an entity identified by a hexadecimal identifier.
#[must_use]
pub fn hex_row_key(prefix: &str, hex: &str) -> String {
    format!("{prefix}_{hex}")
}

/// Row key for an entity identified by free-form parts, such as a quote id,
/// which may not fit the key alphabet.
#[must_use]
pub fn digest_row_key(prefix: &str, parts: &[&[u8]]) -> String {
    let mut engine = sha256::Hash::engine();
    for part in parts {
        engine.input(&(part.len() as u64).to_be_bytes());
        engine.input(part);
    }
    format!(
        "{prefix}_{}",
        crate::util::hex::encode(sha256::Hash::from_engine(engine).to_byte_array())
    )
}

/// Encode one row as canonical JSON, so every member stores and hashes the
/// same bytes, rejecting a key that is already present.
pub fn insert_row<T>(rows: &mut FederationStateRows, key: String, value: &T) -> Result<()>
where
    T: Serialize,
{
    match rows.insert(key, canonical_json_bytes(value)?) {
        Some(_) => Err(FederationError::StateRowsInvalid {
            reason: "two entities map to the same row key",
        }),
        None => Ok(()),
    }
}

/// Decode one row stored under `key`, checking that `key_of` the decoded
/// value is that key.
pub fn decode_row<T, K>(key: &str, bytes: &[u8], key_of: K) -> Result<T>
where
    T: DeserializeOwned,
    K: FnOnce(&T) -> Result<String>,
{
    let value = serde_json::from_slice(bytes)?;
    if key_of(&value)? != key {
        return Err(FederationError::StateRowsInvalid {
            reason: "row key does not match its content",
        });
    }
    Ok(value)
}

/// The kind prefix of a row key.
pub fn row_kind(key: &str) -> Result<&str> {
    key.split_once('_')
        .map(|(kind, _)| kind)
        .ok_or(FederationError::StateRowsInvalid {
            reason: "row key has no kind prefix",
        })
}

/// Error for a row whose kind the projection does not store.
#[must_use]
pub fn unknown_row_kind() -> FederationError {
    FederationError::StateRowsInvalid {
        reason: "row key has an unknown kind",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::btree_map::Entry;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use bitcoin::absolute::LockTime;
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
    use bitcoin::sighash::{Prevouts, SighashCache, TapSighashType};
    use bitcoin::transaction::Version;
    use bitcoin::{
        Amount, BlockHash, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
        Witness,
    };

    use super::*;
    use crate::database::mint::{validate_kvstore_params, KVStoreDatabase, KVStoreTransaction};
    use crate::database::DbTransactionFinalizer;
    use crate::federation::address_allocation::{
        apply_federation_address_allocation_kv, read_federation_address_allocation_kv,
        FederationAddressAllocation, FederationAddressAllocationOperation,
        FederationAddressAllocationOutcome, FederationAddressAllocationScope,
        FederationAddressAllocationState, FederationAddressKeychain,
        FEDERATION_ADDRESS_ALLOCATION_INDEX_NAMESPACE,
    };
    use crate::federation::chain_observation::{
        apply_federation_deposit_observation_kv, apply_federation_reorg_observation_kv,
        load_federation_quote_payment_at_or_before_kv,
        mark_federation_quote_liability_issued_at_or_before_kv,
        read_federation_quote_payment_at_or_before_kv, reserve_federation_chain_utxos_kv,
        spend_reserved_federation_chain_utxos_kv, FederationAllocatedAddressRef,
        FederationChainObservationState, FederationChainSourceDescriptor,
        FederationChainSourceScope, FederationDepositConfirmation, FederationDepositObservation,
        FederationDepositReorgObservation, FEDERATION_CHAIN_OBSERVATION_INDEX_NAMESPACE,
    };
    use crate::federation::transaction_proposal::{
        add_pending_federation_onchain_intent_kv, authorize_federation_transaction_broadcast_kv,
        load_federation_accepted_transaction_kv, load_federation_onchain_intent_kv,
        load_federation_transaction_lineage_for_quote_settlement_kv,
        load_federation_utxo_reservation_kv, mark_federation_transaction_signed_kv,
        read_federation_accepted_transaction_kv, read_federation_completed_payment_for_quote_kv,
        record_federation_transaction_broadcast_observation_kv, FederationFrostInputSignatureAudit,
        FederationOnchainPaymentBatch, FederationOnchainPaymentIntent,
        FederationOnchainPaymentIntentRecord, FederationSignedTransaction,
        FederationTransactionBroadcastIntent, FederationTransactionBroadcastObservation,
        FederationTransactionBroadcastResult, FederationTransactionChangeOutput,
        FederationTransactionInput, FederationTransactionPolicyAudit,
        FederationTransactionProposal, FederationTransactionRecipientOutput,
        FederationTransactionScope, FederationTransactionSettlementPolicy,
        FederationTransactionState, FederationTransactionStateOutcome,
        LoadedFederationTransactionLineage, FEDERATION_TRANSACTION_INDEX_NAMESPACE,
    };
    use crate::federation::{FederationFrostApplicationId, FederationOperationId, MemberId};

    type KvKey = (String, String, String);
    type KvMap = BTreeMap<KvKey, Vec<u8>>;

    #[derive(Debug, Default)]
    struct KvCounters {
        reads: AtomicUsize,
        writes: AtomicUsize,
        removes: AtomicUsize,
        lists: AtomicUsize,
        entries: AtomicUsize,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct KvCountSnapshot {
        reads: usize,
        writes: usize,
        removes: usize,
        lists: usize,
        entries: usize,
    }

    impl KvCounters {
        fn reset(&self) {
            self.reads.store(0, Ordering::SeqCst);
            self.writes.store(0, Ordering::SeqCst);
            self.removes.store(0, Ordering::SeqCst);
            self.lists.store(0, Ordering::SeqCst);
            self.entries.store(0, Ordering::SeqCst);
        }

        fn snapshot(&self) -> KvCountSnapshot {
            KvCountSnapshot {
                reads: self.reads.load(Ordering::SeqCst),
                writes: self.writes.load(Ordering::SeqCst),
                removes: self.removes.load(Ordering::SeqCst),
                lists: self.lists.load(Ordering::SeqCst),
                entries: self.entries.load(Ordering::SeqCst),
            }
        }
    }

    #[derive(Clone, Default)]
    struct CountingKvStore {
        data: Arc<Mutex<KvMap>>,
        counters: Arc<KvCounters>,
    }

    impl CountingKvStore {
        fn new() -> Self {
            Self::default()
        }

        fn begin_tx(&self) -> Box<CountingKvTx> {
            let staged = self.data.lock().expect("lock store").clone();
            Box::new(CountingKvTx {
                store: self.data.clone(),
                staged,
                counters: self.counters.clone(),
            })
        }

        fn reset_counts(&self) {
            self.counters.reset();
        }

        fn counts(&self) -> KvCountSnapshot {
            self.counters.snapshot()
        }

        fn snapshot_all(&self) -> KvMap {
            self.data.lock().expect("lock store").clone()
        }

        fn namespace_rows(&self, primary: &str, secondary: &str) -> FederationStateRows {
            self.data
                .lock()
                .expect("lock store")
                .iter()
                .filter(|((p, s, _), _)| p == primary && s == secondary)
                .map(|((_, _, k), v)| (k.clone(), v.clone()))
                .collect()
        }

        fn seed_raw_rows(&self, primary: &str, secondary: &str, rows: &FederationStateRows) {
            let mut map = self.data.lock().expect("lock store");
            for (key, value) in rows {
                validate_kvstore_params(primary, secondary, Some(key)).expect("valid kv key");
                map.insert(
                    (primary.to_string(), secondary.to_string(), key.clone()),
                    value.clone(),
                );
            }
        }
    }

    struct CountingKvTx {
        store: Arc<Mutex<KvMap>>,
        staged: KvMap,
        counters: Arc<KvCounters>,
    }

    #[async_trait]
    impl DbTransactionFinalizer for CountingKvTx {
        type Err = database::Error;

        async fn commit(self: Box<Self>) -> std::result::Result<(), Self::Err> {
            *self.store.lock().expect("lock store") = self.staged;
            Ok(())
        }

        async fn rollback(self: Box<Self>) -> std::result::Result<(), Self::Err> {
            Ok(())
        }
    }

    #[async_trait]
    impl KVStoreTransaction<database::Error> for CountingKvTx {
        async fn kv_read(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
            key: &str,
        ) -> std::result::Result<Option<Vec<u8>>, database::Error> {
            validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;
            self.counters.reads.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .staged
                .get(&(
                    primary_namespace.to_string(),
                    secondary_namespace.to_string(),
                    key.to_string(),
                ))
                .cloned())
        }

        async fn kv_write(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
            key: &str,
            value: &[u8],
        ) -> std::result::Result<(), database::Error> {
            validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;
            self.counters.writes.fetch_add(1, Ordering::SeqCst);
            self.staged.insert(
                (
                    primary_namespace.to_string(),
                    secondary_namespace.to_string(),
                    key.to_string(),
                ),
                value.to_vec(),
            );
            Ok(())
        }

        async fn kv_write_if_absent(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
            key: &str,
            value: &[u8],
        ) -> std::result::Result<bool, database::Error> {
            validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;
            self.counters.writes.fetch_add(1, Ordering::SeqCst);
            let k = (
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
                key.to_string(),
            );
            match self.staged.entry(k) {
                Entry::Vacant(entry) => {
                    entry.insert(value.to_vec());
                    Ok(true)
                }
                Entry::Occupied(_) => Ok(false),
            }
        }

        async fn kv_remove(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
            key: &str,
        ) -> std::result::Result<(), database::Error> {
            validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;
            self.counters.removes.fetch_add(1, Ordering::SeqCst);
            self.staged.remove(&(
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
                key.to_string(),
            ));
            Ok(())
        }

        async fn kv_write_if_equals(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
            key: &str,
            expected: &[u8],
            replacement: &[u8],
        ) -> std::result::Result<bool, database::Error> {
            validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;
            self.counters.writes.fetch_add(1, Ordering::SeqCst);
            let k = (
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
                key.to_string(),
            );
            if self.staged.get(&k).map(Vec::as_slice) == Some(expected) {
                self.staged.insert(k, replacement.to_vec());
                Ok(true)
            } else {
                Ok(false)
            }
        }

        async fn kv_list(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
        ) -> std::result::Result<Vec<String>, database::Error> {
            validate_kvstore_params(primary_namespace, secondary_namespace, None)?;
            self.counters.lists.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .staged
                .keys()
                .filter(|(p, s, _)| p == primary_namespace && s == secondary_namespace)
                .map(|(_, _, k)| k.clone())
                .collect())
        }

        async fn kv_entries(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
        ) -> std::result::Result<Vec<(String, Vec<u8>)>, database::Error> {
            validate_kvstore_params(primary_namespace, secondary_namespace, None)?;
            self.counters.entries.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .staged
                .iter()
                .filter(|((p, s, _), _)| p == primary_namespace && s == secondary_namespace)
                .map(|((_, _, k), v)| (k.clone(), v.clone()))
                .collect())
        }
    }

    #[async_trait]
    impl KVStoreDatabase for CountingKvStore {
        type Err = database::Error;

        async fn kv_read(
            &self,
            primary_namespace: &str,
            secondary_namespace: &str,
            key: &str,
        ) -> std::result::Result<Option<Vec<u8>>, Self::Err> {
            validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;
            self.counters.reads.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .data
                .lock()
                .expect("lock store")
                .get(&(
                    primary_namespace.to_string(),
                    secondary_namespace.to_string(),
                    key.to_string(),
                ))
                .cloned())
        }

        async fn kv_list(
            &self,
            primary_namespace: &str,
            secondary_namespace: &str,
        ) -> std::result::Result<Vec<String>, Self::Err> {
            validate_kvstore_params(primary_namespace, secondary_namespace, None)?;
            self.counters.lists.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .data
                .lock()
                .expect("lock store")
                .keys()
                .filter(|(p, s, _)| p == primary_namespace && s == secondary_namespace)
                .map(|(_, _, k)| k.clone())
                .collect())
        }

        async fn kv_entries(
            &self,
            primary_namespace: &str,
            secondary_namespace: &str,
        ) -> std::result::Result<Vec<(String, Vec<u8>)>, Self::Err> {
            validate_kvstore_params(primary_namespace, secondary_namespace, None)?;
            self.counters.entries.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .data
                .lock()
                .expect("lock store")
                .iter()
                .filter(|((p, s, _), _)| p == primary_namespace && s == secondary_namespace)
                .map(|((_, _, k), v)| (k.clone(), v.clone()))
                .collect())
        }
    }

    fn op_id(index: u64) -> FederationOperationId {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&index.to_be_bytes());
        bytes[31] = 1;
        FederationOperationId::from_bytes(bytes)
    }

    fn outpoint_for(index: u64) -> OutPoint {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&index.to_be_bytes());
        bytes[31] = 9;
        OutPoint::new(Txid::from_byte_array(bytes), 0)
    }

    fn app_id(seed: u8) -> FederationFrostApplicationId {
        FederationFrostApplicationId::from_bytes([seed; 32]).expect("application id")
    }

    fn member(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    #[tokio::test]
    async fn address_allocation_incremental_is_o1_and_matches_full_state() {
        let store = CountingKvStore::new();
        let mut reference = FederationAddressAllocationState::new();
        let application = app_id(7);
        let scope = FederationAddressAllocationScope {
            application_id: application,
            application_key_epoch: 3,
        };

        let apply_one = |store: &CountingKvStore,
                         reference: &mut FederationAddressAllocationState,
                         idx: u64,
                         keychain: FederationAddressKeychain| {
            let store = store.clone();
            let op = FederationAddressAllocationOperation::new(
                format!("quote-{idx}"),
                application,
                3,
                keychain,
            )
            .expect("allocation op");
            let expected = reference.apply(&op, op_id(idx), idx).expect("ref apply");
            async move {
                let mut tx = store.begin_tx();
                let actual =
                    apply_federation_address_allocation_kv(tx.as_mut(), &op, op_id(idx), idx)
                        .await
                        .expect("inc apply");
                tx.commit().await.expect("commit");
                assert_eq!(actual, expected);
            }
        };

        for idx in 1..=5_u64 {
            apply_one(
                &store,
                &mut reference,
                idx,
                FederationAddressKeychain::External,
            )
            .await;
        }

        // Measure O(1) counts at N = 5
        store.reset_counts();
        apply_one(
            &store,
            &mut reference,
            6,
            FederationAddressKeychain::External,
        )
        .await;
        let small_new = store.counts();

        store.reset_counts();
        let dup_op = FederationAddressAllocationOperation::new(
            "quote-6",
            application,
            3,
            FederationAddressKeychain::External,
        )
        .expect("dup op");
        let mut tx = store.begin_tx();
        let dup = apply_federation_address_allocation_kv(tx.as_mut(), &dup_op, op_id(999), 999)
            .await
            .expect("dup apply");
        tx.commit().await.expect("commit");
        assert!(matches!(
            dup,
            FederationAddressAllocationOutcome::Existing(_)
        ));
        let small_dup = store.counts();

        store.reset_counts();
        let found = read_federation_address_allocation_kv(&store, scope, "quote-6")
            .await
            .expect("read allocation");
        assert_eq!(found.as_ref(), reference.allocation(scope, "quote-6"));
        let small_read = store.counts();

        // Grow history to N = 200 across both keychains
        for idx in 7..=200_u64 {
            let kc = if idx % 3 == 0 {
                FederationAddressKeychain::Internal
            } else {
                FederationAddressKeychain::External
            };
            apply_one(&store, &mut reference, idx, kc).await;
        }

        // Measure O(1) counts at N = 200
        store.reset_counts();
        apply_one(
            &store,
            &mut reference,
            201,
            FederationAddressKeychain::External,
        )
        .await;
        let large_new = store.counts();

        store.reset_counts();
        let dup_op_large = FederationAddressAllocationOperation::new(
            "quote-201",
            application,
            3,
            FederationAddressKeychain::External,
        )
        .expect("dup op");
        let mut tx = store.begin_tx();
        let dup_large =
            apply_federation_address_allocation_kv(tx.as_mut(), &dup_op_large, op_id(1000), 1000)
                .await
                .expect("dup apply");
        tx.commit().await.expect("commit");
        assert!(matches!(
            dup_large,
            FederationAddressAllocationOutcome::Existing(_)
        ));
        let large_dup = store.counts();

        store.reset_counts();
        let found_large = read_federation_address_allocation_kv(&store, scope, "quote-201")
            .await
            .expect("read allocation");
        assert_eq!(
            found_large.as_ref(),
            reference.allocation(scope, "quote-201")
        );
        let large_read = store.counts();

        assert_eq!(small_new.entries, 0);
        assert_eq!(small_new.lists, 0);
        assert_eq!(large_new, small_new);
        assert_eq!(small_dup.entries, 0);
        assert_eq!(small_dup.writes, 0);
        assert_eq!(large_dup, small_dup);
        assert_eq!(small_read.entries, 0);
        assert_eq!(small_read.reads, 1);
        assert_eq!(large_read, small_read);

        // Byte-for-byte materialized-row and digest equivalence
        let stored_rows = store.namespace_rows(
            FEDERATION_STATE_PRIMARY_NAMESPACE,
            FederationAddressAllocationState::SECONDARY_NAMESPACE,
        );
        assert_eq!(stored_rows, reference.to_rows().expect("ref rows"));
        assert_eq!(
            FederationAddressAllocationState::from_rows(&stored_rows).expect("from_rows"),
            reference
        );
    }

    #[tokio::test]
    async fn chain_observation_incremental_is_o1_and_matches_full_state() {
        let store = CountingKvStore::new();
        let mut reference = FederationChainObservationState::new();
        let application = app_id(11);
        let script = ScriptBuf::from_bytes(vec![0x51, 0x20, 1, 2, 3, 4]);

        let make_deposit = |idx: u64, observer_id: u16, depth: u32| {
            let quote = format!("chain-quote-{idx}");
            let alloc = FederationAddressAllocation {
                quote: quote.clone(),
                keychain: FederationAddressKeychain::External,
                derivation_index: idx as u32,
                operation_id: op_id(idx),
                operation_index: idx,
            };
            let obs = FederationDepositObservation {
                allocation: FederationAllocatedAddressRef {
                    application_id: application,
                    application_key_epoch: 3,
                    quote,
                    keychain: FederationAddressKeychain::External,
                    derivation_index: idx as u32,
                },
                outpoint: outpoint_for(idx),
                txout: TxOut {
                    value: Amount::from_sat(10_000 + idx),
                    script_pubkey: script.clone(),
                },
                confirmation: Some(FederationDepositConfirmation {
                    block_hash: BlockHash::from_byte_array([42; 32]),
                    block_height: 100,
                    confirmation_depth: depth,
                }),
                observation_epoch: 0,
                observer: member(observer_id),
                source: FederationChainSourceDescriptor::new(
                    FederationChainSourceScope::Independent,
                    format!("src-{observer_id}"),
                )
                .expect("source"),
            };
            (alloc, obs)
        };

        let seed_spendable =
            |store: CountingKvStore, reference: &mut FederationChainObservationState, idx: u64| {
                let (alloc, obs1) = make_deposit(idx, 1, 2);
                let (_, obs2) = make_deposit(idx, 2, 2);
                let script = script.clone();
                let o1 = reference
                    .apply_deposit(
                        &obs1,
                        &alloc,
                        script.as_script(),
                        2,
                        2,
                        op_id(idx * 10),
                        idx * 10,
                    )
                    .expect("ref obs1");
                let o2 = reference
                    .apply_deposit(
                        &obs2,
                        &alloc,
                        script.as_script(),
                        2,
                        2,
                        op_id(idx * 10 + 1),
                        idx * 10 + 1,
                    )
                    .expect("ref obs2");
                async move {
                    let mut tx = store.begin_tx();
                    let a1 = apply_federation_deposit_observation_kv(
                        tx.as_mut(),
                        &obs1,
                        &alloc,
                        script.as_script(),
                        2,
                        2,
                        op_id(idx * 10),
                        idx * 10,
                    )
                    .await
                    .expect("inc obs1");
                    tx.commit().await.expect("commit");
                    assert_eq!(a1, o1);

                    let mut tx = store.begin_tx();
                    let a2 = apply_federation_deposit_observation_kv(
                        tx.as_mut(),
                        &obs2,
                        &alloc,
                        script.as_script(),
                        2,
                        2,
                        op_id(idx * 10 + 1),
                        idx * 10 + 1,
                    )
                    .await
                    .expect("inc obs2");
                    tx.commit().await.expect("commit");
                    assert_eq!(a2, o2);
                }
            };

        for idx in 1..=5_u64 {
            seed_spendable(store.clone(), &mut reference, idx).await;
        }

        // Measure deposit, quote payment, liability, reserve, spend, reorg at N = 5
        store.reset_counts();
        seed_spendable(store.clone(), &mut reference, 6).await;
        let small_deposit = store.counts();

        store.reset_counts();
        let payment_small =
            read_federation_quote_payment_at_or_before_kv(&store, "chain-quote-6", u64::MAX)
                .await
                .expect("quote payment")
                .expect("some payment");
        let small_quote_read = store.counts();
        assert_eq!(
            Some(payment_small.clone()),
            reference
                .quote_payment_at_or_before("chain-quote-6", u64::MAX)
                .expect("ref payment")
        );

        store.reset_counts();
        let mut tx = store.begin_tx();
        let marked_small = mark_federation_quote_liability_issued_at_or_before_kv(
            tx.as_mut(),
            "chain-quote-6",
            u64::MAX,
            payment_small.amount_sat,
            payment_small.payment_id,
        )
        .await
        .expect("mark liability");
        tx.commit().await.expect("commit");
        let small_liability = store.counts();
        let ref_marked = reference
            .mark_quote_liability_issued_at_or_before(
                "chain-quote-6",
                u64::MAX,
                payment_small.amount_sat,
                payment_small.payment_id,
            )
            .expect("ref mark");
        assert_eq!(marked_small, ref_marked);

        store.reset_counts();
        let mut tx = store.begin_tx();
        reserve_federation_chain_utxos_kv(tx.as_mut(), &[outpoint_for(6)])
            .await
            .expect("reserve");
        tx.commit().await.expect("commit");
        let small_reserve = store.counts();
        reference
            .reserve_utxos(&[outpoint_for(6)])
            .expect("ref reserve");

        store.reset_counts();
        let mut tx = store.begin_tx();
        spend_reserved_federation_chain_utxos_kv(tx.as_mut(), &[outpoint_for(6)], 5000)
            .await
            .expect("spend");
        tx.commit().await.expect("commit");
        let small_spend = store.counts();
        reference
            .spend_reserved_utxos(&[outpoint_for(6)], 5000)
            .expect("ref spend");

        // Reorg on UTXO 5
        let reorg_small = FederationDepositReorgObservation {
            outpoint: outpoint_for(5),
            disconnected_block_hash: BlockHash::from_byte_array([42; 32]),
            disconnected_block_height: 100,
            observation_epoch: 0,
            observer: member(1),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Independent,
                "src-1",
            )
            .expect("source"),
        };
        store.reset_counts();
        let mut tx = store.begin_tx();
        let r_actual =
            apply_federation_reorg_observation_kv(tx.as_mut(), &reorg_small, op_id(5001), 5001)
                .await
                .expect("reorg");
        tx.commit().await.expect("commit");
        let small_reorg = store.counts();
        let r_expected = reference
            .apply_reorg(&reorg_small, op_id(5001), 5001)
            .expect("ref reorg");
        assert_eq!(r_actual, r_expected);

        // Grow history to N = 200 UTXOs
        for idx in 7..=200_u64 {
            seed_spendable(store.clone(), &mut reference, idx).await;
        }

        // Measure the exact same operations at N = 200
        store.reset_counts();
        seed_spendable(store.clone(), &mut reference, 201).await;
        let large_deposit = store.counts();

        store.reset_counts();
        let payment_large =
            read_federation_quote_payment_at_or_before_kv(&store, "chain-quote-201", u64::MAX)
                .await
                .expect("quote payment")
                .expect("some payment");
        let large_quote_read = store.counts();
        assert_eq!(
            Some(payment_large.clone()),
            reference
                .quote_payment_at_or_before("chain-quote-201", u64::MAX)
                .expect("ref payment")
        );

        store.reset_counts();
        let mut tx = store.begin_tx();
        mark_federation_quote_liability_issued_at_or_before_kv(
            tx.as_mut(),
            "chain-quote-201",
            u64::MAX,
            payment_large.amount_sat,
            payment_large.payment_id,
        )
        .await
        .expect("mark liability");
        tx.commit().await.expect("commit");
        let large_liability = store.counts();
        reference
            .mark_quote_liability_issued_at_or_before(
                "chain-quote-201",
                u64::MAX,
                payment_large.amount_sat,
                payment_large.payment_id,
            )
            .expect("ref mark");

        store.reset_counts();
        let mut tx = store.begin_tx();
        reserve_federation_chain_utxos_kv(tx.as_mut(), &[outpoint_for(201)])
            .await
            .expect("reserve");
        tx.commit().await.expect("commit");
        let large_reserve = store.counts();
        reference
            .reserve_utxos(&[outpoint_for(201)])
            .expect("ref reserve");

        store.reset_counts();
        let mut tx = store.begin_tx();
        spend_reserved_federation_chain_utxos_kv(tx.as_mut(), &[outpoint_for(201)], 9000)
            .await
            .expect("spend");
        tx.commit().await.expect("commit");
        let large_spend = store.counts();
        reference
            .spend_reserved_utxos(&[outpoint_for(201)], 9000)
            .expect("ref spend");

        let reorg_large = FederationDepositReorgObservation {
            outpoint: outpoint_for(200),
            disconnected_block_hash: BlockHash::from_byte_array([42; 32]),
            disconnected_block_height: 100,
            observation_epoch: 0,
            observer: member(1),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Independent,
                "src-1",
            )
            .expect("source"),
        };
        store.reset_counts();
        let mut tx = store.begin_tx();
        let r_actual_large =
            apply_federation_reorg_observation_kv(tx.as_mut(), &reorg_large, op_id(9001), 9001)
                .await
                .expect("reorg");
        tx.commit().await.expect("commit");
        let large_reorg = store.counts();
        let r_expected_large = reference
            .apply_reorg(&reorg_large, op_id(9001), 9001)
            .expect("ref reorg");
        assert_eq!(r_actual_large, r_expected_large);

        // Verify strictly O(1) independent of N
        for (small, large) in [
            (small_deposit, large_deposit),
            (small_quote_read, large_quote_read),
            (small_liability, large_liability),
            (small_reserve, large_reserve),
            (small_spend, large_spend),
            (small_reorg, large_reorg),
        ] {
            assert_eq!(small.entries, 0);
            assert_eq!(small.lists, 0);
            assert_eq!(large, small);
        }

        // Spent UTXO quote payment remains readable via tombstone both before & after retention
        let spent_payment =
            read_federation_quote_payment_at_or_before_kv(&store, "chain-quote-6", u64::MAX)
                .await
                .expect("spent quote payment");
        assert_eq!(spent_payment, Some(payment_small.clone()));

        // Byte-for-byte materialized-row parity
        let stored_rows = store.namespace_rows(
            FEDERATION_STATE_PRIMARY_NAMESPACE,
            FederationChainObservationState::SECONDARY_NAMESPACE,
        );
        assert_eq!(stored_rows, reference.to_rows().expect("ref rows"));
        assert_eq!(
            federation_state_rows_at_floor::<FederationChainObservationState>(
                stored_rows.clone(),
                6000
            )
            .expect("inc floor"),
            federation_state_rows_at_floor::<FederationChainObservationState>(
                reference.to_rows().expect("ref rows"),
                6000
            )
            .expect("ref floor")
        );

        // Retention prunes UTXO 6 (spent at 5000 < 6000) to a spent tombstone and keeps quote index intact
        let mut tx = store.begin_tx();
        let pruned = retire_federation_state_below_kv::<FederationChainObservationState, _>(
            tx.as_mut(),
            6000,
            10,
        )
        .await
        .expect("retire below");
        tx.commit().await.expect("commit");
        assert!(pruned > 0);
        assert_eq!(
            read_federation_quote_payment_at_or_before_kv(&store, "chain-quote-6", u64::MAX)
                .await
                .expect("tombstone quote payment"),
            Some(payment_small)
        );
    }

    fn build_batch_lifecycle_fixture(
        idx: u64,
        application: FederationFrostApplicationId,
    ) -> (
        FederationOnchainPaymentIntentRecord,
        FederationTransactionProposal,
        FederationTransactionPolicyAudit,
        FederationSignedTransaction,
        FederationTransactionBroadcastIntent,
        FederationTransactionBroadcastObservation,
        FederationTransactionBroadcastObservation,
    ) {
        let secp = Secp256k1::new();
        let secret = SecretKey::from_slice(&[3; 32]).expect("secret key");
        let keypair = Keypair::from_secret_key(&secp, &secret);
        let (output_key, _) = XOnlyPublicKey::from_keypair(&keypair);
        let mut input_script = vec![0x51, 0x20];
        input_script.extend_from_slice(&output_key.serialize());
        let input_script = ScriptBuf::from_bytes(input_script);
        let scope = FederationTransactionScope {
            root_key_epoch: 1,
            application_id: application,
            application_key_epoch: 3,
            network: Network::Regtest,
            policy_version: 1,
        };
        let intent = FederationOnchainPaymentIntent::new(
            format!("melt-quote-{idx}"),
            op_id(idx * 100),
            ScriptBuf::from_bytes(vec![0x51]),
            10_000,
            2_000,
            0,
            1,
            idx,
            scope,
        )
        .expect("intent");
        let record = FederationOnchainPaymentIntentRecord::pending(intent.clone()).expect("record");
        let batch = FederationOnchainPaymentBatch::new(vec![intent.clone()], idx).expect("batch");
        let outpoint = outpoint_for(idx);
        let fee_sat = 500_u64;
        let change_sat = 9_500_u64;
        let previous_output = TxOut {
            value: Amount::from_sat(10_000 + fee_sat + change_sat),
            script_pubkey: input_script.clone(),
        };
        let selected_input = FederationTransactionInput {
            outpoint,
            previous_output: previous_output.clone(),
            allocation: FederationAllocatedAddressRef {
                application_id: application,
                application_key_epoch: 3,
                quote: format!("deposit-{idx}"),
                keychain: FederationAddressKeychain::External,
                derivation_index: idx as u32,
            },
        };
        let recipient = TxOut {
            value: Amount::from_sat(intent.amount_sat),
            script_pubkey: intent.destination_script.clone(),
        };
        let change = TxOut {
            value: Amount::from_sat(change_sat),
            script_pubkey: input_script,
        };
        let unsigned_transaction = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![recipient.clone(), change.clone()],
        };
        let proposal = FederationTransactionProposal::new(
            &batch,
            member(1),
            unsigned_transaction,
            vec![selected_input],
            vec![FederationTransactionRecipientOutput {
                intent_id: record.intent_id,
                output_index: 0,
                output: recipient,
                fee_contribution_sat: fee_sat,
            }],
            Some(FederationTransactionChangeOutput {
                derivation_index: idx as u32,
                output_index: 1,
                output: change,
            }),
            fee_sat,
            None,
        )
        .expect("proposal");
        let sighash = SighashCache::new(&proposal.unsigned_transaction)
            .taproot_key_spend_signature_hash(
                0,
                &Prevouts::All(std::slice::from_ref(&previous_output)),
                TapSighashType::Default,
            )
            .expect("sighash")
            .to_byte_array();
        let proposal_id = proposal.proposal_id().expect("proposal id");
        let policy_audit = FederationTransactionPolicyAudit {
            proposal_id,
            txid: proposal.unsigned_transaction.compute_txid(),
            fee_sat: proposal.fee_sat,
            fee_rate_sat_per_kwu: proposal.fee_rate_sat_per_kwu,
            weight: proposal.estimated_signed_weight(),
            fee_contributions: BTreeMap::from([(record.intent_id, fee_sat)]),
            taproot_sighashes: vec![sighash],
        };
        let mut temp_state = FederationTransactionState::new();
        temp_state
            .add_pending_intent(record.clone())
            .expect("temp intent");
        temp_state
            .accept_transaction(
                proposal.clone(),
                policy_audit.clone(),
                op_id(idx * 100 + 1),
                idx * 100 + 1,
            )
            .expect("temp accept");
        let accepted = temp_state
            .accepted_transaction(proposal_id)
            .expect("accepted")
            .clone();
        let signature = secp
            .sign_schnorr_no_aux_rand(&Message::from_digest(sighash), &keypair)
            .serialize()
            .to_vec();
        let mut signed_tx = proposal.unsigned_transaction.clone();
        signed_tx.input[0].witness.push(&signature);
        let signed = FederationSignedTransaction::new(
            &accepted,
            &signed_tx,
            vec![FederationFrostInputSignatureAudit {
                input_index: 0,
                session_id: [11; 32],
                attempt: 1,
                signer_set: vec![1, 2],
                package_hash: [12; 32],
                signature,
            }],
        )
        .expect("signed");
        let broadcast_intent = FederationTransactionBroadcastIntent {
            proposal_id,
            txid: signed.txid,
            settlement_policy: FederationTransactionSettlementPolicy::new(2, 1)
                .expect("settlement policy"),
        };
        let obs_mempool = FederationTransactionBroadcastObservation {
            proposal_id,
            txid: signed.txid,
            observer: member(1),
            attempt: 1,
            result: FederationTransactionBroadcastResult::Accepted,
            detail: None,
        };
        let obs_confirm = FederationTransactionBroadcastObservation {
            proposal_id,
            txid: signed.txid,
            observer: member(1),
            attempt: 2,
            result: FederationTransactionBroadcastResult::Confirmed {
                block_hash: BlockHash::from_byte_array([77; 32]),
                block_height: 300,
                confirmation_depth: 2,
            },
            detail: None,
        };
        (
            record,
            proposal,
            policy_audit,
            signed,
            broadcast_intent,
            obs_mempool,
            obs_confirm,
        )
    }

    #[tokio::test]
    async fn transaction_incremental_is_o1_and_matches_full_state_including_retirement() {
        let store = CountingKvStore::new();
        let mut reference = FederationTransactionState::new();
        let application = app_id(15);

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        struct LifecycleCounts {
            intent: KvCountSnapshot,
            proposal: KvCountSnapshot,
            signed: KvCountSnapshot,
            broadcast_intent: KvCountSnapshot,
            obs_mempool: KvCountSnapshot,
            obs_confirm: KvCountSnapshot,
            completed_read: KvCountSnapshot,
            settle_and_retire: KvCountSnapshot,
        }

        let run_lifecycle = |store: CountingKvStore,
                             reference: &mut FederationTransactionState,
                             idx: u64,
                             retire: bool| {
            let (record, proposal, audit, signed, broadcast_intent, obs_mempool, obs_confirm) =
                build_batch_lifecycle_fixture(idx, application);
            let proposal_id = proposal.proposal_id().expect("proposal id");
            let base = idx * 100;
            reference
                .add_pending_intent(record.clone())
                .expect("ref intent");
            reference
                .accept_transaction(proposal.clone(), audit.clone(), op_id(base + 1), base + 1)
                .expect("ref accept");
            reference
                .mark_transaction_signed(proposal_id, signed.clone())
                .expect("ref sign");
            reference
                .authorize_transaction_broadcast(broadcast_intent, op_id(base + 2), base + 2)
                .expect("ref broadcast intent");
            reference
                .record_transaction_broadcast_observation(
                    obs_mempool.clone(),
                    op_id(base + 3),
                    base + 3,
                )
                .expect("ref obs mempool");
            reference
                .record_transaction_broadcast_observation(
                    obs_confirm.clone(),
                    op_id(base + 4),
                    base + 4,
                )
                .expect("ref obs confirm");
            let expected_receipt = reference
                .completed_payment_for_quote(&record.intent.quote)
                .expect("ref receipt");
            if retire {
                let (batch_id, _) = reference
                    .settle_completed_intent(&record.intent.quote, base + 5)
                    .expect("ref settle")
                    .expect("lineage settled");
                reference
                    .retire_settled_lineage(batch_id, base + 5)
                    .expect("ref retire");
            }
            async move {
                store.reset_counts();
                let mut tx = store.begin_tx();
                assert_eq!(
                    add_pending_federation_onchain_intent_kv(tx.as_mut(), record.clone())
                        .await
                        .expect("inc intent"),
                    FederationTransactionStateOutcome::Updated
                );
                tx.commit().await.expect("commit");
                let intent_counts = store.counts();

                store.reset_counts();
                let mut tx = store.begin_tx();
                let mut lineage =
                    LoadedFederationTransactionLineage::load_kv(tx.as_mut(), proposal.batch_id)
                        .await
                        .expect("load lineage");
                let loaded_intent =
                    load_federation_onchain_intent_kv(tx.as_mut(), record.intent_id)
                        .await
                        .expect("load intent")
                        .expect("some intent");
                lineage
                    .insert_pending_intents(BTreeMap::from([(record.intent_id, loaded_intent)]))
                    .expect("stage pending");
                assert_eq!(
                    lineage
                        .accept_transaction(proposal.clone(), audit, op_id(base + 1), base + 1)
                        .expect("accept"),
                    FederationTransactionStateOutcome::Updated
                );
                lineage.save_kv(tx.as_mut()).await.expect("save lineage");
                tx.commit().await.expect("commit");
                let proposal_counts = store.counts();

                store.reset_counts();
                let mut tx = store.begin_tx();
                assert_eq!(
                    mark_federation_transaction_signed_kv(tx.as_mut(), proposal_id, signed)
                        .await
                        .expect("sign"),
                    FederationTransactionStateOutcome::Updated
                );
                tx.commit().await.expect("commit");
                let signed_counts = store.counts();

                store.reset_counts();
                let mut tx = store.begin_tx();
                assert_eq!(
                    authorize_federation_transaction_broadcast_kv(
                        tx.as_mut(),
                        broadcast_intent,
                        op_id(base + 2),
                        base + 2,
                    )
                    .await
                    .expect("broadcast intent"),
                    FederationTransactionStateOutcome::Updated
                );
                tx.commit().await.expect("commit");
                let broadcast_intent_counts = store.counts();

                store.reset_counts();
                let mut tx = store.begin_tx();
                assert_eq!(
                    record_federation_transaction_broadcast_observation_kv(
                        tx.as_mut(),
                        obs_mempool,
                        op_id(base + 3),
                        base + 3,
                    )
                    .await
                    .expect("obs mempool"),
                    FederationTransactionStateOutcome::Updated
                );
                tx.commit().await.expect("commit");
                let obs_mempool_counts = store.counts();

                store.reset_counts();
                let mut tx = store.begin_tx();
                assert_eq!(
                    record_federation_transaction_broadcast_observation_kv(
                        tx.as_mut(),
                        obs_confirm,
                        op_id(base + 4),
                        base + 4,
                    )
                    .await
                    .expect("obs confirm"),
                    FederationTransactionStateOutcome::Updated
                );
                tx.commit().await.expect("commit");
                let obs_confirm_counts = store.counts();

                store.reset_counts();
                let actual_receipt =
                    read_federation_completed_payment_for_quote_kv(&store, &record.intent.quote)
                        .await
                        .expect("receipt");
                assert_eq!(actual_receipt, expected_receipt);
                let completed_read_counts = store.counts();

                store.reset_counts();
                if retire {
                    let mut tx = store.begin_tx();
                    let mut lineage = load_federation_transaction_lineage_for_quote_settlement_kv(
                        tx.as_mut(),
                        &record.intent.quote,
                    )
                    .await
                    .expect("load settle")
                    .expect("some lineage");
                    let (batch_id, _) = lineage
                        .settle_completed_intent(&record.intent.quote, base + 5)
                        .expect("settle")
                        .expect("all settled");
                    lineage
                        .retire_settled_lineage(batch_id, base + 5)
                        .expect("retire");
                    lineage.save_kv(tx.as_mut()).await.expect("save retired");
                    tx.commit().await.expect("commit");
                }
                let settle_and_retire_counts = store.counts();

                (
                    proposal_id,
                    record.intent_id,
                    proposal.selected_inputs[0].outpoint,
                    record.intent.quote,
                    LifecycleCounts {
                        intent: intent_counts,
                        proposal: proposal_counts,
                        signed: signed_counts,
                        broadcast_intent: broadcast_intent_counts,
                        obs_mempool: obs_mempool_counts,
                        obs_confirm: obs_confirm_counts,
                        completed_read: completed_read_counts,
                        settle_and_retire: settle_and_retire_counts,
                    },
                )
            }
        };

        // Seed 5 initial batches (mix of active & retired)
        for idx in 1..=5_u64 {
            run_lifecycle(store.clone(), &mut reference, idx, idx % 2 == 0).await;
        }

        // Measure at N = 5
        let (retired_pid, retired_iid, retired_op, retired_quote, small) =
            run_lifecycle(store.clone(), &mut reference, 6, true).await;

        // Terminal lineage is invisible to point reads BEFORE history retention deletes it
        assert!(read_federation_accepted_transaction_kv(&store, retired_pid)
            .await
            .expect("read retired tx")
            .is_none());
        assert!(
            read_federation_completed_payment_for_quote_kv(&store, &retired_quote)
                .await
                .expect("read retired quote")
                .is_none()
        );
        let mut tx = store.begin_tx();
        assert!(load_federation_onchain_intent_kv(tx.as_mut(), retired_iid)
            .await
            .expect("load retired intent")
            .is_none());
        assert!(load_federation_utxo_reservation_kv(tx.as_mut(), retired_op)
            .await
            .expect("load retired reservation")
            .is_none());
        tx.rollback().await.expect("rollback");

        // Grow history to N = 150 batches
        for idx in 7..=150_u64 {
            run_lifecycle(store.clone(), &mut reference, idx, idx % 2 == 0).await;
        }

        // Measure at N = 150
        let (_, _, _, _, large) = run_lifecycle(store.clone(), &mut reference, 151, true).await;

        // Assert strictly O(1) independent of N across every single operation!
        assert_eq!(small.intent.entries, 0);
        assert_eq!(small.proposal.entries, 0);
        assert_eq!(small.signed.entries, 0);
        assert_eq!(small.signed.reads, 1);
        assert_eq!(small.signed.writes, 1);
        assert_eq!(small.broadcast_intent.entries, 0);
        assert_eq!(small.broadcast_intent.reads, 1);
        assert_eq!(small.broadcast_intent.writes, 1);
        assert_eq!(small.obs_mempool.entries, 0);
        assert_eq!(small.obs_confirm.entries, 0);
        assert_eq!(small.completed_read.entries, 0);
        assert_eq!(small.settle_and_retire.entries, 0);
        assert_eq!(large, small);

        // Byte-for-byte materialized-row and checkpoint-floor parity
        let stored_rows = store.namespace_rows(
            FEDERATION_STATE_PRIMARY_NAMESPACE,
            FederationTransactionState::SECONDARY_NAMESPACE,
        );
        assert_eq!(stored_rows, reference.to_rows().expect("ref tx rows"));
        assert_eq!(
            federation_state_rows_at_floor::<FederationTransactionState>(
                stored_rows.clone(),
                10_000
            )
            .expect("inc floor"),
            federation_state_rows_at_floor::<FederationTransactionState>(
                reference.to_rows().expect("ref tx rows"),
                10_000
            )
            .expect("ref floor")
        );

        // History retention prunes retired lineages below floor and cleans transaction_idx
        let mut tx = store.begin_tx();
        let pruned = retire_federation_state_below_kv::<FederationTransactionState, _>(
            tx.as_mut(),
            10_000,
            1000,
        )
        .await
        .expect("retire below");
        tx.commit().await.expect("commit");
        assert!(pruned > 0);
        let idx_rows = store.namespace_rows(
            FEDERATION_STATE_PRIMARY_NAMESPACE,
            FEDERATION_TRANSACTION_INDEX_NAMESPACE,
        );
        assert!(!idx_rows.contains_key(&hex_row_key("intent_batch", &retired_iid.to_hex())));

        // Verify state after retention is still 100% valid and matches reference at that floor
        let post_rows = store.namespace_rows(
            FEDERATION_STATE_PRIMARY_NAMESPACE,
            FederationTransactionState::SECONDARY_NAMESPACE,
        );
        assert_eq!(
            federation_state_rows_at_floor::<FederationTransactionState>(post_rows, 10_000)
                .expect("post floor"),
            federation_state_rows_at_floor::<FederationTransactionState>(
                reference.to_rows().expect("ref rows"),
                10_000
            )
            .expect("ref floor")
        );
    }

    #[tokio::test]
    async fn aborted_transaction_never_leaks_entity_or_index_mutations() {
        let store = CountingKvStore::new();
        let application = app_id(21);
        let scope = FederationAddressAllocationScope {
            application_id: application,
            application_key_epoch: 3,
        };

        // Commit 1 initial allocation so namespaces are warm
        let op1 = FederationAddressAllocationOperation::new(
            "committed-1",
            application,
            3,
            FederationAddressKeychain::External,
        )
        .expect("op1");
        let mut tx = store.begin_tx();
        let first = apply_federation_address_allocation_kv(tx.as_mut(), &op1, op_id(1), 1)
            .await
            .expect("first");
        tx.commit().await.expect("commit");
        assert_eq!(first.allocation().derivation_index, 0);

        let before = store.snapshot_all();

        // Execute mutations across all three projections and roll back
        let op_aborted = FederationAddressAllocationOperation::new(
            "aborted-quote",
            application,
            3,
            FederationAddressKeychain::External,
        )
        .expect("op aborted");
        let script = ScriptBuf::from_bytes(vec![0x51, 0x20, 9, 9]);
        let deposit_aborted = FederationDepositObservation {
            allocation: FederationAllocatedAddressRef {
                application_id: application,
                application_key_epoch: 3,
                quote: "committed-1".to_string(),
                keychain: FederationAddressKeychain::External,
                derivation_index: 0,
            },
            outpoint: outpoint_for(99),
            txout: TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: script.clone(),
            },
            confirmation: None,
            observation_epoch: 0,
            observer: member(1),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Independent,
                "src-1",
            )
            .expect("source"),
        };
        let (intent_aborted, ..) = build_batch_lifecycle_fixture(99, application);

        let mut tx = store.begin_tx();
        apply_federation_address_allocation_kv(tx.as_mut(), &op_aborted, op_id(2), 2)
            .await
            .expect("aborted alloc");
        apply_federation_deposit_observation_kv(
            tx.as_mut(),
            &deposit_aborted,
            first.allocation(),
            script.as_script(),
            1,
            1,
            op_id(3),
            3,
        )
        .await
        .expect("aborted deposit");
        add_pending_federation_onchain_intent_kv(tx.as_mut(), intent_aborted)
            .await
            .expect("aborted intent");
        tx.rollback().await.expect("rollback");

        assert_eq!(store.snapshot_all(), before);
        assert!(
            read_federation_address_allocation_kv(&store, scope, "aborted-quote")
                .await
                .expect("read aborted")
                .is_none()
        );

        // Next committed allocation receives derivation index 1 (counter was NOT leaked)
        let op2 = FederationAddressAllocationOperation::new(
            "committed-2",
            application,
            3,
            FederationAddressKeychain::External,
        )
        .expect("op2");
        let mut tx = store.begin_tx();
        let second = apply_federation_address_allocation_kv(tx.as_mut(), &op2, op_id(4), 4)
            .await
            .expect("second");
        tx.commit().await.expect("commit");
        assert_eq!(second.allocation().derivation_index, 1);
    }

    #[tokio::test]
    async fn lazy_backfill_from_unindexed_legacy_rows_runs_once() {
        let store = CountingKvStore::new();
        let application = app_id(33);
        let script = ScriptBuf::from_bytes(vec![0x51, 0x20, 7, 8]);

        // Seed raw entity rows directly WITHOUT touching any *_idx namespace
        let mut alloc_state = FederationAddressAllocationState::new();
        for idx in 1..=10_u64 {
            let op = FederationAddressAllocationOperation::new(
                format!("legacy-q-{idx}"),
                application,
                3,
                FederationAddressKeychain::External,
            )
            .expect("op");
            alloc_state.apply(&op, op_id(idx), idx).expect("seed alloc");
        }
        store.seed_raw_rows(
            FEDERATION_STATE_PRIMARY_NAMESPACE,
            FederationAddressAllocationState::SECONDARY_NAMESPACE,
            &alloc_state.to_rows().expect("alloc rows"),
        );

        let mut chain_state = FederationChainObservationState::new();
        let scope = FederationAddressAllocationScope {
            application_id: application,
            application_key_epoch: 3,
        };
        let alloc1 = alloc_state
            .allocation(scope, "legacy-q-1")
            .expect("alloc1")
            .clone();
        let dep1 = FederationDepositObservation {
            allocation: FederationAllocatedAddressRef {
                application_id: application,
                application_key_epoch: 3,
                quote: alloc1.quote.clone(),
                keychain: alloc1.keychain,
                derivation_index: alloc1.derivation_index,
            },
            outpoint: outpoint_for(1),
            txout: TxOut {
                value: Amount::from_sat(30_000),
                script_pubkey: script.clone(),
            },
            confirmation: Some(FederationDepositConfirmation {
                block_hash: BlockHash::from_byte_array([55; 32]),
                block_height: 150,
                confirmation_depth: 2,
            }),
            observation_epoch: 0,
            observer: member(1),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Independent,
                "src-1",
            )
            .expect("src"),
        };
        chain_state
            .apply_deposit(&dep1, &alloc1, script.as_script(), 1, 2, op_id(20), 20)
            .expect("seed deposit");
        store.seed_raw_rows(
            FEDERATION_STATE_PRIMARY_NAMESPACE,
            FederationChainObservationState::SECONDARY_NAMESPACE,
            &chain_state.to_rows().expect("chain rows"),
        );

        // Verify *_idx namespaces start completely empty
        assert!(store
            .namespace_rows(
                FEDERATION_STATE_PRIMARY_NAMESPACE,
                FEDERATION_ADDRESS_ALLOCATION_INDEX_NAMESPACE,
            )
            .is_empty());
        assert!(store
            .namespace_rows(
                FEDERATION_STATE_PRIMARY_NAMESPACE,
                FEDERATION_CHAIN_OBSERVATION_INDEX_NAMESPACE,
            )
            .is_empty());

        // First incremental op backfills indexes once and assigns index 10
        let op11 = FederationAddressAllocationOperation::new(
            "legacy-q-11",
            application,
            3,
            FederationAddressKeychain::External,
        )
        .expect("op11");
        let mut tx = store.begin_tx();
        let res11 = apply_federation_address_allocation_kv(tx.as_mut(), &op11, op_id(11), 11)
            .await
            .expect("apply 11");
        let quote_pay =
            load_federation_quote_payment_at_or_before_kv(tx.as_mut(), "legacy-q-1", u64::MAX)
                .await
                .expect("load quote pay");
        tx.commit().await.expect("commit");
        assert_eq!(res11.allocation().derivation_index, 10);
        assert_eq!(quote_pay.expect("some payment").amount_sat, 30_000);

        // Subsequent operations perform ZERO table scans
        store.reset_counts();
        let op12 = FederationAddressAllocationOperation::new(
            "legacy-q-12",
            application,
            3,
            FederationAddressKeychain::External,
        )
        .expect("op12");
        let mut tx = store.begin_tx();
        let res12 = apply_federation_address_allocation_kv(tx.as_mut(), &op12, op_id(12), 12)
            .await
            .expect("apply 12");
        let quote_pay2 =
            load_federation_quote_payment_at_or_before_kv(tx.as_mut(), "legacy-q-1", u64::MAX)
                .await
                .expect("load quote pay 2");
        tx.commit().await.expect("commit");
        assert_eq!(res12.allocation().derivation_index, 11);
        assert_eq!(quote_pay2.expect("some payment").amount_sat, 30_000);
        let counts = store.counts();
        assert_eq!(counts.entries, 0);
        assert_eq!(counts.lists, 0);

        // Also verify tx index namespace backfill via load_federation_accepted_transaction_kv
        let (rec, prop, audit, ..) = build_batch_lifecycle_fixture(1, application);
        let pid = prop.proposal_id().expect("pid");
        let mut tx_state = FederationTransactionState::new();
        tx_state.add_pending_intent(rec).expect("intent");
        tx_state
            .accept_transaction(prop, audit, op_id(50), 50)
            .expect("accept");
        store.seed_raw_rows(
            FEDERATION_STATE_PRIMARY_NAMESPACE,
            FederationTransactionState::SECONDARY_NAMESPACE,
            &tx_state.to_rows().expect("tx rows"),
        );
        store.reset_counts();
        let mut tx = store.begin_tx();
        let loaded = load_federation_accepted_transaction_kv(tx.as_mut(), pid)
            .await
            .expect("load accepted");
        tx.rollback().await.expect("rollback");
        assert!(loaded.is_some());
        assert_eq!(store.counts().entries, 0);
        assert_eq!(store.counts().reads, 1);
    }
}

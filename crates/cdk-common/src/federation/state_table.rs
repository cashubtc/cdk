//! Row storage for consensus state projections.
//!
//! Each projection is stored as one key-value row per entity (intent,
//! transaction, UTXO, allocation, reservation) in its own namespace. A load
//! reads every row of the namespace in one query and validates the whole
//! projection; a save writes only the rows that changed and removes the rows
//! that disappeared, so a write is proportional to the change rather than to
//! retained history.

use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use bitcoin::hashes::{sha256, Hash, HashEngine};
use bitcoin::OutPoint;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::canonical::canonical_json_bytes;
use super::{FederationError, FederationMaterializedStateRow, Result};
use crate::database::mint::{DynMintDatabase, DynMintTransaction};
use crate::error::Error;

/// Primary key-value namespace shared by every consensus state table.
pub const FEDERATION_STATE_PRIMARY_NAMESPACE: &str = "federation";

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

    /// Validate the projection and encode one row per entity.
    fn to_rows(&self) -> Result<FederationStateRows>;

    /// Decode every row and validate the rebuilt projection. Every row key must
    /// be the key its content would be stored under.
    fn from_rows(rows: &FederationStateRows) -> Result<Self>;

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

/// Read and validate a whole projection outside a database transaction, as
/// operations see it (terminal entities retired).
pub async fn read_federation_state<S>(db: &DynMintDatabase) -> std::result::Result<S, Error>
where
    S: FederationStateTable,
{
    let rows = read_federation_state_rows::<S>(db).await?;
    Ok(S::from_rows(&S::retire_terminal_rows(rows)?)?)
}

/// Read every stored row of a projection, terminal entities included.
pub async fn read_federation_state_rows<S>(
    db: &DynMintDatabase,
) -> std::result::Result<FederationStateRows, Error>
where
    S: FederationStateTable,
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
    let stored = load_federation_state_rows::<S>(transaction).await?;
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
    Ok(changed)
}

/// A projection loaded inside a database transaction, together with the rows
/// it was loaded from so [`Self::save`] writes only what changed.
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

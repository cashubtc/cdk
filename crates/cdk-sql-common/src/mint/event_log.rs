//! Append-only transparency event log persistence.
//!
//! See `docs/adr/0001-append-only-transparency-log.md`. Every mutation
//! that touches an entity in [`LoggedEntity`] calls [`append_event`] on
//! the *same* connection it just used for its own `UPDATE`/`DELETE`, so
//! that a rolled-back mutation can never leave an orphaned log entry
//! behind, and a committed mutation can never lose its log entry either.

use async_trait::async_trait;
use cdk_common::database::mint::{
    EventOp, LoggedEntity, MintEventLogEntry, TransparencyLogDatabase,
};
use cdk_common::database::{self, Error};
use cdk_common::util::unix_time;
use cdk_tlog::merkle::leaf_hash;

use super::SQLMintDatabase;
use crate::database::DatabaseExecutor;
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{column_as_binary, column_as_number, column_as_string, unpack_into};

/// Appends one entry to `mint_event_log`.
///
/// `payload` should be a canonical (e.g. `serde_json`) encoding of
/// whatever fields the caller's mutation just wrote — see the call sites
/// in `quotes.rs`, `proofs.rs`, `keys.rs`, and `signatures.rs` for the
/// per-entity payload shapes.
pub(super) async fn append_event<C>(
    conn: &C,
    entity_type: LoggedEntity,
    entity_id: &str,
    op: EventOp,
    payload: &[u8],
) -> Result<(), Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    let created_time = unix_time();
    let preimage = database::event_leaf_preimage(entity_type, entity_id, op, payload, created_time);
    let hash = leaf_hash(&preimage);

    query(
        r#"
        INSERT INTO mint_event_log (entity_type, entity_id, op, payload, leaf_hash, created_time)
        VALUES (:entity_type, :entity_id, :op, :payload, :leaf_hash, :created_time)
        "#,
    )?
    .bind("entity_type", entity_type.as_str().to_string())
    .bind("entity_id", entity_id.to_string())
    .bind("op", i64::from(op.as_i16()))
    .bind("payload", payload.to_vec())
    .bind("leaf_hash", hash.to_vec())
    .bind("created_time", created_time as i64)
    .execute(conn)
    .await?;

    Ok(())
}

fn row_to_event_log_entry(row: Vec<Column>) -> Result<MintEventLogEntry, Error> {
    unpack_into!(
        let (leaf_index, entity_type, entity_id, op, payload, leaf_hash, created_time) = row
    );

    let entity_type: String = column_as_string!(&entity_type);
    let leaf_hash: Vec<u8> = column_as_binary!(leaf_hash);
    let leaf_hash: [u8; 32] = leaf_hash
        .try_into()
        .map_err(|_| Error::Internal("leaf_hash column was not 32 bytes".to_string()))?;

    Ok(MintEventLogEntry {
        seq: column_as_number!(leaf_index),
        entity_type: entity_type.parse()?,
        entity_id: column_as_string!(&entity_id),
        op: {
            let op: i64 = column_as_number!(op);
            EventOp::try_from(op as i16)?
        },
        payload: column_as_binary!(payload),
        leaf_hash,
        created_time: column_as_number!(created_time),
    })
}

#[async_trait]
impl<RM> TransparencyLogDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn assign_leaf_indices(&self, max: u64) -> Result<Vec<MintEventLogEntry>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;

        let next_index: u64 = {
            let row = query(r#"SELECT COALESCE(MAX(leaf_index) + 1, 0) FROM mint_event_log"#)?
                .fetch_one(&*conn)
                .await?;
            match row {
                Some(row) => column_as_number!(row[0].clone()),
                None => 0,
            }
        };

        // Number the oldest `max` unindexed committed rows densely from
        // `next_index`, in `seq` order. Assigned one row at a time, in
        // ascending order, so a crash partway through still leaves a dense
        // contiguous prefix — the next tick simply continues from
        // MAX(leaf_index) + 1. Only the single appender task calls this
        // (see the trait docs), so there is no concurrent assigner to race
        // with.
        let unindexed_seqs: Vec<i64> = query(
            r#"
            SELECT seq FROM mint_event_log
            WHERE leaf_index IS NULL
            ORDER BY seq ASC
            LIMIT :max
            "#,
        )?
        .bind("max", max as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(|row| -> Result<i64, Error> {
            Ok(column_as_number!(row
                .first()
                .ok_or_else(|| Error::Internal("empty row".to_string()))?
                .clone()))
        })
        .collect::<Result<Vec<i64>, Error>>()?;

        for (offset, seq) in unindexed_seqs.iter().enumerate() {
            query(
                r#"
                UPDATE mint_event_log SET leaf_index = :leaf_index
                WHERE seq = :seq AND leaf_index IS NULL
                "#,
            )?
            .bind("leaf_index", (next_index + offset as u64) as i64)
            .bind("seq", *seq)
            .execute(&*conn)
            .await?;
        }

        // Release before re-querying: `get_event_log_range` takes its own
        // pool connection, and in-memory SQLite pools may only have one.
        drop(conn);

        self.get_event_log_range(next_index, next_index.saturating_add(max))
            .await
    }

    async fn get_event_log_range(
        &self,
        start: u64,
        end: u64,
    ) -> Result<Vec<MintEventLogEntry>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT leaf_index, entity_type, entity_id, op, payload, leaf_hash, created_time
            FROM mint_event_log
            WHERE leaf_index >= :start AND leaf_index < :end
            ORDER BY leaf_index ASC
            "#,
        )?
        .bind("start", start as i64)
        .bind("end", end as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(row_to_event_log_entry)
        .collect()
    }
}

#[cfg(test)]
mod tests {
    /// Pins the exact bytes `serde_json` produces for the payload shapes
    /// the mutation call sites write. These bytes are part of the leaf
    /// hash preimage (NUT-XX), so their key order and formatting are a
    /// consensus-critical wire format, not an implementation detail:
    /// `serde_json`'s default map is a BTreeMap (keys sorted), but if any
    /// crate anywhere in the dependency graph ever enables the
    /// `preserve_order` feature, cargo feature unification flips maps to
    /// insertion order for the whole build and every newly computed leaf
    /// hash silently changes. This test turns that silent divergence into
    /// a loud CI failure.
    #[test]
    fn canonical_payload_bytes_are_pinned() {
        let proof_update = serde_json::to_vec(&serde_json::json!({ "state": "SPENT" })).unwrap();
        assert_eq!(proof_update, br#"{"state":"SPENT"}"#);

        let melt_quote_update = serde_json::to_vec(&serde_json::json!({
            "state": "PAID",
            "fee_reserve": 10u64,
            "estimated_blocks": Option::<u64>::None,
            "selected_fee_index": Option::<u64>::None,
            "paid_time": 1_782_920_900u64,
            "payment_proof": "preimage",
        }))
        .unwrap();
        // Keys must serialize alphabetically, regardless of insertion
        // order above.
        assert_eq!(
            melt_quote_update,
            br#"{"estimated_blocks":null,"fee_reserve":10,"paid_time":1782920900,"payment_proof":"preimage","selected_fee_index":null,"state":"PAID"}"#
        );

        let blind_signature_update = serde_json::to_vec(&serde_json::json!({
            "c": "02aa",
            "dleq_e": Option::<String>::None,
            "dleq_s": Option::<String>::None,
            "signed_time": 100u64,
            "amount": 8u64,
        }))
        .unwrap();
        assert_eq!(
            blind_signature_update,
            br#"{"amount":8,"c":"02aa","dleq_e":null,"dleq_s":null,"signed_time":100}"#
        );

        let keyset_update = serde_json::to_vec(&serde_json::json!({ "active": true })).unwrap();
        assert_eq!(keyset_update, br#"{"active":true}"#);

        let proof_insert = serde_json::to_vec(&serde_json::json!({
            "amount": 8u64,
            "keyset_id": "00916bbf7ef91a36",
            "state": "UNSPENT",
        }))
        .unwrap();
        assert_eq!(
            proof_insert,
            br#"{"amount":8,"keyset_id":"00916bbf7ef91a36","state":"UNSPENT"}"#
        );

        let blind_signature_insert = serde_json::to_vec(&serde_json::json!({
            "amount": 8u64,
            "keyset_id": "00916bbf7ef91a36",
            "c": "02aa",
            "dleq_e": Option::<String>::None,
            "dleq_s": Option::<String>::None,
            "signed_time": 100u64,
        }))
        .unwrap();
        assert_eq!(
            blind_signature_insert,
            br#"{"amount":8,"c":"02aa","dleq_e":null,"dleq_s":null,"keyset_id":"00916bbf7ef91a36","signed_time":100}"#
        );

        let melt_quote_insert = serde_json::to_vec(&serde_json::json!({
            "amount": 100u64,
            "unit": "sat",
            "fee_reserve": 1u64,
            "state": "UNPAID",
            "expiry": 1_782_920_900u64,
            "payment_method": "bolt11",
            "request_lookup_id": Option::<String>::None,
            "request_lookup_id_kind": Option::<String>::None,
        }))
        .unwrap();
        assert_eq!(
            melt_quote_insert,
            br#"{"amount":100,"expiry":1782920900,"fee_reserve":1,"payment_method":"bolt11","request_lookup_id":null,"request_lookup_id_kind":null,"state":"UNPAID","unit":"sat"}"#
        );

        let keyset_insert = serde_json::to_vec(&serde_json::json!({
            "unit": "sat",
            "active": true,
            "valid_from": 0u64,
            "valid_to": Option::<u64>::None,
            "input_fee_ppk": 0u64,
        }))
        .unwrap();
        assert_eq!(
            keyset_insert,
            br#"{"active":true,"input_fee_ppk":0,"unit":"sat","valid_from":0,"valid_to":null}"#
        );
    }
}

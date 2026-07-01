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
        let (seq, entity_type, entity_id, op, payload, leaf_hash, created_time) = row
    );

    let entity_type: String = column_as_string!(&entity_type);
    let leaf_hash: Vec<u8> = column_as_binary!(leaf_hash);
    let leaf_hash: [u8; 32] = leaf_hash
        .try_into()
        .map_err(|_| Error::Internal("leaf_hash column was not 32 bytes".to_string()))?;

    Ok(MintEventLogEntry {
        seq: column_as_number!(seq),
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

    async fn latest_event_log_seq(&self) -> Result<u64, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let row = query(r#"SELECT COALESCE(MAX(seq), 0) FROM mint_event_log"#)?
            .fetch_one(&*conn)
            .await?;
        match row {
            Some(row) => Ok(column_as_number!(row[0].clone())),
            None => Ok(0),
        }
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
            SELECT seq, entity_type, entity_id, op, payload, leaf_hash, created_time
            FROM mint_event_log
            WHERE seq >= :start AND seq < :end
            ORDER BY seq ASC
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

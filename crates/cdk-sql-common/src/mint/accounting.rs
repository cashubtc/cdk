//! Durable keyset accounting and atomic batch aggregation.

use std::collections::BTreeMap;

use cdk_common::database::Error;

use super::{SQLMintDatabase, SQLTransaction};
use crate::database::{ConnectionWithTransaction, DatabaseExecutor};
use crate::pool::DatabasePool;
use crate::stmt::query;
use crate::{column_as_number, column_as_string, unpack_into};

impl<RM> SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    pub(super) async fn journal_accounting(
        &self,
        keyset_id: String,
        issued: i64,
        redeemed: i64,
        fees: i64,
    ) -> Result<(), Error> {
        if issued == 0 && redeemed == 0 && fees == 0 {
            return Ok(());
        }
        query(
            "INSERT INTO keyset_accounting
             (event_id, keyset_id, total_issued, total_redeemed, fee_collected)
             VALUES (:event_id, :keyset_id, :issued, :redeemed, :fees)",
        )?
        .bind("event_id", uuid::Uuid::new_v4().to_string())
        .bind("keyset_id", keyset_id)
        .bind("issued", issued)
        .bind("redeemed", redeemed)
        .bind("fees", fees)
        .execute(&self.inner)
        .await?;
        Ok(())
    }
}

impl<RM> SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    pub(super) async fn flush_accounting(&self, batch_size: usize) -> Result<usize, Error> {
        if batch_size == 0 {
            return Err(Error::Internal(
                "Accounting batch size must be positive".to_owned(),
            ));
        }
        let batch_size = i64::try_from(batch_size)?;
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let tx = ConnectionWithTransaction::new(conn).await?;
        let locks = if RM::Connection::name() == "postgres" {
            "FOR UPDATE SKIP LOCKED"
        } else {
            ""
        };
        let rows = query(&format!(
            "SELECT event_id, keyset_id, total_issued, total_redeemed, fee_collected
             FROM keyset_accounting ORDER BY event_id LIMIT :batch_size {locks}"
        ))?
        .bind("batch_size", batch_size)
        .fetch_all(&tx)
        .await?;
        let mut ids = Vec::with_capacity(rows.len());
        let mut totals = BTreeMap::<String, [i64; 3]>::new();
        for row in rows {
            unpack_into!(let (event_id, keyset_id, issued, redeemed, fees) = row);
            ids.push(column_as_string!(event_id));
            let total = totals.entry(column_as_string!(keyset_id)).or_default();
            let deltas: [i64; 3] = [
                column_as_number!(issued),
                column_as_number!(redeemed),
                column_as_number!(fees),
            ];
            for (value, delta) in total.iter_mut().zip(deltas) {
                *value = value
                    .checked_add(delta)
                    .ok_or_else(|| Error::Internal("Keyset accounting overflow".to_owned()))?;
            }
        }
        // All workers visit shared totals in the same order.
        for (keyset_id, [issued, redeemed, fees]) in totals {
            let affected = query(
                "INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed, fee_collected)
                 VALUES (:keyset_id, :issued, :redeemed, :fees)
                 ON CONFLICT (keyset_id) DO UPDATE SET
                     total_issued = keyset_amounts.total_issued + EXCLUDED.total_issued,
                     total_redeemed = keyset_amounts.total_redeemed + EXCLUDED.total_redeemed,
                     fee_collected = keyset_amounts.fee_collected + EXCLUDED.fee_collected
                 WHERE keyset_amounts.total_issued <= :max - EXCLUDED.total_issued
                   AND keyset_amounts.total_redeemed <= :max - EXCLUDED.total_redeemed
                   AND keyset_amounts.fee_collected <= :max - EXCLUDED.fee_collected"
            )?
            .bind("keyset_id", keyset_id)
            .bind("issued", issued)
            .bind("redeemed", redeemed)
            .bind("fees", fees)
            .bind("max", i64::MAX)
            .execute(&tx)
            .await?;
            if affected != 1 {
                return Err(Error::Internal("Keyset accounting overflow".to_owned()));
            }
        }
        let count = ids.len();
        if count > 0 {
            let deleted = query("DELETE FROM keyset_accounting WHERE event_id IN (:ids)")?
                .bind_vec("ids", ids)?
                .execute(&tx)
                .await?;
            if deleted != count {
                return Err(Error::ConcurrentUpdate);
            }
        }
        tx.commit().await?;
        Ok(count)
    }
}

//! Per-keyset `keyset_amounts` changes, held in the transaction and written on commit
//!
//! Every swap, mint and melt on a keyset updates the same row. On Postgres that
//! update locks the row until the transaction ends, so writing it just before
//! `COMMIT` holds the lock only for the write and the commit.

use std::collections::BTreeMap;

use cdk_common::database::Error;
use cdk_common::{Amount, Id};

use crate::database::DatabaseExecutor;
use crate::stmt::query;

/// What one transaction adds to a keyset's totals.
#[derive(Debug, Default)]
struct KeysetTotalsChange {
    issued: Amount,
    redeemed: Amount,
    fee_collected: Amount,
}

/// The changes a transaction makes to `keyset_amounts`, by keyset ID.
#[derive(Debug, Default)]
pub(super) struct PendingKeysetTotals {
    changes: BTreeMap<Id, KeysetTotalsChange>,
}

impl PendingKeysetTotals {
    pub(super) fn add_issued(&mut self, keyset_id: Id, amount: Amount) -> Result<(), Error> {
        add_to(
            &mut self.changes.entry(keyset_id).or_default().issued,
            amount,
        )
    }

    pub(super) fn add_redeemed(&mut self, keyset_id: Id, amount: Amount) -> Result<(), Error> {
        add_to(
            &mut self.changes.entry(keyset_id).or_default().redeemed,
            amount,
        )
    }

    pub(super) fn add_fee_collected(&mut self, keyset_id: Id, amount: Amount) -> Result<(), Error> {
        add_to(
            &mut self.changes.entry(keyset_id).or_default().fee_collected,
            amount,
        )
    }

    /// Upserts one row per keyset in ID order, so concurrent transactions lock
    /// shared rows in the same order and cannot deadlock.
    pub(super) async fn write<C>(&self, conn: &C) -> Result<(), Error>
    where
        C: DatabaseExecutor + Send + Sync,
    {
        for (keyset_id, change) in &self.changes {
            query(
                r#"
                INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed, fee_collected)
                VALUES (:keyset_id, :issued, :redeemed, :fee_collected)
                ON CONFLICT (keyset_id)
                DO UPDATE SET
                    total_issued = keyset_amounts.total_issued + EXCLUDED.total_issued,
                    total_redeemed = keyset_amounts.total_redeemed + EXCLUDED.total_redeemed,
                    fee_collected = keyset_amounts.fee_collected + EXCLUDED.fee_collected
                "#,
            )?
            .bind("keyset_id", keyset_id.to_string())
            .bind("issued", i64::try_from(change.issued.to_u64())?)
            .bind("redeemed", i64::try_from(change.redeemed.to_u64())?)
            .bind(
                "fee_collected",
                i64::try_from(change.fee_collected.to_u64())?,
            )
            .execute(conn)
            .await?;
        }

        Ok(())
    }
}

fn add_to(total: &mut Amount, amount: Amount) -> Result<(), Error> {
    *total = total.checked_add(amount).ok_or(Error::AmountOverflow)?;
    Ok(())
}

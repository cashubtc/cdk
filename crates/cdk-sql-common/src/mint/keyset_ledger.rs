//! Per-keyset ledger of what the mint issued against what it owes.
//!
//! A mint can only ever owe what it signed, so what a keyset is holding plus
//! what it has already paid out must never overtake what it issued. Holding
//! that line bounds the damage from a leaked signing key to the keyset's
//! outstanding float instead of letting an attacker redeem without limit, since
//! forged proofs are otherwise indistinguishable from honest ones.

use std::collections::BTreeMap;
use std::str::FromStr;

use cdk_common::database::Error;
use cdk_common::{Amount, Id};

use crate::database::DatabaseExecutor;
use crate::stmt::query;
use crate::{column_as_number, column_as_string, unpack_into};

/// Movements to apply to each keyset, keyed in ID order because every entry
/// locks its row for the rest of the transaction, and concurrent operations
/// sharing a keyset would otherwise deadlock by locking in opposite orders.
pub(super) type LedgerMoves = BTreeMap<Id, LedgerMove>;

/// What one keyset's counters move by in a single locked pass.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct LedgerMove {
    issued_in: Amount,
    reserved_in: Amount,
    reserved_out: Amount,
    redeemed_in: Amount,
}

impl LedgerMove {
    /// The mint signed this amount against the keyset.
    pub(super) fn issue(amount: Amount) -> Self {
        Self {
            issued_in: amount,
            ..Default::default()
        }
    }

    /// The mint took custody of proofs worth this amount.
    pub(super) fn reserve(amount: Amount) -> Self {
        Self {
            reserved_in: amount,
            ..Default::default()
        }
    }

    /// The mint gave proofs back after a failed swap or melt.
    pub(super) fn release(amount: Amount) -> Self {
        Self {
            reserved_out: amount,
            ..Default::default()
        }
    }

    /// Proofs the mint was holding are burned, which moves the amount across
    /// without changing what the keyset owes in total.
    pub(super) fn redeem(amount: Amount) -> Self {
        Self {
            reserved_out: amount,
            redeemed_in: amount,
            ..Default::default()
        }
    }
}

/// Fold one movement into `moves`, so a whole batch costs a single locked pass
/// per keyset.
pub(super) fn accumulate(
    moves: &mut LedgerMoves,
    keyset_id: Id,
    movement: LedgerMove,
) -> Result<(), Error> {
    let entry = moves.entry(keyset_id).or_default();

    entry.issued_in = add(entry.issued_in, movement.issued_in)?;
    entry.reserved_in = add(entry.reserved_in, movement.reserved_in)?;
    entry.reserved_out = add(entry.reserved_out, movement.reserved_out)?;
    entry.redeemed_in = add(entry.redeemed_in, movement.redeemed_in)?;

    Ok(())
}

fn add(left: Amount, right: Amount) -> Result<Amount, Error> {
    left.checked_add(right).ok_or(Error::AmountOverflow)
}

fn to_delta(amount: Amount) -> Result<i64, Error> {
    amount.to_i64().ok_or(Error::AmountOverflow)
}

/// One keyset's counters. A keyset with no row has issued nothing, so every
/// field reads as zero and any reservation against it is refused.
#[derive(Debug, Clone, Copy, Default)]
struct KeysetTotals {
    issued: Amount,
    redeemed: Amount,
    reserved: Amount,
}

async fn read_for_update<C>(conn: &C, keyset_id: &Id) -> Result<KeysetTotals, Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    let Some(row) = query(
        r#"
        SELECT total_issued, total_redeemed, total_reserved
        FROM keyset_amounts
        WHERE keyset_id = :keyset_id
        FOR UPDATE
        "#,
    )?
    .bind("keyset_id", keyset_id.to_string())
    .fetch_one(conn)
    .await?
    else {
        return Ok(KeysetTotals::default());
    };

    unpack_into!(let (issued, redeemed, reserved) = row);

    let issued: u64 = column_as_number!(issued);
    let redeemed: u64 = column_as_number!(redeemed);
    let reserved: u64 = column_as_number!(reserved);

    Ok(KeysetTotals {
        issued: Amount::from(issued),
        redeemed: Amount::from(redeemed),
        reserved: Amount::from(reserved),
    })
}

/// Apply `moves`, refusing any that would leave a keyset owing more than it
/// issued.
///
/// Only a movement that raises what a keyset owes can be refused. Burning
/// proofs the mint already holds moves the amount from reserved to redeemed
/// and leaves the total untouched, which is what makes it safe to call this on
/// the melt path after the invoice has been paid.
pub(super) async fn apply<C>(conn: &C, moves: &LedgerMoves) -> Result<(), Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    for (keyset_id, movement) in moves {
        let totals = read_for_update(conn, keyset_id).await?;

        let owed_before = add(totals.redeemed, totals.reserved)?;
        let issued = add(totals.issued, movement.issued_in)?;
        let redeemed = add(totals.redeemed, movement.redeemed_in)?;

        let reserved =
            match add(totals.reserved, movement.reserved_in)?.checked_sub(movement.reserved_out) {
                Some(reserved) => reserved,
                None => {
                    tracing::error!(
                        "keyset {} is releasing {} of a {} reservation, its counters have drifted",
                        keyset_id,
                        movement.reserved_out,
                        totals.reserved
                    );
                    Amount::ZERO
                }
            };

        let owed = add(redeemed, reserved)?;

        if owed > issued {
            if owed > owed_before {
                tracing::error!(
                    "refusing to let keyset {} owe {} against the {} it issued",
                    keyset_id,
                    owed,
                    issued
                );
                return Err(Error::KeysetOverRedeemed(*keyset_id));
            }

            tracing::error!(
                "keyset {} already owes {} against the {} it issued, allowing a movement that does not raise it",
                keyset_id,
                owed,
                issued
            );
        }

        let reserved_delta =
            i64::try_from(i128::from(reserved.to_u64()) - i128::from(totals.reserved.to_u64()))
                .map_err(|_| Error::AmountOverflow)?;

        query(
            r#"
            INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed, total_reserved)
            VALUES (:keyset_id, :total_issued, :total_redeemed, :total_reserved)
            ON CONFLICT (keyset_id)
            DO UPDATE SET
                total_issued   = keyset_amounts.total_issued   + EXCLUDED.total_issued,
                total_redeemed = keyset_amounts.total_redeemed + EXCLUDED.total_redeemed,
                total_reserved = keyset_amounts.total_reserved + EXCLUDED.total_reserved
            "#,
        )?
        .bind("keyset_id", keyset_id.to_string())
        .bind("total_issued", to_delta(movement.issued_in)?)
        .bind("total_redeemed", to_delta(movement.redeemed_in)?)
        .bind("total_reserved", reserved_delta)
        .execute(conn)
        .await?;
    }

    Ok(())
}

/// Raise `total_issued` to cover what a keyset already owes, so a keyset
/// imported from another mint implementation, or restored from a partial
/// backup, does not freeze every proof held against it.
///
/// Widening the cap is deliberate and is logged at error level on every startup
/// until an operator acts on it.
pub(super) async fn reconcile<C>(conn: &C) -> Result<(), Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    let over_redeemed = query(
        r#"
        SELECT keyset_id, total_issued, total_redeemed, total_reserved
        FROM keyset_amounts
        WHERE total_redeemed + total_reserved > total_issued
        ORDER BY keyset_id
        FOR UPDATE
        "#,
    )?
    .fetch_all(conn)
    .await?;

    if over_redeemed.is_empty() {
        return Ok(());
    }

    for row in over_redeemed {
        unpack_into!(let (keyset_id, issued, redeemed, reserved) = row);

        let keyset_id = column_as_string!(keyset_id, Id::from_str, Id::from_bytes);
        let issued: u64 = column_as_number!(issued);
        let redeemed: u64 = column_as_number!(redeemed);
        let reserved: u64 = column_as_number!(reserved);
        let owed = redeemed
            .checked_add(reserved)
            .ok_or(Error::AmountOverflow)?;

        tracing::error!(
            "keyset {} owes {} against the {} it issued, raising its issued total by {}. Its issuance history is incomplete or its signing key has leaked",
            keyset_id,
            owed,
            issued,
            owed.saturating_sub(issued)
        );
    }

    query(
        r#"
        UPDATE keyset_amounts
        SET total_issued = total_redeemed + total_reserved
        WHERE total_redeemed + total_reserved > total_issued
        "#,
    )?
    .execute(conn)
    .await?;

    Ok(())
}

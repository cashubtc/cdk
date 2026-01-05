//! Proofs database implementation

use std::collections::HashMap;
use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::database::{self, Error, MintProofsDatabase};
use cdk_common::mint::Operation;
use cdk_common::nut00::ProofsMethods;
use cdk_common::quote_id::QuoteId;
use cdk_common::secret::Secret;
use cdk_common::{Amount, Id, Proof, Proofs, PublicKey, State};

use super::{SQLMintDatabase, SQLTransaction};
use crate::database::DatabaseExecutor;
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{column_as_nullable_string, column_as_number, column_as_string, unpack_into};

pub(super) async fn get_current_states<C>(
    conn: &C,
    ys: &[PublicKey],
    for_update: bool,
) -> Result<HashMap<PublicKey, State>, Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    if ys.is_empty() {
        return Ok(Default::default());
    }
    let for_update_clause = if for_update { "FOR UPDATE" } else { "" };

    query(&format!(
        r#"SELECT y, state FROM proof WHERE y IN (:ys) {}"#,
        for_update_clause
    ))?
    .bind_vec("ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|row| {
        Ok((
            column_as_string!(&row[0], PublicKey::from_hex, PublicKey::from_slice),
            column_as_string!(&row[1], State::from_str),
        ))
    })
    .collect::<Result<HashMap<_, _>, _>>()
}

pub(super) fn sql_row_to_proof(row: Vec<Column>) -> Result<Proof, Error> {
    unpack_into!(
        let (
            amount,
            keyset_id,
            secret,
            c,
            witness
        ) = row
    );

    let amount: u64 = column_as_number!(amount);
    Ok(Proof {
        amount: Amount::from(amount),
        keyset_id: column_as_string!(keyset_id, Id::from_str),
        secret: column_as_string!(secret, Secret::from_str),
        c: column_as_string!(c, PublicKey::from_hex, PublicKey::from_slice),
        witness: column_as_nullable_string!(witness).and_then(|w| serde_json::from_str(&w).ok()),
        dleq: None,
    })
}

pub(super) fn sql_row_to_proof_with_state(
    row: Vec<Column>,
) -> Result<(Proof, Option<State>), Error> {
    unpack_into!(
        let (
            keyset_id, amount, secret, c, witness, state
        ) = row
    );

    let amount: u64 = column_as_number!(amount);
    let state = column_as_nullable_string!(state).and_then(|s| State::from_str(&s).ok());

    Ok((
        Proof {
            amount: Amount::from(amount),
            keyset_id: column_as_string!(keyset_id, Id::from_str, Id::from_bytes),
            secret: column_as_string!(secret, Secret::from_str),
            c: column_as_string!(c, PublicKey::from_hex, PublicKey::from_slice),
            witness: column_as_nullable_string!(witness)
                .and_then(|w| serde_json::from_str(&w).ok()),
            dleq: None,
        },
        state,
    ))
}

pub(super) fn sql_row_to_hashmap_amount(row: Vec<Column>) -> Result<(Id, Amount), Error> {
    unpack_into!(
        let (
            keyset_id, amount
        ) = row
    );

    let amount: u64 = column_as_number!(amount);
    Ok((
        column_as_string!(keyset_id, Id::from_str, Id::from_bytes),
        Amount::from(amount),
    ))
}

#[async_trait]
impl<RM> database::MintProofsTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_proofs(
        &mut self,
        proofs: Proofs,
        quote_id: Option<QuoteId>,
        operation: &Operation,
    ) -> Result<(), Self::Err> {
        let current_time = cdk_common::util::unix_time();

        // Check any previous proof, this query should return None in order to proceed storing
        // Any result here would error
        match query(r#"SELECT state FROM proof WHERE y IN (:ys) LIMIT 1 FOR UPDATE"#)?
            .bind_vec(
                "ys",
                proofs
                    .iter()
                    .map(|y| y.y().map(|y| y.to_bytes().to_vec()))
                    .collect::<Result<_, _>>()?,
            )
            .pluck(&self.inner)
            .await?
            .map(|state| Ok::<_, Error>(column_as_string!(&state, State::from_str)))
            .transpose()?
        {
            Some(State::Spent) => Err(database::Error::AttemptUpdateSpentProof),
            Some(_) => Err(database::Error::Duplicate),
            None => Ok(()), // no previous record
        }?;

        for proof in proofs {
            let y = proof.y()?;

            query(
                r#"
                  INSERT INTO proof
                  (y, amount, keyset_id, secret, c, witness, state, quote_id, created_time, operation_kind, operation_id)
                  VALUES
                  (:y, :amount, :keyset_id, :secret, :c, :witness, :state, :quote_id, :created_time, :operation_kind, :operation_id)
                  "#,
            )?
            .bind("y", y.to_bytes().to_vec())
            .bind("amount", proof.amount.to_i64())
            .bind("keyset_id", proof.keyset_id.to_string())
            .bind("secret", proof.secret.to_string())
            .bind("c", proof.c.to_bytes().to_vec())
            .bind(
                "witness",
                proof.witness.and_then(|w| serde_json::to_string(&w).inspect_err(|e| tracing::error!("Failed to serialize witness: {:?}", e)).ok()),
            )
            .bind("state", "UNSPENT".to_string())
            .bind("quote_id", quote_id.clone().map(|q| q.to_string()))
            .bind("created_time", current_time as i64)
            .bind("operation_kind", operation.kind().to_string())
            .bind("operation_id", operation.id().to_string())
            .execute(&self.inner)
            .await?;
        }

        Ok(())
    }

    async fn update_proofs_states(
        &mut self,
        ys: &[PublicKey],
        new_state: State,
    ) -> Result<Vec<Option<State>>, Self::Err> {
        let mut current_states = get_current_states(&self.inner, ys, true).await?;

        if current_states.len() != ys.len() {
            tracing::warn!(
                "Attempted to update state of non-existent proof {} {}",
                current_states.len(),
                ys.len()
            );
            return Err(database::Error::ProofNotFound);
        }

        query(r#"UPDATE proof SET state = :new_state WHERE y IN (:ys)"#)?
            .bind("new_state", new_state.to_string())
            .bind_vec("ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
            .execute(&self.inner)
            .await?;

        if new_state == State::Spent {
            query(
                r#"
                INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed)
                SELECT keyset_id, 0, COALESCE(SUM(amount), 0)
                FROM proof
                WHERE y IN (:ys)
                GROUP BY keyset_id
                ON CONFLICT (keyset_id)
                DO UPDATE SET total_redeemed = keyset_amounts.total_redeemed + EXCLUDED.total_redeemed
                "#,
            )?
            .bind_vec("ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
            .execute(&self.inner)
            .await?;
        }

        Ok(ys.iter().map(|y| current_states.remove(y)).collect())
    }

    async fn remove_proofs(
        &mut self,
        ys: &[PublicKey],
        _quote_id: Option<QuoteId>,
    ) -> Result<(), Self::Err> {
        if ys.is_empty() {
            return Ok(());
        }
        let total_deleted = query(
            r#"
            DELETE FROM proof WHERE y IN (:ys) AND state NOT IN (:exclude_state)
            "#,
        )?
        .bind_vec("ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
        .bind_vec("exclude_state", vec![State::Spent.to_string()])
        .execute(&self.inner)
        .await?;

        if total_deleted != ys.len() {
            // Query current states to provide detailed logging
            let current_states = get_current_states(&self.inner, ys, true).await?;

            let missing_count = ys.len() - current_states.len();
            let spent_count = current_states
                .values()
                .filter(|s| **s == State::Spent)
                .count();

            if missing_count > 0 {
                tracing::warn!(
                    "remove_proofs: {} of {} proofs do not exist in database (already removed?)",
                    missing_count,
                    ys.len()
                );
            }

            if spent_count > 0 {
                tracing::warn!(
                    "remove_proofs: {} of {} proofs are in Spent state and cannot be removed",
                    spent_count,
                    ys.len()
                );
            }

            tracing::debug!(
                "remove_proofs details: requested={}, deleted={}, missing={}, spent={}",
                ys.len(),
                total_deleted,
                missing_count,
                spent_count
            );

            return Err(Self::Err::AttemptRemoveSpentProof);
        }

        Ok(())
    }

    async fn get_proof_ys_by_quote_id(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Vec<PublicKey>, Self::Err> {
        Ok(query(
            r#"
            SELECT
                amount,
                keyset_id,
                secret,
                c,
                witness
            FROM
                proof
            WHERE
                quote_id = :quote_id
            FOR UPDATE
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .map(sql_row_to_proof)
        .collect::<Result<Vec<Proof>, _>>()?
        .ys()?)
    }

    async fn get_proof_ys_by_operation_id(
        &mut self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<PublicKey>, Self::Err> {
        Ok(query(
            r#"
            SELECT
                y
            FROM
                proof
            WHERE
                operation_id = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .map(|row| -> Result<PublicKey, Error> {
            Ok(column_as_string!(
                &row[0],
                PublicKey::from_hex,
                PublicKey::from_slice
            ))
        })
        .collect::<Result<Vec<_>, _>>()?)
    }

    async fn get_proofs_states(
        &mut self,
        ys: &[PublicKey],
    ) -> Result<Vec<Option<State>>, Self::Err> {
        let mut current_states = get_current_states(&self.inner, ys, true).await?;

        Ok(ys.iter().map(|y| current_states.remove(y)).collect())
    }
}

#[async_trait]
impl<RM> MintProofsDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_proofs_by_ys(&self, ys: &[PublicKey]) -> Result<Vec<Option<Proof>>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        let mut proofs = query(
            r#"
            SELECT
                amount,
                keyset_id,
                secret,
                c,
                witness,
                y
            FROM
                proof
            WHERE
                y IN (:ys)
            "#,
        )?
        .bind_vec("ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(|mut row| {
            Ok((
                column_as_string!(
                    row.pop().ok_or(Error::InvalidDbResponse)?,
                    PublicKey::from_hex,
                    PublicKey::from_slice
                ),
                sql_row_to_proof(row)?,
            ))
        })
        .collect::<Result<HashMap<_, _>, Error>>()?;

        Ok(ys.iter().map(|y| proofs.remove(y)).collect())
    }

    async fn get_proof_ys_by_quote_id(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Vec<PublicKey>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                amount,
                keyset_id,
                secret,
                c,
                witness
            FROM
                proof
            WHERE
                quote_id = :quote_id
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_proof)
        .collect::<Result<Vec<Proof>, _>>()?
        .ys()?)
    }

    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        let mut current_states = get_current_states(&*conn, ys, false).await?;

        Ok(ys.iter().map(|y| current_states.remove(y)).collect())
    }

    async fn get_proofs_by_keyset_id(
        &self,
        keyset_id: &Id,
    ) -> Result<(Proofs, Vec<Option<State>>), Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
               keyset_id,
               amount,
               secret,
               c,
               witness,
               state
            FROM
                proof
            WHERE
                keyset_id=:keyset_id
            "#,
        )?
        .bind("keyset_id", keyset_id.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_proof_with_state)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .unzip())
    }

    /// Get total proofs redeemed by keyset id
    async fn get_total_redeemed(&self) -> Result<HashMap<Id, Amount>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT
                keyset_id,
                total_redeemed as amount
            FROM
                keyset_amounts
        "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_hashmap_amount)
        .collect()
    }

    async fn get_proof_ys_by_operation_id(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<PublicKey>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT
                y
            FROM
                proof
            WHERE
                operation_id = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(|row| -> Result<PublicKey, Error> {
            Ok(column_as_string!(
                &row[0],
                PublicKey::from_hex,
                PublicKey::from_slice
            ))
        })
        .collect::<Result<Vec<_>, _>>()
    }
}

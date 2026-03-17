//! Signatures database implementation

use std::collections::HashMap;
use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::database::mint::{
    BlindSignatureFilter, BlindSignatureListResult, BlindSignatureRecord,
};
use cdk_common::database::{self, Error, MintSignatureTransaction, MintSignaturesDatabase};
use cdk_common::quote_id::QuoteId;
use cdk_common::util::unix_time;
use cdk_common::{Amount, BlindSignature, BlindSignatureDleq, Id, PublicKey, SecretKey};

use super::filters::{
    apply_pagination_peek_ahead, bind_date_range, bind_keyset_ids, bind_operations, bind_units,
    build_pagination_clause, build_where_clause, order_direction,
};
use super::proofs::sql_row_to_hashmap_amount;
use super::{SQLMintDatabase, SQLTransaction};
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{
    column_as_nullable_number, column_as_nullable_string, column_as_number, column_as_string,
    unpack_into,
};

pub(crate) fn sql_row_to_blind_signature(row: Vec<Column>) -> Result<BlindSignature, Error> {
    unpack_into!(
        let (
            keyset_id, amount, c, dleq_e, dleq_s
        ) = row
    );

    let dleq = match (
        column_as_nullable_string!(dleq_e),
        column_as_nullable_string!(dleq_s),
    ) {
        (Some(e), Some(s)) => Some(BlindSignatureDleq {
            e: SecretKey::from_hex(e)?,
            s: SecretKey::from_hex(s)?,
        }),
        _ => None,
    };

    let amount: u64 = column_as_number!(amount);

    Ok(BlindSignature {
        amount: Amount::from(amount),
        keyset_id: column_as_string!(keyset_id, Id::from_str, Id::from_bytes),
        c: column_as_string!(c, PublicKey::from_hex, PublicKey::from_slice),
        dleq,
    })
}

fn sql_row_to_blind_signature_record(row: Vec<Column>) -> Result<BlindSignatureRecord, Error> {
    unpack_into!(
        let (
            amount, keyset_id, quote_id, created_time, signed_time, operation_kind, operation_id
        ) = row
    );

    let amount: u64 = column_as_number!(amount);
    let created_time: u64 = column_as_number!(created_time);
    let signed_time: Option<u64> = column_as_nullable_number!(signed_time);

    Ok(BlindSignatureRecord {
        amount: Amount::from(amount),
        keyset_id: column_as_string!(keyset_id, Id::from_str, Id::from_bytes),
        quote_id: column_as_nullable_string!(quote_id),
        created_time,
        signed_time,
        operation_kind: column_as_nullable_string!(operation_kind),
        operation_id: column_as_nullable_string!(operation_id),
    })
}

#[async_trait]
impl<RM> MintSignatureTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_blind_signatures(
        &mut self,
        blinded_messages: &[PublicKey],
        blind_signatures: &[BlindSignature],
        quote_id: Option<QuoteId>,
    ) -> Result<(), Self::Err> {
        let current_time = unix_time();

        if blinded_messages.len() != blind_signatures.len() {
            return Err(database::Error::Internal(
                "Mismatched array lengths for blinded messages and blind signatures".to_string(),
            ));
        }

        // Select all existing rows for the given blinded messages at once
        let mut existing_rows = query(
            r#"
            SELECT blinded_message, c, dleq_e, dleq_s
            FROM blind_signature
            WHERE blinded_message IN (:blinded_messages)
            FOR UPDATE
            "#,
        )?
        .bind_vec(
            "blinded_messages",
            blinded_messages
                .iter()
                .map(|message| message.to_bytes().to_vec())
                .collect(),
        )
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .map(|mut row| {
            Ok((
                column_as_string!(&row.remove(0), PublicKey::from_hex, PublicKey::from_slice),
                (row[0].clone(), row[1].clone(), row[2].clone()),
            ))
        })
        .collect::<Result<HashMap<_, _>, Error>>()?;

        // Iterate over the provided blinded messages and signatures
        for (message, signature) in blinded_messages.iter().zip(blind_signatures) {
            match existing_rows.remove(message) {
                None => {
                    // Unknown blind message: Insert new row with all columns
                    query(
                        r#"
                        INSERT INTO blind_signature
                        (blinded_message, amount, keyset_id, c, quote_id, dleq_e, dleq_s, created_time, signed_time)
                        VALUES
                        (:blinded_message, :amount, :keyset_id, :c, :quote_id, :dleq_e, :dleq_s, :created_time, :signed_time)
                        "#,
                    )?
                    .bind("blinded_message", message.to_bytes().to_vec())
                    .bind("amount", u64::from(signature.amount) as i64)
                    .bind("keyset_id", signature.keyset_id.to_string())
                    .bind("c", signature.c.to_bytes().to_vec())
                    .bind("quote_id", quote_id.as_ref().map(|q| q.to_string()))
                    .bind(
                        "dleq_e",
                        signature.dleq.as_ref().map(|dleq| dleq.e.to_secret_hex()),
                    )
                    .bind(
                        "dleq_s",
                        signature.dleq.as_ref().map(|dleq| dleq.s.to_secret_hex()),
                    )
                    .bind("created_time", current_time as i64)
                    .bind("signed_time", current_time as i64)
                    .execute(&self.inner)
                    .await?;

                    query(
                        r#"
                        INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed)
                        VALUES (:keyset_id, :amount, 0)
                        ON CONFLICT (keyset_id)
                        DO UPDATE SET total_issued = keyset_amounts.total_issued + EXCLUDED.total_issued
                        "#,
                    )?
                    .bind("amount", u64::from(signature.amount) as i64)
                    .bind("keyset_id", signature.keyset_id.to_string())
                    .execute(&self.inner)
                    .await?;
                }
                Some((c, _dleq_e, _dleq_s)) => {
                    // Blind message exists: check if c is NULL
                    match c {
                        Column::Null => {
                            // Blind message with no c: Update with missing columns c, dleq_e, dleq_s
                            query(
                                r#"
                                UPDATE blind_signature
                                SET c = :c, dleq_e = :dleq_e, dleq_s = :dleq_s, signed_time = :signed_time, amount = :amount
                                WHERE blinded_message = :blinded_message
                                "#,
                            )?
                            .bind("c", signature.c.to_bytes().to_vec())
                            .bind(
                                "dleq_e",
                                signature.dleq.as_ref().map(|dleq| dleq.e.to_secret_hex()),
                            )
                            .bind(
                                "dleq_s",
                                signature.dleq.as_ref().map(|dleq| dleq.s.to_secret_hex()),
                            )
                            .bind("blinded_message", message.to_bytes().to_vec())
                            .bind("signed_time", current_time as i64)
                            .bind("amount", u64::from(signature.amount) as i64)
                            .execute(&self.inner)
                            .await?;

                            query(
                                r#"
                                INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed)
                                VALUES (:keyset_id, :amount, 0)
                                ON CONFLICT (keyset_id)
                                DO UPDATE SET total_issued = keyset_amounts.total_issued + EXCLUDED.total_issued
                                "#,
                            )?
                            .bind("amount", u64::from(signature.amount) as i64)
                            .bind("keyset_id", signature.keyset_id.to_string())
                            .execute(&self.inner)
                            .await?;
                        }
                        _ => {
                            // Blind message already has c: Error
                            tracing::error!(
                                "Attempting to add signature to message already signed {}",
                                message
                            );

                            return Err(database::Error::Duplicate);
                        }
                    }
                }
            }
        }

        debug_assert!(
            existing_rows.is_empty(),
            "Unexpected existing rows remain: {:?}",
            existing_rows.keys().collect::<Vec<_>>()
        );

        if !existing_rows.is_empty() {
            tracing::error!("Did not check all existing rows");
            return Err(Error::Internal(
                "Did not check all existing rows".to_string(),
            ));
        }

        Ok(())
    }

    async fn get_blind_signatures(
        &mut self,
        blinded_messages: &[PublicKey],
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err> {
        let mut blinded_signatures = query(
            r#"SELECT
                keyset_id,
                amount,
                c,
                dleq_e,
                dleq_s,
                blinded_message
            FROM
                blind_signature
            WHERE blinded_message IN (:b) AND c IS NOT NULL
            "#,
        )?
        .bind_vec(
            "b",
            blinded_messages
                .iter()
                .map(|b| b.to_bytes().to_vec())
                .collect(),
        )
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .map(|mut row| {
            Ok((
                column_as_string!(
                    &row.pop().ok_or(Error::InvalidDbResponse)?,
                    PublicKey::from_hex,
                    PublicKey::from_slice
                ),
                sql_row_to_blind_signature(row)?,
            ))
        })
        .collect::<Result<HashMap<_, _>, Error>>()?;
        Ok(blinded_messages
            .iter()
            .map(|y| blinded_signatures.remove(y))
            .collect())
    }
}

#[async_trait]
impl<RM> MintSignaturesDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        let mut blinded_signatures = query(
            r#"SELECT
                keyset_id,
                amount,
                c,
                dleq_e,
                dleq_s,
                blinded_message
            FROM
                blind_signature
            WHERE blinded_message IN (:b) AND c IS NOT NULL
            "#,
        )?
        .bind_vec(
            "b",
            blinded_messages
                .iter()
                .map(|b_| b_.to_bytes().to_vec())
                .collect(),
        )
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(|mut row| {
            Ok((
                column_as_string!(
                    &row.pop().ok_or(Error::InvalidDbResponse)?,
                    PublicKey::from_hex,
                    PublicKey::from_slice
                ),
                sql_row_to_blind_signature(row)?,
            ))
        })
        .collect::<Result<HashMap<_, _>, Error>>()?;
        Ok(blinded_messages
            .iter()
            .map(|y| blinded_signatures.remove(y))
            .collect())
    }

    async fn get_blind_signatures_for_keyset(
        &self,
        keyset_id: &Id,
    ) -> Result<Vec<BlindSignature>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                keyset_id,
                amount,
                c,
                dleq_e,
                dleq_s
            FROM
                blind_signature
            WHERE
                keyset_id=:keyset_id AND c IS NOT NULL
            "#,
        )?
        .bind("keyset_id", keyset_id.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_blind_signature)
        .collect::<Result<Vec<BlindSignature>, _>>()?)
    }

    /// Get [`BlindSignature`]s for quote
    async fn get_blind_signatures_for_quote(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Vec<BlindSignature>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                keyset_id,
                amount,
                c,
                dleq_e,
                dleq_s
            FROM
                blind_signature
            WHERE
                quote_id=:quote_id AND c IS NOT NULL
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_blind_signature)
        .collect::<Result<Vec<BlindSignature>, _>>()?)
    }

    /// Get total proofs redeemed by keyset id
    async fn get_total_issued(&self) -> Result<HashMap<Id, Amount>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT
                keyset_id,
                total_issued as amount
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

    async fn get_blinded_secrets_by_operation_id(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<PublicKey>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT
                blinded_message
            FROM
                blind_signature
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

    async fn list_blind_signatures_filtered(
        &self,
        filter: BlindSignatureFilter,
    ) -> Result<BlindSignatureListResult, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        // Build dynamic WHERE clauses
        let mut where_clauses: Vec<String> = Vec::new();
        let needs_keyset_join = !filter.units.is_empty();

        if filter.creation_date_start.is_some() {
            where_clauses.push("bs.created_time >= :creation_date_start".into());
        }
        if filter.creation_date_end.is_some() {
            where_clauses.push("bs.created_time <= :creation_date_end".into());
        }
        if !filter.keyset_ids.is_empty() {
            where_clauses.push("bs.keyset_id IN (:keyset_ids)".into());
        }
        if !filter.units.is_empty() {
            where_clauses.push("k.unit IN (:units)".into());
        }
        if !filter.operations.is_empty() {
            where_clauses.push("bs.operation_kind IN (:operations)".into());
        }

        let where_clause = build_where_clause(&where_clauses);
        let join_clause = if needs_keyset_join {
            "JOIN keyset k ON bs.keyset_id = k.id"
        } else {
            ""
        };
        let (limit_clause, requested_limit) = build_pagination_clause(filter.limit, filter.offset);

        let query_str = format!(
            r#"
            SELECT bs.amount, bs.keyset_id, bs.quote_id, bs.created_time,
                   bs.signed_time, bs.operation_kind, bs.operation_id
            FROM blind_signature bs
            {join_clause}
            {where_clause}
            ORDER BY bs.created_time {order}
            {limit_clause}
            "#,
            join_clause = join_clause,
            where_clause = where_clause,
            order = order_direction(filter.reversed),
            limit_clause = limit_clause,
        );

        let stmt = query(&query_str)?;
        let stmt = bind_date_range(stmt, filter.creation_date_start, filter.creation_date_end);
        let stmt = bind_keyset_ids(stmt, &filter.keyset_ids);
        let stmt = bind_units(stmt, &filter.units);
        let stmt = bind_operations(stmt, &filter.operations);

        let mut signatures = stmt
            .fetch_all(&*conn)
            .await?
            .into_iter()
            .map(sql_row_to_blind_signature_record)
            .collect::<Result<Vec<_>, _>>()?;

        let has_more = apply_pagination_peek_ahead(&mut signatures, requested_limit);

        Ok(BlindSignatureListResult {
            signatures,
            has_more,
        })
    }
}

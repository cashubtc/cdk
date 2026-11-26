//! Signatures database implementation

use std::collections::HashMap;
use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::database::{self, Error, MintSignatureTransaction, MintSignaturesDatabase};
use cdk_common::quote_id::QuoteId;
use cdk_common::util::unix_time;
use cdk_common::{Amount, BlindSignature, Id, PublicKey};

use super::proofs::sql_row_to_hashmap_amount;
use super::{SQLMintDatabase, SQLTransaction};
use crate::pool::DatabasePool;
use crate::stmt::{execute_rows, fetch_in_chunks, query, Column};
use crate::value::Value;
use crate::{column_as_number, column_as_string, unpack_into};

pub(crate) fn sql_row_to_blind_signature(row: Vec<Column>) -> Result<BlindSignature, Error> {
    unpack_into!(
        let (
            keyset_id, amount, c
        ) = row
    );

    let amount: u64 = column_as_number!(amount);

    Ok(BlindSignature {
        amount: Amount::from(amount),
        keyset_id: column_as_string!(keyset_id, Id::from_str, Id::from_bytes),
        c: column_as_string!(c, PublicKey::from_hex, PublicKey::from_slice),
        dleq: None,
    })
}

pub(crate) fn sql_row_to_blind_signature_and_secret(
    row: Vec<Column>,
) -> Result<(BlindSignature, PublicKey), Error> {
    unpack_into!(
        let (
            blinded_message, keyset_id, amount, c
        ) = row
    );

    let amount: u64 = column_as_number!(amount);

    Ok((
        BlindSignature {
            amount: Amount::from(amount),
            keyset_id: column_as_string!(keyset_id, Id::from_str, Id::from_bytes),
            c: column_as_string!(c, PublicKey::from_hex, PublicKey::from_slice),
            dleq: None,
        },
        column_as_string!(blinded_message, PublicKey::from_hex, PublicKey::from_slice),
    ))
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

        let mut locked_messages = blinded_messages
            .iter()
            .map(|message| message.to_bytes().to_vec())
            .collect::<Vec<_>>();
        locked_messages.sort_unstable();

        // Select all existing rows for the given blinded messages at once
        let mut existing_rows = fetch_in_chunks(
            &self.inner,
            r#"
            SELECT blinded_message, c
            FROM blind_signature
            WHERE blinded_message IN (:blinded_messages)
            ORDER BY blinded_message
            FOR UPDATE
            "#,
            "blinded_messages",
            locked_messages,
        )
        .await?
        .into_iter()
        .map(|mut row| {
            Ok((
                column_as_string!(&row.remove(0), PublicKey::from_hex, PublicKey::from_slice),
                (row[0].clone()),
            ))
        })
        .collect::<Result<HashMap<_, _>, Error>>()?;

        let mut ordered_signatures = blinded_messages
            .iter()
            .zip(blind_signatures)
            .enumerate()
            .collect::<Vec<_>>();
        ordered_signatures.sort_unstable_by(|(_, (left, _)), (_, (right, _))| {
            left.to_bytes().cmp(&right.to_bytes())
        });

        // Split by what the pre-read found, in blinded-message order, so each group becomes one
        // statement while keeping the request order index and the row lock order.
        let mut unknown = Vec::new();
        let mut reserved = Vec::new();

        for (i, (message, signature)) in ordered_signatures {
            match existing_rows.remove(message) {
                None => unknown.push((i, message, signature)),
                Some(Column::Null) => reserved.push((message, signature)),
                Some(_) => {
                    tracing::error!(
                        "Attempting to add signature to message already signed {}",
                        message
                    );

                    return Err(database::Error::Duplicate);
                }
            }
        }

        let rows = unknown
            .iter()
            .map(|(i, message, signature)| {
                Ok(vec![
                    Value::from(message.to_bytes().to_vec()),
                    Value::from(i64::try_from(u64::from(signature.amount))?),
                    Value::from(signature.keyset_id.to_string()),
                    Value::from(signature.c.to_bytes().to_vec()),
                    Value::from(quote_id.as_ref().map(|q| q.to_string())),
                    Value::from(i64::try_from(current_time)?),
                    Value::from(i64::try_from(current_time)?),
                    Value::from(i64::try_from(*i)?),
                ])
            })
            .collect::<Result<Vec<_>, Error>>()?;

        execute_rows(
            &self.inner,
            r#"
            INSERT INTO blind_signature
            (blinded_message, amount, keyset_id, c, quote_id, created_time, signed_time, order_index)
            VALUES :rows
            "#,
            "rows",
            rows,
        )
        .await?;

        for (message, signature) in &reserved {
            let affected = query(
                r#"
                UPDATE blind_signature
                SET c = :c, signed_time = :signed_time, amount = :amount
                WHERE blinded_message = :blinded_message AND c IS NULL
                "#,
            )?
            .bind("c", signature.c.to_bytes().to_vec())
            .bind("blinded_message", message.to_bytes().to_vec())
            .bind("signed_time", i64::try_from(current_time)?)
            .bind("amount", i64::try_from(u64::from(signature.amount))?)
            .execute(&self.inner)
            .await?;

            if affected != 1 {
                return Err(Error::ConcurrentUpdate);
            }
        }

        // Sum the rows just written, so one upsert per keyset replaces the per-row accumulation.
        // Must run after both groups, or a reservation's stale amount would be counted.
        let summed = fetch_in_chunks(
            &self.inner,
            r#"
            SELECT keyset_id, CAST(COALESCE(SUM(amount), 0) AS BIGINT)
            FROM blind_signature
            WHERE blinded_message IN (:blinded_messages)
            GROUP BY keyset_id
            "#,
            "blinded_messages",
            blinded_messages
                .iter()
                .map(|message| message.to_bytes().to_vec())
                .collect::<Vec<_>>(),
        )
        .await?;

        let mut issued_by_keyset: HashMap<Id, u64> = HashMap::new();

        for row in summed {
            let keyset_id: Id = column_as_string!(&row[0], Id::from_str, Id::from_bytes);
            let issued: u64 = column_as_number!(row[1].clone());

            *issued_by_keyset.entry(keyset_id).or_default() += issued;
        }

        let mut issued_in_keyset_lock_order = issued_by_keyset.into_iter().collect::<Vec<_>>();
        issued_in_keyset_lock_order.sort_unstable_by_key(|(keyset_id, _)| *keyset_id);

        let rows = issued_in_keyset_lock_order
            .into_iter()
            .map(|(keyset_id, issued)| {
                Ok(vec![
                    Value::from(keyset_id.to_string()),
                    Value::from(i64::try_from(issued)?),
                    Value::from(0_i64),
                ])
            })
            .collect::<Result<Vec<_>, Error>>()?;

        execute_rows(
            &self.inner,
            r#"
            INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed)
            VALUES :rows
            ON CONFLICT (keyset_id)
            DO UPDATE SET total_issued = keyset_amounts.total_issued + EXCLUDED.total_issued
            "#,
            "rows",
            rows,
        )
        .await?;

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
        if blinded_messages.is_empty() {
            return Ok(Vec::new());
        }

        let mut blinded_signatures = fetch_in_chunks(
            &self.inner,
            r#"SELECT
                keyset_id,
                amount,
                c,
                blinded_message
            FROM
                blind_signature
            WHERE blinded_message IN (:b) AND c IS NOT NULL
            "#,
            "b",
            blinded_messages
                .iter()
                .map(|b| b.to_bytes().to_vec())
                .collect::<Vec<_>>(),
        )
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
        if blinded_messages.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let mut blinded_signatures = fetch_in_chunks(
            &*conn,
            r#"SELECT
                keyset_id,
                amount,
                c,
                blinded_message
            FROM
                blind_signature
            WHERE blinded_message IN (:b) AND c IS NOT NULL
            "#,
            "b",
            blinded_messages
                .iter()
                .map(|b_| b_.to_bytes().to_vec())
                .collect::<Vec<_>>(),
        )
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
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                keyset_id,
                amount,
                c
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
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                keyset_id,
                amount,
                c
            FROM
                blind_signature
            WHERE
                quote_id=:quote_id AND c IS NOT NULL
            ORDER BY order_index ASC
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_blind_signature)
        .collect::<Result<Vec<BlindSignature>, _>>()?)
    }

    /// Get stored [`BlindSignature`]s and their corresponding blinded secrets (B′)
    /// for a quote, ordered by output index.
    async fn get_blind_signatures_and_secret_for_quote(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Vec<(BlindSignature, PublicKey)>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                blinded_message,
                keyset_id,
                amount,
                c
            FROM
                blind_signature
            WHERE
                quote_id=:quote_id AND c IS NOT NULL
            ORDER BY order_index ASC
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_blind_signature_and_secret)
        .collect::<Result<Vec<(BlindSignature, PublicKey)>, _>>()?)
    }

    /// Get total proofs redeemed by keyset id
    async fn get_total_issued(&self) -> Result<HashMap<Id, Amount>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
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
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
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
}

//! SQL database implementation of the Mint
//!
//! This is a generic SQL implementation for the mint storage layer. Any database can be plugged in
//! as long as standard ANSI SQL is used, as Postgres and SQLite would understand it.
//!
//! This implementation also has a rudimentary but standard migration and versioning system.
//!
//! The trait expects an asynchronous interaction, but it also provides tools to spawn blocking
//! clients in a pool and expose them to an asynchronous environment, making them compatible with
//! Mint.
use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use bitcoin::bip32::DerivationPath;
use cdk_common::database::mint::{validate_kvstore_params, SagaDatabase, SagaTransaction};
use cdk_common::database::{
    self, ConversionError, DbTransactionFinalizer, Error, MintDatabase, MintKeyDatabaseTransaction,
    MintKeysDatabase, MintProofsDatabase, MintQuotesDatabase, MintQuotesTransaction,
    MintSignatureTransaction, MintSignaturesDatabase,
};
use cdk_common::mint::{
    self, IncomingPayment, Issuance, MeltPaymentRequest, MeltQuote, MintKeySetInfo, MintQuote,
    Operation,
};
use cdk_common::nut00::ProofsMethods;
use cdk_common::payment::PaymentIdentifier;
use cdk_common::quote_id::QuoteId;
use cdk_common::secret::Secret;
use cdk_common::state::{check_melt_quote_state_transition, check_state_transition};
use cdk_common::util::unix_time;
use cdk_common::{
    Amount, BlindSignature, BlindSignatureDleq, BlindedMessage, CurrencyUnit, Id, MeltQuoteState,
    PaymentMethod, Proof, Proofs, PublicKey, SecretKey, State,
};
use lightning_invoice::Bolt11Invoice;
use migrations::MIGRATIONS;
use tracing::instrument;

use crate::common::migrate;
use crate::database::{ConnectionWithTransaction, DatabaseExecutor};
use crate::pool::{DatabasePool, Pool, PooledResource};
use crate::stmt::{query, Column};
use crate::{
    column_as_nullable_number, column_as_nullable_string, column_as_number, column_as_string,
    unpack_into,
};

#[cfg(feature = "auth")]
mod auth;

#[rustfmt::skip]
mod migrations {
    include!(concat!(env!("OUT_DIR"), "/migrations_mint.rs"));
}

#[cfg(feature = "auth")]
pub use auth::SQLMintAuthDatabase;
#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;

/// Mint SQL Database
#[derive(Debug, Clone)]
pub struct SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    pool: Arc<Pool<RM>>,
}

/// SQL Transaction Writer
pub struct SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    inner: ConnectionWithTransaction<RM::Connection, PooledResource<RM>>,
}

#[inline(always)]
async fn get_current_states<C>(
    conn: &C,
    ys: &[PublicKey],
) -> Result<HashMap<PublicKey, State>, Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    if ys.is_empty() {
        return Ok(Default::default());
    }
    query(r#"SELECT y, state FROM proof WHERE y IN (:ys)"#)?
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

impl<RM> SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    /// Creates a new instance
    pub async fn new<X>(db: X) -> Result<Self, Error>
    where
        X: Into<RM::Config>,
    {
        let pool = Pool::new(db.into());

        Self::migrate(pool.get().map_err(|e| Error::Database(Box::new(e)))?).await?;

        Ok(Self { pool })
    }

    /// Migrate
    async fn migrate(conn: PooledResource<RM>) -> Result<(), Error> {
        let tx = ConnectionWithTransaction::new(conn).await?;
        migrate(&tx, RM::Connection::name(), MIGRATIONS).await?;
        tx.commit().await?;
        Ok(())
    }
}

#[async_trait]
impl<RM> database::MintProofsTransaction<'_> for SQLTransaction<RM>
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
        let current_time = unix_time();

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
            query(
                r#"
                  INSERT INTO proof
                  (y, amount, keyset_id, secret, c, witness, state, quote_id, created_time, operation_kind, operation_id)
                  VALUES
                  (:y, :amount, :keyset_id, :secret, :c, :witness, :state, :quote_id, :created_time, :operation_kind, :operation_id)
                  "#,
            )?
            .bind("y", proof.y()?.to_bytes().to_vec())
            .bind("amount", proof.amount.to_i64())
            .bind("keyset_id", proof.keyset_id.to_string())
            .bind("secret", proof.secret.to_string())
            .bind("c", proof.c.to_bytes().to_vec())
            .bind(
                "witness",
                proof.witness.map(|w| serde_json::to_string(&w).unwrap()),
            )
            .bind("state", "UNSPENT".to_string())
            .bind("quote_id", quote_id.clone().map(|q| q.to_string()))
            .bind("created_time", current_time as i64)
            .bind("operation_kind", operation.kind())
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
        let mut current_states = get_current_states(&self.inner, ys).await?;

        if current_states.len() != ys.len() {
            tracing::warn!(
                "Attempted to update state of non-existent proof {} {}",
                current_states.len(),
                ys.len()
            );
            return Err(database::Error::ProofNotFound);
        }

        for state in current_states.values() {
            check_state_transition(*state, new_state)?;
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
            return Err(Self::Err::AttemptRemoveSpentProof);
        }

        Ok(())
    }

    async fn get_proof_ys_by_quote_id(
        &self,
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
}

#[async_trait]
impl<RM> database::MintTransaction<'_, Error> for SQLTransaction<RM> where RM: DatabasePool + 'static
{}

#[async_trait]
impl<RM> DbTransactionFinalizer for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn commit(self: Box<Self>) -> Result<(), Error> {
        let result = self.inner.commit().await;
        #[cfg(feature = "prometheus")]
        {
            let success = result.is_ok();
            METRICS.record_mint_operation("transaction_commit", success);
            METRICS.record_mint_operation_histogram("transaction_commit", success, 1.0);
        }

        Ok(result?)
    }

    async fn rollback(self: Box<Self>) -> Result<(), Error> {
        let result = self.inner.rollback().await;

        #[cfg(feature = "prometheus")]
        {
            let success = result.is_ok();
            METRICS.record_mint_operation("transaction_rollback", success);
            METRICS.record_mint_operation_histogram("transaction_rollback", success, 1.0);
        }
        Ok(result?)
    }
}

#[inline(always)]
async fn get_mint_quote_payments<C>(
    conn: &C,
    quote_id: &QuoteId,
) -> Result<Vec<IncomingPayment>, Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    // Get payment IDs and timestamps from the mint_quote_payments table
    query(
        r#"
        SELECT
            payment_id,
            timestamp,
            amount
        FROM
            mint_quote_payments
        WHERE
            quote_id=:quote_id
        "#,
    )?
    .bind("quote_id", quote_id.to_string())
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|row| {
        let amount: u64 = column_as_number!(row[2].clone());
        let time: u64 = column_as_number!(row[1].clone());
        Ok(IncomingPayment::new(
            amount.into(),
            column_as_string!(&row[0]),
            time,
        ))
    })
    .collect()
}

#[inline(always)]
async fn get_mint_quote_issuance<C>(conn: &C, quote_id: &QuoteId) -> Result<Vec<Issuance>, Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    // Get payment IDs and timestamps from the mint_quote_payments table
    query(
        r#"
SELECT amount, timestamp
FROM mint_quote_issued
WHERE quote_id=:quote_id
            "#,
    )?
    .bind("quote_id", quote_id.to_string())
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|row| {
        let time: u64 = column_as_number!(row[1].clone());
        Ok(Issuance::new(
            Amount::from_i64(column_as_number!(row[0].clone()))
                .expect("Is amount when put into db"),
            time,
        ))
    })
    .collect()
}

#[async_trait]
impl<RM> MintKeyDatabaseTransaction<'_, Error> for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    async fn add_keyset_info(&mut self, keyset: MintKeySetInfo) -> Result<(), Error> {
        query(
            r#"
        INSERT INTO
            keyset (
                id, unit, active, valid_from, valid_to, derivation_path,
                max_order, amounts, input_fee_ppk, derivation_path_index
            )
        VALUES (
            :id, :unit, :active, :valid_from, :valid_to, :derivation_path,
            :max_order, :amounts, :input_fee_ppk, :derivation_path_index
        )
        ON CONFLICT(id) DO UPDATE SET
            unit = excluded.unit,
            active = excluded.active,
            valid_from = excluded.valid_from,
            valid_to = excluded.valid_to,
            derivation_path = excluded.derivation_path,
            max_order = excluded.max_order,
            amounts = excluded.amounts,
            input_fee_ppk = excluded.input_fee_ppk,
            derivation_path_index = excluded.derivation_path_index
        "#,
        )?
        .bind("id", keyset.id.to_string())
        .bind("unit", keyset.unit.to_string())
        .bind("active", keyset.active)
        .bind("valid_from", keyset.valid_from as i64)
        .bind("valid_to", keyset.final_expiry.map(|v| v as i64))
        .bind("derivation_path", keyset.derivation_path.to_string())
        .bind("max_order", keyset.max_order)
        .bind("amounts", serde_json::to_string(&keyset.amounts).ok())
        .bind("input_fee_ppk", keyset.input_fee_ppk as i64)
        .bind("derivation_path_index", keyset.derivation_path_index)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn set_active_keyset(&mut self, unit: CurrencyUnit, id: Id) -> Result<(), Error> {
        query(r#"UPDATE keyset SET active=FALSE WHERE unit = :unit"#)?
            .bind("unit", unit.to_string())
            .execute(&self.inner)
            .await?;

        query(r#"UPDATE keyset SET active=TRUE WHERE unit = :unit AND id = :id"#)?
            .bind("unit", unit.to_string())
            .bind("id", id.to_string())
            .execute(&self.inner)
            .await?;

        Ok(())
    }
}

#[async_trait]
impl<RM> MintKeysDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn MintKeyDatabaseTransaction<'a, Error> + Send + Sync + 'a>, Error> {
        let tx = SQLTransaction {
            inner: ConnectionWithTransaction::new(
                self.pool.get().map_err(|e| Error::Database(Box::new(e)))?,
            )
            .await?,
        };

        Ok(Box::new(tx))
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(
            query(r#" SELECT id FROM keyset WHERE active = :active AND unit = :unit"#)?
                .bind("active", true)
                .bind("unit", unit.to_string())
                .pluck(&*conn)
                .await?
                .map(|id| match id {
                    Column::Text(text) => Ok(Id::from_str(&text)?),
                    Column::Blob(id) => Ok(Id::from_bytes(&id)?),
                    _ => Err(Error::InvalidKeysetId),
                })
                .transpose()?,
        )
    }

    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(
            query(r#"SELECT id, unit FROM keyset WHERE active = :active"#)?
                .bind("active", true)
                .fetch_all(&*conn)
                .await?
                .into_iter()
                .map(|row| {
                    Ok((
                        column_as_string!(&row[1], CurrencyUnit::from_str),
                        column_as_string!(&row[0], Id::from_str, Id::from_bytes),
                    ))
                })
                .collect::<Result<HashMap<_, _>, Error>>()?,
        )
    }

    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"SELECT
                id,
                unit,
                active,
                valid_from,
                valid_to,
                derivation_path,
                derivation_path_index,
                max_order,
                amounts,
                input_fee_ppk
            FROM
                keyset
                WHERE id=:id"#,
        )?
        .bind("id", id.to_string())
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_keyset_info)
        .transpose()?)
    }

    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"SELECT
                id,
                unit,
                active,
                valid_from,
                valid_to,
                derivation_path,
                derivation_path_index,
                max_order,
                amounts,
                input_fee_ppk
            FROM
                keyset
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_keyset_info)
        .collect::<Result<Vec<_>, _>>()?)
    }
}

#[async_trait]
impl<RM> MintQuotesTransaction<'_> for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_melt_request(
        &mut self,
        quote_id: &QuoteId,
        inputs_amount: Amount,
        inputs_fee: Amount,
    ) -> Result<(), Self::Err> {
        // Insert melt_request
        query(
            r#"
            INSERT INTO melt_request
            (quote_id, inputs_amount, inputs_fee)
            VALUES
            (:quote_id, :inputs_amount, :inputs_fee)
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .bind("inputs_amount", inputs_amount.to_i64())
        .bind("inputs_fee", inputs_fee.to_i64())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_blinded_messages(
        &mut self,
        quote_id: Option<&QuoteId>,
        blinded_messages: &[BlindedMessage],
        operation: &Operation,
    ) -> Result<(), Self::Err> {
        let current_time = unix_time();

        // Insert blinded_messages directly into blind_signature with c = NULL
        // Let the database constraint handle duplicate detection
        for message in blinded_messages {
            match query(
                r#"
                INSERT INTO blind_signature
                (blinded_message, amount, keyset_id, c, quote_id, created_time, operation_kind, operation_id)
                VALUES
                (:blinded_message, :amount, :keyset_id, NULL, :quote_id, :created_time, :operation_kind, :operation_id)
                "#,
            )?
            .bind(
                "blinded_message",
                message.blinded_secret.to_bytes().to_vec(),
            )
            .bind("amount", message.amount.to_i64())
            .bind("keyset_id", message.keyset_id.to_string())
            .bind("quote_id", quote_id.map(|q| q.to_string()))
            .bind("created_time", current_time as i64)
            .bind("operation_kind", operation.kind())
            .bind("operation_id", operation.id().to_string())
            .execute(&self.inner)
            .await
            {
                Ok(_) => continue,
                Err(database::Error::Duplicate) => {
                    // Primary key constraint violation - blinded message already exists
                    // This could be either:
                    // 1. Already signed (c IS NOT NULL) - definitely an error
                    // 2. Already pending (c IS NULL) - also an error
                    return Err(database::Error::Duplicate);
                }
                Err(err) => return Err(err),
            }
        }

        Ok(())
    }

    async fn delete_blinded_messages(
        &mut self,
        blinded_secrets: &[PublicKey],
    ) -> Result<(), Self::Err> {
        if blinded_secrets.is_empty() {
            return Ok(());
        }

        // Delete blinded messages from blind_signature table where c IS NULL
        // (only delete unsigned blinded messages)
        query(
            r#"
            DELETE FROM blind_signature
            WHERE blinded_message IN (:blinded_secrets) AND c IS NULL
            "#,
        )?
        .bind_vec(
            "blinded_secrets",
            blinded_secrets
                .iter()
                .map(|secret| secret.to_bytes().to_vec())
                .collect(),
        )
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn get_melt_request_and_blinded_messages(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Option<database::mint::MeltRequestInfo>, Self::Err> {
        let melt_request_row = query(
            r#"
            SELECT inputs_amount, inputs_fee
            FROM melt_request
            WHERE quote_id = :quote_id
            FOR UPDATE
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .fetch_one(&self.inner)
        .await?;

        if let Some(row) = melt_request_row {
            let inputs_amount: u64 = column_as_number!(row[0].clone());
            let inputs_fee: u64 = column_as_number!(row[1].clone());

            // Get blinded messages from blind_signature table where c IS NULL
            let blinded_messages_rows = query(
                r#"
                SELECT blinded_message, keyset_id, amount
                FROM blind_signature
                WHERE quote_id = :quote_id AND c IS NULL
                "#,
            )?
            .bind("quote_id", quote_id.to_string())
            .fetch_all(&self.inner)
            .await?;

            let blinded_messages: Result<Vec<BlindedMessage>, Error> = blinded_messages_rows
                .into_iter()
                .map(|row| -> Result<BlindedMessage, Error> {
                    let blinded_message_key =
                        column_as_string!(&row[0], PublicKey::from_hex, PublicKey::from_slice);
                    let keyset_id = column_as_string!(&row[1], Id::from_str, Id::from_bytes);
                    let amount: u64 = column_as_number!(row[2].clone());

                    Ok(BlindedMessage {
                        blinded_secret: blinded_message_key,
                        keyset_id,
                        amount: Amount::from(amount),
                        witness: None, // Not storing witness in database currently
                    })
                })
                .collect();
            let blinded_messages = blinded_messages?;

            Ok(Some(database::mint::MeltRequestInfo {
                inputs_amount: Amount::from(inputs_amount),
                inputs_fee: Amount::from(inputs_fee),
                change_outputs: blinded_messages,
            }))
        } else {
            Ok(None)
        }
    }

    async fn delete_melt_request(&mut self, quote_id: &QuoteId) -> Result<(), Self::Err> {
        // Delete from melt_request table
        query(
            r#"
            DELETE FROM melt_request
            WHERE quote_id = :quote_id
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .execute(&self.inner)
        .await?;

        // Also delete blinded messages (where c IS NULL) from blind_signature table
        query(
            r#"
            DELETE FROM blind_signature
            WHERE quote_id = :quote_id AND c IS NULL
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn increment_mint_quote_amount_paid(
        &mut self,
        quote_id: &QuoteId,
        amount_paid: Amount,
        payment_id: String,
    ) -> Result<Amount, Self::Err> {
        if amount_paid == Amount::ZERO {
            tracing::warn!("Amount payments of zero amount should not be recorded.");
            return Err(Error::Duplicate);
        }

        // Check if payment_id already exists in mint_quote_payments
        let exists = query(
            r#"
            SELECT payment_id
            FROM mint_quote_payments
            WHERE payment_id = :payment_id
            FOR UPDATE
            "#,
        )?
        .bind("payment_id", payment_id.clone())
        .fetch_one(&self.inner)
        .await?;

        if exists.is_some() {
            tracing::error!("Payment ID already exists: {}", payment_id);
            return Err(database::Error::Duplicate);
        }

        // Get current amount_paid from quote
        let current_amount = query(
            r#"
            SELECT amount_paid
            FROM mint_quote
            WHERE id = :quote_id
            FOR UPDATE
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .fetch_one(&self.inner)
        .await
        .inspect_err(|err| {
            tracing::error!("SQLite could not get mint quote amount_paid: {}", err);
        })?;

        let current_amount_paid = if let Some(current_amount) = current_amount {
            let amount: u64 = column_as_number!(current_amount[0].clone());
            Amount::from(amount)
        } else {
            Amount::ZERO
        };

        // Calculate new amount_paid with overflow check
        let new_amount_paid = current_amount_paid
            .checked_add(amount_paid)
            .ok_or_else(|| database::Error::AmountOverflow)?;

        tracing::debug!(
            "Mint quote {} amount paid was {} is now {}.",
            quote_id,
            current_amount_paid,
            new_amount_paid
        );

        // Update the amount_paid
        query(
            r#"
            UPDATE mint_quote
            SET amount_paid = :amount_paid
            WHERE id = :quote_id
            "#,
        )?
        .bind("amount_paid", new_amount_paid.to_i64())
        .bind("quote_id", quote_id.to_string())
        .execute(&self.inner)
        .await
        .inspect_err(|err| {
            tracing::error!("SQLite could not update mint quote amount_paid: {}", err);
        })?;

        // Add payment_id to mint_quote_payments table
        query(
            r#"
            INSERT INTO mint_quote_payments
            (quote_id, payment_id, amount, timestamp)
            VALUES (:quote_id, :payment_id, :amount, :timestamp)
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .bind("payment_id", payment_id)
        .bind("amount", amount_paid.to_i64())
        .bind("timestamp", unix_time() as i64)
        .execute(&self.inner)
        .await
        .map_err(|err| {
            tracing::error!("SQLite could not insert payment ID: {}", err);
            err
        })?;

        Ok(new_amount_paid)
    }

    #[instrument(skip_all)]
    async fn increment_mint_quote_amount_issued(
        &mut self,
        quote_id: &QuoteId,
        amount_issued: Amount,
    ) -> Result<Amount, Self::Err> {
        // Get current amount_issued from quote
        let current_amounts = query(
            r#"
            SELECT amount_issued, amount_paid
            FROM mint_quote
            WHERE id = :quote_id
            FOR UPDATE
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .fetch_one(&self.inner)
        .await
        .inspect_err(|err| {
            tracing::error!("SQLite could not get mint quote amount_issued: {}", err);
        })?
        .ok_or(Error::QuoteNotFound)?;

        let new_amount_issued = {
            // Make sure the db protects issuing not paid quotes
            unpack_into!(
                let (current_amount_issued, current_amount_paid) = current_amounts
            );

            let current_amount_issued: u64 = column_as_number!(current_amount_issued);
            let current_amount_paid: u64 = column_as_number!(current_amount_paid);

            let current_amount_issued = Amount::from(current_amount_issued);
            let current_amount_paid = Amount::from(current_amount_paid);

            // Calculate new amount_issued with overflow check
            let new_amount_issued = current_amount_issued
                .checked_add(amount_issued)
                .ok_or_else(|| database::Error::AmountOverflow)?;

            current_amount_paid
                .checked_sub(new_amount_issued)
                .ok_or(Error::Internal("Over-issued not allowed".to_owned()))?;

            new_amount_issued
        };

        // Update the amount_issued
        query(
            r#"
            UPDATE mint_quote
            SET amount_issued = :amount_issued
            WHERE id = :quote_id
            "#,
        )?
        .bind("amount_issued", new_amount_issued.to_i64())
        .bind("quote_id", quote_id.to_string())
        .execute(&self.inner)
        .await
        .inspect_err(|err| {
            tracing::error!("SQLite could not update mint quote amount_issued: {}", err);
        })?;

        let current_time = unix_time();

        query(
            r#"
INSERT INTO mint_quote_issued
(quote_id, amount, timestamp)
VALUES (:quote_id, :amount, :timestamp);
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .bind("amount", amount_issued.to_i64())
        .bind("timestamp", current_time as i64)
        .execute(&self.inner)
        .await?;

        Ok(new_amount_issued)
    }

    #[instrument(skip_all)]
    async fn add_mint_quote(&mut self, quote: MintQuote) -> Result<(), Self::Err> {
        query(
            r#"
                INSERT INTO mint_quote (
                id, amount, unit, request, expiry, request_lookup_id, pubkey, created_time, payment_method, request_lookup_id_kind
                )
                VALUES (
                :id, :amount, :unit, :request, :expiry, :request_lookup_id, :pubkey, :created_time, :payment_method, :request_lookup_id_kind
                )
            "#,
        )?
        .bind("id", quote.id.to_string())
        .bind("amount", quote.amount.map(|a| a.to_i64()))
        .bind("unit", quote.unit.to_string())
        .bind("request", quote.request)
        .bind("expiry", quote.expiry as i64)
        .bind(
            "request_lookup_id",
            quote.request_lookup_id.to_string(),
        )
        .bind("pubkey", quote.pubkey.map(|p| p.to_string()))
        .bind("created_time", quote.created_time as i64)
        .bind("payment_method", quote.payment_method.to_string())
        .bind("request_lookup_id_kind", quote.request_lookup_id.kind())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_melt_quote(&mut self, quote: mint::MeltQuote) -> Result<(), Self::Err> {
        // Now insert the new quote
        query(
            r#"
            INSERT INTO melt_quote
            (
                id, unit, amount, request, fee_reserve, state,
                expiry, payment_preimage, request_lookup_id,
                created_time, paid_time, options, request_lookup_id_kind, payment_method
            )
            VALUES
            (
                :id, :unit, :amount, :request, :fee_reserve, :state,
                :expiry, :payment_preimage, :request_lookup_id,
                :created_time, :paid_time, :options, :request_lookup_id_kind, :payment_method
            )
        "#,
        )?
        .bind("id", quote.id.to_string())
        .bind("unit", quote.unit.to_string())
        .bind("amount", quote.amount.to_i64())
        .bind("request", serde_json::to_string(&quote.request)?)
        .bind("fee_reserve", quote.fee_reserve.to_i64())
        .bind("state", quote.state.to_string())
        .bind("expiry", quote.expiry as i64)
        .bind("payment_preimage", quote.payment_preimage)
        .bind(
            "request_lookup_id",
            quote.request_lookup_id.as_ref().map(|id| id.to_string()),
        )
        .bind("created_time", quote.created_time as i64)
        .bind("paid_time", quote.paid_time.map(|t| t as i64))
        .bind(
            "options",
            quote.options.map(|o| serde_json::to_string(&o).ok()),
        )
        .bind(
            "request_lookup_id_kind",
            quote.request_lookup_id.map(|id| id.kind()),
        )
        .bind("payment_method", quote.payment_method.to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn update_melt_quote_request_lookup_id(
        &mut self,
        quote_id: &QuoteId,
        new_request_lookup_id: &PaymentIdentifier,
    ) -> Result<(), Self::Err> {
        query(r#"UPDATE melt_quote SET request_lookup_id = :new_req_id, request_lookup_id_kind = :new_kind WHERE id = :id"#)?
            .bind("new_req_id", new_request_lookup_id.to_string())
            .bind("new_kind",new_request_lookup_id.kind() )
            .bind("id", quote_id.to_string())
            .execute(&self.inner)
            .await?;
        Ok(())
    }

    async fn update_melt_quote_state(
        &mut self,
        quote_id: &QuoteId,
        state: MeltQuoteState,
        payment_proof: Option<String>,
    ) -> Result<(MeltQuoteState, mint::MeltQuote), Self::Err> {
        let mut quote = query(
            r#"
            SELECT
                id,
                unit,
                amount,
                request,
                fee_reserve,
                expiry,
                state,
                payment_preimage,
                request_lookup_id,
                created_time,
                paid_time,
                payment_method,
                options,
                request_lookup_id_kind
            FROM
                melt_quote
            WHERE
                id=:id
                AND state != :state
            "#,
        )?
        .bind("id", quote_id.to_string())
        .bind("state", state.to_string())
        .fetch_one(&self.inner)
        .await?
        .map(sql_row_to_melt_quote)
        .transpose()?
        .ok_or(Error::QuoteNotFound)?;

        check_melt_quote_state_transition(quote.state, state)?;

        let rec = if state == MeltQuoteState::Paid {
            let current_time = unix_time();
            query(r#"UPDATE melt_quote SET state = :state, paid_time = :paid_time, payment_preimage = :payment_preimage WHERE id = :id"#)?
                .bind("state", state.to_string())
                .bind("paid_time", current_time as i64)
                .bind("payment_preimage", payment_proof)
                .bind("id", quote_id.to_string())
                .execute(&self.inner)
                .await
        } else {
            query(r#"UPDATE melt_quote SET state = :state WHERE id = :id"#)?
                .bind("state", state.to_string())
                .bind("id", quote_id.to_string())
                .execute(&self.inner)
                .await
        };

        match rec {
            Ok(_) => {}
            Err(err) => {
                tracing::error!("SQLite Could not update melt quote");
                return Err(err);
            }
        };

        let old_state = quote.state;
        quote.state = state;

        if state == MeltQuoteState::Unpaid || state == MeltQuoteState::Failed {
            self.delete_melt_request(quote_id).await?;
        }

        Ok((old_state, quote))
    }

    async fn get_mint_quote(&mut self, quote_id: &QuoteId) -> Result<Option<MintQuote>, Self::Err> {
        let payments = get_mint_quote_payments(&self.inner, quote_id).await?;
        let issuance = get_mint_quote_issuance(&self.inner, quote_id).await?;

        Ok(query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                amount_paid,
                amount_issued,
                payment_method,
                request_lookup_id_kind
            FROM
                mint_quote
            WHERE id = :id
            FOR UPDATE
            "#,
        )?
        .bind("id", quote_id.to_string())
        .fetch_one(&self.inner)
        .await?
        .map(|row| sql_row_to_mint_quote(row, payments, issuance))
        .transpose()?)
    }

    async fn get_melt_quote(
        &mut self,
        quote_id: &QuoteId,
    ) -> Result<Option<mint::MeltQuote>, Self::Err> {
        Ok(query(
            r#"
            SELECT
                id,
                unit,
                amount,
                request,
                fee_reserve,
                expiry,
                state,
                payment_preimage,
                request_lookup_id,
                created_time,
                paid_time,
                payment_method,
                options,
                request_lookup_id
            FROM
                melt_quote
            WHERE
                id=:id
            "#,
        )?
        .bind("id", quote_id.to_string())
        .fetch_one(&self.inner)
        .await?
        .map(sql_row_to_melt_quote)
        .transpose()?)
    }

    async fn get_mint_quote_by_request(
        &mut self,
        request: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let mut mint_quote = query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                amount_paid,
                amount_issued,
                payment_method,
                request_lookup_id_kind
            FROM
                mint_quote
            WHERE request = :request
            FOR UPDATE
            "#,
        )?
        .bind("request", request.to_string())
        .fetch_one(&self.inner)
        .await?
        .map(|row| sql_row_to_mint_quote(row, vec![], vec![]))
        .transpose()?;

        if let Some(quote) = mint_quote.as_mut() {
            let payments = get_mint_quote_payments(&self.inner, &quote.id).await?;
            let issuance = get_mint_quote_issuance(&self.inner, &quote.id).await?;
            quote.issuance = issuance;
            quote.payments = payments;
        }

        Ok(mint_quote)
    }

    async fn get_mint_quote_by_request_lookup_id(
        &mut self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let mut mint_quote = query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                amount_paid,
                amount_issued,
                payment_method,
                request_lookup_id_kind
            FROM
                mint_quote
            WHERE request_lookup_id = :request_lookup_id
            AND request_lookup_id_kind = :request_lookup_id_kind
            FOR UPDATE
            "#,
        )?
        .bind("request_lookup_id", request_lookup_id.to_string())
        .bind("request_lookup_id_kind", request_lookup_id.kind())
        .fetch_one(&self.inner)
        .await?
        .map(|row| sql_row_to_mint_quote(row, vec![], vec![]))
        .transpose()?;

        if let Some(quote) = mint_quote.as_mut() {
            let payments = get_mint_quote_payments(&self.inner, &quote.id).await?;
            let issuance = get_mint_quote_issuance(&self.inner, &quote.id).await?;
            quote.issuance = issuance;
            quote.payments = payments;
        }

        Ok(mint_quote)
    }
}

#[async_trait]
impl<RM> MintQuotesDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_mint_quote(&self, quote_id: &QuoteId) -> Result<Option<MintQuote>, Self::Err> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("get_mint_quote");

        #[cfg(feature = "prometheus")]
        let start_time = std::time::Instant::now();
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let result = async {
            let payments = get_mint_quote_payments(&*conn, quote_id).await?;
            let issuance = get_mint_quote_issuance(&*conn, quote_id).await?;

            query(
                r#"
                SELECT
                    id,
                    amount,
                    unit,
                    request,
                    expiry,
                    request_lookup_id,
                    pubkey,
                    created_time,
                    amount_paid,
                    amount_issued,
                    payment_method,
                    request_lookup_id_kind
                FROM
                    mint_quote
                WHERE id = :id"#,
            )?
            .bind("id", quote_id.to_string())
            .fetch_one(&*conn)
            .await?
            .map(|row| sql_row_to_mint_quote(row, payments, issuance))
            .transpose()
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            let success = result.is_ok();

            METRICS.record_mint_operation("get_mint_quote", success);
            METRICS.record_mint_operation_histogram(
                "get_mint_quote",
                success,
                start_time.elapsed().as_secs_f64(),
            );
            METRICS.dec_in_flight_requests("get_mint_quote");
        }

        result
    }

    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        let mut mint_quote = query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                amount_paid,
                amount_issued,
                payment_method,
                request_lookup_id_kind
            FROM
                mint_quote
            WHERE request = :request"#,
        )?
        .bind("request", request.to_owned())
        .fetch_one(&*conn)
        .await?
        .map(|row| sql_row_to_mint_quote(row, vec![], vec![]))
        .transpose()?;

        if let Some(quote) = mint_quote.as_mut() {
            let payments = get_mint_quote_payments(&*conn, &quote.id).await?;
            let issuance = get_mint_quote_issuance(&*conn, &quote.id).await?;
            quote.issuance = issuance;
            quote.payments = payments;
        }

        Ok(mint_quote)
    }

    async fn get_mint_quote_by_request_lookup_id(
        &self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        let mut mint_quote = query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                amount_paid,
                amount_issued,
                payment_method,
                request_lookup_id_kind
            FROM
                mint_quote
            WHERE request_lookup_id = :request_lookup_id
            AND request_lookup_id_kind = :request_lookup_id_kind
            "#,
        )?
        .bind("request_lookup_id", request_lookup_id.to_string())
        .bind("request_lookup_id_kind", request_lookup_id.kind())
        .fetch_one(&*conn)
        .await?
        .map(|row| sql_row_to_mint_quote(row, vec![], vec![]))
        .transpose()?;

        // TODO: these should use an sql join so they can be done in one query
        if let Some(quote) = mint_quote.as_mut() {
            let payments = get_mint_quote_payments(&*conn, &quote.id).await?;
            let issuance = get_mint_quote_issuance(&*conn, &quote.id).await?;
            quote.issuance = issuance;
            quote.payments = payments;
        }

        Ok(mint_quote)
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        let mut mint_quotes = query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                amount_paid,
                amount_issued,
                payment_method,
                request_lookup_id_kind
            FROM
                mint_quote
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(|row| sql_row_to_mint_quote(row, vec![], vec![]))
        .collect::<Result<Vec<_>, _>>()?;

        for quote in mint_quotes.as_mut_slice() {
            let payments = get_mint_quote_payments(&*conn, &quote.id).await?;
            let issuance = get_mint_quote_issuance(&*conn, &quote.id).await?;
            quote.issuance = issuance;
            quote.payments = payments;
        }

        Ok(mint_quotes)
    }

    async fn get_melt_quote(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<mint::MeltQuote>, Self::Err> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("get_melt_quote");

        #[cfg(feature = "prometheus")]
        let start_time = std::time::Instant::now();
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let result = async {
            query(
                r#"
                SELECT
                    id,
                    unit,
                    amount,
                    request,
                    fee_reserve,
                    expiry,
                    state,
                    payment_preimage,
                    request_lookup_id,
                    created_time,
                    paid_time,
                    payment_method,
                    options,
                    request_lookup_id_kind
                FROM
                    melt_quote
                WHERE
                    id=:id
                "#,
            )?
            .bind("id", quote_id.to_string())
            .fetch_one(&*conn)
            .await?
            .map(sql_row_to_melt_quote)
            .transpose()
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            let success = result.is_ok();

            METRICS.record_mint_operation("get_melt_quote", success);
            METRICS.record_mint_operation_histogram(
                "get_melt_quote",
                success,
                start_time.elapsed().as_secs_f64(),
            );
            METRICS.dec_in_flight_requests("get_melt_quote");
        }

        result
    }

    async fn get_melt_quotes(&self) -> Result<Vec<mint::MeltQuote>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                id,
                unit,
                amount,
                request,
                fee_reserve,
                expiry,
                state,
                payment_preimage,
                request_lookup_id,
                created_time,
                paid_time,
                payment_method,
                options,
                request_lookup_id_kind
            FROM
                melt_quote
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_melt_quote)
        .collect::<Result<Vec<_>, _>>()?)
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
        let mut current_states = get_current_states(&*conn, ys).await?;

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
}

#[async_trait]
impl<RM> MintSignatureTransaction<'_> for SQLTransaction<RM>
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
}

#[async_trait]
impl<RM> database::MintKVStoreTransaction<'_, Error> for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    async fn kv_read(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Error> {
        // Validate parameters according to KV store requirements
        validate_kvstore_params(primary_namespace, secondary_namespace, key)?;
        Ok(query(
            r#"
            SELECT value
            FROM kv_store
            WHERE primary_namespace = :primary_namespace
            AND secondary_namespace = :secondary_namespace
            AND key = :key
            "#,
        )?
        .bind("primary_namespace", primary_namespace.to_owned())
        .bind("secondary_namespace", secondary_namespace.to_owned())
        .bind("key", key.to_owned())
        .pluck(&self.inner)
        .await?
        .and_then(|col| match col {
            Column::Blob(data) => Some(data),
            _ => None,
        }))
    }

    async fn kv_write(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), Error> {
        // Validate parameters according to KV store requirements
        validate_kvstore_params(primary_namespace, secondary_namespace, key)?;

        let current_time = unix_time();

        query(
            r#"
            INSERT INTO kv_store
            (primary_namespace, secondary_namespace, key, value, created_time, updated_time)
            VALUES (:primary_namespace, :secondary_namespace, :key, :value, :created_time, :updated_time)
            ON CONFLICT(primary_namespace, secondary_namespace, key)
            DO UPDATE SET
                value = excluded.value,
                updated_time = excluded.updated_time
            "#,
        )?
        .bind("primary_namespace", primary_namespace.to_owned())
        .bind("secondary_namespace", secondary_namespace.to_owned())
        .bind("key", key.to_owned())
        .bind("value", value.to_vec())
        .bind("created_time", current_time as i64)
        .bind("updated_time", current_time as i64)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn kv_remove(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), Error> {
        // Validate parameters according to KV store requirements
        validate_kvstore_params(primary_namespace, secondary_namespace, key)?;
        query(
            r#"
            DELETE FROM kv_store
            WHERE primary_namespace = :primary_namespace
            AND secondary_namespace = :secondary_namespace
            AND key = :key
            "#,
        )?
        .bind("primary_namespace", primary_namespace.to_owned())
        .bind("secondary_namespace", secondary_namespace.to_owned())
        .bind("key", key.to_owned())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn kv_list(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Error> {
        // Validate namespace parameters according to KV store requirements
        cdk_common::database::mint::validate_kvstore_string(primary_namespace)?;
        cdk_common::database::mint::validate_kvstore_string(secondary_namespace)?;

        // Check empty namespace rules
        if primary_namespace.is_empty() && !secondary_namespace.is_empty() {
            return Err(Error::KVStoreInvalidKey(
                "If primary_namespace is empty, secondary_namespace must also be empty".to_string(),
            ));
        }
        Ok(query(
            r#"
            SELECT key
            FROM kv_store
            WHERE primary_namespace = :primary_namespace
            AND secondary_namespace = :secondary_namespace
            ORDER BY key
            "#,
        )?
        .bind("primary_namespace", primary_namespace.to_owned())
        .bind("secondary_namespace", secondary_namespace.to_owned())
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .map(|row| Ok(column_as_string!(&row[0])))
        .collect::<Result<Vec<_>, Error>>()?)
    }
}

#[async_trait]
impl<RM> database::MintKVStoreDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Error> {
        // Validate parameters according to KV store requirements
        validate_kvstore_params(primary_namespace, secondary_namespace, key)?;

        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT value
            FROM kv_store
            WHERE primary_namespace = :primary_namespace
            AND secondary_namespace = :secondary_namespace
            AND key = :key
            "#,
        )?
        .bind("primary_namespace", primary_namespace.to_owned())
        .bind("secondary_namespace", secondary_namespace.to_owned())
        .bind("key", key.to_owned())
        .pluck(&*conn)
        .await?
        .and_then(|col| match col {
            Column::Blob(data) => Some(data),
            _ => None,
        }))
    }

    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Error> {
        // Validate namespace parameters according to KV store requirements
        cdk_common::database::mint::validate_kvstore_string(primary_namespace)?;
        cdk_common::database::mint::validate_kvstore_string(secondary_namespace)?;

        // Check empty namespace rules
        if primary_namespace.is_empty() && !secondary_namespace.is_empty() {
            return Err(Error::KVStoreInvalidKey(
                "If primary_namespace is empty, secondary_namespace must also be empty".to_string(),
            ));
        }

        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT key
            FROM kv_store
            WHERE primary_namespace = :primary_namespace
            AND secondary_namespace = :secondary_namespace
            ORDER BY key
            "#,
        )?
        .bind("primary_namespace", primary_namespace.to_owned())
        .bind("secondary_namespace", secondary_namespace.to_owned())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(|row| Ok(column_as_string!(&row[0])))
        .collect::<Result<Vec<_>, Error>>()?)
    }
}

#[async_trait]
impl<RM> database::MintKVStore for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn database::MintKVStoreTransaction<'a, Self::Err> + Send + Sync + 'a>, Error>
    {
        Ok(Box::new(SQLTransaction {
            inner: ConnectionWithTransaction::new(
                self.pool.get().map_err(|e| Error::Database(Box::new(e)))?,
            )
            .await?,
        }))
    }
}

#[async_trait]
impl<RM> SagaTransaction<'_> for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_saga(
        &mut self,
        operation_id: &uuid::Uuid,
    ) -> Result<Option<mint::Saga>, Self::Err> {
        Ok(query(
            r#"
            SELECT
                operation_id,
                operation_kind,
                state,
                blinded_secrets,
                input_ys,
                quote_id,
                created_at,
                updated_at
            FROM
                saga_state
            WHERE
                operation_id = :operation_id
            FOR UPDATE
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .fetch_one(&self.inner)
        .await?
        .map(sql_row_to_saga)
        .transpose()?)
    }

    async fn add_saga(&mut self, saga: &mint::Saga) -> Result<(), Self::Err> {
        let current_time = unix_time();

        let blinded_secrets_json = serde_json::to_string(&saga.blinded_secrets)
            .map_err(|e| Error::Internal(format!("Failed to serialize blinded_secrets: {e}")))?;

        let input_ys_json = serde_json::to_string(&saga.input_ys)
            .map_err(|e| Error::Internal(format!("Failed to serialize input_ys: {e}")))?;

        query(
            r#"
            INSERT INTO saga_state
            (operation_id, operation_kind, state, blinded_secrets, input_ys, quote_id, created_at, updated_at)
            VALUES
            (:operation_id, :operation_kind, :state, :blinded_secrets, :input_ys, :quote_id, :created_at, :updated_at)
            "#,
        )?
        .bind("operation_id", saga.operation_id.to_string())
        .bind("operation_kind", saga.operation_kind.to_string())
        .bind("state", saga.state.state())
        .bind("blinded_secrets", blinded_secrets_json)
        .bind("input_ys", input_ys_json)
        .bind("quote_id", saga.quote_id.as_deref())
        .bind("created_at", saga.created_at as i64)
        .bind("updated_at", current_time as i64)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn update_saga(
        &mut self,
        operation_id: &uuid::Uuid,
        new_state: mint::SagaStateEnum,
    ) -> Result<(), Self::Err> {
        let current_time = unix_time();

        query(
            r#"
            UPDATE saga_state
            SET state = :state, updated_at = :updated_at
            WHERE operation_id = :operation_id
            "#,
        )?
        .bind("state", new_state.state())
        .bind("updated_at", current_time as i64)
        .bind("operation_id", operation_id.to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn delete_saga(&mut self, operation_id: &uuid::Uuid) -> Result<(), Self::Err> {
        query(
            r#"
            DELETE FROM saga_state
            WHERE operation_id = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl<RM> SagaDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_incomplete_sagas(
        &self,
        operation_kind: mint::OperationKind,
    ) -> Result<Vec<mint::Saga>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                operation_id,
                operation_kind,
                state,
                blinded_secrets,
                input_ys,
                quote_id,
                created_at,
                updated_at
            FROM
                saga_state
            WHERE
                operation_kind = :operation_kind
            ORDER BY created_at ASC
            "#,
        )?
        .bind("operation_kind", operation_kind.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_saga)
        .collect::<Result<Vec<_>, _>>()?)
    }
}

#[async_trait]
impl<RM> MintDatabase<Error> for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn database::MintTransaction<'a, Error> + Send + Sync + 'a>, Error> {
        let tx = SQLTransaction {
            inner: ConnectionWithTransaction::new(
                self.pool.get().map_err(|e| Error::Database(Box::new(e)))?,
            )
            .await?,
        };

        Ok(Box::new(tx))
    }
}

fn sql_row_to_keyset_info(row: Vec<Column>) -> Result<MintKeySetInfo, Error> {
    unpack_into!(
        let (
            id,
            unit,
            active,
            valid_from,
            valid_to,
            derivation_path,
            derivation_path_index,
            max_order,
            amounts,
            row_keyset_ppk
        ) = row
    );

    let max_order: u8 = column_as_number!(max_order);
    let amounts = column_as_nullable_string!(amounts)
        .and_then(|str| serde_json::from_str(&str).ok())
        .unwrap_or_else(|| (0..max_order).map(|m| 2u64.pow(m.into())).collect());

    Ok(MintKeySetInfo {
        id: column_as_string!(id, Id::from_str, Id::from_bytes),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        active: matches!(active, Column::Integer(1)),
        valid_from: column_as_number!(valid_from),
        derivation_path: column_as_string!(derivation_path, DerivationPath::from_str),
        derivation_path_index: column_as_nullable_number!(derivation_path_index),
        max_order,
        amounts,
        input_fee_ppk: column_as_number!(row_keyset_ppk),
        final_expiry: column_as_nullable_number!(valid_to),
    })
}

#[instrument(skip_all)]
fn sql_row_to_mint_quote(
    row: Vec<Column>,
    payments: Vec<IncomingPayment>,
    issueances: Vec<Issuance>,
) -> Result<MintQuote, Error> {
    unpack_into!(
        let (
            id, amount, unit, request, expiry, request_lookup_id,
            pubkey, created_time, amount_paid, amount_issued, payment_method, request_lookup_id_kind
        ) = row
    );

    let request_str = column_as_string!(&request);
    let request_lookup_id = column_as_nullable_string!(&request_lookup_id).unwrap_or_else(|| {
        Bolt11Invoice::from_str(&request_str)
            .map(|invoice| invoice.payment_hash().to_string())
            .unwrap_or_else(|_| request_str.clone())
    });
    let request_lookup_id_kind = column_as_string!(request_lookup_id_kind);

    let pubkey = column_as_nullable_string!(&pubkey)
        .map(|pk| PublicKey::from_hex(&pk))
        .transpose()?;

    let id = column_as_string!(id);
    let amount: Option<u64> = column_as_nullable_number!(amount);
    let amount_paid: u64 = column_as_number!(amount_paid);
    let amount_issued: u64 = column_as_number!(amount_issued);
    let payment_method = column_as_string!(payment_method, PaymentMethod::from_str);

    Ok(MintQuote::new(
        Some(QuoteId::from_str(&id)?),
        request_str,
        column_as_string!(unit, CurrencyUnit::from_str),
        amount.map(Amount::from),
        column_as_number!(expiry),
        PaymentIdentifier::new(&request_lookup_id_kind, &request_lookup_id)
            .map_err(|_| ConversionError::MissingParameter("Payment id".to_string()))?,
        pubkey,
        amount_paid.into(),
        amount_issued.into(),
        payment_method,
        column_as_number!(created_time),
        payments,
        issueances,
    ))
}

fn sql_row_to_melt_quote(row: Vec<Column>) -> Result<mint::MeltQuote, Error> {
    unpack_into!(
        let (
                id,
                unit,
                amount,
                request,
                fee_reserve,
                expiry,
                state,
                payment_preimage,
                request_lookup_id,
                created_time,
                paid_time,
                payment_method,
                options,
                request_lookup_id_kind
        ) = row
    );

    let id = column_as_string!(id);
    let amount: u64 = column_as_number!(amount);
    let fee_reserve: u64 = column_as_number!(fee_reserve);

    let expiry = column_as_number!(expiry);
    let payment_preimage = column_as_nullable_string!(payment_preimage);
    let options = column_as_nullable_string!(options);
    let options = options.and_then(|o| serde_json::from_str(&o).ok());
    let created_time: i64 = column_as_number!(created_time);
    let paid_time = column_as_nullable_number!(paid_time);
    let payment_method = PaymentMethod::from_str(&column_as_string!(payment_method))?;

    let state =
        MeltQuoteState::from_str(&column_as_string!(&state)).map_err(ConversionError::from)?;

    let unit = column_as_string!(unit);
    let request = column_as_string!(request);

    let request_lookup_id_kind = column_as_nullable_string!(request_lookup_id_kind);

    let request_lookup_id = column_as_nullable_string!(&request_lookup_id).or_else(|| {
        Bolt11Invoice::from_str(&request)
            .ok()
            .map(|invoice| invoice.payment_hash().to_string())
    });

    let request_lookup_id = if let (Some(id_kind), Some(request_lookup_id)) =
        (request_lookup_id_kind, request_lookup_id)
    {
        Some(
            PaymentIdentifier::new(&id_kind, &request_lookup_id)
                .map_err(|_| ConversionError::MissingParameter("Payment id".to_string()))?,
        )
    } else {
        None
    };

    let request = match serde_json::from_str(&request) {
        Ok(req) => req,
        Err(err) => {
            tracing::debug!(
                "Melt quote from pre migrations defaulting to bolt11 {}.",
                err
            );
            let bolt11 = Bolt11Invoice::from_str(&request).unwrap();
            MeltPaymentRequest::Bolt11 { bolt11 }
        }
    };

    Ok(MeltQuote {
        id: QuoteId::from_str(&id)?,
        unit: CurrencyUnit::from_str(&unit)?,
        amount: Amount::from(amount),
        request,
        fee_reserve: Amount::from(fee_reserve),
        state,
        expiry,
        payment_preimage,
        request_lookup_id,
        options,
        created_time: created_time as u64,
        paid_time,
        payment_method,
    })
}

fn sql_row_to_proof(row: Vec<Column>) -> Result<Proof, Error> {
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

fn sql_row_to_hashmap_amount(row: Vec<Column>) -> Result<(Id, Amount), Error> {
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

fn sql_row_to_proof_with_state(row: Vec<Column>) -> Result<(Proof, Option<State>), Error> {
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

fn sql_row_to_blind_signature(row: Vec<Column>) -> Result<BlindSignature, Error> {
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

fn sql_row_to_saga(row: Vec<Column>) -> Result<mint::Saga, Error> {
    unpack_into!(
        let (
            operation_id,
            operation_kind,
            state,
            blinded_secrets,
            input_ys,
            quote_id,
            created_at,
            updated_at
        ) = row
    );

    let operation_id_str = column_as_string!(&operation_id);
    let operation_id = uuid::Uuid::parse_str(&operation_id_str)
        .map_err(|e| Error::Internal(format!("Invalid operation_id UUID: {e}")))?;

    let operation_kind_str = column_as_string!(&operation_kind);
    let operation_kind = mint::OperationKind::from_str(&operation_kind_str)
        .map_err(|e| Error::Internal(format!("Invalid operation kind: {e}")))?;

    let state_str = column_as_string!(&state);
    let state = mint::SagaStateEnum::new(operation_kind, &state_str)
        .map_err(|e| Error::Internal(format!("Invalid saga state: {e}")))?;

    let blinded_secrets_str = column_as_string!(&blinded_secrets);
    let blinded_secrets: Vec<PublicKey> = serde_json::from_str(&blinded_secrets_str)
        .map_err(|e| Error::Internal(format!("Failed to deserialize blinded_secrets: {e}")))?;

    let input_ys_str = column_as_string!(&input_ys);
    let input_ys: Vec<PublicKey> = serde_json::from_str(&input_ys_str)
        .map_err(|e| Error::Internal(format!("Failed to deserialize input_ys: {e}")))?;

    let quote_id = match &quote_id {
        Column::Text(s) => {
            if s.is_empty() {
                None
            } else {
                Some(s.clone())
            }
        }
        Column::Null => None,
        _ => None,
    };

    let created_at: u64 = column_as_number!(created_at);
    let updated_at: u64 = column_as_number!(updated_at);

    Ok(mint::Saga {
        operation_id,
        operation_kind,
        state,
        blinded_secrets,
        input_ys,
        quote_id,
        created_at,
        updated_at,
    })
}

#[cfg(test)]
mod test {
    use super::*;

    mod max_order_to_amounts_migrations {
        use super::*;

        #[test]
        fn legacy_payload() {
            let result = sql_row_to_keyset_info(vec![
                Column::Text("0083a60439303340".to_owned()),
                Column::Text("sat".to_owned()),
                Column::Integer(1),
                Column::Integer(1749844864),
                Column::Null,
                Column::Text("0'/0'/0'".to_owned()),
                Column::Integer(0),
                Column::Integer(32),
                Column::Null,
                Column::Integer(0),
            ]);
            assert!(result.is_ok());
        }

        #[test]
        fn migrated_payload() {
            let legacy = sql_row_to_keyset_info(vec![
                Column::Text("0083a60439303340".to_owned()),
                Column::Text("sat".to_owned()),
                Column::Integer(1),
                Column::Integer(1749844864),
                Column::Null,
                Column::Text("0'/0'/0'".to_owned()),
                Column::Integer(0),
                Column::Integer(32),
                Column::Null,
                Column::Integer(0),
            ]);
            assert!(legacy.is_ok());

            let amounts = (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>();
            let migrated = sql_row_to_keyset_info(vec![
                Column::Text("0083a60439303340".to_owned()),
                Column::Text("sat".to_owned()),
                Column::Integer(1),
                Column::Integer(1749844864),
                Column::Null,
                Column::Text("0'/0'/0'".to_owned()),
                Column::Integer(0),
                Column::Integer(32),
                Column::Text(serde_json::to_string(&amounts).expect("valid json")),
                Column::Integer(0),
            ]);
            assert!(migrated.is_ok());
            assert_eq!(legacy.unwrap(), migrated.unwrap());
        }

        #[test]
        fn amounts_over_max_order() {
            let legacy = sql_row_to_keyset_info(vec![
                Column::Text("0083a60439303340".to_owned()),
                Column::Text("sat".to_owned()),
                Column::Integer(1),
                Column::Integer(1749844864),
                Column::Null,
                Column::Text("0'/0'/0'".to_owned()),
                Column::Integer(0),
                Column::Integer(32),
                Column::Null,
                Column::Integer(0),
            ]);
            assert!(legacy.is_ok());

            let amounts = (0..16).map(|x| 2u64.pow(x)).collect::<Vec<_>>();
            let migrated = sql_row_to_keyset_info(vec![
                Column::Text("0083a60439303340".to_owned()),
                Column::Text("sat".to_owned()),
                Column::Integer(1),
                Column::Integer(1749844864),
                Column::Null,
                Column::Text("0'/0'/0'".to_owned()),
                Column::Integer(0),
                Column::Integer(32),
                Column::Text(serde_json::to_string(&amounts).expect("valid json")),
                Column::Integer(0),
            ]);
            assert!(migrated.is_ok());
            let migrated = migrated.unwrap();
            assert_ne!(legacy.unwrap(), migrated);
            assert_eq!(migrated.amounts.len(), 16);
        }
    }
}

//! SQLite Wallet Database

use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::common::ProofInfo;
use cdk_common::database::{ConversionError, Error, WalletDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{MeltQuoteState, MintQuoteState};
use cdk_common::secret::Secret;
use cdk_common::wallet::{self, MintQuote, Transaction, TransactionDirection, TransactionId};
use cdk_common::{
    database, Amount, CurrencyUnit, Id, KeySet, KeySetInfo, Keys, MintInfo, PaymentMethod, Proof,
    ProofDleq, PublicKey, SecretKey, SpendingConditions, State,
};
use tracing::instrument;
use uuid::Uuid;

use crate::common::migrate;
use crate::database::{ConnectionWithTransaction, DatabaseExecutor};
use crate::pool::{DatabasePool, Pool, PooledResource};
use crate::stmt::{query, Column};
use crate::{
    column_as_binary, column_as_nullable_binary, column_as_nullable_number,
    column_as_nullable_string, column_as_number, column_as_string, unpack_into,
};

#[rustfmt::skip]
mod migrations {
    include!(concat!(env!("OUT_DIR"), "/migrations_wallet.rs"));
}

/// Wallet SQLite Database
#[derive(Debug, Clone)]
pub struct SQLWalletDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    pool: Arc<Pool<RM>>,
}

impl<RM> SQLWalletDatabase<RM>
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

    /// Migrate [`WalletSqliteDatabase`]
    async fn migrate(conn: PooledResource<RM>) -> Result<(), Error> {
        let tx = ConnectionWithTransaction::new(conn).await?;
        migrate(&tx, RM::Connection::name(), migrations::MIGRATIONS).await?;
        // Update any existing keys with missing keyset_u32 values
        Self::add_keyset_u32(&tx).await?;
        tx.commit().await?;

        Ok(())
    }

    async fn add_keyset_u32<T>(conn: &T) -> Result<(), Error>
    where
        T: DatabaseExecutor,
    {
        // First get the keysets where keyset_u32 on key is null
        let keys_without_u32: Vec<Vec<Column>> = query(
            r#"
            SELECT
                id
            FROM key
            WHERE keyset_u32 IS NULL
            "#,
        )?
        .fetch_all(conn)
        .await?;

        for row in keys_without_u32 {
            unpack_into!(let (id) = row);
            let id = column_as_string!(id);

            if let Ok(id) = Id::from_str(&id) {
                query(
                    r#"
            UPDATE
                key
            SET keyset_u32 = :u32_keyset
            WHERE id = :keyset_id
            "#,
                )?
                .bind("u32_keyset", u32::from(id))
                .bind("keyset_id", id.to_string())
                .execute(conn)
                .await?;
            }
        }

        // Also update keysets where keyset_u32 is null
        let keysets_without_u32: Vec<Vec<Column>> = query(
            r#"
            SELECT
                id
            FROM keyset
            WHERE keyset_u32 IS NULL
            "#,
        )?
        .fetch_all(conn)
        .await?;

        for row in keysets_without_u32 {
            unpack_into!(let (id) = row);
            let id = column_as_string!(id);

            if let Ok(id) = Id::from_str(&id) {
                query(
                    r#"
            UPDATE
                keyset
            SET keyset_u32 = :u32_keyset
            WHERE id = :keyset_id
            "#,
                )?
                .bind("u32_keyset", u32::from(id))
                .bind("keyset_id", id.to_string())
                .execute(conn)
                .await?;
            }
        }

        Ok(())
    }
}

#[async_trait]
impl<RM> WalletDatabase<database::Error> for SQLWalletDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    #[instrument(skip(self))]
    async fn get_melt_quotes(&self) -> Result<Vec<wallet::MeltQuote>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        Ok(query(
            r#"
              SELECT
                  id,
                  unit,
                  amount,
                  request,
                  fee_reserve,
                  state,
                  expiry,
                  payment_preimage,
                  payment_method,
                  used_by_operation,
                  version
              FROM
                  melt_quote
              "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_melt_quote)
        .collect::<Result<_, _>>()?)
    }

    #[instrument(skip(self))]
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                name,
                pubkey,
                version,
                description,
                description_long,
                contact,
                nuts,
                icon_url,
                motd,
                urls,
                mint_time,
                tos_url
            FROM
                mint
            WHERE mint_url = :mint_url
            "#,
        )?
        .bind("mint_url", mint_url.to_string())
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_mint_info)
        .transpose()?)
    }

    #[instrument(skip(self))]
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
                SELECT
                    name,
                    pubkey,
                    version,
                    description,
                    description_long,
                    contact,
                    nuts,
                    icon_url,
                    motd,
                    urls,
                    mint_time,
                    tos_url,
                    mint_url
                FROM
                    mint
                "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(|mut row| {
            let url = column_as_string!(
                row.pop().ok_or(ConversionError::MissingColumn(0, 1))?,
                MintUrl::from_str
            );

            Ok((url, sql_row_to_mint_info(row).ok()))
        })
        .collect::<Result<HashMap<_, _>, Error>>()?)
    }

    #[instrument(skip(self))]
    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let keysets = query(
            r#"
            SELECT
                id,
                unit,
                active,
                input_fee_ppk,
                final_expiry
            FROM
                keyset
            WHERE mint_url = :mint_url
            "#,
        )?
        .bind("mint_url", mint_url.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_keyset)
        .collect::<Result<Vec<_>, Error>>()?;

        match keysets.is_empty() {
            false => Ok(Some(keysets)),
            true => Ok(None),
        }
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keyset_by_id(
        &self,
        keyset_id: &Id,
    ) -> Result<Option<KeySetInfo>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT
                id,
                unit,
                active,
                input_fee_ppk,
                final_expiry
            FROM
                keyset
            WHERE id = :id
            "#,
        )?
        .bind("id", keyset_id.to_string())
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_keyset)
        .transpose()
    }

    #[instrument(skip(self))]
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT
                id,
                mint_url,
                amount,
                unit,
                request,
                state,
                expiry,
                secret_key,
                payment_method,
                amount_issued,
                amount_paid,
                used_by_operation,
                version
            FROM
                mint_quote
            WHERE
                id = :id
            "#,
        )?
        .bind("id", quote_id.to_string())
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_mint_quote)
        .transpose()
    }

    #[instrument(skip(self))]
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                id,
                mint_url,
                amount,
                unit,
                request,
                state,
                expiry,
                secret_key,
                payment_method,
                amount_issued,
                amount_paid,
                used_by_operation,
                version
            FROM
                mint_quote
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_mint_quote)
        .collect::<Result<_, _>>()?)
    }

    #[instrument(skip(self))]
    async fn get_unissued_mint_quotes(&self) -> Result<Vec<MintQuote>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                id,
                mint_url,
                amount,
                unit,
                request,
                state,
                expiry,
                secret_key,
                payment_method,
                amount_issued,
                amount_paid,
                used_by_operation,
                version
            FROM
                mint_quote
            WHERE
                amount_issued = 0
                OR
                payment_method = 'bolt12'
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_mint_quote)
        .collect::<Result<_, _>>()?)
    }

    #[instrument(skip(self))]
    async fn get_melt_quote(
        &self,
        quote_id: &str,
    ) -> Result<Option<wallet::MeltQuote>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT
                id,
                unit,
                amount,
                request,
                fee_reserve,
                state,
                expiry,
                payment_preimage,
                payment_method,
                used_by_operation,
                version
            FROM
                melt_quote
            WHERE
                id=:id
            "#,
        )?
        .bind("id", quote_id.to_owned())
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_melt_quote)
        .transpose()
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keys(&self, keyset_id: &Id) -> Result<Option<Keys>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT
                keys
            FROM key
            WHERE id = :id
            "#,
        )?
        .bind("id", keyset_id.to_string())
        .pluck(&*conn)
        .await?
        .map(|keys| {
            let keys = column_as_string!(keys);
            serde_json::from_str(&keys).map_err(Error::from)
        })
        .transpose()
    }

    #[instrument(skip(self, state, spending_conditions))]
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                amount,
                unit,
                keyset_id,
                secret,
                c,
                witness,
                dleq_e,
                dleq_s,
                dleq_r,
                y,
                mint_url,
                state,
                spending_condition,
                used_by_operation,
                created_by_operation
            FROM proof
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .filter_map(|row| {
            let row = sql_row_to_proof_info(row).ok()?;

            if row.matches_conditions(&mint_url, &unit, &state, &spending_conditions) {
                Some(row)
            } else {
                None
            }
        })
        .collect::<Vec<_>>())
    }

    #[instrument(skip(self, ys))]
    async fn get_proofs_by_ys(
        &self,
        ys: Vec<PublicKey>,
    ) -> Result<Vec<ProofInfo>, database::Error> {
        if ys.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                amount,
                unit,
                keyset_id,
                secret,
                c,
                witness,
                dleq_e,
                dleq_s,
                dleq_r,
                y,
                mint_url,
                state,
                spending_condition,
                used_by_operation,
                created_by_operation
            FROM proof
            WHERE y IN (:ys)
        "#,
        )?
        .bind_vec("ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .filter_map(|row| sql_row_to_proof_info(row).ok())
        .collect::<Vec<_>>())
    }

    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        states: Option<Vec<State>>,
    ) -> Result<u64, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let mut query_str = "SELECT COALESCE(SUM(amount), 0) as total FROM proof".to_string();
        let mut where_clauses = Vec::new();
        let states = states
            .unwrap_or_default()
            .into_iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>();

        if mint_url.is_some() {
            where_clauses.push("mint_url = :mint_url");
        }
        if unit.is_some() {
            where_clauses.push("unit = :unit");
        }
        if !states.is_empty() {
            where_clauses.push("state IN (:states)");
        }

        if !where_clauses.is_empty() {
            query_str.push_str(" WHERE ");
            query_str.push_str(&where_clauses.join(" AND "));
        }

        let mut q = query(&query_str)?;

        if let Some(ref mint_url) = mint_url {
            q = q.bind("mint_url", mint_url.to_string());
        }
        if let Some(ref unit) = unit {
            q = q.bind("unit", unit.to_string());
        }

        if !states.is_empty() {
            q = q.bind_vec("states", states);
        }

        let balance = q
            .pluck(&*conn)
            .await?
            .map(|n| {
                // SQLite SUM returns INTEGER which we need to convert to u64
                match n {
                    crate::stmt::Column::Integer(i) => Ok(i as u64),
                    crate::stmt::Column::Real(f) => Ok(f as u64),
                    _ => Err(Error::Database(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Invalid balance type",
                    )))),
                }
            })
            .transpose()?
            .unwrap_or(0);

        Ok(balance)
    }

    #[instrument(skip(self))]
    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                mint_url,
                direction,
                unit,
                amount,
                fee,
                ys,
                timestamp,
                memo,
                metadata,
                quote_id,
                payment_request,
                payment_proof,
                payment_method,
                saga_id
            FROM
                transactions
            WHERE
                id = :id
            "#,
        )?
        .bind("id", transaction_id.as_slice().to_vec())
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_transaction)
        .transpose()?)
    }

    #[instrument(skip(self))]
    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        Ok(query(
            r#"
            SELECT
                mint_url,
                direction,
                unit,
                amount,
                fee,
                ys,
                timestamp,
                memo,
                metadata,
                quote_id,
                payment_request,
                payment_proof,
                payment_method,
                saga_id
            FROM
                transactions
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .filter_map(|row| {
            // TODO: Avoid a table scan by passing the heavy lifting of checking to the DB engine
            let transaction = sql_row_to_transaction(row).ok()?;
            if transaction.matches_conditions(&mint_url, &direction, &unit) {
                Some(transaction)
            } else {
                None
            }
        })
        .collect::<Vec<_>>())
    }

    #[instrument(skip(self))]
    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        let tx = ConnectionWithTransaction::new(conn).await?;

        for proof in added {
            query(
                r#"
    INSERT INTO proof
    (y, mint_url, state, spending_condition, unit, amount, keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r, used_by_operation, created_by_operation)
    VALUES
    (:y, :mint_url, :state, :spending_condition, :unit, :amount, :keyset_id, :secret, :c, :witness, :dleq_e, :dleq_s, :dleq_r, :used_by_operation, :created_by_operation)
    ON CONFLICT(y) DO UPDATE SET
        mint_url = excluded.mint_url,
        state = excluded.state,
        spending_condition = excluded.spending_condition,
        unit = excluded.unit,
        amount = excluded.amount,
        keyset_id = excluded.keyset_id,
        secret = excluded.secret,
        c = excluded.c,
        witness = excluded.witness,
        dleq_e = excluded.dleq_e,
        dleq_s = excluded.dleq_s,
        dleq_r = excluded.dleq_r,
        used_by_operation = excluded.used_by_operation,
        created_by_operation = excluded.created_by_operation
    ;
            "#,
            )?
            .bind("y", proof.y.to_bytes().to_vec())
            .bind("mint_url", proof.mint_url.to_string())
            .bind("state", proof.state.to_string())
            .bind(
                "spending_condition",
                proof
                    .spending_condition
                    .map(|s| serde_json::to_string(&s).ok()),
            )
            .bind("unit", proof.unit.to_string())
            .bind("amount", u64::from(proof.proof.amount) as i64)
            .bind("keyset_id", proof.proof.keyset_id.to_string())
            .bind("secret", proof.proof.secret.to_string())
            .bind("c", proof.proof.c.to_bytes().to_vec())
            .bind(
                "witness",
                proof
                    .proof
                    .witness
                    .and_then(|w| serde_json::to_string(&w).ok()),
            )
            .bind(
                "dleq_e",
                proof.proof.dleq.as_ref().map(|dleq| dleq.e.to_secret_bytes().to_vec()),
            )
            .bind(
                "dleq_s",
                proof.proof.dleq.as_ref().map(|dleq| dleq.s.to_secret_bytes().to_vec()),
            )
            .bind(
                "dleq_r",
                proof.proof.dleq.as_ref().map(|dleq| dleq.r.to_secret_bytes().to_vec()),
            )
            .bind("used_by_operation", proof.used_by_operation.map(|id| id.to_string()))
            .bind("created_by_operation", proof.created_by_operation.map(|id| id.to_string()))
            .execute(&tx)
            .await?;
        }

        if !removed_ys.is_empty() {
            query(r#"DELETE FROM proof WHERE y IN (:ys)"#)?
                .bind_vec(
                    "ys",
                    removed_ys.iter().map(|y| y.to_bytes().to_vec()).collect(),
                )
                .execute(&tx)
                .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: State,
    ) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query("UPDATE proof SET state = :state WHERE y IN (:ys)")?
            .bind_vec("ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
            .bind("state", state.to_string())
            .execute(&*conn)
            .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let mint_url = transaction.mint_url.to_string();
        let direction = transaction.direction.to_string();
        let unit = transaction.unit.to_string();
        let amount = u64::from(transaction.amount) as i64;
        let fee = u64::from(transaction.fee) as i64;
        let ys = transaction
            .ys
            .iter()
            .flat_map(|y| y.to_bytes().to_vec())
            .collect::<Vec<_>>();

        let id = transaction.id();

        query(
               r#"
   INSERT INTO transactions
   (id, mint_url, direction, unit, amount, fee, ys, timestamp, memo, metadata, quote_id, payment_request, payment_proof, payment_method, saga_id)
   VALUES
   (:id, :mint_url, :direction, :unit, :amount, :fee, :ys, :timestamp, :memo, :metadata, :quote_id, :payment_request, :payment_proof, :payment_method, :saga_id)
   ON CONFLICT(id) DO UPDATE SET
       mint_url = excluded.mint_url,
       direction = excluded.direction,
       unit = excluded.unit,
       amount = excluded.amount,
       fee = excluded.fee,
       timestamp = excluded.timestamp,
       memo = excluded.memo,
       metadata = excluded.metadata,
       quote_id = excluded.quote_id,
       payment_request = excluded.payment_request,
       payment_proof = excluded.payment_proof,
       payment_method = excluded.payment_method,
       saga_id = excluded.saga_id
   ;
           "#,
           )?
           .bind("id", id.as_slice().to_vec())
           .bind("mint_url", mint_url)
           .bind("direction", direction)
           .bind("unit", unit)
           .bind("amount", amount)
           .bind("fee", fee)
           .bind("ys", ys)
           .bind("timestamp", transaction.timestamp as i64)
           .bind("memo", transaction.memo)
           .bind(
               "metadata",
               serde_json::to_string(&transaction.metadata).map_err(Error::from)?,
           )
           .bind("quote_id", transaction.quote_id)
           .bind("payment_request", transaction.payment_request)
           .bind("payment_proof", transaction.payment_proof)
           .bind("payment_method", transaction.payment_method.map(|pm| pm.to_string()))
           .bind("saga_id", transaction.saga_id.map(|id| id.to_string()))
           .execute(&*conn)
           .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        let tx = ConnectionWithTransaction::new(conn).await?;
        let tables = ["mint_quote", "proof"];

        for table in &tables {
            query(&format!(
                r#"
                UPDATE {table}
                SET mint_url = :new_mint_url
                WHERE mint_url = :old_mint_url
            "#
            ))?
            .bind("new_mint_url", new_mint_url.to_string())
            .bind("old_mint_url", old_mint_url.to_string())
            .execute(&tx)
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn increment_keyset_counter(
        &self,
        keyset_id: &Id,
        count: u32,
    ) -> Result<u32, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let new_counter = query(
            r#"
            INSERT INTO keyset_counter (keyset_id, counter)
            VALUES (:keyset_id, :count)
            ON CONFLICT(keyset_id) DO UPDATE SET
                counter = keyset_counter.counter + :count
            RETURNING counter
            "#,
        )?
        .bind("keyset_id", keyset_id.to_string())
        .bind("count", count)
        .pluck(&*conn)
        .await?
        .map(|n| Ok::<_, Error>(column_as_number!(n)))
        .transpose()?
        .ok_or_else(|| Error::Internal("Counter update returned no value".to_owned()))?;

        Ok(new_counter)
    }

    #[instrument(skip(self, mint_info))]
    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let (
            name,
            pubkey,
            version,
            description,
            description_long,
            contact,
            nuts,
            icon_url,
            urls,
            motd,
            time,
            tos_url,
        ) = match mint_info {
            Some(mint_info) => {
                let MintInfo {
                    name,
                    pubkey,
                    version,
                    description,
                    description_long,
                    contact,
                    nuts,
                    icon_url,
                    urls,
                    motd,
                    time,
                    tos_url,
                } = mint_info;

                (
                    name,
                    pubkey.map(|p| p.to_bytes().to_vec()),
                    version.map(|v| serde_json::to_string(&v).ok()),
                    description,
                    description_long,
                    contact.map(|c| serde_json::to_string(&c).ok()),
                    serde_json::to_string(&nuts).ok(),
                    icon_url,
                    urls.map(|c| serde_json::to_string(&c).ok()),
                    motd,
                    time,
                    tos_url,
                )
            }
            None => (
                None, None, None, None, None, None, None, None, None, None, None, None,
            ),
        };

        query(
            r#"
   INSERT INTO mint
   (
       mint_url, name, pubkey, version, description, description_long,
       contact, nuts, icon_url, urls, motd, mint_time, tos_url
   )
   VALUES
   (
       :mint_url, :name, :pubkey, :version, :description, :description_long,
       :contact, :nuts, :icon_url, :urls, :motd, :mint_time, :tos_url
   )
   ON CONFLICT(mint_url) DO UPDATE SET
       name = excluded.name,
       pubkey = excluded.pubkey,
       version = excluded.version,
       description = excluded.description,
       description_long = excluded.description_long,
       contact = excluded.contact,
       nuts = excluded.nuts,
       icon_url = excluded.icon_url,
       urls = excluded.urls,
       motd = excluded.motd,
       mint_time = excluded.mint_time,
       tos_url = excluded.tos_url
   ;
           "#,
        )?
        .bind("mint_url", mint_url.to_string())
        .bind("name", name)
        .bind("pubkey", pubkey)
        .bind("version", version)
        .bind("description", description)
        .bind("description_long", description_long)
        .bind("contact", contact)
        .bind("nuts", nuts)
        .bind("icon_url", icon_url)
        .bind("urls", urls)
        .bind("motd", motd)
        .bind("mint_time", time.map(|v| v as i64))
        .bind("tos_url", tos_url)
        .execute(&*conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query(r#"DELETE FROM mint WHERE mint_url=:mint_url"#)?
            .bind("mint_url", mint_url.to_string())
            .execute(&*conn)
            .await?;

        Ok(())
    }

    #[instrument(skip(self, keysets))]
    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        let tx = ConnectionWithTransaction::new(conn).await?;

        for keyset in keysets {
            query(
                r#"
        INSERT INTO keyset
        (mint_url, id, unit, active, input_fee_ppk, final_expiry, keyset_u32)
        VALUES
        (:mint_url, :id, :unit, :active, :input_fee_ppk, :final_expiry, :keyset_u32)
        ON CONFLICT(id) DO UPDATE SET
            active = excluded.active,
            input_fee_ppk = excluded.input_fee_ppk
        "#,
            )?
            .bind("mint_url", mint_url.to_string())
            .bind("id", keyset.id.to_string())
            .bind("unit", keyset.unit.to_string())
            .bind("active", keyset.active)
            .bind("input_fee_ppk", keyset.input_fee_ppk as i64)
            .bind("final_expiry", keyset.final_expiry.map(|v| v as i64))
            .bind("keyset_u32", u32::from(keyset.id))
            .execute(&tx)
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let expected_version = quote.version;
        let new_version = expected_version.wrapping_add(1);

        let rows_affected = query(
                r#"
    INSERT INTO mint_quote
    (id, mint_url, amount, unit, request, state, expiry, secret_key, payment_method, amount_issued, amount_paid, version)
    VALUES
    (:id, :mint_url, :amount, :unit, :request, :state, :expiry, :secret_key, :payment_method, :amount_issued, :amount_paid, :version)
    ON CONFLICT(id) DO UPDATE SET
        mint_url = excluded.mint_url,
        amount = excluded.amount,
        unit = excluded.unit,
        request = excluded.request,
        state = excluded.state,
        expiry = excluded.expiry,
        secret_key = excluded.secret_key,
        payment_method = excluded.payment_method,
        amount_issued = excluded.amount_issued,
        amount_paid = excluded.amount_paid,
        version = :new_version
    WHERE mint_quote.version = :expected_version
    ;
            "#,
            )?
            .bind("id", quote.id.to_string())
            .bind("mint_url", quote.mint_url.to_string())
            .bind("amount", quote.amount.map(|a| a.to_i64()))
            .bind("unit", quote.unit.to_string())
            .bind("request", quote.request)
            .bind("state", quote.state.to_string())
            .bind("expiry", quote.expiry as i64)
            .bind("secret_key", quote.secret_key.map(|p| p.to_string()))
            .bind("payment_method", quote.payment_method.to_string())
            .bind("amount_issued", quote.amount_issued.to_i64())
            .bind("amount_paid", quote.amount_paid.to_i64())
            .bind("version", quote.version as i64)
            .bind("new_version", new_version as i64)
            .bind("expected_version", expected_version as i64)
            .execute(&*conn).await?;

        if rows_affected == 0 {
            return Err(database::Error::ConcurrentUpdate);
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query(r#"DELETE FROM mint_quote WHERE id=:id"#)?
            .bind("id", quote_id.to_string())
            .execute(&*conn)
            .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_melt_quote(&self, quote: wallet::MeltQuote) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let expected_version = quote.version;
        let new_version = expected_version.wrapping_add(1);

        let rows_affected = query(
            r#"
 INSERT INTO melt_quote
 (id, unit, amount, request, fee_reserve, state, expiry, payment_method, version)
 VALUES
 (:id, :unit, :amount, :request, :fee_reserve, :state, :expiry, :payment_method, :version)
 ON CONFLICT(id) DO UPDATE SET
     unit = excluded.unit,
     amount = excluded.amount,
     request = excluded.request,
     fee_reserve = excluded.fee_reserve,
     state = excluded.state,
     expiry = excluded.expiry,
     payment_method = excluded.payment_method,
     version = :new_version
 WHERE melt_quote.version = :expected_version
 ;
         "#,
        )?
        .bind("id", quote.id.to_string())
        .bind("unit", quote.unit.to_string())
        .bind("amount", u64::from(quote.amount) as i64)
        .bind("request", quote.request)
        .bind("fee_reserve", u64::from(quote.fee_reserve) as i64)
        .bind("state", quote.state.to_string())
        .bind("expiry", quote.expiry as i64)
        .bind("payment_method", quote.payment_method.to_string())
        .bind("version", quote.version as i64)
        .bind("new_version", new_version as i64)
        .bind("expected_version", expected_version as i64)
        .execute(&*conn)
        .await?;

        if rows_affected == 0 {
            return Err(database::Error::ConcurrentUpdate);
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query(r#"DELETE FROM melt_quote WHERE id=:id"#)?
            .bind("id", quote_id.to_owned())
            .execute(&*conn)
            .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_keys(&self, keyset: KeySet) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        keyset.verify_id()?;

        query(
            r#"
                INSERT INTO key
                (id, keys, keyset_u32)
                VALUES
                (:id, :keys, :keyset_u32)
            "#,
        )?
        .bind("id", keyset.id.to_string())
        .bind(
            "keys",
            serde_json::to_string(&keyset.keys).map_err(Error::from)?,
        )
        .bind("keyset_u32", u32::from(keyset.id))
        .execute(&*conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_keys(&self, id: &Id) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query(r#"DELETE FROM key WHERE id = :id"#)?
            .bind("id", id.to_string())
            .execute(&*conn)
            .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query(r#"DELETE FROM transactions WHERE id=:id"#)?
            .bind("id", transaction_id.as_slice().to_vec())
            .execute(&*conn)
            .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn add_saga(&self, saga: wallet::WalletSaga) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let state_json = serde_json::to_string(&saga.state).map_err(|e| {
            Error::Database(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to serialize saga state: {}", e),
            )))
        })?;

        let data_json = serde_json::to_string(&saga.data).map_err(|e| {
            Error::Database(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to serialize saga data: {}", e),
            )))
        })?;

        query(
            r#"
            INSERT INTO wallet_sagas
            (id, kind, state, amount, mint_url, unit, quote_id, created_at, updated_at, data, version)
            VALUES
            (:id, :kind, :state, :amount, :mint_url, :unit, :quote_id, :created_at, :updated_at, :data, :version)
            "#,
        )?
        .bind("id", saga.id.to_string())
        .bind("kind", saga.kind.to_string())
        .bind("state", state_json)
        .bind("amount", u64::from(saga.amount) as i64)
        .bind("mint_url", saga.mint_url.to_string())
        .bind("unit", saga.unit.to_string())
        .bind("quote_id", saga.quote_id)
        .bind("created_at", saga.created_at as i64)
        .bind("updated_at", saga.updated_at as i64)
        .bind("data", data_json)
        .bind("version", saga.version as i64)
        .execute(&*conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_saga(
        &self,
        id: &uuid::Uuid,
    ) -> Result<Option<wallet::WalletSaga>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let rows = query(
            r#"
            SELECT id, kind, state, amount, mint_url, unit, quote_id, created_at, updated_at, data, version
            FROM wallet_sagas
            WHERE id = :id
            "#,
        )?
        .bind("id", id.to_string())
        .fetch_all(&*conn)
        .await?;

        match rows.into_iter().next() {
            Some(row) => Ok(Some(sql_row_to_wallet_saga(row)?)),
            None => Ok(None),
        }
    }

    #[instrument(skip(self))]
    async fn update_saga(&self, saga: wallet::WalletSaga) -> Result<bool, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let state_json = serde_json::to_string(&saga.state).map_err(|e| {
            Error::Database(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to serialize saga state: {}", e),
            )))
        })?;

        let data_json = serde_json::to_string(&saga.data).map_err(|e| {
            Error::Database(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to serialize saga data: {}", e),
            )))
        })?;

        // Optimistic locking: only update if the version matches the expected value.
        // The saga.version has already been incremented by the caller, so we check
        // for (saga.version - 1) in the WHERE clause.
        let expected_version = saga.version.saturating_sub(1);

        let rows_affected = query(
            r#"
            UPDATE wallet_sagas
            SET kind = :kind, state = :state, amount = :amount, mint_url = :mint_url,
                unit = :unit, quote_id = :quote_id, updated_at = :updated_at, data = :data,
                version = :new_version
            WHERE id = :id AND version = :expected_version
            "#,
        )?
        .bind("id", saga.id.to_string())
        .bind("kind", saga.kind.to_string())
        .bind("state", state_json)
        .bind("amount", u64::from(saga.amount) as i64)
        .bind("mint_url", saga.mint_url.to_string())
        .bind("unit", saga.unit.to_string())
        .bind("quote_id", saga.quote_id)
        .bind("updated_at", saga.updated_at as i64)
        .bind("data", data_json)
        .bind("new_version", saga.version as i64)
        .bind("expected_version", expected_version as i64)
        .execute(&*conn)
        .await?;

        // Return true if the update succeeded (version matched), false if version mismatch
        Ok(rows_affected > 0)
    }

    #[instrument(skip(self))]
    async fn delete_saga(&self, id: &uuid::Uuid) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query(r#"DELETE FROM wallet_sagas WHERE id = :id"#)?
            .bind("id", id.to_string())
            .execute(&*conn)
            .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_incomplete_sagas(&self) -> Result<Vec<wallet::WalletSaga>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let rows = query(
            r#"
            SELECT id, kind, state, amount, mint_url, unit, quote_id, created_at, updated_at, data, version
            FROM wallet_sagas
            ORDER BY created_at ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?;

        rows.into_iter().map(sql_row_to_wallet_saga).collect()
    }

    #[instrument(skip(self))]
    async fn reserve_proofs(
        &self,
        ys: Vec<PublicKey>,
        operation_id: &uuid::Uuid,
    ) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        for y in ys {
            let rows_affected = query(
                r#"
                UPDATE proof
                SET state = 'RESERVED', used_by_operation = :operation_id
                WHERE y = :y AND state = 'UNSPENT'
                "#,
            )?
            .bind("y", y.to_bytes().to_vec())
            .bind("operation_id", operation_id.to_string())
            .execute(&*conn)
            .await?;

            if rows_affected == 0 {
                return Err(database::Error::ProofNotUnspent);
            }
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn release_proofs(&self, operation_id: &uuid::Uuid) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query(
            r#"
            UPDATE proof
            SET state = 'UNSPENT', used_by_operation = NULL
            WHERE used_by_operation = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .execute(&*conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_reserved_proofs(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<ProofInfo>, database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let rows = query(
            r#"
            SELECT
                amount,
                unit,
                keyset_id,
                secret,
                c,
                witness,
                dleq_e,
                dleq_s,
                dleq_r,
                y,
                mint_url,
                state,
                spending_condition,
                used_by_operation,
                created_by_operation
            FROM proof
            WHERE used_by_operation = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .fetch_all(&*conn)
        .await?;

        rows.into_iter().map(sql_row_to_proof_info).collect()
    }

    #[instrument(skip(self))]
    async fn reserve_melt_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let rows_affected = query(
            r#"
            UPDATE melt_quote
            SET used_by_operation = :operation_id
            WHERE id = :quote_id AND used_by_operation IS NULL
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .bind("quote_id", quote_id)
        .execute(&*conn)
        .await?;

        if rows_affected == 0 {
            // Check if the quote exists
            let exists = query(
                r#"
                SELECT 1 FROM melt_quote WHERE id = :quote_id
                "#,
            )?
            .bind("quote_id", quote_id)
            .fetch_one(&*conn)
            .await?;

            if exists.is_none() {
                return Err(database::Error::UnknownQuote);
            }
            return Err(database::Error::QuoteAlreadyInUse);
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn release_melt_quote(&self, operation_id: &uuid::Uuid) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query(
            r#"
            UPDATE melt_quote
            SET used_by_operation = NULL
            WHERE used_by_operation = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .execute(&*conn)
        .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn reserve_mint_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let rows_affected = query(
            r#"
            UPDATE mint_quote
            SET used_by_operation = :operation_id
            WHERE id = :quote_id AND used_by_operation IS NULL
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .bind("quote_id", quote_id)
        .execute(&*conn)
        .await?;

        if rows_affected == 0 {
            // Check if the quote exists
            let exists = query(
                r#"
                SELECT 1 FROM mint_quote WHERE id = :quote_id
                "#,
            )?
            .bind("quote_id", quote_id)
            .fetch_one(&*conn)
            .await?;

            if exists.is_none() {
                return Err(database::Error::UnknownQuote);
            }
            return Err(database::Error::QuoteAlreadyInUse);
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn release_mint_quote(&self, operation_id: &uuid::Uuid) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query(
            r#"
            UPDATE mint_quote
            SET used_by_operation = NULL
            WHERE used_by_operation = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .execute(&*conn)
        .await?;

        Ok(())
    }

    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, database::Error> {
        crate::keyvalue::kv_read(&self.pool, primary_namespace, secondary_namespace, key).await
    }

    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, database::Error> {
        crate::keyvalue::kv_list(&self.pool, primary_namespace, secondary_namespace).await
    }

    async fn kv_write(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        crate::keyvalue::kv_write_standalone(
            &*conn,
            primary_namespace,
            secondary_namespace,
            key,
            value,
        )
        .await?;
        Ok(())
    }

    async fn kv_remove(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), database::Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        crate::keyvalue::kv_remove_standalone(&*conn, primary_namespace, secondary_namespace, key)
            .await?;
        Ok(())
    }
}

fn sql_row_to_mint_info(row: Vec<Column>) -> Result<MintInfo, Error> {
    unpack_into!(
        let (
            name,
            pubkey,
            version,
            description,
            description_long,
            contact,
            nuts,
            icon_url,
            motd,
            urls,
            mint_time,
            tos_url
        ) = row
    );

    Ok(MintInfo {
        name: column_as_nullable_string!(&name),
        pubkey: column_as_nullable_string!(&pubkey, |v| serde_json::from_str(v).ok(), |v| {
            serde_json::from_slice(v).ok()
        }),
        version: column_as_nullable_string!(&version).and_then(|v| serde_json::from_str(&v).ok()),
        description: column_as_nullable_string!(description),
        description_long: column_as_nullable_string!(description_long),
        contact: column_as_nullable_string!(contact, |v| serde_json::from_str(&v).ok()),
        nuts: column_as_nullable_string!(nuts, |v| serde_json::from_str(&v).ok())
            .unwrap_or_default(),
        urls: column_as_nullable_string!(urls, |v| serde_json::from_str(&v).ok()),
        icon_url: column_as_nullable_string!(icon_url),
        motd: column_as_nullable_string!(motd),
        time: column_as_nullable_number!(mint_time).map(|t| t),
        tos_url: column_as_nullable_string!(tos_url),
    })
}

#[instrument(skip_all)]
fn sql_row_to_keyset(row: Vec<Column>) -> Result<KeySetInfo, Error> {
    unpack_into!(
        let (
            id,
            unit,
            active,
            input_fee_ppk,
            final_expiry
        ) = row
    );

    Ok(KeySetInfo {
        id: column_as_string!(id, Id::from_str, Id::from_bytes),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        active: matches!(active, Column::Integer(1)),
        input_fee_ppk: column_as_nullable_number!(input_fee_ppk).unwrap_or(0),
        final_expiry: column_as_nullable_number!(final_expiry),
    })
}

fn sql_row_to_mint_quote(row: Vec<Column>) -> Result<MintQuote, Error> {
    unpack_into!(
        let (
            id,
            mint_url,
            amount,
            unit,
            request,
            state,
            expiry,
            secret_key,
            row_method,
            row_amount_minted,
            row_amount_paid,
            used_by_operation,
            version
        ) = row
    );

    let amount: Option<i64> = column_as_nullable_number!(amount);

    let amount_paid: u64 = column_as_number!(row_amount_paid);
    let amount_minted: u64 = column_as_number!(row_amount_minted);
    let expiry_val: u64 = column_as_number!(expiry);
    let version_val: u32 = column_as_number!(version);
    let payment_method =
        PaymentMethod::from_str(&column_as_string!(row_method)).map_err(Error::from)?;

    Ok(MintQuote {
        id: column_as_string!(id),
        mint_url: column_as_string!(mint_url, MintUrl::from_str),
        amount: amount.and_then(Amount::from_i64),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        request: column_as_string!(request),
        state: column_as_string!(state, MintQuoteState::from_str),
        expiry: expiry_val,
        secret_key: column_as_nullable_string!(secret_key, |s| SecretKey::from_str(&s).ok()),
        payment_method,
        amount_issued: Amount::from(amount_minted),
        amount_paid: Amount::from(amount_paid),
        used_by_operation: column_as_nullable_string!(used_by_operation),
        version: version_val,
    })
}

fn sql_row_to_melt_quote(row: Vec<Column>) -> Result<wallet::MeltQuote, Error> {
    unpack_into!(
        let (
            id,
            unit,
            amount,
            request,
            fee_reserve,
            state,
            expiry,
            payment_preimage,
            row_method,
            used_by_operation,
            version
        ) = row
    );

    let payment_method =
        PaymentMethod::from_str(&column_as_string!(row_method)).map_err(Error::from)?;

    let amount_val: u64 = column_as_number!(amount);
    let fee_reserve_val: u64 = column_as_number!(fee_reserve);
    let expiry_val: u64 = column_as_number!(expiry);
    let version_val: u32 = column_as_number!(version);

    Ok(wallet::MeltQuote {
        id: column_as_string!(id),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        amount: Amount::from(amount_val),
        request: column_as_string!(request),
        fee_reserve: Amount::from(fee_reserve_val),
        state: column_as_string!(state, MeltQuoteState::from_str),
        expiry: expiry_val,
        payment_preimage: column_as_nullable_string!(payment_preimage),
        payment_method,
        used_by_operation: column_as_nullable_string!(used_by_operation),
        version: version_val,
    })
}

fn sql_row_to_proof_info(row: Vec<Column>) -> Result<ProofInfo, Error> {
    unpack_into!(
        let (
            amount,
            unit,
            keyset_id,
            secret,
            c,
            witness,
            dleq_e,
            dleq_s,
            dleq_r,
            y,
            mint_url,
            state,
            spending_condition,
            used_by_operation,
            created_by_operation
        ) = row
    );

    let dleq = match (
        column_as_nullable_binary!(dleq_e),
        column_as_nullable_binary!(dleq_s),
        column_as_nullable_binary!(dleq_r),
    ) {
        (Some(e), Some(s), Some(r)) => {
            let e_key = SecretKey::from_slice(&e)?;
            let s_key = SecretKey::from_slice(&s)?;
            let r_key = SecretKey::from_slice(&r)?;

            Some(ProofDleq::new(e_key, s_key, r_key))
        }
        _ => None,
    };

    let amount: u64 = column_as_number!(amount);
    let proof = Proof {
        amount: Amount::from(amount),
        keyset_id: column_as_string!(keyset_id, Id::from_str),
        secret: column_as_string!(secret, Secret::from_str),
        witness: column_as_nullable_string!(witness, |v| { serde_json::from_str(&v).ok() }, |v| {
            serde_json::from_slice(&v).ok()
        }),
        c: column_as_string!(c, PublicKey::from_str, PublicKey::from_slice),
        dleq,
    };

    let used_by_operation =
        column_as_nullable_string!(used_by_operation).and_then(|id| Uuid::from_str(&id).ok());
    let created_by_operation =
        column_as_nullable_string!(created_by_operation).and_then(|id| Uuid::from_str(&id).ok());

    Ok(ProofInfo {
        proof,
        y: column_as_string!(y, PublicKey::from_str, PublicKey::from_slice),
        mint_url: column_as_string!(mint_url, MintUrl::from_str),
        state: column_as_string!(state, State::from_str),
        spending_condition: column_as_nullable_string!(
            spending_condition,
            |r| { serde_json::from_str(&r).ok() },
            |r| { serde_json::from_slice(&r).ok() }
        ),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        used_by_operation,
        created_by_operation,
    })
}

fn sql_row_to_wallet_saga(row: Vec<Column>) -> Result<wallet::WalletSaga, Error> {
    unpack_into!(
        let (
            id,
            kind,
            state,
            amount,
            mint_url,
            unit,
            quote_id,
            created_at,
            updated_at,
            data,
            version
        ) = row
    );

    let id_str: String = column_as_string!(id);
    let id = uuid::Uuid::parse_str(&id_str).map_err(|e| {
        Error::Database(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Invalid UUID: {}", e),
        )))
    })?;
    let kind_str: String = column_as_string!(kind);
    let state_json: String = column_as_string!(state);
    let amount: u64 = column_as_number!(amount);
    let mint_url: MintUrl = column_as_string!(mint_url, MintUrl::from_str);
    let unit: CurrencyUnit = column_as_string!(unit, CurrencyUnit::from_str);
    let quote_id: Option<String> = column_as_nullable_string!(quote_id);
    let created_at: u64 = column_as_number!(created_at);
    let updated_at: u64 = column_as_number!(updated_at);
    let data_json: String = column_as_string!(data);
    let version: u32 = column_as_number!(version);

    let kind = wallet::OperationKind::from_str(&kind_str).map_err(|_| {
        Error::Database(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Invalid operation kind: {}", kind_str),
        )))
    })?;
    let state: wallet::WalletSagaState = serde_json::from_str(&state_json).map_err(|e| {
        Error::Database(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to deserialize saga state: {}", e),
        )))
    })?;
    let data: wallet::OperationData = serde_json::from_str(&data_json).map_err(|e| {
        Error::Database(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to deserialize saga data: {}", e),
        )))
    })?;

    Ok(wallet::WalletSaga {
        id,
        kind,
        state,
        amount: Amount::from(amount),
        mint_url,
        unit,
        quote_id,
        created_at,
        updated_at,
        data,
        version,
    })
}

fn sql_row_to_transaction(row: Vec<Column>) -> Result<Transaction, Error> {
    unpack_into!(
        let (
            mint_url,
            direction,
            unit,
            amount,
            fee,
            ys,
            timestamp,
            memo,
            metadata,
            quote_id,
            payment_request,
            payment_proof,
            payment_method,
            saga_id
        ) = row
    );

    let amount: u64 = column_as_number!(amount);
    let fee: u64 = column_as_number!(fee);

    let saga_id: Option<Uuid> = column_as_nullable_string!(saga_id)
        .map(|id| Uuid::from_str(&id).ok())
        .flatten();

    Ok(Transaction {
        mint_url: column_as_string!(mint_url, MintUrl::from_str),
        direction: column_as_string!(direction, TransactionDirection::from_str),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        amount: Amount::from(amount),
        fee: Amount::from(fee),
        ys: column_as_binary!(ys)
            .chunks(33)
            .map(PublicKey::from_slice)
            .collect::<Result<Vec<_>, _>>()?,
        timestamp: column_as_number!(timestamp),
        memo: column_as_nullable_string!(memo),
        metadata: column_as_nullable_string!(metadata, |v| serde_json::from_str(&v).ok(), |v| {
            serde_json::from_slice(&v).ok()
        })
        .unwrap_or_default(),
        quote_id: column_as_nullable_string!(quote_id),
        payment_request: column_as_nullable_string!(payment_request),
        payment_proof: column_as_nullable_string!(payment_proof),
        payment_method: column_as_nullable_string!(payment_method)
            .map(|v| PaymentMethod::from_str(&v))
            .transpose()
            .map_err(Error::from)?,
        saga_id,
    })
}

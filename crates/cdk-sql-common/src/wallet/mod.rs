//! SQLite Wallet Database

use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::common::ProofInfo;
use cdk_common::database::{
    ConversionError, DbTransactionFinalizer, Error, WalletDatabase, WalletDatabaseTransaction,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{MeltQuoteState, MintQuoteState};
use cdk_common::secret::Secret;
use cdk_common::wallet::{self, MintQuote, Transaction, TransactionDirection, TransactionId};
use cdk_common::{
    database, Amount, CurrencyUnit, Id, KeySet, KeySetInfo, Keys, MintInfo, PaymentMethod, Proof,
    ProofDleq, PublicKey, SecretKey, SpendingConditions, State,
};
use tracing::instrument;

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

/// SQL Transaction Writer
pub struct SQLWalletTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    inner: ConnectionWithTransaction<RM::Connection, PooledResource<RM>>,
}

#[async_trait]
impl<'a, RM> WalletDatabaseTransaction<'a, Error> for SQLWalletTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    #[instrument(skip(self, mint_info))]
    async fn add_mint(
        &mut self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Error> {
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
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_mint(&mut self, mint_url: MintUrl) -> Result<(), Error> {
        query(r#"DELETE FROM mint WHERE mint_url=:mint_url"#)?
            .bind("mint_url", mint_url.to_string())
            .execute(&self.inner)
            .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn update_mint_url(
        &mut self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Error> {
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
            .execute(&self.inner)
            .await?;
        }

        Ok(())
    }

    #[instrument(skip(self, keysets))]
    async fn add_mint_keysets(
        &mut self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Error> {
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
            .execute(&self.inner)
            .await?;
        }

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_mint_quote(&mut self, quote: MintQuote) -> Result<(), Error> {
        query(
                r#"
    INSERT INTO mint_quote
    (id, mint_url, amount, unit, request, state, expiry, secret_key, payment_method, amount_issued, amount_paid)
    VALUES
    (:id, :mint_url, :amount, :unit, :request, :state, :expiry, :secret_key, :payment_method, :amount_issued, :amount_paid)
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
        amount_paid = excluded.amount_paid
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
            .execute(&self.inner).await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_mint_quote(&mut self, quote_id: &str) -> Result<(), Error> {
        query(r#"DELETE FROM mint_quote WHERE id=:id"#)?
            .bind("id", quote_id.to_string())
            .execute(&self.inner)
            .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_melt_quote(&mut self, quote: wallet::MeltQuote) -> Result<(), Error> {
        query(
            r#"
 INSERT INTO melt_quote
 (id, unit, amount, request, fee_reserve, state, expiry, payment_method)
 VALUES
 (:id, :unit, :amount, :request, :fee_reserve, :state, :expiry, :payment_method)
 ON CONFLICT(id) DO UPDATE SET
     unit = excluded.unit,
     amount = excluded.amount,
     request = excluded.request,
     fee_reserve = excluded.fee_reserve,
     state = excluded.state,
     expiry = excluded.expiry,
     payment_method = excluded.payment_method
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
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_melt_quote(&mut self, quote_id: &str) -> Result<(), Error> {
        query(r#"DELETE FROM melt_quote WHERE id=:id"#)?
            .bind("id", quote_id.to_owned())
            .execute(&self.inner)
            .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_keys(&mut self, keyset: KeySet) -> Result<(), Error> {
        // Recompute ID for verification
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
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn add_transaction(&mut self, transaction: Transaction) -> Result<(), Error> {
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
   (id, mint_url, direction, unit, amount, fee, ys, timestamp, memo, metadata, quote_id, payment_request, payment_proof)
   VALUES
   (:id, :mint_url, :direction, :unit, :amount, :fee, :ys, :timestamp, :memo, :metadata, :quote_id, :payment_request, :payment_proof)
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
       payment_proof = excluded.payment_proof
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
           .execute(&self.inner)
           .await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_transaction(&mut self, transaction_id: TransactionId) -> Result<(), Error> {
        query(r#"DELETE FROM transactions WHERE id=:id"#)?
            .bind("id", transaction_id.as_slice().to_vec())
            .execute(&self.inner)
            .await?;

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn increment_keyset_counter(&mut self, keyset_id: &Id, count: u32) -> Result<u32, Error> {
        // Lock the row and get current counter from keyset_counter table
        let current_counter = query(
            r#"
               SELECT counter
               FROM keyset_counter
               WHERE keyset_id=:keyset_id
               FOR UPDATE
               "#,
        )?
        .bind("keyset_id", keyset_id.to_string())
        .pluck(&self.inner)
        .await?
        .map(|n| Ok::<_, Error>(column_as_number!(n)))
        .transpose()?
        .unwrap_or(0);

        let new_counter = current_counter + count;

        // Upsert the new counter value
        query(
            r#"
               INSERT INTO keyset_counter (keyset_id, counter)
               VALUES (:keyset_id, :new_counter)
               ON CONFLICT(keyset_id) DO UPDATE SET
                   counter = excluded.counter
               "#,
        )?
        .bind("keyset_id", keyset_id.to_string())
        .bind("new_counter", new_counter)
        .execute(&self.inner)
        .await?;

        Ok(new_counter)
    }

    #[instrument(skip(self))]
    async fn remove_keys(&mut self, id: &Id) -> Result<(), Error> {
        query(r#"DELETE FROM key WHERE id = :id"#)?
            .bind("id", id.to_string())
            .pluck(&self.inner)
            .await?;

        Ok(())
    }

    async fn update_proofs_state(&mut self, ys: Vec<PublicKey>, state: State) -> Result<(), Error> {
        query("UPDATE proof SET state = :state WHERE y IN (:ys)")?
            .bind_vec("ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
            .bind("state", state.to_string())
            .execute(&self.inner)
            .await?;

        Ok(())
    }

    async fn update_proofs(
        &mut self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Error> {
        // TODO: Use a transaction for all these operations
        for proof in added {
            query(
                r#"
    INSERT INTO proof
    (y, mint_url, state, spending_condition, unit, amount, keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r)
    VALUES
    (:y, :mint_url, :state, :spending_condition, :unit, :amount, :keyset_id, :secret, :c, :witness, :dleq_e, :dleq_s, :dleq_r)
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
        dleq_r = excluded.dleq_r
    ;
            "#,
            )?
            .bind("y", proof.y.to_bytes().to_vec())
            .bind("mint_url", proof.mint_url.to_string())
            .bind("state",proof.state.to_string())
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
                    .map(|w| serde_json::to_string(&w).unwrap()),
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
            .execute(&self.inner).await?;
        }
        if !removed_ys.is_empty() {
            query(r#"DELETE FROM proof WHERE y IN (:ys)"#)?
                .bind_vec(
                    "ys",
                    removed_ys.iter().map(|y| y.to_bytes().to_vec()).collect(),
                )
                .execute(&self.inner)
                .await?;
        }

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keyset_by_id(&mut self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Error> {
        Ok(query(
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
            FOR UPDATE
            "#,
        )?
        .bind("id", keyset_id.to_string())
        .fetch_one(&self.inner)
        .await?
        .map(sql_row_to_keyset)
        .transpose()?)
    }

    #[instrument(skip(self))]
    async fn get_mint_quote(&mut self, quote_id: &str) -> Result<Option<MintQuote>, Error> {
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
                amount_paid
            FROM
                mint_quote
            WHERE
                id = :id
            FOR UPDATE
            "#,
        )?
        .bind("id", quote_id.to_string())
        .fetch_one(&self.inner)
        .await?
        .map(sql_row_to_mint_quote)
        .transpose()?)
    }

    #[instrument(skip(self))]
    async fn get_melt_quote(&mut self, quote_id: &str) -> Result<Option<wallet::MeltQuote>, Error> {
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
                   payment_method
               FROM
                   melt_quote
               WHERE
                   id=:id
                FOR UPDATE
               "#,
        )?
        .bind("id", quote_id.to_owned())
        .fetch_one(&self.inner)
        .await?
        .map(sql_row_to_melt_quote)
        .transpose()?)
    }

    #[instrument(skip(self, state, spending_conditions))]
    async fn get_proofs(
        &mut self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Error> {
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
                spending_condition
            FROM proof
            FOR UPDATE
        "#,
        )?
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .filter_map(|row| {
            let row = sql_row_to_proof_info(row).ok()?;

            // convert matches_conditions to SQL to lock only affected rows
            if row.matches_conditions(&mint_url, &unit, &state, &spending_conditions) {
                Some(row)
            } else {
                None
            }
        })
        .collect::<Vec<_>>())
    }
}

#[async_trait]
impl<RM> DbTransactionFinalizer for SQLWalletTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn commit(self: Box<Self>) -> Result<(), Error> {
        Ok(self.inner.commit().await?)
    }

    async fn rollback(self: Box<Self>) -> Result<(), Error> {
        Ok(self.inner.rollback().await?)
    }
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

        for id in keys_without_u32 {
            let id = column_as_string!(id.first().unwrap());

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

        for id in keysets_without_u32 {
            let id = column_as_string!(id.first().unwrap());

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
impl<RM> WalletDatabase for SQLWalletDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = database::Error;

    async fn begin_db_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn WalletDatabaseTransaction<'a, Self::Err> + Send + Sync + 'a>, Self::Err>
    {
        Ok(Box::new(SQLWalletTransaction {
            inner: ConnectionWithTransaction::new(
                self.pool.get().map_err(|e| Error::Database(Box::new(e)))?,
            )
            .await?,
        }))
    }

    #[instrument(skip(self))]
    async fn get_melt_quotes(&self) -> Result<Vec<wallet::MeltQuote>, Self::Err> {
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
                  payment_method
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
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, Self::Err> {
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
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, Self::Err> {
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
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err> {
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
    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
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
        .transpose()?)
    }

    #[instrument(skip(self))]
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
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
                amount_paid
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
        .transpose()?)
    }

    #[instrument(skip(self))]
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
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
                amount_paid
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
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<wallet::MeltQuote>, Self::Err> {
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
                payment_method
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
        .transpose()?)
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keys(&self, keyset_id: &Id) -> Result<Option<Keys>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
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
        .transpose()?)
    }

    #[instrument(skip(self, state, spending_conditions))]
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Self::Err> {
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
                spending_condition
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

    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        states: Option<Vec<State>>,
    ) -> Result<u64, Self::Err> {
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
    ) -> Result<Option<Transaction>, Self::Err> {
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
                payment_proof
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
    ) -> Result<Vec<Transaction>, Self::Err> {
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
                payment_proof
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
        input_fee_ppk: column_as_nullable_number!(input_fee_ppk).unwrap_or_default(),
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
            row_amount_paid
        ) = row
    );

    let amount: Option<i64> = column_as_nullable_number!(amount);

    let amount_paid: u64 = column_as_number!(row_amount_paid);
    let amount_minted: u64 = column_as_number!(row_amount_minted);
    let payment_method =
        PaymentMethod::from_str(&column_as_string!(row_method)).map_err(Error::from)?;

    Ok(MintQuote {
        id: column_as_string!(id),
        mint_url: column_as_string!(mint_url, MintUrl::from_str),
        amount: amount.and_then(Amount::from_i64),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        request: column_as_string!(request),
        state: column_as_string!(state, MintQuoteState::from_str),
        expiry: column_as_number!(expiry),
        secret_key: column_as_nullable_string!(secret_key)
            .map(|v| SecretKey::from_str(&v))
            .transpose()?,
        payment_method,
        amount_issued: amount_minted.into(),
        amount_paid: amount_paid.into(),
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
            row_method
        ) = row
    );

    let amount: u64 = column_as_number!(amount);
    let fee_reserve: u64 = column_as_number!(fee_reserve);

    let payment_method =
        PaymentMethod::from_str(&column_as_string!(row_method)).map_err(Error::from)?;

    Ok(wallet::MeltQuote {
        id: column_as_string!(id),
        amount: Amount::from(amount),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        request: column_as_string!(request),
        fee_reserve: Amount::from(fee_reserve),
        state: column_as_string!(state, MeltQuoteState::from_str),
        expiry: column_as_number!(expiry),
        payment_preimage: column_as_nullable_string!(payment_preimage),
        payment_method,
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
            spending_condition
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
            payment_proof
        ) = row
    );

    let amount: u64 = column_as_number!(amount);
    let fee: u64 = column_as_number!(fee);

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
    })
}

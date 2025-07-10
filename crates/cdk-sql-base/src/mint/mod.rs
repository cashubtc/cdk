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
use std::marker::PhantomData;
use std::str::FromStr;

use async_trait::async_trait;
use bitcoin::bip32::DerivationPath;
use cdk_common::common::QuoteTTL;
use cdk_common::database::{
    self, Error, MintDatabase, MintDbWriterFinalizer, MintKeyDatabaseTransaction, MintKeysDatabase,
    MintProofsDatabase, MintProofsTransaction, MintQuotesDatabase, MintQuotesTransaction,
    MintSignatureTransaction, MintSignaturesDatabase,
};
use cdk_common::mint::{self, MintKeySetInfo, MintQuote};
use cdk_common::nut00::ProofsMethods;
use cdk_common::nut05::QuoteState;
use cdk_common::secret::Secret;
use cdk_common::state::check_state_transition;
use cdk_common::util::unix_time;
use cdk_common::{
    Amount, BlindSignature, BlindSignatureDleq, CurrencyUnit, Id, MeltQuoteState, MintInfo,
    MintQuoteState, Proof, Proofs, PublicKey, SecretKey, State,
};
use lightning_invoice::Bolt11Invoice;
use migrations::MIGRATIONS;
use uuid::Uuid;

use crate::common::migrate;
use crate::database::{DatabaseConnector, DatabaseExecutor, DatabaseTransaction};
use crate::stmt::{query, Column};
use crate::{
    column_as_nullable_number, column_as_nullable_string, column_as_number, column_as_string,
    unpack_into,
};

#[cfg(feature = "auth")]
mod auth;

#[rustfmt::skip]
mod migrations;


#[cfg(feature = "auth")]
pub use auth::SQLMintAuthDatabase;

/// Mint SQL Database
#[derive(Debug, Clone)]
pub struct SQLMintDatabase<DB>
where
    DB: DatabaseConnector,
{
    db: DB,
}

/// SQL Transaction Writer
pub struct SQLTransaction<'a, T>
where
    T: DatabaseTransaction<'a>,
{
    inner: T,
    _phantom: PhantomData<&'a ()>,
}

#[inline(always)]
async fn get_current_states<C>(
    conn: &C,
    ys: &[PublicKey],
) -> Result<HashMap<PublicKey, State>, Error>
where
    C: DatabaseExecutor + Send + Sync,
{
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

#[inline(always)]
async fn set_to_config<C, V>(conn: &C, id: &str, value: &V) -> Result<(), Error>
where
    C: DatabaseExecutor + Send + Sync,
    V: ?Sized + serde::Serialize,
{
    query(
        r#"
        INSERT INTO config (id, value) VALUES (:id, :value)
            ON CONFLICT(id) DO UPDATE SET value = excluded.value
            "#,
    )?
    .bind("id", id.to_owned())
    .bind("value", serde_json::to_string(&value)?)
    .execute(conn)
    .await?;

    Ok(())
}

impl<DB> SQLMintDatabase<DB>
where
    DB: DatabaseConnector,
{
    /// Creates a new instance
    pub async fn new<X>(db: X) -> Result<Self, Error>
    where
        X: Into<DB>,
    {
        let db = db.into();
        Self::migrate(&db).await?;
        Ok(Self { db })
    }

    /// Migrate
    async fn migrate(conn: &DB) -> Result<(), Error> {
        let tx = conn.begin().await?;
        migrate(&tx, DB::name(), MIGRATIONS).await?;
        tx.commit().await?;
        Ok(())
    }

    #[inline(always)]
    async fn fetch_from_config<R>(&self, id: &str) -> Result<R, Error>
    where
        R: serde::de::DeserializeOwned,
    {
        let value = column_as_string!(query(r#"SELECT value FROM config WHERE id = :id LIMIT 1"#)?
            .bind("id", id.to_owned())
            .pluck(&self.db)
            .await?
            .ok_or(Error::UnknownQuoteTTL)?);

        Ok(serde_json::from_str(&value)?)
    }
}

#[async_trait]
impl<'a, T> database::MintTransaction<'a, Error> for SQLTransaction<'a, T>
where
    T: DatabaseTransaction<'a>,
{
    async fn set_mint_info(&mut self, mint_info: MintInfo) -> Result<(), Error> {
        Ok(set_to_config(&self.inner, "mint_info", &mint_info).await?)
    }

    async fn set_quote_ttl(&mut self, quote_ttl: QuoteTTL) -> Result<(), Error> {
        Ok(set_to_config(&self.inner, "quote_ttl", &quote_ttl).await?)
    }
}

#[async_trait]
impl<'a, T> MintDbWriterFinalizer for SQLTransaction<'a, T>
where
    T: DatabaseTransaction<'a>,
{
    type Err = Error;

    async fn commit(self: Box<Self>) -> Result<(), Error> {
        Ok(self.inner.commit().await?)
    }

    async fn rollback(self: Box<Self>) -> Result<(), Error> {
        Ok(self.inner.rollback().await?)
    }
}

#[async_trait]
impl<'a, T> MintKeyDatabaseTransaction<'a, Error> for SQLTransaction<'a, T>
where
    T: DatabaseTransaction<'a>,
{
    async fn add_keyset_info(&mut self, keyset: MintKeySetInfo) -> Result<(), Error> {
        query(
            r#"
        INSERT INTO
            keyset (
                id, unit, active, valid_from, valid_to, derivation_path,
                max_order, input_fee_ppk, derivation_path_index
            )
        VALUES (
            :id, :unit, :active, :valid_from, :valid_to, :derivation_path,
            :max_order, :input_fee_ppk, :derivation_path_index
        )
        ON CONFLICT(id) DO UPDATE SET
            unit = excluded.unit,
            active = excluded.active,
            valid_from = excluded.valid_from,
            valid_to = excluded.valid_to,
            derivation_path = excluded.derivation_path,
            max_order = excluded.max_order,
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
        .bind("input_fee_ppk", keyset.input_fee_ppk as i64)
        .bind("derivation_path_index", keyset.derivation_path_index)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn set_active_keyset(&mut self, unit: CurrencyUnit, id: Id) -> Result<(), Error> {
        query(r#"UPDATE keyset SET active=FALSE WHERE unit IS :unit"#)?
            .bind("unit", unit.to_string())
            .execute(&self.inner)
            .await?;

        query(r#"UPDATE keyset SET active=TRUE WHERE unit IS :unit AND id IS :id"#)?
            .bind("unit", unit.to_string())
            .bind("id", id.to_string())
            .execute(&self.inner)
            .await?;

        Ok(())
    }
}

#[async_trait]
impl<DB> MintKeysDatabase for SQLMintDatabase<DB>
where
    DB: DatabaseConnector,
{
    type Err = Error;

    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn MintKeyDatabaseTransaction<'a, Error> + Send + Sync + 'a>, Error> {
        Ok(Box::new(SQLTransaction {
            inner: self.db.begin().await?,
            _phantom: PhantomData,
        }))
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        Ok(
            query(r#" SELECT id FROM keyset WHERE active = 1 AND unit IS :unit"#)?
                .bind("unit", unit.to_string())
                .pluck(&self.db)
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
        Ok(query(r#"SELECT id, unit FROM keyset WHERE active = 1"#)?
            .fetch_all(&self.db)
            .await?
            .into_iter()
            .map(|row| {
                Ok((
                    column_as_string!(&row[1], CurrencyUnit::from_str),
                    column_as_string!(&row[0], Id::from_str, Id::from_bytes),
                ))
            })
            .collect::<Result<HashMap<_, _>, Error>>()?)
    }

    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err> {
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
                input_fee_ppk
            FROM
                keyset
                WHERE id=:id"#,
        )?
        .bind("id", id.to_string())
        .fetch_one(&self.db)
        .await?
        .map(sql_row_to_keyset_info)
        .transpose()?)
    }

    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err> {
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
                input_fee_ppk
            FROM
                keyset
            "#,
        )?
        .fetch_all(&self.db)
        .await?
        .into_iter()
        .map(sql_row_to_keyset_info)
        .collect::<Result<Vec<_>, _>>()?)
    }
}

#[async_trait]
impl<'a, T> MintQuotesTransaction<'a> for SQLTransaction<'a, T>
where
    T: DatabaseTransaction<'a>,
{
    type Err = Error;

    async fn add_or_replace_mint_quote(&mut self, quote: MintQuote) -> Result<(), Self::Err> {
        query(
            r#"
                INSERT OR REPLACE INTO mint_quote (
                    id, amount, unit, request, state, expiry, request_lookup_id,
                    pubkey, created_time, paid_time, issued_time
                )
                VALUES (
                    :id, :amount, :unit, :request, :state, :expiry, :request_lookup_id,
                    :pubkey, :created_time, :paid_time, :issued_time
                )
            "#,
        )?
        .bind("id", quote.id.to_string())
        .bind("amount", u64::from(quote.amount) as i64)
        .bind("unit", quote.unit.to_string())
        .bind("request", quote.request)
        .bind("state", quote.state.to_string())
        .bind("expiry", quote.expiry as i64)
        .bind("request_lookup_id", quote.request_lookup_id)
        .bind("pubkey", quote.pubkey.map(|p| p.to_string()))
        .bind("created_time", quote.created_time as i64)
        .bind("paid_time", quote.paid_time.map(|t| t as i64))
        .bind("issued_time", quote.issued_time.map(|t| t as i64))
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn remove_mint_quote(&mut self, quote_id: &Uuid) -> Result<(), Self::Err> {
        query(r#"DELETE FROM mint_quote WHERE id=:id"#)?
            .bind("id", quote_id.as_hyphenated().to_string())
            .execute(&self.inner)
            .await?;
        Ok(())
    }

    async fn add_melt_quote(&mut self, quote: mint::MeltQuote) -> Result<(), Self::Err> {
        // First try to find and replace any expired UNPAID quotes with the same request_lookup_id
        let current_time = unix_time();
        let row_affected = query(
            r#"
            DELETE FROM melt_quote
            WHERE request_lookup_id = :request_lookup_id
            AND state = :state
            AND expiry < :current_time
            "#,
        )?
        .bind("request_lookup_id", quote.request_lookup_id.to_string())
        .bind("state", MeltQuoteState::Unpaid.to_string())
        .bind("current_time", current_time as i64)
        .execute(&self.inner)
        .await?;

        if row_affected > 0 {
            tracing::info!("Received new melt quote for existing invoice with expired quote.");
        }

        // Now insert the new quote
        query(
            r#"
            INSERT INTO melt_quote
            (
                id, unit, amount, request, fee_reserve, state,
                expiry, payment_preimage, request_lookup_id, msat_to_pay,
                created_time, paid_time
            )
            VALUES
            (
                :id, :unit, :amount, :request, :fee_reserve, :state,
                :expiry, :payment_preimage, :request_lookup_id, :msat_to_pay,
                :created_time, :paid_time
            )
        "#,
        )?
        .bind("id", quote.id.to_string())
        .bind("unit", quote.unit.to_string())
        .bind("amount", u64::from(quote.amount) as i64)
        .bind("request", quote.request)
        .bind("fee_reserve", u64::from(quote.fee_reserve) as i64)
        .bind("state", quote.state.to_string())
        .bind("expiry", quote.expiry as i64)
        .bind("payment_preimage", quote.payment_preimage)
        .bind("request_lookup_id", quote.request_lookup_id)
        .bind(
            "msat_to_pay",
            quote.msat_to_pay.map(|a| u64::from(a) as i64),
        )
        .bind("created_time", quote.created_time as i64)
        .bind("paid_time", quote.paid_time.map(|t| t as i64))
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn update_melt_quote_request_lookup_id(
        &mut self,
        quote_id: &Uuid,
        new_request_lookup_id: &str,
    ) -> Result<(), Self::Err> {
        query(r#"UPDATE melt_quote SET request_lookup_id = :new_req_id WHERE id = :id"#)?
            .bind("new_req_id", new_request_lookup_id.to_owned())
            .bind("id", quote_id.as_hyphenated().to_string())
            .execute(&self.inner)
            .await?;
        Ok(())
    }

    async fn update_melt_quote_state(
        &mut self,
        quote_id: &Uuid,
        state: MeltQuoteState,
    ) -> Result<(MeltQuoteState, mint::MeltQuote), Self::Err> {
        let mut quote = query(
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
                request_lookup_id,
                msat_to_pay,
                created_time,
                paid_time
            FROM
                melt_quote
            WHERE
                id=:id
                AND state != :state
            "#,
        )?
        .bind("id", quote_id.as_hyphenated().to_string())
        .bind("state", state.to_string())
        .fetch_one(&self.inner)
        .await?
        .map(sql_row_to_melt_quote)
        .transpose()?
        .ok_or(Error::QuoteNotFound)?;

        let rec = if state == MeltQuoteState::Paid {
            let current_time = unix_time();
            query(r#"UPDATE melt_quote SET state = :state, paid_time = :paid_time WHERE id = :id"#)?
                .bind("state", state.to_string())
                .bind("paid_time", current_time as i64)
                .bind("id", quote_id.as_hyphenated().to_string())
                .execute(&self.inner)
                .await
        } else {
            query(r#"UPDATE melt_quote SET state = :state WHERE id = :id"#)?
                .bind("state", state.to_string())
                .bind("id", quote_id.as_hyphenated().to_string())
                .execute(&self.inner)
                .await
        };

        match rec {
            Ok(_) => {}
            Err(err) => {
                tracing::error!("SQL Could not update melt quote");
                return Err(err);
            }
        };

        let old_state = quote.state;
        quote.state = state;

        Ok((old_state, quote))
    }

    async fn remove_melt_quote(&mut self, quote_id: &Uuid) -> Result<(), Self::Err> {
        query(
            r#"
            DELETE FROM melt_quote
            WHERE id=?
            "#,
        )?
        .bind("id", quote_id.as_hyphenated().to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn update_mint_quote_state(
        &mut self,
        quote_id: &Uuid,
        state: MintQuoteState,
    ) -> Result<MintQuoteState, Self::Err> {
        let quote = query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                state,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                paid_time,
                issued_time
            FROM
                mint_quote
            WHERE id = :id"#,
        )?
        .bind("id", quote_id.as_hyphenated().to_string())
        .fetch_one(&self.inner)
        .await?
        .map(sql_row_to_mint_quote)
        .ok_or(Error::QuoteNotFound)??;

        let update_query = match state {
            MintQuoteState::Paid => {
                r#"UPDATE mint_quote SET state = :state, paid_time = :current_time WHERE id = :quote_id"#
            }
            MintQuoteState::Issued => {
                r#"UPDATE mint_quote SET state = :state, issued_time = :current_time WHERE id = :quote_id"#
            }
            _ => r#"UPDATE mint_quote SET state = :state WHERE id = :quote_id"#,
        };

        let current_time = unix_time();

        let update = match state {
            MintQuoteState::Paid => query(update_query)?
                .bind("state", state.to_string())
                .bind("current_time", current_time as i64)
                .bind("quote_id", quote_id.as_hyphenated().to_string()),
            MintQuoteState::Issued => query(update_query)?
                .bind("state", state.to_string())
                .bind("current_time", current_time as i64)
                .bind("quote_id", quote_id.as_hyphenated().to_string()),
            _ => query(update_query)?
                .bind("state", state.to_string())
                .bind("quote_id", quote_id.as_hyphenated().to_string()),
        };

        match update.execute(&self.inner).await {
            Ok(_) => Ok(quote.state),
            Err(err) => {
                tracing::error!("SQL Could not update keyset: {:?}", err);

                return Err(err);
            }
        }
    }

    async fn get_mint_quote(&mut self, quote_id: &Uuid) -> Result<Option<MintQuote>, Self::Err> {
        Ok(query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                state,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                paid_time,
                issued_time
            FROM
                mint_quote
            WHERE id = :id"#,
        )?
        .bind("id", quote_id.as_hyphenated().to_string())
        .fetch_one(&self.inner)
        .await?
        .map(sql_row_to_mint_quote)
        .transpose()?)
    }

    async fn get_melt_quote(
        &mut self,
        quote_id: &Uuid,
    ) -> Result<Option<mint::MeltQuote>, Self::Err> {
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
                request_lookup_id,
                msat_to_pay,
                created_time,
                paid_time
            FROM
                melt_quote
            WHERE
                id=:id
            "#,
        )?
        .bind("id", quote_id.as_hyphenated().to_string())
        .fetch_one(&self.inner)
        .await?
        .map(sql_row_to_melt_quote)
        .transpose()?)
    }

    async fn get_mint_quote_by_request(
        &mut self,
        request: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        Ok(query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                state,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                paid_time,
                issued_time
            FROM
                mint_quote
            WHERE request = :request"#,
        )?
        .bind("request", request.to_owned())
        .fetch_one(&self.inner)
        .await?
        .map(sql_row_to_mint_quote)
        .transpose()?)
    }
}

#[async_trait]
impl<DB> MintQuotesDatabase for SQLMintDatabase<DB>
where
    DB: DatabaseConnector,
{
    type Err = Error;

    async fn get_mint_quote(&self, quote_id: &Uuid) -> Result<Option<MintQuote>, Self::Err> {
        Ok(query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                state,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                paid_time,
                issued_time
            FROM
                mint_quote
            WHERE id = :id"#,
        )?
        .bind("id", quote_id.as_hyphenated().to_string())
        .fetch_one(&self.db)
        .await?
        .map(sql_row_to_mint_quote)
        .transpose()?)
    }

    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        Ok(query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                state,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                paid_time,
                issued_time
            FROM
                mint_quote
            WHERE request = :request"#,
        )?
        .bind("request", request.to_owned())
        .fetch_one(&self.db)
        .await?
        .map(sql_row_to_mint_quote)
        .transpose()?)
    }

    async fn get_mint_quote_by_request_lookup_id(
        &self,
        request_lookup_id: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        Ok(query(
            r#"
            SELECT
                id,
                amount,
                unit,
                request,
                state,
                expiry,
                request_lookup_id,
                pubkey,
                created_time,
                paid_time,
                issued_time
            FROM
                mint_quote
            WHERE request_lookup_id = :request_lookup_id"#,
        )?
        .bind("request_lookup_id", request_lookup_id.to_owned())
        .fetch_one(&self.db)
        .await?
        .map(sql_row_to_mint_quote)
        .transpose()?)
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        Ok(query(
            r#"
                   SELECT
                       id,
                       amount,
                       unit,
                       request,
                       state,
                       expiry,
                       request_lookup_id,
                       pubkey,
                       created_time,
                       paid_time,
                       issued_time
                   FROM
                       mint_quote
                  "#,
        )?
        .fetch_all(&self.db)
        .await?
        .into_iter()
        .map(sql_row_to_mint_quote)
        .collect::<Result<Vec<_>, _>>()?)
    }

    async fn get_mint_quotes_with_state(
        &self,
        state: MintQuoteState,
    ) -> Result<Vec<MintQuote>, Self::Err> {
        Ok(query(
            r#"
                   SELECT
                       id,
                       amount,
                       unit,
                       request,
                       state,
                       expiry,
                       request_lookup_id,
                       pubkey,
                       created_time,
                       paid_time,
                       issued_time
                   FROM
                       mint_quote
                    WHERE
                        state = :state
                  "#,
        )?
        .bind("state", state.to_string())
        .fetch_all(&self.db)
        .await?
        .into_iter()
        .map(sql_row_to_mint_quote)
        .collect::<Result<Vec<_>, _>>()?)
    }

    async fn get_melt_quote(&self, quote_id: &Uuid) -> Result<Option<mint::MeltQuote>, Self::Err> {
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
                request_lookup_id,
                msat_to_pay,
                created_time,
                paid_time
            FROM
                melt_quote
            WHERE
                id=:id
            "#,
        )?
        .bind("id", quote_id.as_hyphenated().to_string())
        .fetch_one(&self.db)
        .await?
        .map(sql_row_to_melt_quote)
        .transpose()?)
    }

    async fn get_melt_quotes(&self) -> Result<Vec<mint::MeltQuote>, Self::Err> {
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
                request_lookup_id,
                msat_to_pay,
                created_time,
                paid_time
            FROM
                melt_quote
            "#,
        )?
        .fetch_all(&self.db)
        .await?
        .into_iter()
        .map(sql_row_to_melt_quote)
        .collect::<Result<Vec<_>, _>>()?)
    }
}

#[async_trait]
impl<'a, T> MintProofsTransaction<'a> for SQLTransaction<'a, T>
where
    T: DatabaseTransaction<'a>,
{
    type Err = Error;

    async fn add_proofs(
        &mut self,
        proofs: Proofs,
        quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err> {
        let current_time = unix_time();

        // Check any previous proof, this query should return None in order to proceed storing
        // Any result here would error
        match query(r#"SELECT state FROM proof WHERE y IN (:ys) LIMIT 1"#)?
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
            Some(State::Spent) => Err(Error::AttemptUpdateSpentProof),
            Some(_) => Err(Error::Duplicate),
            None => Ok(()), // no previous record
        }?;

        for proof in proofs {
            query(
                r#"
                  INSERT INTO proof
                  (y, amount, keyset_id, secret, c, witness, state, quote_id, created_time)
                  VALUES
                  (:y, :amount, :keyset_id, :secret, :c, :witness, :state, :quote_id, :created_time)
                  "#,
            )?
            .bind("y", proof.y()?.to_bytes().to_vec())
            .bind("amount", u64::from(proof.amount) as i64)
            .bind("keyset_id", proof.keyset_id.to_string())
            .bind("secret", proof.secret.to_string())
            .bind("c", proof.c.to_bytes().to_vec())
            .bind(
                "witness",
                proof.witness.map(|w| serde_json::to_string(&w).unwrap()),
            )
            .bind("state", "UNSPENT".to_string())
            .bind("quote_id", quote_id.map(|q| q.hyphenated().to_string()))
            .bind("created_time", current_time as i64)
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
            return Err(Error::ProofNotFound);
        }

        for state in current_states.values() {
            check_state_transition(*state, new_state)?;
        }

        query(r#"UPDATE proof SET state = :new_state WHERE y IN (:ys)"#)?
            .bind("new_state", new_state.to_string())
            .bind_vec("ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
            .execute(&self.inner)
            .await?;

        Ok(ys.iter().map(|y| current_states.remove(y)).collect())
    }

    async fn remove_proofs(
        &mut self,
        ys: &[PublicKey],
        _quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err> {
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
}

#[async_trait]
impl<DB> MintProofsDatabase for SQLMintDatabase<DB>
where
    DB: DatabaseConnector,
{
    type Err = Error;

    async fn get_proofs_by_ys(&self, ys: &[PublicKey]) -> Result<Vec<Option<Proof>>, Self::Err> {
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
        .fetch_all(&self.db)
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

    async fn get_proof_ys_by_quote_id(&self, quote_id: &Uuid) -> Result<Vec<PublicKey>, Self::Err> {
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
        .bind("quote_id", quote_id.as_hyphenated().to_string())
        .fetch_all(&self.db)
        .await?
        .into_iter()
        .map(sql_row_to_proof)
        .collect::<Result<Vec<Proof>, _>>()?
        .ys()?)
    }

    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err> {
        let mut current_states = get_current_states(&self.db, ys).await?;

        Ok(ys.iter().map(|y| current_states.remove(y)).collect())
    }

    async fn get_proofs_by_keyset_id(
        &self,
        keyset_id: &Id,
    ) -> Result<(Proofs, Vec<Option<State>>), Self::Err> {
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
                keyset_id=?
            "#,
        )?
        .bind("keyset_id", keyset_id.to_string())
        .fetch_all(&self.db)
        .await?
        .into_iter()
        .map(sql_row_to_proof_with_state)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .unzip())
    }
}

#[async_trait]
impl<'a, T> MintSignatureTransaction<'a> for SQLTransaction<'a, T>
where
    T: DatabaseTransaction<'a>,
{
    type Err = Error;

    async fn add_blind_signatures(
        &mut self,
        blinded_messages: &[PublicKey],
        blind_signatures: &[BlindSignature],
        quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err> {
        let current_time = unix_time();

        for (message, signature) in blinded_messages.iter().zip(blind_signatures) {
            query(
                r#"
                    INSERT INTO blind_signature
                    (blinded_message, amount, keyset_id, c, quote_id, dleq_e, dleq_s, created_time)
                    VALUES
                    (:blinded_message, :amount, :keyset_id, :c, :quote_id, :dleq_e, :dleq_s, :created_time)
                "#,
            )?
            .bind("blinded_message", message.to_bytes().to_vec())
            .bind("amount", u64::from(signature.amount) as i64)
            .bind("keyset_id", signature.keyset_id.to_string())
            .bind("c", signature.c.to_bytes().to_vec())
            .bind("quote_id", quote_id.map(|q| q.hyphenated().to_string()))
            .bind(
                "dleq_e",
                signature.dleq.as_ref().map(|dleq| dleq.e.to_secret_hex()),
            )
            .bind(
                "dleq_s",
                signature.dleq.as_ref().map(|dleq| dleq.s.to_secret_hex()),
            )
            .bind("created_time", current_time as i64)
            .execute(&self.inner)
            .await?;
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
            WHERE blinded_message IN (:y)
            "#,
        )?
        .bind_vec(
            "y",
            blinded_messages
                .iter()
                .map(|y| y.to_bytes().to_vec())
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
impl<DB> MintSignaturesDatabase for SQLMintDatabase<DB>
where
    DB: DatabaseConnector,
{
    type Err = Error;

    async fn get_blind_signatures(
        &self,
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
            WHERE blinded_message IN (:blinded_message)
            "#,
        )?
        .bind_vec(
            "blinded_message",
            blinded_messages
                .iter()
                .map(|b_| b_.to_bytes().to_vec())
                .collect(),
        )
        .fetch_all(&self.db)
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
                keyset_id=:keyset_id
            "#,
        )?
        .bind("keyset_id", keyset_id.to_string())
        .fetch_all(&self.db)
        .await?
        .into_iter()
        .map(sql_row_to_blind_signature)
        .collect::<Result<Vec<BlindSignature>, _>>()?)
    }

    /// Get [`BlindSignature`]s for quote
    async fn get_blind_signatures_for_quote(
        &self,
        quote_id: &Uuid,
    ) -> Result<Vec<BlindSignature>, Self::Err> {
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
                quote_id=:quote_id
            "#,
        )?
        .bind("quote_id", quote_id.to_string())
        .fetch_all(&self.db)
        .await?
        .into_iter()
        .map(sql_row_to_blind_signature)
        .collect::<Result<Vec<BlindSignature>, _>>()?)
    }
}

#[async_trait]
impl<DB> MintDatabase<Error> for SQLMintDatabase<DB>
where
    DB: DatabaseConnector,
{
    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn database::MintTransaction<'a, Error> + Send + Sync + 'a>, Error> {
        Ok(Box::new(SQLTransaction {
            inner: self.db.begin().await?,
            _phantom: PhantomData,
        }))
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        Ok(self.fetch_from_config("mint_info").await?)
    }

    async fn get_quote_ttl(&self) -> Result<QuoteTTL, Error> {
        Ok(self.fetch_from_config("quote_ttl").await?)
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
            row_keyset_ppk
        ) = row
    );

    Ok(MintKeySetInfo {
        id: column_as_string!(id, Id::from_str, Id::from_bytes),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        active: matches!(active, Column::Integer(1)),
        valid_from: column_as_number!(valid_from),
        derivation_path: column_as_string!(derivation_path, DerivationPath::from_str),
        derivation_path_index: column_as_nullable_number!(derivation_path_index),
        max_order: column_as_number!(max_order),
        input_fee_ppk: column_as_number!(row_keyset_ppk),
        final_expiry: column_as_nullable_number!(valid_to),
    })
}

fn sql_row_to_mint_quote(row: Vec<Column>) -> Result<MintQuote, Error> {
    unpack_into!(
        let (
            id, amount, unit, request, state, expiry, request_lookup_id,
            pubkey, created_time, paid_time, issued_time
        ) = row
    );

    let request = column_as_string!(&request);
    let request_lookup_id = column_as_nullable_string!(&request_lookup_id).unwrap_or_else(|| {
        Bolt11Invoice::from_str(&request)
            .map(|invoice| invoice.payment_hash().to_string())
            .unwrap_or_else(|_| request.clone())
    });

    let pubkey = column_as_nullable_string!(&pubkey)
        .map(|pk| PublicKey::from_hex(&pk))
        .transpose()?;

    let id = column_as_string!(id);
    let amount: u64 = column_as_number!(amount);

    Ok(MintQuote {
        id: Uuid::parse_str(&id).map_err(|_| Error::InvalidUuid(id))?,
        amount: Amount::from(amount),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        request,
        state: column_as_string!(state, MintQuoteState::from_str),
        expiry: column_as_number!(expiry),
        request_lookup_id,
        pubkey,
        created_time: column_as_number!(created_time),
        paid_time: column_as_nullable_number!(paid_time).map(|p| p),
        issued_time: column_as_nullable_number!(issued_time).map(|p| p),
    })
}

fn sql_row_to_melt_quote(row: Vec<Column>) -> Result<mint::MeltQuote, Error> {
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
            request_lookup_id,
            msat_to_pay,
            created_time,
            paid_time
        ) = row
    );

    let id = column_as_string!(id);
    let amount: u64 = column_as_number!(amount);
    let fee_reserve: u64 = column_as_number!(fee_reserve);

    let request = column_as_string!(&request);
    let request_lookup_id = column_as_nullable_string!(&request_lookup_id).unwrap_or_else(|| {
        Bolt11Invoice::from_str(&request)
            .map(|invoice| invoice.payment_hash().to_string())
            .unwrap_or_else(|_| request.clone())
    });
    let msat_to_pay: Option<u64> = column_as_nullable_number!(msat_to_pay);

    Ok(mint::MeltQuote {
        id: Uuid::parse_str(&id).map_err(|_| Error::InvalidUuid(id))?,
        amount: Amount::from(amount),
        fee_reserve: Amount::from(fee_reserve),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        request,
        payment_preimage: column_as_nullable_string!(payment_preimage),
        msat_to_pay: msat_to_pay.map(Amount::from),
        state: column_as_string!(state, QuoteState::from_str),
        expiry: column_as_number!(expiry),
        request_lookup_id,
        created_time: column_as_number!(created_time),
        paid_time: column_as_nullable_number!(paid_time).map(|p| p),
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

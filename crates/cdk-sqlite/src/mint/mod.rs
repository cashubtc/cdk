//! SQLite Mint

use std::collections::HashMap;
use std::ops::DerefMut;
use std::path::Path;
use std::str::FromStr;

use async_rusqlite::{query, DatabaseExecutor, Transaction};
use async_trait::async_trait;
use bitcoin::bip32::DerivationPath;
use cdk_common::common::QuoteTTL;
use cdk_common::database::{
    self, MintDatabase, MintDbWriterFinalizer, MintKeyDatabaseTransaction, MintKeysDatabase,
    MintProofsDatabase, MintProofsTransaction, MintQuotesDatabase, MintQuotesTransaction,
    MintSignatureTransaction, MintSignaturesDatabase,
};
use cdk_common::mint::{
    self, IncomingPayment, Issuance, MeltPaymentRequest, MeltQuote, MintKeySetInfo, MintQuote,
};
use cdk_common::nut00::ProofsMethods;
use cdk_common::payment::PaymentIdentifier;
use cdk_common::secret::Secret;
use cdk_common::state::check_state_transition;
use cdk_common::util::unix_time;
use cdk_common::{
    Amount, BlindSignature, BlindSignatureDleq, CurrencyUnit, Id, MeltQuoteState, MintInfo,
    PaymentMethod, Proof, Proofs, PublicKey, SecretKey, State,
};
use error::Error;
use lightning_invoice::Bolt11Invoice;
use tracing::instrument;
use uuid::Uuid;

use crate::common::{create_sqlite_pool, migrate};
use crate::stmt::Column;
use crate::{
    column_as_nullable_number, column_as_nullable_string, column_as_number, column_as_string,
    unpack_into,
};

mod async_rusqlite;
#[cfg(feature = "auth")]
mod auth;
pub mod error;
pub mod memory;

#[rustfmt::skip]
mod migrations;

#[cfg(feature = "auth")]
pub use auth::MintSqliteAuthDatabase;

/// Mint SQLite Database
#[derive(Debug, Clone)]
pub struct MintSqliteDatabase {
    pool: async_rusqlite::AsyncRusqlite,
}

#[inline(always)]
async fn get_current_states<C>(
    conn: &C,
    ys: &[PublicKey],
) -> Result<HashMap<PublicKey, State>, Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    query(r#"SELECT y, state FROM proof WHERE y IN (:ys)"#)
        .bind_vec(":ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
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
async fn set_to_config<T, C>(conn: &C, id: &str, value: &T) -> Result<(), Error>
where
    T: ?Sized + serde::Serialize,
    C: DatabaseExecutor + Send + Sync,
{
    query(
        r#"
        INSERT INTO config (id, value) VALUES (:id, :value)
            ON CONFLICT(id) DO UPDATE SET value = excluded.value
            "#,
    )
    .bind(":id", id.to_owned())
    .bind(":value", serde_json::to_string(&value)?)
    .execute(conn)
    .await?;

    Ok(())
}

#[inline(always)]
async fn get_mint_quote_payments<C>(
    conn: &C,
    quote_id: &Uuid,
) -> Result<Vec<IncomingPayment>, Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    // Get payment IDs and timestamps from the mint_quote_payments table
    query(
        r#"
SELECT payment_id, timestamp, amount
FROM mint_quote_payments
WHERE quote_id=:quote_id;
            "#,
    )
    .bind(":quote_id", quote_id.as_hyphenated().to_string())
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
async fn get_mint_quote_issuance<C>(conn: &C, quote_id: &Uuid) -> Result<Vec<Issuance>, Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    // Get payment IDs and timestamps from the mint_quote_payments table
    query(
        r#"
SELECT amount, timestamp
FROM mint_quote_issued
WHERE quote_id=:quote_id;
            "#,
    )
    .bind(":quote_id", quote_id.as_hyphenated().to_string())
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

impl MintSqliteDatabase {
    /// Create new [`MintSqliteDatabase`]
    #[cfg(not(feature = "sqlcipher"))]
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let pool = create_sqlite_pool(path.as_ref().to_str().ok_or(Error::InvalidDbPath)?);
        migrate(pool.get()?.deref_mut(), migrations::MIGRATIONS)?;

        Ok(Self {
            pool: async_rusqlite::AsyncRusqlite::new(pool),
        })
    }

    /// Create new [`MintSqliteDatabase`]
    #[cfg(feature = "sqlcipher")]
    pub async fn new<P: AsRef<Path>>(path: P, password: String) -> Result<Self, Error> {
        let pool = create_sqlite_pool(
            path.as_ref().to_str().ok_or(Error::InvalidDbPath)?,
            password,
        );
        migrate(pool.get()?.deref_mut(), migrations::MIGRATIONS)?;

        Ok(Self {
            pool: async_rusqlite::AsyncRusqlite::new(pool),
        })
    }

    #[inline(always)]
    async fn fetch_from_config<T>(&self, id: &str) -> Result<T, Error>
    where
        T: serde::de::DeserializeOwned,
    {
        let value = column_as_string!(query(r#"SELECT value FROM config WHERE id = :id LIMIT 1"#)
            .bind(":id", id.to_owned())
            .pluck(&self.pool)
            .await?
            .ok_or_else(|| match id {
                "mint_info" => Error::UnknownMintInfo,
                "quote_ttl" => Error::UnknownQuoteTTL,
                unknown => Error::UnknownConfigKey(unknown.to_string()),
            })?);

        Ok(serde_json::from_str(&value)?)
    }
}

/// Sqlite Writer
pub struct SqliteTransaction<'a> {
    inner: Transaction<'a>,
}

#[async_trait]
impl<'a> database::MintTransaction<'a, database::Error> for SqliteTransaction<'a> {
    async fn set_mint_info(&mut self, mint_info: MintInfo) -> Result<(), database::Error> {
        Ok(set_to_config(&self.inner, "mint_info", &mint_info).await?)
    }

    async fn set_quote_ttl(&mut self, quote_ttl: QuoteTTL) -> Result<(), database::Error> {
        Ok(set_to_config(&self.inner, "quote_ttl", &quote_ttl).await?)
    }
}

#[async_trait]
impl MintDbWriterFinalizer for SqliteTransaction<'_> {
    type Err = database::Error;

    async fn commit(self: Box<Self>) -> Result<(), database::Error> {
        Ok(self.inner.commit().await?)
    }

    async fn rollback(self: Box<Self>) -> Result<(), database::Error> {
        Ok(self.inner.rollback().await?)
    }
}

#[async_trait]
impl<'a> MintKeyDatabaseTransaction<'a, database::Error> for SqliteTransaction<'a> {
    async fn add_keyset_info(&mut self, keyset: MintKeySetInfo) -> Result<(), database::Error> {
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
        )
        .bind(":id", keyset.id.to_string())
        .bind(":unit", keyset.unit.to_string())
        .bind(":active", keyset.active)
        .bind(":valid_from", keyset.valid_from as i64)
        .bind(":valid_to", keyset.final_expiry.map(|v| v as i64))
        .bind(":derivation_path", keyset.derivation_path.to_string())
        .bind(":max_order", keyset.max_order)
        .bind(":input_fee_ppk", keyset.input_fee_ppk as i64)
        .bind(":derivation_path_index", keyset.derivation_path_index)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn set_active_keyset(
        &mut self,
        unit: CurrencyUnit,
        id: Id,
    ) -> Result<(), database::Error> {
        query(r#"UPDATE keyset SET active=FALSE WHERE unit IS :unit"#)
            .bind(":unit", unit.to_string())
            .execute(&self.inner)
            .await?;

        query(r#"UPDATE keyset SET active=TRUE WHERE unit IS :unit AND id IS :id"#)
            .bind(":unit", unit.to_string())
            .bind(":id", id.to_string())
            .execute(&self.inner)
            .await?;

        Ok(())
    }
}

#[async_trait]
impl MintKeysDatabase for MintSqliteDatabase {
    type Err = database::Error;

    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<
        Box<dyn MintKeyDatabaseTransaction<'a, database::Error> + Send + Sync + 'a>,
        database::Error,
    > {
        Ok(Box::new(SqliteTransaction {
            inner: self.pool.begin().await?,
        }))
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        Ok(
            query(r#" SELECT id FROM keyset WHERE active = 1 AND unit IS :unit"#)
                .bind(":unit", unit.to_string())
                .pluck(&self.pool)
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
        Ok(query(r#"SELECT id, unit FROM keyset WHERE active = 1"#)
            .fetch_all(&self.pool)
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
        )
        .bind(":id", id.to_string())
        .fetch_one(&self.pool)
        .await?
        .map(sqlite_row_to_keyset_info)
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
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(sqlite_row_to_keyset_info)
        .collect::<Result<Vec<_>, _>>()?)
    }
}

#[async_trait]
impl<'a> MintQuotesTransaction<'a> for SqliteTransaction<'a> {
    type Err = database::Error;

    #[instrument(skip(self))]
    async fn increment_mint_quote_amount_paid(
        &mut self,
        quote_id: &Uuid,
        amount_paid: Amount,
        payment_id: String,
    ) -> Result<Amount, Self::Err> {
        // Check if payment_id already exists in mint_quote_payments
        let exists = query(
            r#"
            SELECT payment_id
            FROM mint_quote_payments
            WHERE payment_id = :payment_id
            "#,
        )
        .bind(":payment_id", payment_id.clone())
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
            "#,
        )
        .bind(":quote_id", quote_id.as_hyphenated().to_string())
        .fetch_one(&self.inner)
        .await
        .map_err(|err| {
            tracing::error!("SQLite could not get mint quote amount_paid");
            err
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

        // Update the amount_paid
        query(
            r#"
            UPDATE mint_quote 
            SET amount_paid = :amount_paid 
            WHERE id = :quote_id
            "#,
        )
        .bind(":amount_paid", new_amount_paid.to_i64())
        .bind(":quote_id", quote_id.as_hyphenated().to_string())
        .execute(&self.inner)
        .await
        .map_err(|err| {
            tracing::error!("SQLite could not update mint quote amount_paid");
            err
        })?;

        // Add payment_id to mint_quote_payments table
        query(
            r#"
            INSERT INTO mint_quote_payments
            (quote_id, payment_id, amount, timestamp)
            VALUES (:quote_id, :payment_id, :amount, :timestamp)
            "#,
        )
        .bind(":quote_id", quote_id.as_hyphenated().to_string())
        .bind(":payment_id", payment_id)
        .bind(":amount", amount_paid.to_i64())
        .bind(":timestamp", unix_time() as i64)
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
        quote_id: &Uuid,
        amount_issued: Amount,
    ) -> Result<Amount, Self::Err> {
        // Get current amount_issued from quote
        let current_amount = query(
            r#"
            SELECT amount_issued 
            FROM mint_quote
            WHERE id = :quote_id
            "#,
        )
        .bind(":quote_id", quote_id.as_hyphenated().to_string())
        .fetch_one(&self.inner)
        .await
        .map_err(|err| {
            tracing::error!("SQLite could not get mint quote amount_issued");
            err
        })?;

        let current_amount_issued = if let Some(current_amount) = current_amount {
            let amount: u64 = column_as_number!(current_amount[0].clone());
            Amount::from(amount)
        } else {
            Amount::ZERO
        };

        // Calculate new amount_issued with overflow check
        let new_amount_issued = current_amount_issued
            .checked_add(amount_issued)
            .ok_or_else(|| database::Error::AmountOverflow)?;

        // Update the amount_issued
        query(
            r#"
            UPDATE mint_quote 
            SET amount_issued = :amount_issued 
            WHERE id = :quote_id
            "#,
        )
        .bind(":amount_issued", new_amount_issued.to_i64())
        .bind(":quote_id", quote_id.as_hyphenated().to_string())
        .execute(&self.inner)
        .await
        .map_err(|err| {
            tracing::error!("SQLite could not update mint quote amount_issued");
            err
        })?;

        let current_time = unix_time();

        query(
            r#"
INSERT INTO mint_quote_issued
(quote_id, amount, timestamp)
VALUES (:quote_id, :amount, :timestamp);
            "#,
        )
        .bind(":quote_id", quote_id.as_hyphenated().to_string())
        .bind(":amount", amount_issued.to_i64())
        .bind(":timestamp", current_time as i64)
        .execute(&self.inner)
        .await?;

        Ok(new_amount_issued)
    }

    #[instrument(skip_all)]
    async fn add_mint_quote(&mut self, quote: MintQuote) -> Result<(), Self::Err> {
        tracing::debug!("Adding quote with: {}", quote.payment_method.to_string());
        println!("Adding quote with: {}", quote.payment_method.to_string());
        query(
            r#"
                INSERT INTO mint_quote (
                id, amount, unit, request, expiry, request_lookup_id, pubkey, created_time, payment_method, request_lookup_id_kind
                )
                VALUES (
                :id, :amount, :unit, :request, :expiry, :request_lookup_id, :pubkey, :created_time, :payment_method, :request_lookup_id_kind
                )
            "#,
        )
        .bind(":id", quote.id.to_string())
        .bind(":amount", quote.amount.map(|a| a.to_i64()))
        .bind(":unit", quote.unit.to_string())
        .bind(":request", quote.request)
        .bind(":expiry", quote.expiry as i64)
        .bind(
            ":request_lookup_id",
            quote.request_lookup_id.to_string(),
        )
        .bind(":pubkey", quote.pubkey.map(|p| p.to_string()))
        .bind(":created_time", quote.created_time as i64)
        .bind(":payment_method", quote.payment_method.to_string())
        .bind(":request_lookup_id_kind", quote.request_lookup_id.kind())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn remove_mint_quote(&mut self, quote_id: &Uuid) -> Result<(), Self::Err> {
        query(r#"DELETE FROM mint_quote WHERE id=:id"#)
            .bind(":id", quote_id.as_hyphenated().to_string())
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
        )
        .bind(":request_lookup_id", quote.request_lookup_id.to_string())
        .bind(":state", MeltQuoteState::Unpaid.to_string())
        .bind(":current_time", current_time as i64)
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
                expiry, payment_preimage, request_lookup_id,
                created_time, paid_time, options, request_lookup_id_kind
            )
            VALUES
            (
                :id, :unit, :amount, :request, :fee_reserve, :state,
                :expiry, :payment_preimage, :request_lookup_id,
                :created_time, :paid_time, :options, :request_lookup_id_kind
            )
        "#,
        )
        .bind(":id", quote.id.to_string())
        .bind(":unit", quote.unit.to_string())
        .bind(":amount", quote.amount.to_i64())
        .bind(":request", serde_json::to_string(&quote.request)?)
        .bind(":fee_reserve", quote.fee_reserve.to_i64())
        .bind(":state", quote.state.to_string())
        .bind(":expiry", quote.expiry as i64)
        .bind(":payment_preimage", quote.payment_preimage)
        .bind(":request_lookup_id", quote.request_lookup_id.to_string())
        .bind(":created_time", quote.created_time as i64)
        .bind(":paid_time", quote.paid_time.map(|t| t as i64))
        .bind(
            ":options",
            quote.options.map(|o| serde_json::to_string(&o).ok()),
        )
        .bind(":request_lookup_id_kind", quote.request_lookup_id.kind())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn update_melt_quote_request_lookup_id(
        &mut self,
        quote_id: &Uuid,
        new_request_lookup_id: &PaymentIdentifier,
    ) -> Result<(), Self::Err> {
        query(r#"UPDATE melt_quote SET request_lookup_id = :new_req_id, request_lookup_id_kind = :new_kind WHERE id = :id"#)
            .bind(":new_req_id", new_request_lookup_id.to_string())
            .bind(":new_kind",new_request_lookup_id.kind() )
            .bind(":id", quote_id.as_hyphenated().to_string())
            .execute(&self.inner)
            .await?;
        Ok(())
    }

    async fn update_melt_quote_state(
        &mut self,
        quote_id: &Uuid,
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
        )
        .bind(":id", quote_id.as_hyphenated().to_string())
        .bind(":state", state.to_string())
        .fetch_one(&self.inner)
        .await?
        .map(sqlite_row_to_melt_quote)
        .transpose()?
        .ok_or(Error::QuoteNotFound)?;

        let rec = if state == MeltQuoteState::Paid {
            let current_time = unix_time();
            query(r#"UPDATE melt_quote SET state = :state, paid_time = :paid_time, payment_preimage = :payment_preimage WHERE id = :id"#)
                .bind(":state", state.to_string())
                .bind(":paid_time", current_time as i64)
                .bind(":payment_preimage", payment_proof)
                .bind(":id", quote_id.as_hyphenated().to_string())
                .execute(&self.inner)
                .await
        } else {
            query(r#"UPDATE melt_quote SET state = :state WHERE id = :id"#)
                .bind(":state", state.to_string())
                .bind(":id", quote_id.as_hyphenated().to_string())
                .execute(&self.inner)
                .await
        };

        match rec {
            Ok(_) => {}
            Err(err) => {
                tracing::error!("SQLite Could not update melt quote");
                return Err(err.into());
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
        )
        .bind(":id", quote_id.as_hyphenated().to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn get_mint_quote(&mut self, quote_id: &Uuid) -> Result<Option<MintQuote>, Self::Err> {
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
            WHERE id = :id"#,
        )
        .bind(":id", quote_id.as_hyphenated().to_string())
        .fetch_one(&self.inner)
        .await?
        .map(|row| sqlite_row_to_mint_quote(row, payments, issuance))
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
        )
        .bind(":id", quote_id.as_hyphenated().to_string())
        .fetch_one(&self.inner)
        .await?
        .map(sqlite_row_to_melt_quote)
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
            WHERE request = :request"#,
        )
        .bind(":request", request.to_string())
        .fetch_one(&self.inner)
        .await?
        .map(|row| sqlite_row_to_mint_quote(row, vec![], vec![]))
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
            "#,
        )
        .bind(":request_lookup_id", request_lookup_id.to_string())
        .bind(":request_lookup_id_kind", request_lookup_id.kind())
        .fetch_one(&self.inner)
        .await?
        .map(|row| sqlite_row_to_mint_quote(row, vec![], vec![]))
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
impl MintQuotesDatabase for MintSqliteDatabase {
    type Err = database::Error;

    async fn get_mint_quote(&self, quote_id: &Uuid) -> Result<Option<MintQuote>, Self::Err> {
        let payments = get_mint_quote_payments(&self.pool, quote_id).await?;
        let issuance = get_mint_quote_issuance(&self.pool, quote_id).await?;

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
            WHERE id = :id"#,
        )
        .bind(":id", quote_id.as_hyphenated().to_string())
        .fetch_one(&self.pool)
        .await?
        .map(|row| sqlite_row_to_mint_quote(row, payments, issuance))
        .transpose()?)
    }

    async fn get_mint_quote_by_request(
        &self,
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
            WHERE request = :request"#,
        )
        .bind(":request", request.to_owned())
        .fetch_one(&self.pool)
        .await?
        .map(|row| sqlite_row_to_mint_quote(row, vec![], vec![]))
        .transpose()?;

        if let Some(quote) = mint_quote.as_mut() {
            let payments = get_mint_quote_payments(&self.pool, &quote.id).await?;
            let issuance = get_mint_quote_issuance(&self.pool, &quote.id).await?;
            quote.issuance = issuance;
            quote.payments = payments;
        }

        Ok(mint_quote)
    }

    async fn get_mint_quote_by_request_lookup_id(
        &self,
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
            WHERE request_lookup_id = :request_lookup_id"#,
        )
        .bind(
            ":request_lookup_id",
            serde_json::to_string(request_lookup_id)?,
        )
        .fetch_one(&self.pool)
        .await?
        .map(|row| sqlite_row_to_mint_quote(row, vec![], vec![]))
        .transpose()?;

        // TODO: these should use an sql join so they can be done in one query
        if let Some(quote) = mint_quote.as_mut() {
            let payments = get_mint_quote_payments(&self.pool, &quote.id).await?;
            let issuance = get_mint_quote_issuance(&self.pool, &quote.id).await?;
            quote.issuance = issuance;
            quote.payments = payments;
        }

        Ok(mint_quote)
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
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
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| sqlite_row_to_mint_quote(row, vec![], vec![]))
        .collect::<Result<Vec<_>, _>>()?;

        for quote in mint_quotes.as_mut_slice() {
            let payments = get_mint_quote_payments(&self.pool, &quote.id).await?;
            let issuance = get_mint_quote_issuance(&self.pool, &quote.id).await?;
            quote.issuance = issuance;
            quote.payments = payments;
        }

        Ok(mint_quotes)
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
        )
        .bind(":id", quote_id.as_hyphenated().to_string())
        .fetch_one(&self.pool)
        .await?
        .map(sqlite_row_to_melt_quote)
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
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(sqlite_row_to_melt_quote)
        .collect::<Result<Vec<_>, _>>()?)
    }
}

#[async_trait]
impl<'a> MintProofsTransaction<'a> for SqliteTransaction<'a> {
    type Err = database::Error;

    async fn add_proofs(
        &mut self,
        proofs: Proofs,
        quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err> {
        let current_time = unix_time();

        // Check any previous proof, this query should return None in order to proceed storing
        // Any result here would error
        match query(r#"SELECT state FROM proof WHERE y IN (:ys) LIMIT 1"#)
            .bind_vec(
                ":ys",
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
                  (y, amount, keyset_id, secret, c, witness, state, quote_id, created_time)
                  VALUES
                  (:y, :amount, :keyset_id, :secret, :c, :witness, :state, :quote_id, :created_time)
                  "#,
            )
            .bind(":y", proof.y()?.to_bytes().to_vec())
            .bind(":amount", proof.amount.to_i64())
            .bind(":keyset_id", proof.keyset_id.to_string())
            .bind(":secret", proof.secret.to_string())
            .bind(":c", proof.c.to_bytes().to_vec())
            .bind(
                ":witness",
                proof.witness.map(|w| serde_json::to_string(&w).unwrap()),
            )
            .bind(":state", "UNSPENT".to_string())
            .bind(":quote_id", quote_id.map(|q| q.hyphenated().to_string()))
            .bind(":created_time", current_time as i64)
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

        query(r#"UPDATE proof SET state = :new_state WHERE y IN (:ys)"#)
            .bind(":new_state", new_state.to_string())
            .bind_vec(":ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
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
        )
        .bind_vec(":ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
        .bind_vec(":exclude_state", vec![State::Spent.to_string()])
        .execute(&self.inner)
        .await?;

        if total_deleted != ys.len() {
            return Err(Self::Err::AttemptRemoveSpentProof);
        }

        Ok(())
    }
}

#[async_trait]
impl MintProofsDatabase for MintSqliteDatabase {
    type Err = database::Error;

    #[instrument(skip_all)]
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
        )
        .bind_vec(":ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|mut row| {
            Ok((
                column_as_string!(
                    row.pop().ok_or(Error::InvalidDbPath)?,
                    PublicKey::from_hex,
                    PublicKey::from_slice
                ),
                sqlite_row_to_proof(row)?,
            ))
        })
        .collect::<Result<HashMap<_, _>, Error>>()?;

        Ok(ys.iter().map(|y| proofs.remove(y)).collect())
    }

    #[instrument(skip(self))]
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
        )
        .bind(":quote_id", quote_id.as_hyphenated().to_string())
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(sqlite_row_to_proof)
        .collect::<Result<Vec<Proof>, _>>()?
        .ys()?)
    }

    #[instrument(skip_all)]
    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err> {
        let mut current_states = get_current_states(&self.pool, ys).await?;

        Ok(ys.iter().map(|y| current_states.remove(y)).collect())
    }

    #[instrument(skip_all)]
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
        )
        .bind(":keyset_id", keyset_id.to_string())
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(sqlite_row_to_proof_with_state)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .unzip())
    }
}

#[async_trait]
impl<'a> MintSignatureTransaction<'a> for SqliteTransaction<'a> {
    type Err = database::Error;

    #[instrument(skip_all)]
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
            )
            .bind(":blinded_message", message.to_bytes().to_vec())
            .bind(":amount", signature.amount.to_i64())
            .bind(":keyset_id", signature.keyset_id.to_string())
            .bind(":c", signature.c.to_hex())
            .bind(":quote_id", quote_id.map(|q| q.hyphenated().to_string()))
            .bind(
                ":dleq_e",
                signature.dleq.as_ref().map(|dleq| dleq.e.to_secret_hex()),
            )
            .bind(
                ":dleq_s",
                signature.dleq.as_ref().map(|dleq| dleq.s.to_secret_hex()),
            )
            .bind(":created_time", current_time as i64)
            .execute(&self.inner)
            .await?;
        }

        Ok(())
    }

    #[instrument(skip_all)]
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
            WHERE blinded_message IN (:blinded_message)
            "#,
        )
        .bind_vec(
            ":blinded_message",
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
                sqlite_row_to_blind_signature(row)?,
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
impl MintSignaturesDatabase for MintSqliteDatabase {
    type Err = database::Error;

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
        )
        .bind_vec(
            ":blinded_message",
            blinded_messages
                .iter()
                .map(|b_| b_.to_bytes().to_vec())
                .collect(),
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|mut row| {
            Ok((
                column_as_string!(
                    &row.pop().ok_or(Error::InvalidDbResponse)?,
                    PublicKey::from_hex,
                    PublicKey::from_slice
                ),
                sqlite_row_to_blind_signature(row)?,
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
        )
        .bind(":keyset_id", keyset_id.to_string())
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(sqlite_row_to_blind_signature)
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
        )
        .bind(":quote_id", quote_id.to_string())
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(sqlite_row_to_blind_signature)
        .collect::<Result<Vec<BlindSignature>, _>>()?)
    }
}

#[async_trait]
impl MintDatabase<database::Error> for MintSqliteDatabase {
    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<
        Box<dyn database::MintTransaction<'a, database::Error> + Send + Sync + 'a>,
        database::Error,
    > {
        Ok(Box::new(SqliteTransaction {
            inner: self.pool.begin().await?,
        }))
    }

    async fn get_mint_info(&self) -> Result<MintInfo, database::Error> {
        Ok(self.fetch_from_config("mint_info").await?)
    }

    async fn get_quote_ttl(&self) -> Result<QuoteTTL, database::Error> {
        Ok(self.fetch_from_config("quote_ttl").await?)
    }
}

fn sqlite_row_to_keyset_info(row: Vec<Column>) -> Result<MintKeySetInfo, Error> {
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

#[instrument(skip_all)]
fn sqlite_row_to_mint_quote(
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
        Some(Uuid::parse_str(&id).map_err(|_| Error::InvalidUuid(id))?),
        request_str,
        column_as_string!(unit, CurrencyUnit::from_str),
        amount.map(Amount::from),
        column_as_number!(expiry),
        PaymentIdentifier::new(&request_lookup_id_kind, &request_lookup_id)
            .map_err(|_| Error::MissingParameter("Payment id".to_string()))?,
        pubkey,
        amount_paid.into(),
        amount_issued.into(),
        payment_method,
        column_as_number!(created_time),
        payments,
        issueances,
    ))
}

fn sqlite_row_to_melt_quote(row: Vec<Column>) -> Result<mint::MeltQuote, Error> {
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

    let state = MeltQuoteState::from_str(&column_as_string!(&state))?;

    let unit = column_as_string!(unit);
    let request = column_as_string!(request);

    let mut request_lookup_id_kind = column_as_string!(request_lookup_id_kind);

    let request_lookup_id = column_as_nullable_string!(&request_lookup_id).unwrap_or_else(|| {
        Bolt11Invoice::from_str(&request)
            .map(|invoice| invoice.payment_hash().to_string())
            .unwrap_or_else(|_| {
                request_lookup_id_kind = "custom".to_string();
                request.clone()
            })
    });

    let request_lookup_id = PaymentIdentifier::new(&request_lookup_id_kind, &request_lookup_id)
        .map_err(|_| Error::MissingParameter("Payment id".to_string()))?;

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
        id: Uuid::parse_str(&id).map_err(|_| Error::InvalidUuid(id))?,
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

fn sqlite_row_to_proof(row: Vec<Column>) -> Result<Proof, Error> {
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

fn sqlite_row_to_proof_with_state(row: Vec<Column>) -> Result<(Proof, Option<State>), Error> {
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

fn sqlite_row_to_blind_signature(row: Vec<Column>) -> Result<BlindSignature, Error> {
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

#[cfg(test)]
mod tests {
    use std::fs::remove_file;

    use cdk_common::mint::MintKeySetInfo;
    use cdk_common::{mint_db_test, Amount};

    use super::*;

    #[tokio::test]
    async fn test_remove_spent_proofs() {
        let db = memory::empty().await.unwrap();

        // Create a keyset and add it to the database
        let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();
        let keyset_info = MintKeySetInfo {
            id: keyset_id,
            unit: CurrencyUnit::Sat,
            active: true,
            valid_from: 0,
            derivation_path: bitcoin::bip32::DerivationPath::from_str("m/0'/0'/0'").unwrap(),
            derivation_path_index: Some(0),
            max_order: 32,
            input_fee_ppk: 0,
            final_expiry: None,
        };
        let mut tx = MintKeysDatabase::begin_transaction(&db).await.unwrap();
        tx.add_keyset_info(keyset_info).await.unwrap();
        tx.commit().await.unwrap();

        let proofs = vec![
            Proof {
                amount: Amount::from(100),
                keyset_id,
                secret: Secret::generate(),
                c: SecretKey::generate().public_key(),
                witness: None,
                dleq: None,
            },
            Proof {
                amount: Amount::from(200),
                keyset_id,
                secret: Secret::generate(),
                c: SecretKey::generate().public_key(),
                witness: None,
                dleq: None,
            },
        ];

        // Add proofs to database
        let mut tx = MintDatabase::begin_transaction(&db).await.unwrap();
        tx.add_proofs(proofs.clone(), None).await.unwrap();

        // Mark one proof as spent
        tx.update_proofs_states(&[proofs[0].y().unwrap()], State::Spent)
            .await
            .unwrap();

        tx.commit().await.unwrap();

        // Verify both proofs still exist
        let states = db
            .get_proofs_states(&[proofs[0].y().unwrap(), proofs[1].y().unwrap()])
            .await
            .unwrap();

        assert_eq!(states.len(), 2);
        assert_eq!(states[0], Some(State::Spent));
        assert_eq!(states[1], Some(State::Unspent));
    }

    #[tokio::test]
    async fn test_update_spent_proofs() {
        let db = memory::empty().await.unwrap();

        // Create a keyset and add it to the database
        let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();
        let keyset_info = MintKeySetInfo {
            id: keyset_id,
            unit: CurrencyUnit::Sat,
            active: true,
            valid_from: 0,
            derivation_path: bitcoin::bip32::DerivationPath::from_str("m/0'/0'/0'").unwrap(),
            derivation_path_index: Some(0),
            max_order: 32,
            input_fee_ppk: 0,
            final_expiry: None,
        };
        let mut tx = MintKeysDatabase::begin_transaction(&db)
            .await
            .expect("begin");
        tx.add_keyset_info(keyset_info).await.unwrap();
        tx.commit().await.expect("commit");

        let proofs = vec![
            Proof {
                amount: Amount::from(100),
                keyset_id,
                secret: Secret::generate(),
                c: SecretKey::generate().public_key(),
                witness: None,
                dleq: None,
            },
            Proof {
                amount: Amount::from(200),
                keyset_id,
                secret: Secret::generate(),
                c: SecretKey::generate().public_key(),
                witness: None,
                dleq: None,
            },
        ];

        // Add proofs to database
        let mut tx = MintDatabase::begin_transaction(&db).await.unwrap();
        tx.add_proofs(proofs.clone(), None).await.unwrap();

        // Mark one proof as spent
        tx.update_proofs_states(&[proofs[0].y().unwrap()], State::Spent)
            .await
            .unwrap();

        // Try to update both proofs - should fail because one is spent
        let result = tx
            .update_proofs_states(&[proofs[0].y().unwrap()], State::Unspent)
            .await;

        tx.commit().await.unwrap();

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            database::Error::AttemptUpdateSpentProof
        ));

        // Verify states haven't changed
        let states = db
            .get_proofs_states(&[proofs[0].y().unwrap(), proofs[1].y().unwrap()])
            .await
            .unwrap();

        assert_eq!(states.len(), 2);
        assert_eq!(states[0], Some(State::Spent));
        assert_eq!(states[1], Some(State::Unspent));
    }

    async fn provide_db() -> MintSqliteDatabase {
        memory::empty().await.unwrap()
    }

    mint_db_test!(provide_db);

    #[tokio::test]
    async fn open_legacy_and_migrate() {
        let file = format!(
            "{}/db.sqlite",
            std::env::temp_dir().to_str().unwrap_or_default()
        );

        {
            let _ = remove_file(&file);
            #[cfg(not(feature = "sqlcipher"))]
            let legacy = create_sqlite_pool(&file);
            #[cfg(feature = "sqlcipher")]
            let legacy = create_sqlite_pool(&file, "test".to_owned());
            let y = legacy.get().expect("pool");
            y.execute_batch(include_str!("../../tests/legacy-sqlx.sql"))
                .expect("create former db failed");
        }

        #[cfg(not(feature = "sqlcipher"))]
        let conn = MintSqliteDatabase::new(&file).await;

        #[cfg(feature = "sqlcipher")]
        let conn = MintSqliteDatabase::new(&file, "test".to_owned()).await;

        assert!(conn.is_ok(), "Failed with {:?}", conn.unwrap_err());

        let _ = remove_file(&file);
    }

    #[tokio::test]
    async fn test_fetch_from_config_error_handling() {
        use cdk_common::common::QuoteTTL;
        use cdk_common::MintInfo;

        let db = memory::empty().await.unwrap();

        // Test 1: Unknown mint_info should return UnknownMintInfo error
        let result: Result<MintInfo, Error> = db.fetch_from_config("mint_info").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::UnknownMintInfo));

        // Test 2: Unknown quote_ttl should return UnknownQuoteTTL error
        let result: Result<QuoteTTL, Error> = db.fetch_from_config("quote_ttl").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::UnknownQuoteTTL));

        // Test 3: Unknown config key should return UnknownConfigKey error
        let result: Result<String, Error> = db.fetch_from_config("unknown_config_key").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::UnknownConfigKey(key) => {
                assert_eq!(key, "unknown_config_key");
            }
            other => panic!("Expected UnknownConfigKey error, got: {:?}", other),
        }

        // Test 4: Another unknown config key with different name
        let result: Result<String, Error> = db.fetch_from_config("some_other_key").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::UnknownConfigKey(key) => {
                assert_eq!(key, "some_other_key");
            }
            other => panic!("Expected UnknownConfigKey error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_config_round_trip() {
        use cdk_common::common::QuoteTTL;
        use cdk_common::{MintInfo, Nuts};

        let db = memory::empty().await.unwrap();

        // Test mint_info round trip
        let mint_info = MintInfo {
            name: Some("Test Mint".to_string()),
            description: Some("A test mint".to_string()),
            pubkey: None,
            version: None,
            description_long: None,
            contact: None,
            nuts: Nuts::default(),
            icon_url: None,
            urls: None,
            motd: None,
            time: None,
            tos_url: None,
        };

        // Store mint_info
        let mut tx = cdk_common::database::MintDatabase::begin_transaction(&db)
            .await
            .unwrap();
        tx.set_mint_info(mint_info.clone()).await.unwrap();
        tx.commit().await.unwrap();

        // Retrieve mint_info
        let retrieved_mint_info: MintInfo = db.fetch_from_config("mint_info").await.unwrap();
        assert_eq!(mint_info.name, retrieved_mint_info.name);
        assert_eq!(mint_info.description, retrieved_mint_info.description);

        // Test quote_ttl round trip
        let quote_ttl = QuoteTTL {
            mint_ttl: 3600,
            melt_ttl: 1800,
        };

        // Store quote_ttl
        let mut tx = cdk_common::database::MintDatabase::begin_transaction(&db)
            .await
            .unwrap();
        tx.set_quote_ttl(quote_ttl.clone()).await.unwrap();
        tx.commit().await.unwrap();

        // Retrieve quote_ttl
        let retrieved_quote_ttl: QuoteTTL = db.fetch_from_config("quote_ttl").await.unwrap();
        assert_eq!(quote_ttl.mint_ttl, retrieved_quote_ttl.mint_ttl);
        assert_eq!(quote_ttl.melt_ttl, retrieved_quote_ttl.melt_ttl);
    }
}

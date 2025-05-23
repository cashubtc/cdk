//! SQLite Mint

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use bitcoin::bip32::DerivationPath;
use cdk_common::common::{PaymentProcessorKey, QuoteTTL};
use cdk_common::database::{
    self, MintDatabase, MintKeysDatabase, MintProofsDatabase, MintQuotesDatabase,
    MintSignaturesDatabase,
};
use cdk_common::mint::{self, MintKeySetInfo, MintQuote};
use cdk_common::nut00::ProofsMethods;
use cdk_common::nut05::QuoteState;
use cdk_common::secret::Secret;
use cdk_common::state::check_state_transition;
use cdk_common::util::unix_time;
use cdk_common::{
    Amount, BlindSignature, BlindSignatureDleq, CurrencyUnit, Id, MeltQuoteState, MeltRequest,
    MintInfo, MintQuoteState, PaymentMethod, Proof, Proofs, PublicKey, SecretKey, State,
};
use error::Error;
use lightning_invoice::Bolt11Invoice;
use sqlx::sqlite::SqliteRow;
use sqlx::{Executor, Pool, Row, Sqlite};
use uuid::fmt::Hyphenated;
use uuid::Uuid;

use crate::common::create_sqlite_pool;

#[cfg(feature = "auth")]
mod auth;
pub mod error;
pub mod memory;

#[cfg(feature = "auth")]
pub use auth::MintSqliteAuthDatabase;

/// Mint SQLite Database
#[derive(Debug, Clone)]
pub struct MintSqliteDatabase {
    pool: Pool<Sqlite>,
}

impl MintSqliteDatabase {
    /// Check if any proofs are spent
    async fn check_for_spent_proofs<'e, 'c: 'e, E>(
        &self,
        transaction: E,
        ys: &[PublicKey],
    ) -> Result<bool, database::Error>
    where
        E: Executor<'c, Database = Sqlite>,
    {
        if ys.is_empty() {
            return Ok(false);
        }

        let check_sql = format!(
            "SELECT state FROM proof WHERE y IN ({}) AND state = 'SPENT'",
            std::iter::repeat("?")
                .take(ys.len())
                .collect::<Vec<_>>()
                .join(",")
        );

        let spent_count = ys
            .iter()
            .fold(sqlx::query(&check_sql), |query, y| {
                query.bind(y.to_bytes().to_vec())
            })
            .fetch_all(transaction)
            .await
            .map_err(Error::from)?
            .len();

        Ok(spent_count > 0)
    }

    /// Create new [`MintSqliteDatabase`]
    #[cfg(not(feature = "sqlcipher"))]
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let db = Self {
            pool: create_sqlite_pool(path.as_ref().to_str().ok_or(Error::InvalidDbPath)?).await?,
        };
        db.migrate().await?;
        Ok(db)
    }

    /// Create new [`MintSqliteDatabase`]
    #[cfg(feature = "sqlcipher")]
    pub async fn new<P: AsRef<Path>>(path: P, password: String) -> Result<Self, Error> {
        let db = Self {
            pool: create_sqlite_pool(
                path.as_ref().to_str().ok_or(Error::InvalidDbPath)?,
                password,
            )
            .await?,
        };
        db.migrate().await?;
        Ok(db)
    }

    /// Migrate [`MintSqliteDatabase`]
    async fn migrate(&self) -> Result<(), Error> {
        sqlx::migrate!("./src/mint/migrations")
            .run(&self.pool)
            .await
            .map_err(|_| Error::CouldNotInitialize)?;
        Ok(())
    }
}

#[async_trait]
impl MintKeysDatabase for MintSqliteDatabase {
    type Err = database::Error;

    async fn set_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let update_res = sqlx::query(
            r#"
UPDATE keyset
SET active=FALSE
WHERE unit IS ?;
        "#,
        )
        .bind(unit.to_string())
        .execute(&mut *transaction)
        .await;

        match update_res {
            Ok(_) => (),
            Err(err) => {
                tracing::error!("SQLite Could not update keyset");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                return Err(Error::from(err).into());
            }
        };

        let update_res = sqlx::query(
            r#"
UPDATE keyset
SET active=TRUE
WHERE unit IS ?
AND id IS ?;
        "#,
        )
        .bind(unit.to_string())
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await;

        match update_res {
            Ok(_) => (),
            Err(err) => {
                tracing::error!("SQLite Could not update keyset");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                return Err(Error::from(err).into());
            }
        };

        transaction.commit().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT id
FROM keyset
WHERE active = 1
AND unit IS ?
        "#,
        )
        .bind(unit.to_string())
        .fetch_one(&mut *transaction)
        .await;

        let rec = match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;
                rec
            }
            Err(err) => match err {
                sqlx::Error::RowNotFound => {
                    transaction.commit().await.map_err(Error::from)?;
                    return Ok(None);
                }
                _ => {
                    return {
                        if let Err(err) = transaction.rollback().await {
                            tracing::error!("Could not rollback sql transaction: {}", err);
                        }
                        Err(Error::SQLX(err).into())
                    }
                }
            },
        };

        Ok(Some(
            Id::from_str(rec.try_get("id").map_err(Error::from)?).map_err(Error::from)?,
        ))
    }

    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let recs = sqlx::query(
            r#"
SELECT id, unit
FROM keyset
WHERE active = 1
        "#,
        )
        .fetch_all(&mut *transaction)
        .await;

        match recs {
            Ok(recs) => {
                transaction.commit().await.map_err(Error::from)?;

                let keysets = recs
                    .iter()
                    .filter_map(|r| match Id::from_str(r.get("id")) {
                        Ok(id) => Some((
                            CurrencyUnit::from_str(r.get::<'_, &str, &str>("unit")).unwrap(),
                            id,
                        )),
                        Err(_) => None,
                    })
                    .collect();
                Ok(keysets)
            }
            Err(err) => {
                tracing::error!("SQLite could not get active keyset");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }
                Err(Error::from(err).into())
            }
        }
    }

    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let res = sqlx::query(
            r#"
INSERT INTO keyset
(id, unit, active, valid_from, valid_to, derivation_path, max_order, input_fee_ppk, derivation_path_index)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(keyset.id.to_string())
        .bind(keyset.unit.to_string())
        .bind(keyset.active)
        .bind(keyset.valid_from as i64)
        .bind(keyset.valid_to.map(|v| v as i64))
        .bind(keyset.derivation_path.to_string())
        .bind(keyset.max_order)
        .bind(keyset.input_fee_ppk as i64)
            .bind(keyset.derivation_path_index)
        .execute(&mut *transaction)
        .await;

        match res {
            Ok(_) => {
                transaction.commit().await.map_err(Error::from)?;
                Ok(())
            }
            Err(err) => {
                tracing::error!("SQLite could not add keyset info");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                Err(Error::from(err).into())
            }
        }
    }

    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let rec = sqlx::query(
            r#"
SELECT *
FROM keyset
WHERE id=?;
        "#,
        )
        .bind(id.to_string())
        .fetch_one(&mut *transaction)
        .await;

        match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;
                Ok(Some(sqlite_row_to_keyset_info(rec)?))
            }
            Err(err) => match err {
                sqlx::Error::RowNotFound => {
                    transaction.commit().await.map_err(Error::from)?;
                    return Ok(None);
                }
                _ => {
                    tracing::error!("SQLite could not get keyset info");
                    if let Err(err) = transaction.rollback().await {
                        tracing::error!("Could not rollback sql transaction: {}", err);
                    }
                    return Err(Error::SQLX(err).into());
                }
            },
        }
    }

    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let recs = sqlx::query(
            r#"
SELECT *
FROM keyset;
        "#,
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(Error::from);

        match recs {
            Ok(recs) => {
                transaction.commit().await.map_err(Error::from)?;
                Ok(recs
                    .into_iter()
                    .map(sqlite_row_to_keyset_info)
                    .collect::<Result<_, _>>()?)
            }
            Err(err) => {
                tracing::error!("SQLite could not get keyset info");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }
                Err(err.into())
            }
        }
    }
}

#[async_trait]
impl MintQuotesDatabase for MintSqliteDatabase {
    type Err = database::Error;

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let res = sqlx::query(
            r#"
INSERT INTO mint_quote
(id, amount, unit, request, state, expiry, request_lookup_id, pubkey, created_time, paid_time, issued_time)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
    amount = excluded.amount,
    unit = excluded.unit,
    request = excluded.request,
    state = excluded.state,
    expiry = excluded.expiry,
    request_lookup_id = excluded.request_lookup_id,
    created_time = excluded.created_time,
    paid_time = excluded.paid_time,
    issued_time = excluded.issued_time
ON CONFLICT(request_lookup_id) DO UPDATE SET
    amount = excluded.amount,
    unit = excluded.unit,
    request = excluded.request,
    state = excluded.state,
    expiry = excluded.expiry,
    id = excluded.id,
    created_time = excluded.created_time,
    paid_time = excluded.paid_time,
    issued_time = excluded.issued_time
        "#,
        )
        .bind(quote.id.to_string())
        .bind(u64::from(quote.amount) as i64)
        .bind(quote.unit.to_string())
        .bind(quote.request)
        .bind(quote.state.to_string())
        .bind(quote.expiry as i64)
        .bind(quote.request_lookup_id)
        .bind(quote.pubkey.map(|p| p.to_string()))
        .bind(quote.created_time as i64)
        .bind(quote.paid_time.map(|t| t as i64))
        .bind(quote.issued_time.map(|t| t as i64))
        .execute(&mut *transaction)
        .await;

        match res {
            Ok(_) => {
                transaction.commit().await.map_err(Error::from)?;
                Ok(())
            }
            Err(err) => {
                tracing::error!("SQLite Could not update keyset");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                Err(Error::from(err).into())
            }
        }
    }

    async fn get_mint_quote(&self, quote_id: &Uuid) -> Result<Option<MintQuote>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
WHERE id=?;
        "#,
        )
        .bind(quote_id.as_hyphenated())
        .fetch_one(&mut *transaction)
        .await;

        match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;
                Ok(Some(sqlite_row_to_mint_quote(rec)?))
            }
            Err(err) => match err {
                sqlx::Error::RowNotFound => {
                    transaction.commit().await.map_err(Error::from)?;
                    Ok(None)
                }
                _ => {
                    if let Err(err) = transaction.rollback().await {
                        tracing::error!("Could not rollback sql transaction: {}", err);
                    }
                    Err(Error::SQLX(err).into())
                }
            },
        }
    }

    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
WHERE request=?;
        "#,
        )
        .bind(request)
        .fetch_one(&mut *transaction)
        .await;

        match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;
                Ok(Some(sqlite_row_to_mint_quote(rec)?))
            }
            Err(err) => match err {
                sqlx::Error::RowNotFound => {
                    transaction.commit().await.map_err(Error::from)?;
                    Ok(None)
                }
                _ => {
                    if let Err(err) = transaction.rollback().await {
                        tracing::error!("Could not rollback sql transaction: {}", err);
                    }
                    Err(Error::SQLX(err).into())
                }
            },
        }
    }

    async fn get_mint_quote_by_request_lookup_id(
        &self,
        request_lookup_id: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
WHERE request_lookup_id=?;
        "#,
        )
        .bind(request_lookup_id)
        .fetch_one(&mut *transaction)
        .await;

        match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;

                Ok(Some(sqlite_row_to_mint_quote(rec)?))
            }
            Err(err) => match err {
                sqlx::Error::RowNotFound => {
                    transaction.commit().await.map_err(Error::from)?;
                    Ok(None)
                }
                _ => {
                    if let Err(err) = transaction.rollback().await {
                        tracing::error!("Could not rollback sql transaction: {}", err);
                    }
                    Err(Error::SQLX(err).into())
                }
            },
        }
    }

    async fn update_mint_quote_state(
        &self,
        quote_id: &Uuid,
        state: MintQuoteState,
    ) -> Result<MintQuoteState, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
WHERE id=?;
        "#,
        )
        .bind(quote_id.as_hyphenated())
        .fetch_one(&mut *transaction)
        .await;
        let quote = match rec {
            Ok(row) => sqlite_row_to_mint_quote(row)?,
            Err(err) => {
                tracing::error!("SQLite Could not update keyset");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                return Err(Error::from(err).into());
            }
        };

        let update_query = match state {
            MintQuoteState::Paid => {
                r#"UPDATE mint_quote SET state = ?, paid_time = ? WHERE id = ?"#
            }
            MintQuoteState::Issued => {
                r#"UPDATE mint_quote SET state = ?, issued_time = ? WHERE id = ?"#
            }
            _ => r#"UPDATE mint_quote SET state = ? WHERE id = ?"#,
        };

        let current_time = unix_time();

        let update = match state {
            MintQuoteState::Paid => {
                sqlx::query(update_query)
                    .bind(state.to_string())
                    .bind(current_time as i64)
                    .bind(quote_id.as_hyphenated())
                    .execute(&mut *transaction)
                    .await
            }
            MintQuoteState::Issued => {
                sqlx::query(update_query)
                    .bind(state.to_string())
                    .bind(current_time as i64)
                    .bind(quote_id.as_hyphenated())
                    .execute(&mut *transaction)
                    .await
            }
            _ => {
                sqlx::query(update_query)
                    .bind(state.to_string())
                    .bind(quote_id.as_hyphenated())
                    .execute(&mut *transaction)
                    .await
            }
        };

        match update {
            Ok(_) => {
                transaction.commit().await.map_err(Error::from)?;
                Ok(quote.state)
            }
            Err(err) => {
                tracing::error!("SQLite Could not update keyset");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                return Err(Error::from(err).into());
            }
        }
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
        "#,
        )
        .fetch_all(&mut *transaction)
        .await;

        match rec {
            Ok(rows) => {
                transaction.commit().await.map_err(Error::from)?;
                let mint_quotes = rows
                    .into_iter()
                    .map(sqlite_row_to_mint_quote)
                    .collect::<Result<Vec<MintQuote>, _>>()?;

                Ok(mint_quotes)
            }
            Err(err) => {
                tracing::error!("SQLite get mint quotes");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                return Err(Error::from(err).into());
            }
        }
    }

    async fn get_mint_quotes_with_state(
        &self,
        state: MintQuoteState,
    ) -> Result<Vec<MintQuote>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
WHERE state = ?
        "#,
        )
        .bind(state.to_string())
        .fetch_all(&mut *transaction)
        .await;

        match rec {
            Ok(rows) => {
                transaction.commit().await.map_err(Error::from)?;
                let mint_quotes = rows
                    .into_iter()
                    .map(sqlite_row_to_mint_quote)
                    .collect::<Result<Vec<MintQuote>, _>>()?;

                Ok(mint_quotes)
            }
            Err(err) => {
                tracing::error!("SQLite get mint quotes with state");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                return Err(Error::from(err).into());
            }
        }
    }

    async fn remove_mint_quote(&self, quote_id: &Uuid) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let res = sqlx::query(
            r#"
DELETE FROM mint_quote
WHERE id=?
        "#,
        )
        .bind(quote_id.as_hyphenated())
        .execute(&mut *transaction)
        .await;

        match res {
            Ok(_) => {
                transaction.commit().await.map_err(Error::from)?;

                Ok(())
            }
            Err(err) => {
                tracing::error!("SQLite Could not remove mint quote");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                Err(Error::from(err).into())
            }
        }
    }

    async fn add_melt_quote(&self, quote: mint::MeltQuote) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let res = sqlx::query(
            r#"
INSERT INTO melt_quote
(id, unit, amount, request, fee_reserve, state, expiry, payment_preimage, request_lookup_id, msat_to_pay, created_time, paid_time)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
    unit = excluded.unit,
    amount = excluded.amount,
    request = excluded.request,
    fee_reserve = excluded.fee_reserve,
    state = excluded.state,
    expiry = excluded.expiry,
    payment_preimage = excluded.payment_preimage,
    request_lookup_id = excluded.request_lookup_id,
    msat_to_pay = excluded.msat_to_pay,
    created_time = excluded.created_time,
    paid_time = excluded.paid_time
ON CONFLICT(request_lookup_id) DO UPDATE SET
    unit = excluded.unit,
    amount = excluded.amount,
    request = excluded.request,
    fee_reserve = excluded.fee_reserve,
    state = excluded.state,
    expiry = excluded.expiry,
    payment_preimage = excluded.payment_preimage,
    id = excluded.id,
    created_time = excluded.created_time,
    paid_time = excluded.paid_time;
        "#,
        )
        .bind(quote.id.to_string())
        .bind(quote.unit.to_string())
        .bind(u64::from(quote.amount) as i64)
        .bind(quote.request)
        .bind(u64::from(quote.fee_reserve) as i64)
        .bind(quote.state.to_string())
        .bind(quote.expiry as i64)
        .bind(quote.payment_preimage)
        .bind(quote.request_lookup_id)
        .bind(quote.msat_to_pay.map(|a| u64::from(a) as i64))
        .bind(quote.created_time as i64)
        .bind(quote.paid_time.map(|t| t as i64))
        .execute(&mut *transaction)
        .await;

        match res {
            Ok(_) => {
                transaction.commit().await.map_err(Error::from)?;

                Ok(())
            }
            Err(err) => {
                tracing::error!("SQLite Could not remove mint quote");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                Err(Error::from(err).into())
            }
        }
    }
    async fn get_melt_quote(&self, quote_id: &Uuid) -> Result<Option<mint::MeltQuote>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let rec = sqlx::query(
            r#"
SELECT *
FROM melt_quote
WHERE id=?;
        "#,
        )
        .bind(quote_id.as_hyphenated())
        .fetch_one(&mut *transaction)
        .await;

        match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;

                Ok(Some(sqlite_row_to_melt_quote(rec)?))
            }
            Err(err) => match err {
                sqlx::Error::RowNotFound => {
                    transaction.commit().await.map_err(Error::from)?;
                    Ok(None)
                }
                _ => {
                    if let Err(err) = transaction.rollback().await {
                        tracing::error!("Could not rollback sql transaction: {}", err);
                    }

                    Err(Error::SQLX(err).into())
                }
            },
        }
    }

    async fn get_melt_quotes(&self) -> Result<Vec<mint::MeltQuote>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let rec = sqlx::query(
            r#"
SELECT *
FROM melt_quote
        "#,
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(Error::from);

        match rec {
            Ok(rec) => {
                let melt_quotes = rec
                    .into_iter()
                    .map(sqlite_row_to_melt_quote)
                    .collect::<Result<Vec<mint::MeltQuote>, _>>()?;
                Ok(melt_quotes)
            }
            Err(err) => {
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                Err(err.into())
            }
        }
    }

    async fn update_melt_quote_state(
        &self,
        quote_id: &Uuid,
        state: MeltQuoteState,
    ) -> Result<MeltQuoteState, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT *
FROM melt_quote
WHERE id=?;
        "#,
        )
        .bind(quote_id.as_hyphenated())
        .fetch_one(&mut *transaction)
        .await;

        let quote = match rec {
            Ok(rec) => sqlite_row_to_melt_quote(rec)?,
            Err(err) => {
                tracing::error!("SQLite Could not update keyset");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                return Err(Error::from(err).into());
            }
        };

        let update_query = if state == MeltQuoteState::Paid {
            r#"UPDATE melt_quote SET state = ?, paid_time = ? WHERE id = ?"#
        } else {
            r#"UPDATE melt_quote SET state = ? WHERE id = ?"#
        };

        let current_time = unix_time();

        let rec = if state == MeltQuoteState::Paid {
            sqlx::query(update_query)
                .bind(state.to_string())
                .bind(current_time as i64)
                .bind(quote_id.as_hyphenated())
                .execute(&mut *transaction)
                .await
        } else {
            sqlx::query(update_query)
                .bind(state.to_string())
                .bind(quote_id.as_hyphenated())
                .execute(&mut *transaction)
                .await
        };

        match rec {
            Ok(_) => {
                transaction.commit().await.map_err(Error::from)?;
            }
            Err(err) => {
                tracing::error!("SQLite Could not update melt quote");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                return Err(Error::from(err).into());
            }
        };

        Ok(quote.state)
    }

    async fn remove_melt_quote(&self, quote_id: &Uuid) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let res = sqlx::query(
            r#"
DELETE FROM melt_quote
WHERE id=?
        "#,
        )
        .bind(quote_id.as_hyphenated())
        .execute(&mut *transaction)
        .await;

        match res {
            Ok(_) => {
                transaction.commit().await.map_err(Error::from)?;
                Ok(())
            }
            Err(err) => {
                tracing::error!("SQLite Could not update melt quote");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                Err(Error::from(err).into())
            }
        }
    }

    async fn add_melt_request(
        &self,
        melt_request: MeltRequest<Uuid>,
        ln_key: PaymentProcessorKey,
    ) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let res = sqlx::query(
            r#"
INSERT INTO melt_request
(id, inputs, outputs, method, unit)
VALUES (?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
    inputs = excluded.inputs,
    outputs = excluded.outputs,
    method = excluded.method,
    unit = excluded.unit
        "#,
        )
        .bind(melt_request.quote())
        .bind(serde_json::to_string(&melt_request.inputs())?)
        .bind(serde_json::to_string(&melt_request.outputs())?)
        .bind(ln_key.method.to_string())
        .bind(ln_key.unit.to_string())
        .execute(&mut *transaction)
        .await;

        match res {
            Ok(_) => {
                transaction.commit().await.map_err(Error::from)?;
                Ok(())
            }
            Err(err) => {
                tracing::error!("SQLite Could not update keyset");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                Err(Error::from(err).into())
            }
        }
    }

    async fn get_melt_request(
        &self,
        quote_id: &Uuid,
    ) -> Result<Option<(MeltRequest<Uuid>, PaymentProcessorKey)>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT *
FROM melt_request
WHERE id=?;
        "#,
        )
        .bind(quote_id.as_hyphenated())
        .fetch_one(&mut *transaction)
        .await;

        match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;

                let (request, key) = sqlite_row_to_melt_request(rec)?;

                Ok(Some((request, key)))
            }
            Err(err) => match err {
                sqlx::Error::RowNotFound => {
                    transaction.commit().await.map_err(Error::from)?;
                    return Ok(None);
                }
                _ => {
                    return {
                        if let Err(err) = transaction.rollback().await {
                            tracing::error!("Could not rollback sql transaction: {}", err);
                        }
                        Err(Error::SQLX(err).into())
                    }
                }
            },
        }
    }
}

#[async_trait]
impl MintProofsDatabase for MintSqliteDatabase {
    type Err = database::Error;

    async fn add_proofs(&self, proofs: Proofs, quote_id: Option<Uuid>) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let current_time = unix_time();

        for proof in proofs {
            let result = sqlx::query(
                r#"
INSERT OR IGNORE INTO proof
(y, amount, keyset_id, secret, c, witness, state, quote_id, created_time)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);
        "#,
            )
            .bind(proof.y()?.to_bytes().to_vec())
            .bind(u64::from(proof.amount) as i64)
            .bind(proof.keyset_id.to_string())
            .bind(proof.secret.to_string())
            .bind(proof.c.to_bytes().to_vec())
            .bind(proof.witness.map(|w| serde_json::to_string(&w).unwrap()))
            .bind("UNSPENT")
            .bind(quote_id.map(|q| q.hyphenated()))
            .bind(current_time as i64)
            .execute(&mut *transaction)
            .await;

            // We still need to check for foreign key constraint errors
            if let Err(err) = result {
                if let sqlx::Error::Database(db_err) = &err {
                    if db_err.message().contains("FOREIGN KEY constraint failed") {
                        tracing::error!(
                            "Foreign key constraint failed when adding proof: {:?}",
                            err
                        );
                        transaction.rollback().await.map_err(Error::from)?;
                        return Err(database::Error::InvalidKeysetId);
                    }
                }

                // For any other error, roll back and return the error
                tracing::error!("Error adding proof: {:?}", err);
                transaction.rollback().await.map_err(Error::from)?;
                return Err(Error::from(err).into());
            }
        }
        transaction.commit().await.map_err(Error::from)?;

        Ok(())
    }

    async fn remove_proofs(
        &self,
        ys: &[PublicKey],
        _quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        if self.check_for_spent_proofs(&mut *transaction, ys).await? {
            transaction.rollback().await.map_err(Error::from)?;
            return Err(Self::Err::AttemptRemoveSpentProof);
        }

        // If no proofs are spent, proceed with deletion
        let delete_sql = format!(
            "DELETE FROM proof WHERE y IN ({})",
            std::iter::repeat("?")
                .take(ys.len())
                .collect::<Vec<_>>()
                .join(",")
        );

        ys.iter()
            .fold(sqlx::query(&delete_sql), |query, y| {
                query.bind(y.to_bytes().to_vec())
            })
            .execute(&mut *transaction)
            .await
            .map_err(Error::from)?;

        transaction.commit().await.map_err(Error::from)?;
        Ok(())
    }

    async fn get_proofs_by_ys(&self, ys: &[PublicKey]) -> Result<Vec<Option<Proof>>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let sql = format!(
            "SELECT * FROM proof WHERE y IN ({})",
            "?,".repeat(ys.len()).trim_end_matches(',')
        );

        let mut proofs = ys
            .iter()
            .fold(sqlx::query(&sql), |query, y| {
                query.bind(y.to_bytes().to_vec())
            })
            .fetch_all(&mut *transaction)
            .await
            .map_err(|err| {
                tracing::error!("SQLite could not get state of proof: {err:?}");
                Error::SQLX(err)
            })?
            .into_iter()
            .map(|row| {
                PublicKey::from_slice(row.get("y"))
                    .map_err(Error::from)
                    .and_then(|y| sqlite_row_to_proof(row).map(|proof| (y, proof)))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;

        Ok(ys.iter().map(|y| proofs.remove(y)).collect())
    }

    async fn get_proof_ys_by_quote_id(&self, quote_id: &Uuid) -> Result<Vec<PublicKey>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT *
FROM proof
WHERE quote_id=?;
        "#,
        )
        .bind(quote_id.as_hyphenated())
        .fetch_all(&mut *transaction)
        .await;

        let ys = match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;

                let proofs = rec
                    .into_iter()
                    .map(sqlite_row_to_proof)
                    .collect::<Result<Vec<Proof>, _>>()?;

                proofs.ys()?
            }
            Err(err) => match err {
                sqlx::Error::RowNotFound => {
                    transaction.commit().await.map_err(Error::from)?;

                    vec![]
                }
                _ => {
                    if let Err(err) = transaction.rollback().await {
                        tracing::error!("Could not rollback sql transaction: {}", err);
                    }
                    return Err(Error::SQLX(err).into());
                }
            },
        };

        Ok(ys)
    }

    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let sql = format!(
            "SELECT y, state FROM proof WHERE y IN ({})",
            "?,".repeat(ys.len()).trim_end_matches(',')
        );

        let mut current_states = ys
            .iter()
            .fold(sqlx::query(&sql), |query, y| {
                query.bind(y.to_bytes().to_vec())
            })
            .fetch_all(&mut *transaction)
            .await
            .map_err(|err| {
                tracing::error!("SQLite could not get state of proof: {err:?}");
                Error::SQLX(err)
            })?
            .into_iter()
            .map(|row| {
                PublicKey::from_slice(row.get("y"))
                    .map_err(Error::from)
                    .and_then(|y| {
                        let state: String = row.get("state");
                        State::from_str(&state)
                            .map_err(Error::from)
                            .map(|state| (y, state))
                    })
            })
            .collect::<Result<HashMap<_, _>, _>>()?;

        Ok(ys.iter().map(|y| current_states.remove(y)).collect())
    }

    async fn get_proofs_by_keyset_id(
        &self,
        keyset_id: &Id,
    ) -> Result<(Proofs, Vec<Option<State>>), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let rec = sqlx::query(
            r#"
SELECT *
FROM proof
WHERE keyset_id=?;
        "#,
        )
        .bind(keyset_id.to_string())
        .fetch_all(&mut *transaction)
        .await;

        match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;
                let mut proofs_for_id = vec![];
                let mut states = vec![];

                for row in rec {
                    let (proof, state) = sqlite_row_to_proof_with_state(row)?;

                    proofs_for_id.push(proof);
                    states.push(state);
                }

                Ok((proofs_for_id, states))
            }
            Err(err) => {
                tracing::error!("SQLite could not get proofs by keysets id");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                return Err(Error::from(err).into());
            }
        }
    }

    async fn update_proofs_states(
        &self,
        ys: &[PublicKey],
        proofs_state: State,
    ) -> Result<Vec<Option<State>>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let sql = format!(
            "SELECT y, state FROM proof WHERE y IN ({})",
            "?,".repeat(ys.len()).trim_end_matches(',')
        );

        let rows = ys
            .iter()
            .fold(sqlx::query(&sql), |query, y| {
                query.bind(y.to_bytes().to_vec())
            })
            .fetch_all(&mut *transaction)
            .await
            .map_err(|err| {
                tracing::error!("SQLite could not get state of proof: {err:?}");
                Error::SQLX(err)
            })?;

        // Check if all proofs exist
        if rows.len() != ys.len() {
            transaction.rollback().await.map_err(Error::from)?;
            tracing::warn!("Attempted to update state of non-existent proof");
            return Err(database::Error::ProofNotFound);
        }

        let mut current_states = rows
            .into_iter()
            .map(|row| {
                PublicKey::from_slice(row.get("y"))
                    .map_err(Error::from)
                    .and_then(|y| {
                        let state: String = row.get("state");
                        State::from_str(&state)
                            .map_err(Error::from)
                            .map(|state| (y, state))
                    })
            })
            .collect::<Result<HashMap<_, _>, _>>()?;

        let states = current_states.values().collect::<HashSet<_>>();

        for state in states {
            check_state_transition(*state, proofs_state)?;
        }

        // If no proofs are spent, proceed with update
        let update_sql = format!(
            "UPDATE proof SET state = ? WHERE y IN ({})",
            "?,".repeat(ys.len()).trim_end_matches(',')
        );

        ys.iter()
            .fold(
                sqlx::query(&update_sql).bind(proofs_state.to_string()),
                |query, y| query.bind(y.to_bytes().to_vec()),
            )
            .execute(&mut *transaction)
            .await
            .map_err(|err| {
                tracing::error!("SQLite could not update proof state: {err:?}");
                Error::SQLX(err)
            })?;

        transaction.commit().await.map_err(Error::from)?;

        Ok(ys.iter().map(|y| current_states.remove(y)).collect())
    }
}

#[async_trait]
impl MintSignaturesDatabase for MintSqliteDatabase {
    type Err = database::Error;

    async fn add_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
        blinded_signatures: &[BlindSignature],
        quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let current_time = unix_time();

        for (message, signature) in blinded_messages.iter().zip(blinded_signatures) {
            let res = sqlx::query(
                r#"
INSERT INTO blind_signature
(y, amount, keyset_id, c, quote_id, dleq_e, dleq_s, created_time)
VALUES (?, ?, ?, ?, ?, ?, ?, ?);
        "#,
            )
            .bind(message.to_bytes().to_vec())
            .bind(u64::from(signature.amount) as i64)
            .bind(signature.keyset_id.to_string())
            .bind(signature.c.to_bytes().to_vec())
            .bind(quote_id.map(|q| q.hyphenated()))
            .bind(signature.dleq.as_ref().map(|dleq| dleq.e.to_secret_hex()))
            .bind(signature.dleq.as_ref().map(|dleq| dleq.s.to_secret_hex()))
            .bind(current_time as i64)
            .execute(&mut *transaction)
            .await;

            if let Err(err) = res {
                tracing::error!("SQLite could not add blind signature");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }
                return Err(Error::SQLX(err).into());
            }
        }

        transaction.commit().await.map_err(Error::from)?;

        Ok(())
    }

    async fn get_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let sql = format!(
            "SELECT * FROM blind_signature WHERE y IN ({})",
            "?,".repeat(blinded_messages.len()).trim_end_matches(',')
        );

        let mut blinded_signatures = blinded_messages
            .iter()
            .fold(sqlx::query(&sql), |query, y| {
                query.bind(y.to_bytes().to_vec())
            })
            .fetch_all(&mut *transaction)
            .await
            .map_err(|err| {
                tracing::error!("SQLite could not get state of proof: {err:?}");
                Error::SQLX(err)
            })?
            .into_iter()
            .map(|row| {
                PublicKey::from_slice(row.get("y"))
                    .map_err(Error::from)
                    .and_then(|y| sqlite_row_to_blind_signature(row).map(|blinded| (y, blinded)))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;

        Ok(blinded_messages
            .iter()
            .map(|y| blinded_signatures.remove(y))
            .collect())
    }

    async fn get_blind_signatures_for_keyset(
        &self,
        keyset_id: &Id,
    ) -> Result<Vec<BlindSignature>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT *
FROM blind_signature
WHERE keyset_id=?;
        "#,
        )
        .bind(keyset_id.to_string())
        .fetch_all(&mut *transaction)
        .await;

        match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;
                let sigs = rec
                    .into_iter()
                    .map(sqlite_row_to_blind_signature)
                    .collect::<Result<Vec<BlindSignature>, _>>()?;

                Ok(sigs)
            }
            Err(err) => {
                tracing::error!("SQLite could not get vlinf signatures for keyset");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                return Err(Error::from(err).into());
            }
        }
    }

    /// Get [`BlindSignature`]s for quote
    async fn get_blind_signatures_for_quote(
        &self,
        quote_id: &Uuid,
    ) -> Result<Vec<BlindSignature>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let recs = sqlx::query(
            r#"
SELECT *
FROM blind_signature
WHERE quote_id=?;
        "#,
        )
        .bind(quote_id.as_hyphenated())
        .fetch_all(&mut *transaction)
        .await;

        match recs {
            Ok(recs) => {
                transaction.commit().await.map_err(Error::from)?;

                let keysets = recs
                    .into_iter()
                    .map(sqlite_row_to_blind_signature)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(keysets)
            }
            Err(err) => {
                tracing::error!("SQLite could not get active keyset");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }
                Err(Error::from(err).into())
            }
        }
    }
}

#[async_trait]
impl MintDatabase<database::Error> for MintSqliteDatabase {
    async fn set_mint_info(&self, mint_info: MintInfo) -> Result<(), database::Error> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let res = sqlx::query(
            r#"
INSERT INTO config
(id, value)
VALUES (?, ?)
ON CONFLICT(id) DO UPDATE SET
    value = excluded.value
;
        "#,
        )
        .bind("mint_info")
        .bind(serde_json::to_string(&mint_info)?)
        .execute(&mut *transaction)
        .await;

        match res {
            Ok(_) => {
                transaction.commit().await.map_err(Error::from)?;
                Ok(())
            }
            Err(err) => {
                tracing::error!("SQLite Could not update mint info");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                Err(Error::from(err).into())
            }
        }
    }
    async fn get_mint_info(&self) -> Result<MintInfo, database::Error> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT *
FROM config
WHERE id=?;
        "#,
        )
        .bind("mint_info")
        .fetch_one(&mut *transaction)
        .await;

        match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;

                let value: String = rec.try_get("value").map_err(Error::from)?;

                let mint_info = serde_json::from_str(&value)?;

                Ok(mint_info)
            }
            Err(err) => match err {
                sqlx::Error::RowNotFound => {
                    transaction.commit().await.map_err(Error::from)?;
                    return Err(Error::UnknownMintInfo.into());
                }
                _ => {
                    return {
                        if let Err(err) = transaction.rollback().await {
                            tracing::error!("Could not rollback sql transaction: {}", err);
                        }
                        Err(Error::SQLX(err).into())
                    }
                }
            },
        }
    }

    async fn set_quote_ttl(&self, quote_ttl: QuoteTTL) -> Result<(), database::Error> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let res = sqlx::query(
            r#"
INSERT INTO config
(id, value)
VALUES (?, ?)
ON CONFLICT(id) DO UPDATE SET
    value = excluded.value
;
        "#,
        )
        .bind("quote_ttl")
        .bind(serde_json::to_string(&quote_ttl)?)
        .execute(&mut *transaction)
        .await;

        match res {
            Ok(_) => {
                transaction.commit().await.map_err(Error::from)?;
                Ok(())
            }
            Err(err) => {
                tracing::error!("SQLite Could not update mint info");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }

                Err(Error::from(err).into())
            }
        }
    }
    async fn get_quote_ttl(&self) -> Result<QuoteTTL, database::Error> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT *
FROM config
WHERE id=?;
        "#,
        )
        .bind("quote_ttl")
        .fetch_one(&mut *transaction)
        .await;

        match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;

                let value: String = rec.try_get("value").map_err(Error::from)?;

                let quote_ttl = serde_json::from_str(&value)?;

                Ok(quote_ttl)
            }
            Err(err) => match err {
                sqlx::Error::RowNotFound => {
                    transaction.commit().await.map_err(Error::from)?;
                    return Err(Error::UnknownQuoteTTL.into());
                }
                _ => {
                    return {
                        if let Err(err) = transaction.rollback().await {
                            tracing::error!("Could not rollback sql transaction: {}", err);
                        }
                        Err(Error::SQLX(err).into())
                    }
                }
            },
        }
    }
}

fn sqlite_row_to_keyset_info(row: SqliteRow) -> Result<MintKeySetInfo, Error> {
    let row_id: String = row.try_get("id").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let row_active: bool = row.try_get("active").map_err(Error::from)?;
    let row_valid_from: i64 = row.try_get("valid_from").map_err(Error::from)?;
    let row_valid_to: Option<i64> = row.try_get("valid_to").map_err(Error::from)?;
    let row_derivation_path: String = row.try_get("derivation_path").map_err(Error::from)?;
    let row_max_order: u8 = row.try_get("max_order").map_err(Error::from)?;
    let row_keyset_ppk: Option<i64> = row.try_get("input_fee_ppk").ok();
    let row_derivation_path_index: Option<i64> =
        row.try_get("derivation_path_index").map_err(Error::from)?;

    Ok(MintKeySetInfo {
        id: Id::from_str(&row_id).map_err(Error::from)?,
        unit: CurrencyUnit::from_str(&row_unit).map_err(Error::from)?,
        active: row_active,
        valid_from: row_valid_from as u64,
        valid_to: row_valid_to.map(|v| v as u64),
        derivation_path: DerivationPath::from_str(&row_derivation_path).map_err(Error::from)?,
        derivation_path_index: row_derivation_path_index.map(|d| d as u32),
        max_order: row_max_order,
        input_fee_ppk: row_keyset_ppk.unwrap_or(0) as u64,
    })
}

fn sqlite_row_to_mint_quote(row: SqliteRow) -> Result<MintQuote, Error> {
    let row_id: Hyphenated = row.try_get("id").map_err(Error::from)?;
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let row_request: String = row.try_get("request").map_err(Error::from)?;
    let row_state: String = row.try_get("state").map_err(Error::from)?;
    let row_expiry: i64 = row.try_get("expiry").map_err(Error::from)?;
    let row_request_lookup_id: Option<String> =
        row.try_get("request_lookup_id").map_err(Error::from)?;
    let row_pubkey: Option<String> = row.try_get("pubkey").map_err(Error::from)?;

    let row_created_time: i64 = row.try_get("created_time").map_err(Error::from)?;
    let row_paid_time: Option<i64> = row.try_get("paid_time").map_err(Error::from)?;
    let row_issued_time: Option<i64> = row.try_get("issued_time").map_err(Error::from)?;

    let request_lookup_id = match row_request_lookup_id {
        Some(id) => id,
        None => match Bolt11Invoice::from_str(&row_request) {
            Ok(invoice) => invoice.payment_hash().to_string(),
            Err(_) => row_request.clone(),
        },
    };

    let pubkey = row_pubkey
        .map(|key| PublicKey::from_str(&key))
        .transpose()?;

    Ok(MintQuote {
        id: row_id.into_uuid(),
        amount: Amount::from(row_amount as u64),
        unit: CurrencyUnit::from_str(&row_unit).map_err(Error::from)?,
        request: row_request,
        state: MintQuoteState::from_str(&row_state).map_err(Error::from)?,
        expiry: row_expiry as u64,
        request_lookup_id,
        pubkey,
        created_time: row_created_time as u64,
        paid_time: row_paid_time.map(|p| p as u64),
        issued_time: row_issued_time.map(|p| p as u64),
    })
}

fn sqlite_row_to_melt_quote(row: SqliteRow) -> Result<mint::MeltQuote, Error> {
    let row_id: Hyphenated = row.try_get("id").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let row_request: String = row.try_get("request").map_err(Error::from)?;
    let row_fee_reserve: i64 = row.try_get("fee_reserve").map_err(Error::from)?;
    let row_state: String = row.try_get("state").map_err(Error::from)?;
    let row_expiry: i64 = row.try_get("expiry").map_err(Error::from)?;
    let row_preimage: Option<String> = row.try_get("payment_preimage").map_err(Error::from)?;
    let row_request_lookup: Option<String> =
        row.try_get("request_lookup_id").map_err(Error::from)?;

    let request_lookup_id = row_request_lookup.unwrap_or(row_request.clone());

    let row_msat_to_pay: Option<i64> = row.try_get("msat_to_pay").map_err(Error::from)?;

    let row_created_time: i64 = row.try_get("created_time").map_err(Error::from)?;
    let row_paid_time: Option<i64> = row.try_get("paid_time").map_err(Error::from)?;

    Ok(mint::MeltQuote {
        id: row_id.into_uuid(),
        amount: Amount::from(row_amount as u64),
        unit: CurrencyUnit::from_str(&row_unit).map_err(Error::from)?,
        request: row_request,
        fee_reserve: Amount::from(row_fee_reserve as u64),
        state: QuoteState::from_str(&row_state)?,
        expiry: row_expiry as u64,
        payment_preimage: row_preimage,
        request_lookup_id,
        msat_to_pay: row_msat_to_pay.map(|a| Amount::from(a as u64)),
        created_time: row_created_time as u64,
        paid_time: row_paid_time.map(|p| p as u64),
    })
}

fn sqlite_row_to_proof(row: SqliteRow) -> Result<Proof, Error> {
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let keyset_id: String = row.try_get("keyset_id").map_err(Error::from)?;
    let row_secret: String = row.try_get("secret").map_err(Error::from)?;
    let row_c: Vec<u8> = row.try_get("c").map_err(Error::from)?;
    let row_witness: Option<String> = row.try_get("witness").map_err(Error::from)?;

    Ok(Proof {
        amount: Amount::from(row_amount as u64),
        keyset_id: Id::from_str(&keyset_id)?,
        secret: Secret::from_str(&row_secret)?,
        c: PublicKey::from_slice(&row_c)?,
        witness: row_witness.and_then(|w| serde_json::from_str(&w).ok()),
        dleq: None,
    })
}

fn sqlite_row_to_proof_with_state(row: SqliteRow) -> Result<(Proof, Option<State>), Error> {
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let keyset_id: String = row.try_get("keyset_id").map_err(Error::from)?;
    let row_secret: String = row.try_get("secret").map_err(Error::from)?;
    let row_c: Vec<u8> = row.try_get("c").map_err(Error::from)?;
    let row_witness: Option<String> = row.try_get("witness").map_err(Error::from)?;

    let row_state: Option<String> = row.try_get("state").map_err(Error::from)?;

    let state = row_state.and_then(|s| State::from_str(&s).ok());

    Ok((
        Proof {
            amount: Amount::from(row_amount as u64),
            keyset_id: Id::from_str(&keyset_id)?,
            secret: Secret::from_str(&row_secret)?,
            c: PublicKey::from_slice(&row_c)?,
            witness: row_witness.and_then(|w| serde_json::from_str(&w).ok()),
            dleq: None,
        },
        state,
    ))
}

fn sqlite_row_to_blind_signature(row: SqliteRow) -> Result<BlindSignature, Error> {
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let keyset_id: String = row.try_get("keyset_id").map_err(Error::from)?;
    let row_c: Vec<u8> = row.try_get("c").map_err(Error::from)?;
    let row_dleq_e: Option<String> = row.try_get("dleq_e").map_err(Error::from)?;
    let row_dleq_s: Option<String> = row.try_get("dleq_s").map_err(Error::from)?;

    let dleq = match (row_dleq_e, row_dleq_s) {
        (Some(e), Some(s)) => Some(BlindSignatureDleq {
            e: SecretKey::from_hex(e)?,
            s: SecretKey::from_hex(s)?,
        }),
        _ => None,
    };

    Ok(BlindSignature {
        amount: Amount::from(row_amount as u64),
        keyset_id: Id::from_str(&keyset_id)?,
        c: PublicKey::from_slice(&row_c)?,
        dleq,
    })
}

fn sqlite_row_to_melt_request(
    row: SqliteRow,
) -> Result<(MeltRequest<Uuid>, PaymentProcessorKey), Error> {
    let quote_id: Hyphenated = row.try_get("id").map_err(Error::from)?;
    let row_inputs: String = row.try_get("inputs").map_err(Error::from)?;
    let row_outputs: Option<String> = row.try_get("outputs").map_err(Error::from)?;
    let row_method: String = row.try_get("method").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;

    let melt_request = MeltRequest::new(
        quote_id.into_uuid(),
        serde_json::from_str(&row_inputs)?,
        row_outputs.and_then(|o| serde_json::from_str(&o).ok()),
    );

    let ln_key = PaymentProcessorKey {
        unit: CurrencyUnit::from_str(&row_unit)?,
        method: PaymentMethod::from_str(&row_method)?,
    };

    Ok((melt_request, ln_key))
}

#[cfg(test)]
mod tests {
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
            valid_to: None,
            derivation_path: bitcoin::bip32::DerivationPath::from_str("m/0'/0'/0'").unwrap(),
            derivation_path_index: Some(0),
            max_order: 32,
            input_fee_ppk: 0,
        };
        db.add_keyset_info(keyset_info).await.unwrap();

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
        db.add_proofs(proofs.clone(), None).await.unwrap();

        // Mark one proof as spent
        db.update_proofs_states(&[proofs[0].y().unwrap()], State::Spent)
            .await
            .unwrap();

        // Try to remove both proofs - should fail because one is spent
        let result = db
            .remove_proofs(&[proofs[0].y().unwrap(), proofs[1].y().unwrap()], None)
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            database::Error::AttemptRemoveSpentProof
        ));

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
            valid_to: None,
            derivation_path: bitcoin::bip32::DerivationPath::from_str("m/0'/0'/0'").unwrap(),
            derivation_path_index: Some(0),
            max_order: 32,
            input_fee_ppk: 0,
        };
        db.add_keyset_info(keyset_info).await.unwrap();

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
        db.add_proofs(proofs.clone(), None).await.unwrap();

        // Mark one proof as spent
        db.update_proofs_states(&[proofs[0].y().unwrap()], State::Spent)
            .await
            .unwrap();

        // Try to update both proofs - should fail because one is spent
        let result = db
            .update_proofs_states(&[proofs[0].y().unwrap()], State::Unspent)
            .await;

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
}

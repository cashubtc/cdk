//! SQLite Mint

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use bitcoin::bip32::DerivationPath;
use cdk::cdk_database::{self, MintDatabase};
use cdk::mint::{MintKeySetInfo, MintQuote};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut05::QuoteState;
use cdk::nuts::{
    BlindSignature, CurrencyUnit, Id, MeltQuoteState, MintQuoteState, Proof, Proofs, PublicKey,
    State,
};
use cdk::secret::Secret;
use cdk::{mint, Amount};
use error::Error;
use lightning_invoice::Bolt11Invoice;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::Row;

pub mod error;

/// Mint SQLite Database
#[derive(Debug, Clone)]
pub struct MintSqliteDatabase {
    pool: SqlitePool,
}

impl MintSqliteDatabase {
    /// Create new [`MintSqliteDatabase`]
    pub async fn new(path: &Path) -> Result<Self, Error> {
        let path = path.to_str().ok_or(Error::InvalidDbPath)?;
        let db_options = SqliteConnectOptions::from_str(path)?
            .busy_timeout(Duration::from_secs(5))
            .read_only(false)
            .create_if_missing(true)
            .auto_vacuum(sqlx::sqlite::SqliteAutoVacuum::Full);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(db_options)
            .await?;

        Ok(Self { pool })
    }

    /// Migrate [`MintSqliteDatabase`]
    pub async fn migrate(&self) {
        sqlx::migrate!("./src/mint/migrations")
            .run(&self.pool)
            .await
            .expect("Could not run migrations");
    }
}

#[async_trait]
impl MintDatabase for MintSqliteDatabase {
    type Err = cdk_database::Error;

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
        .execute(&mut transaction)
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
        .execute(&mut transaction)
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
        .fetch_one(&mut transaction)
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
        .fetch_all(&mut transaction)
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

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let res = sqlx::query(
            r#"
INSERT OR REPLACE INTO mint_quote
(id, mint_url, amount, unit, request, state, expiry, request_lookup_id)
VALUES (?, ?, ?, ?, ?, ?, ?, ?);
        "#,
        )
        .bind(quote.id.to_string())
        .bind(quote.mint_url.to_string())
        .bind(u64::from(quote.amount) as i64)
        .bind(quote.unit.to_string())
        .bind(quote.request)
        .bind(quote.state.to_string())
        .bind(quote.expiry as i64)
        .bind(quote.request_lookup_id)
        .execute(&mut transaction)
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

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
WHERE id=?;
        "#,
        )
        .bind(quote_id)
        .fetch_one(&mut transaction)
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
        .fetch_one(&mut transaction)
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
        .fetch_one(&mut transaction)
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
        quote_id: &str,
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
        .bind(quote_id)
        .fetch_one(&mut transaction)
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

        let update = sqlx::query(
            r#"
        UPDATE mint_quote SET state = ? WHERE id = ?
        "#,
        )
        .bind(state.to_string())
        .bind(quote_id)
        .execute(&mut transaction)
        .await;

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
        .fetch_all(&mut transaction)
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

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let res = sqlx::query(
            r#"
DELETE FROM mint_quote
WHERE id=?
        "#,
        )
        .bind(quote_id)
        .execute(&mut transaction)
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
INSERT OR REPLACE INTO melt_quote
(id, unit, amount, request, fee_reserve, state, expiry, payment_preimage, request_lookup_id)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);
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
        .execute(&mut transaction)
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
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<mint::MeltQuote>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let rec = sqlx::query(
            r#"
SELECT *
FROM melt_quote
WHERE id=?;
        "#,
        )
        .bind(quote_id)
        .fetch_one(&mut transaction)
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
        .fetch_all(&mut transaction)
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
        quote_id: &str,
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
        .bind(quote_id)
        .fetch_one(&mut transaction)
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

        let rec = sqlx::query(
            r#"
        UPDATE melt_quote SET state = ? WHERE id = ?
        "#,
        )
        .bind(state.to_string())
        .bind(quote_id)
        .execute(&mut transaction)
        .await;

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

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let res = sqlx::query(
            r#"
DELETE FROM melt_quote
WHERE id=?
        "#,
        )
        .bind(quote_id)
        .execute(&mut transaction)
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

    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let res = sqlx::query(
            r#"
INSERT OR REPLACE INTO keyset
(id, unit, active, valid_from, valid_to, derivation_path, max_order, input_fee_ppk, derivation_path_index)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);
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
        .execute(&mut transaction)
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
        .fetch_one(&mut transaction)
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
        .fetch_all(&mut transaction)
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

    async fn add_proofs(&self, proofs: Proofs) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        for proof in proofs {
            if let Err(err) = sqlx::query(
                r#"
INSERT INTO proof
(y, amount, keyset_id, secret, c, witness, state)
VALUES (?, ?, ?, ?, ?, ?, ?);
        "#,
            )
            .bind(proof.y()?.to_bytes().to_vec())
            .bind(u64::from(proof.amount) as i64)
            .bind(proof.keyset_id.to_string())
            .bind(proof.secret.to_string())
            .bind(proof.c.to_bytes().to_vec())
            .bind(proof.witness.map(|w| serde_json::to_string(&w).unwrap()))
            .bind("UNSPENT")
            .execute(&mut transaction)
            .await
            .map_err(Error::from)
            {
                tracing::debug!("Attempting to add known proof. Skipping.... {:?}", err);
            }
        }
        transaction.commit().await.map_err(Error::from)?;

        Ok(())
    }
    async fn get_proofs_by_ys(&self, ys: &[PublicKey]) -> Result<Vec<Option<Proof>>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let mut proofs = Vec::with_capacity(ys.len());

        for y in ys {
            let rec = sqlx::query(
                r#"
SELECT *
FROM proof
WHERE y=?;
        "#,
            )
            .bind(y.to_bytes().to_vec())
            .fetch_one(&mut transaction)
            .await;

            match rec {
                Ok(rec) => {
                    proofs.push(Some(sqlite_row_to_proof(rec)?));
                }
                Err(err) => match err {
                    sqlx::Error::RowNotFound => proofs.push(None),
                    _ => {
                        if let Err(err) = transaction.rollback().await {
                            tracing::error!("Could not rollback sql transaction: {}", err);
                        }
                        return Err(Error::SQLX(err).into());
                    }
                },
            };
        }

        transaction.commit().await.map_err(Error::from)?;

        Ok(proofs)
    }

    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let mut states = Vec::with_capacity(ys.len());

        for y in ys {
            let rec = sqlx::query(
                r#"
SELECT state
FROM proof
WHERE y=?;
        "#,
            )
            .bind(y.to_bytes().to_vec())
            .fetch_one(&mut transaction)
            .await;

            match rec {
                Ok(rec) => {
                    let state: String = rec.get("state");
                    let state = State::from_str(&state).map_err(Error::from)?;
                    states.push(Some(state));
                }
                Err(err) => match err {
                    sqlx::Error::RowNotFound => states.push(None),
                    _ => {
                        if let Err(err) = transaction.rollback().await {
                            tracing::error!("Could not rollback sql transaction: {}", err);
                        }
                        return Err(Error::SQLX(err).into());
                    }
                },
            };
        }

        transaction.commit().await.map_err(Error::from)?;

        Ok(states)
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
        .fetch_all(&mut transaction)
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

        let mut states = Vec::with_capacity(ys.len());

        let proofs_state = proofs_state.to_string();
        for y in ys {
            let current_state;
            let y = y.to_bytes().to_vec();
            let rec = sqlx::query(
                r#"
SELECT state
FROM proof
WHERE y=?;
        "#,
            )
            .bind(&y)
            .fetch_one(&mut transaction)
            .await;

            match rec {
                Ok(rec) => {
                    let state: String = rec.get("state");
                    current_state = Some(State::from_str(&state).map_err(Error::from)?);
                }
                Err(err) => match err {
                    sqlx::Error::RowNotFound => {
                        current_state = None;
                    }
                    _ => {
                        tracing::error!("SQLite could not get state of proof");
                        if let Err(err) = transaction.rollback().await {
                            tracing::error!("Could not rollback sql transaction: {}", err);
                        }
                        return Err(Error::SQLX(err).into());
                    }
                },
            };

            states.push(current_state);

            if current_state != Some(State::Spent) {
                let res = sqlx::query(
                    r#"
        UPDATE proof SET state = ? WHERE y = ?
        "#,
                )
                .bind(&proofs_state)
                .bind(y)
                .execute(&mut transaction)
                .await;

                if let Err(err) = res {
                    tracing::error!("SQLite could not update proof state");
                    if let Err(err) = transaction.rollback().await {
                        tracing::error!("Could not rollback sql transaction: {}", err);
                    }
                    return Err(Error::SQLX(err).into());
                }
            }
        }

        transaction.commit().await.map_err(Error::from)?;

        Ok(states)
    }

    async fn add_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
        blinded_signatures: &[BlindSignature],
    ) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        for (message, signature) in blinded_messages.iter().zip(blinded_signatures) {
            let res = sqlx::query(
                r#"
INSERT INTO blind_signature
(y, amount, keyset_id, c)
VALUES (?, ?, ?, ?);
        "#,
            )
            .bind(message.to_bytes().to_vec())
            .bind(u64::from(signature.amount) as i64)
            .bind(signature.keyset_id.to_string())
            .bind(signature.c.to_bytes().to_vec())
            .execute(&mut transaction)
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

        let mut signatures = Vec::with_capacity(blinded_messages.len());

        for message in blinded_messages {
            let rec = sqlx::query(
                r#"
SELECT *
FROM blind_signature
WHERE y=?;
        "#,
            )
            .bind(message.to_bytes().to_vec())
            .fetch_one(&mut transaction)
            .await;

            if let Ok(row) = rec {
                let blinded = sqlite_row_to_blind_signature(row)?;

                signatures.push(Some(blinded));
            } else {
                signatures.push(None);
            }
        }

        transaction.commit().await.map_err(Error::from)?;

        Ok(signatures)
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
        .fetch_all(&mut transaction)
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
}

fn sqlite_row_to_keyset_info(row: SqliteRow) -> Result<MintKeySetInfo, Error> {
    let row_id: String = row.try_get("id").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let row_active: bool = row.try_get("active").map_err(Error::from)?;
    let row_valid_from: i64 = row.try_get("valid_from").map_err(Error::from)?;
    let row_valid_to: Option<i64> = row.try_get("valid_to").map_err(Error::from)?;
    let row_derivation_path: String = row.try_get("derivation_path").map_err(Error::from)?;
    let row_max_order: u8 = row.try_get("max_order").map_err(Error::from)?;
    let row_keyset_ppk: Option<i64> = row.try_get("input_fee_ppk").map_err(Error::from)?;
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
    let row_id: String = row.try_get("id").map_err(Error::from)?;
    let row_mint_url: String = row.try_get("mint_url").map_err(Error::from)?;
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let row_request: String = row.try_get("request").map_err(Error::from)?;
    let row_state: String = row.try_get("state").map_err(Error::from)?;
    let row_expiry: i64 = row.try_get("expiry").map_err(Error::from)?;
    let row_request_lookup_id: Option<String> =
        row.try_get("request_lookup_id").map_err(Error::from)?;

    let request_lookup_id = match row_request_lookup_id {
        Some(id) => id,
        None => match Bolt11Invoice::from_str(&row_request) {
            Ok(invoice) => invoice.payment_hash().to_string(),
            Err(_) => row_request.clone(),
        },
    };

    Ok(MintQuote {
        id: row_id,
        mint_url: MintUrl::from_str(&row_mint_url)?,
        amount: Amount::from(row_amount as u64),
        unit: CurrencyUnit::from_str(&row_unit).map_err(Error::from)?,
        request: row_request,
        state: MintQuoteState::from_str(&row_state).map_err(Error::from)?,
        expiry: row_expiry as u64,
        request_lookup_id,
    })
}

fn sqlite_row_to_melt_quote(row: SqliteRow) -> Result<mint::MeltQuote, Error> {
    let row_id: String = row.try_get("id").map_err(Error::from)?;
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

    Ok(mint::MeltQuote {
        id: row_id,
        amount: Amount::from(row_amount as u64),
        unit: CurrencyUnit::from_str(&row_unit).map_err(Error::from)?,
        request: row_request,
        fee_reserve: Amount::from(row_fee_reserve as u64),
        state: QuoteState::from_str(&row_state)?,
        expiry: row_expiry as u64,
        payment_preimage: row_preimage,
        request_lookup_id,
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

    Ok(BlindSignature {
        amount: Amount::from(row_amount as u64),
        keyset_id: Id::from_str(&keyset_id)?,
        c: PublicKey::from_slice(&row_c)?,
        dleq: None,
    })
}

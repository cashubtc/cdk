//! SQLite Wallet Database

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::common::ProofInfo;
use cdk_common::database::WalletDatabase;
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{MeltQuoteState, MintQuoteState};
use cdk_common::secret::Secret;
use cdk_common::wallet::{self, MintQuote, Transaction, TransactionDirection, TransactionId};
use cdk_common::{
    database, nut01, Amount, CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, Proof, ProofDleq,
    PublicKey, SecretKey, SpendingConditions, State,
};
use error::Error;
use sqlx::sqlite::SqliteRow;
use sqlx::{Pool, Row, Sqlite};
use tracing::instrument;

use crate::common::create_sqlite_pool;

pub mod error;
pub mod memory;

/// Wallet SQLite Database
#[derive(Debug, Clone)]
pub struct WalletSqliteDatabase {
    pool: Pool<Sqlite>,
}

impl WalletSqliteDatabase {
    /// Create new [`WalletSqliteDatabase`]
    #[cfg(not(feature = "sqlcipher"))]
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        Ok(Self {
            pool: create_sqlite_pool(path.as_ref().to_str().ok_or(Error::InvalidDbPath)?).await?,
        })
    }

    /// Create new [`WalletSqliteDatabase`]
    #[cfg(feature = "sqlcipher")]
    pub async fn new<P: AsRef<Path>>(path: P, password: String) -> Result<Self, Error> {
        Ok(Self {
            pool: create_sqlite_pool(
                path.as_ref().to_str().ok_or(Error::InvalidDbPath)?,
                password,
            )
            .await?,
        })
    }

    /// Migrate [`WalletSqliteDatabase`]
    pub async fn migrate(&self) {
        sqlx::migrate!("./src/wallet/migrations")
            .run(&self.pool)
            .await
            .expect("Could not run migrations");
    }
}

#[async_trait]
impl WalletDatabase for WalletSqliteDatabase {
    type Err = database::Error;

    #[instrument(skip(self, mint_info))]
    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err> {
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

        sqlx::query(
            r#"
INSERT INTO mint
(mint_url, name, pubkey, version, description, description_long, contact, nuts, icon_url, urls, motd, mint_time, tos_url)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        )
        .bind(mint_url.to_string())
        .bind(name)
        .bind(pubkey)
        .bind(version)
        .bind(description)
        .bind(description_long)
        .bind(contact)
        .bind(nuts)
        .bind(icon_url)
        .bind(urls)
        .bind(motd)
        .bind(time.map(|v| v as i64))
        .bind(tos_url)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
DELETE FROM mint
WHERE mint_url=?
        "#,
        )
        .bind(mint_url.to_string())
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM mint
WHERE mint_url=?;
        "#,
        )
        .bind(mint_url.to_string())
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(sqlite_row_to_mint_info(&rec)?))
    }

    #[instrument(skip(self))]
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM mint
        "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)?;

        let mints = rec
            .into_iter()
            .flat_map(|row| {
                let mint_url: String = row.get("mint_url");

                // Attempt to parse mint_url and convert mint_info
                let mint_result = MintUrl::from_str(&mint_url).ok();
                let mint_info = sqlite_row_to_mint_info(&row).ok();

                // Combine mint_result and mint_info into an Option tuple
                mint_result.map(|mint| (mint, mint_info))
            })
            .collect();

        Ok(mints)
    }

    #[instrument(skip(self))]
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Self::Err> {
        let tables = ["mint_quote", "proof"];
        for table in &tables {
            let query = format!(
                r#"
            UPDATE {}
            SET mint_url = ?
            WHERE mint_url = ?;
            "#,
                table
            );

            sqlx::query(&query)
                .bind(new_mint_url.to_string())
                .bind(old_mint_url.to_string())
                .execute(&self.pool)
                .await
                .map_err(Error::from)?;
        }
        Ok(())
    }

    #[instrument(skip(self, keysets))]
    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Self::Err> {
        for keyset in keysets {
            sqlx::query(
                r#"
    INSERT INTO keyset
    (mint_url, id, unit, active, input_fee_ppk)
    VALUES (?, ?, ?, ?, ?)
    ON CONFLICT(id) DO UPDATE SET
        mint_url = excluded.mint_url,
        unit = excluded.unit,
        active = excluded.active,
        input_fee_ppk = excluded.input_fee_ppk;
    "#,
            )
            .bind(mint_url.to_string())
            .bind(keyset.id.to_string())
            .bind(keyset.unit.to_string())
            .bind(keyset.active)
            .bind(keyset.input_fee_ppk as i64)
            .execute(&self.pool)
            .await
            .map_err(Error::from)?;
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err> {
        let recs = sqlx::query(
            r#"
SELECT *
FROM keyset
WHERE mint_url=?
        "#,
        )
        .bind(mint_url.to_string())
        .fetch_all(&self.pool)
        .await;

        let recs = match recs {
            Ok(recs) => recs,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        let keysets = recs
            .iter()
            .map(sqlite_row_to_keyset)
            .collect::<Result<Vec<KeySetInfo>, _>>()?;

        match keysets.is_empty() {
            false => Ok(Some(keysets)),
            true => Ok(None),
        }
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM keyset
WHERE id=?
        "#,
        )
        .bind(keyset_id.to_string())
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(recs) => recs,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(sqlite_row_to_keyset(&rec)?))
    }

    #[instrument(skip_all)]
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
INSERT INTO mint_quote
(id, mint_url, amount, unit, request, state, expiry, secret_key)
VALUES (?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
    mint_url = excluded.mint_url,
    amount = excluded.amount,
    unit = excluded.unit,
    request = excluded.request,
    state = excluded.state,
    expiry = excluded.expiry,
    secret_key = excluded.secret_key
;
        "#,
        )
        .bind(quote.id.to_string())
        .bind(quote.mint_url.to_string())
        .bind(u64::from(quote.amount) as i64)
        .bind(quote.unit.to_string())
        .bind(quote.request)
        .bind(quote.state.to_string())
        .bind(quote.expiry as i64)
        .bind(quote.secret_key.map(|p| p.to_string()))
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
WHERE id=?;
        "#,
        )
        .bind(quote_id)
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(sqlite_row_to_mint_quote(&rec)?))
    }

    #[instrument(skip(self))]
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
        "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)?;

        let mint_quotes = rec
            .iter()
            .map(sqlite_row_to_mint_quote)
            .collect::<Result<_, _>>()?;

        Ok(mint_quotes)
    }

    #[instrument(skip(self))]
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
DELETE FROM mint_quote
WHERE id=?
        "#,
        )
        .bind(quote_id)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_melt_quote(&self, quote: wallet::MeltQuote) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
INSERT INTO melt_quote
(id, unit, amount, request, fee_reserve, state, expiry)
VALUES (?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
    unit = excluded.unit,
    amount = excluded.amount,
    request = excluded.request,
    fee_reserve = excluded.fee_reserve,
    state = excluded.state,
    expiry = excluded.expiry
;
        "#,
        )
        .bind(quote.id.to_string())
        .bind(quote.unit.to_string())
        .bind(u64::from(quote.amount) as i64)
        .bind(quote.request)
        .bind(u64::from(quote.fee_reserve) as i64)
        .bind(quote.state.to_string())
        .bind(quote.expiry as i64)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<wallet::MeltQuote>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM melt_quote
WHERE id=?;
        "#,
        )
        .bind(quote_id)
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(sqlite_row_to_melt_quote(&rec)?))
    }

    #[instrument(skip(self))]
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
DELETE FROM melt_quote
WHERE id=?
        "#,
        )
        .bind(quote_id)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_keys(&self, keys: Keys) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
INSERT INTO key
(id, keys)
VALUES (?, ?)
ON CONFLICT(id) DO UPDATE SET
    keys = excluded.keys
;
        "#,
        )
        .bind(Id::from(&keys).to_string())
        .bind(serde_json::to_string(&keys).map_err(Error::from)?)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keys(&self, keyset_id: &Id) -> Result<Option<Keys>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM key
WHERE id=?;
        "#,
        )
        .bind(keyset_id.to_string())
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        let keys: String = rec.get("keys");

        Ok(serde_json::from_str(&keys).map_err(Error::from)?)
    }

    #[instrument(skip(self))]
    async fn remove_keys(&self, id: &Id) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
DELETE FROM key
WHERE id=?
        "#,
        )
        .bind(id.to_string())
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }

    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Self::Err> {
        for proof in added {
            sqlx::query(
                r#"
    INSERT INTO proof
    (y, mint_url, state, spending_condition, unit, amount, keyset_id, secret, c, witness, dleq_e, dleq_s, dleq_r)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            )
            .bind(proof.y.to_bytes().to_vec())
            .bind(proof.mint_url.to_string())
            .bind(proof.state.to_string())
            .bind(
                proof
                    .spending_condition
                    .map(|s| serde_json::to_string(&s).ok()),
            )
            .bind(proof.unit.to_string())
            .bind(u64::from(proof.proof.amount) as i64)
            .bind(proof.proof.keyset_id.to_string())
            .bind(proof.proof.secret.to_string())
            .bind(proof.proof.c.to_bytes().to_vec())
            .bind(
                proof
                    .proof
                    .witness
                    .map(|w| serde_json::to_string(&w).unwrap()),
            )
            .bind(
                proof.proof.dleq.as_ref().map(|dleq| dleq.e.to_secret_bytes().to_vec()),
            )
            .bind(
                proof.proof.dleq.as_ref().map(|dleq| dleq.s.to_secret_bytes().to_vec()),
            )
            .bind(
                proof.proof.dleq.as_ref().map(|dleq| dleq.r.to_secret_bytes().to_vec()),
            )
            .execute(&self.pool)
            .await
            .map_err(Error::from)?;
        }

        // TODO: Generate a IN clause
        for y in removed_ys {
            sqlx::query(
                r#"
    DELETE FROM proof
    WHERE y = ?
            "#,
            )
            .bind(y.to_bytes().to_vec())
            .execute(&self.pool)
            .await
            .map_err(Error::from)?;
        }

        Ok(())
    }

    #[instrument(skip(self, state, spending_conditions))]
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Self::Err> {
        let recs = sqlx::query(
            r#"
SELECT *
FROM proof;
        "#,
        )
        .fetch_all(&self.pool)
        .await;

        let recs = match recs {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(vec![]),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        let proofs: Vec<ProofInfo> = recs
            .iter()
            .filter_map(|p| match sqlite_row_to_proof_info(p) {
                Ok(proof_info) => {
                    match proof_info.matches_conditions(
                        &mint_url,
                        &unit,
                        &state,
                        &spending_conditions,
                    ) {
                        true => Some(proof_info),
                        false => None,
                    }
                }
                Err(err) => {
                    tracing::error!("Could not deserialize proof row: {}", err);
                    None
                }
            })
            .collect();

        match proofs.is_empty() {
            false => Ok(proofs),
            true => return Ok(vec![]),
        }
    }

    async fn update_proofs_state(&self, ys: Vec<PublicKey>, state: State) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let update_sql = format!(
            "UPDATE proof SET state = ? WHERE y IN ({})",
            "?,".repeat(ys.len()).trim_end_matches(',')
        );

        ys.iter()
            .fold(
                sqlx::query(&update_sql).bind(state.to_string()),
                |query, y| query.bind(y.to_bytes().to_vec()),
            )
            .execute(&mut *transaction)
            .await
            .map_err(|err| {
                tracing::error!("SQLite could not update proof state: {err:?}");
                Error::SQLX(err)
            })?;

        transaction.commit().await.map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        sqlx::query(
            r#"
UPDATE keyset
SET counter=counter+?
WHERE id=?;
        "#,
        )
        .bind(count as i64)
        .bind(keyset_id.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(Error::from)?;

        transaction.commit().await.map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u32>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT counter
FROM keyset
WHERE id=?;
        "#,
        )
        .bind(keyset_id.to_string())
        .fetch_one(&self.pool)
        .await;

        let count = match rec {
            Ok(rec) => {
                let count: Option<u32> = rec.try_get("counter").map_err(Error::from)?;
                count
            }
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(count)
    }

    #[instrument(skip(self))]
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), Self::Err> {
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

        sqlx::query(
            r#"
INSERT INTO transactions
(id, mint_url, direction, unit, amount, fee, ys, timestamp, memo, metadata)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
    mint_url = excluded.mint_url,
    direction = excluded.direction,
    unit = excluded.unit,
    amount = excluded.amount,
    fee = excluded.fee,
    ys = excluded.ys,
    timestamp = excluded.timestamp,
    memo = excluded.memo,
    metadata = excluded.metadata
;
        "#,
        )
        .bind(transaction.id().as_slice())
        .bind(mint_url)
        .bind(direction)
        .bind(unit)
        .bind(amount)
        .bind(fee)
        .bind(ys)
        .bind(transaction.timestamp as i64)
        .bind(transaction.memo)
        .bind(serde_json::to_string(&transaction.metadata).map_err(Error::from)?)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM transactions
WHERE id=?;
        "#,
        )
        .bind(transaction_id.as_slice())
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        let transaction = sqlite_row_to_transaction(&rec)?;

        Ok(Some(transaction))
    }

    #[instrument(skip(self))]
    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, Self::Err> {
        let recs = sqlx::query(
            r#"
SELECT *
FROM transactions;
        "#,
        )
        .fetch_all(&self.pool)
        .await;

        let recs = match recs {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(vec![]),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        let transactions = recs
            .iter()
            .filter_map(|p| {
                let transaction = sqlite_row_to_transaction(p).ok()?;
                if transaction.matches_conditions(&mint_url, &direction, &unit) {
                    Some(transaction)
                } else {
                    None
                }
            })
            .collect();

        Ok(transactions)
    }

    #[instrument(skip(self))]
    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
DELETE FROM transactions
WHERE id=?
        "#,
        )
        .bind(transaction_id.as_slice())
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }
}

fn sqlite_row_to_mint_info(row: &SqliteRow) -> Result<MintInfo, Error> {
    let name: Option<String> = row.try_get("name").map_err(Error::from)?;
    let row_pubkey: Option<Vec<u8>> = row.try_get("pubkey").map_err(Error::from)?;
    let row_version: Option<String> = row.try_get("version").map_err(Error::from)?;
    let description: Option<String> = row.try_get("description").map_err(Error::from)?;
    let description_long: Option<String> = row.try_get("description_long").map_err(Error::from)?;
    let row_contact: Option<String> = row.try_get("contact").map_err(Error::from)?;
    let row_nuts: Option<String> = row.try_get("nuts").map_err(Error::from)?;
    let icon_url: Option<String> = row.try_get("icon_url").map_err(Error::from)?;
    let motd: Option<String> = row.try_get("motd").map_err(Error::from)?;
    let row_urls: Option<String> = row.try_get("urls").map_err(Error::from)?;
    let time: Option<i64> = row.try_get("mint_time").map_err(Error::from)?;
    let tos_url: Option<String> = row.try_get("tos_url").map_err(Error::from)?;
    Ok(MintInfo {
        name,
        pubkey: row_pubkey.and_then(|p| PublicKey::from_slice(&p).ok()),
        version: row_version.and_then(|v| serde_json::from_str(&v).ok()),
        description,
        description_long,
        contact: row_contact.and_then(|c| serde_json::from_str(&c).ok()),
        nuts: row_nuts
            .and_then(|n| serde_json::from_str(&n).ok())
            .unwrap_or_default(),
        icon_url,
        urls: row_urls.and_then(|c| serde_json::from_str(&c).ok()),
        motd,
        time: time.map(|t| t as u64),
        tos_url,
    })
}

fn sqlite_row_to_keyset(row: &SqliteRow) -> Result<KeySetInfo, Error> {
    let row_id: String = row.try_get("id").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let active: bool = row.try_get("active").map_err(Error::from)?;
    let row_keyset_ppk: Option<i64> = row.try_get("input_fee_ppk").map_err(Error::from)?;

    Ok(KeySetInfo {
        id: Id::from_str(&row_id)?,
        unit: CurrencyUnit::from_str(&row_unit).map_err(Error::from)?,
        active,
        input_fee_ppk: row_keyset_ppk.unwrap_or(0) as u64,
    })
}

fn sqlite_row_to_mint_quote(row: &SqliteRow) -> Result<MintQuote, Error> {
    let row_id: String = row.try_get("id").map_err(Error::from)?;
    let row_mint_url: String = row.try_get("mint_url").map_err(Error::from)?;
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let row_request: String = row.try_get("request").map_err(Error::from)?;
    let row_state: String = row.try_get("state").map_err(Error::from)?;
    let row_expiry: i64 = row.try_get("expiry").map_err(Error::from)?;
    let row_secret: Option<String> = row.try_get("secret_key").map_err(Error::from)?;

    let state = MintQuoteState::from_str(&row_state)?;

    let secret_key = row_secret
        .map(|key| SecretKey::from_str(&key))
        .transpose()?;

    Ok(MintQuote {
        id: row_id,
        mint_url: MintUrl::from_str(&row_mint_url)?,
        amount: Amount::from(row_amount as u64),
        unit: CurrencyUnit::from_str(&row_unit).map_err(Error::from)?,
        request: row_request,
        state,
        expiry: row_expiry as u64,
        secret_key,
    })
}

fn sqlite_row_to_melt_quote(row: &SqliteRow) -> Result<wallet::MeltQuote, Error> {
    let row_id: String = row.try_get("id").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let row_request: String = row.try_get("request").map_err(Error::from)?;
    let row_fee_reserve: i64 = row.try_get("fee_reserve").map_err(Error::from)?;
    let row_state: String = row.try_get("state").map_err(Error::from)?;
    let row_expiry: i64 = row.try_get("expiry").map_err(Error::from)?;
    let row_preimage: Option<String> = row.try_get("payment_preimage").map_err(Error::from)?;

    let state = MeltQuoteState::from_str(&row_state)?;
    Ok(wallet::MeltQuote {
        id: row_id,
        amount: Amount::from(row_amount as u64),
        unit: CurrencyUnit::from_str(&row_unit).map_err(Error::from)?,
        request: row_request,
        fee_reserve: Amount::from(row_fee_reserve as u64),
        state,
        expiry: row_expiry as u64,
        payment_preimage: row_preimage,
    })
}

fn sqlite_row_to_proof_info(row: &SqliteRow) -> Result<ProofInfo, Error> {
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let keyset_id: String = row.try_get("keyset_id").map_err(Error::from)?;
    let row_secret: String = row.try_get("secret").map_err(Error::from)?;
    let row_c: Vec<u8> = row.try_get("c").map_err(Error::from)?;
    let row_witness: Option<String> = row.try_get("witness").map_err(Error::from)?;

    // Get DLEQ fields
    let row_dleq_e: Option<Vec<u8>> = row.try_get("dleq_e").map_err(Error::from)?;
    let row_dleq_s: Option<Vec<u8>> = row.try_get("dleq_s").map_err(Error::from)?;
    let row_dleq_r: Option<Vec<u8>> = row.try_get("dleq_r").map_err(Error::from)?;

    let y: Vec<u8> = row.try_get("y").map_err(Error::from)?;
    let row_mint_url: String = row.try_get("mint_url").map_err(Error::from)?;
    let row_state: String = row.try_get("state").map_err(Error::from)?;
    let row_spending_condition: Option<String> =
        row.try_get("spending_condition").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;

    // Create DLEQ proof if all fields are present
    let dleq = match (row_dleq_e, row_dleq_s, row_dleq_r) {
        (Some(e), Some(s), Some(r)) => {
            let e_key = SecretKey::from_slice(&e)?;
            let s_key = SecretKey::from_slice(&s)?;
            let r_key = SecretKey::from_slice(&r)?;

            Some(ProofDleq::new(e_key, s_key, r_key))
        }
        _ => None,
    };

    let proof = Proof {
        amount: Amount::from(row_amount as u64),
        keyset_id: Id::from_str(&keyset_id)?,
        secret: Secret::from_str(&row_secret)?,
        c: PublicKey::from_slice(&row_c)?,
        witness: row_witness.and_then(|w| serde_json::from_str(&w).ok()),
        dleq,
    };

    Ok(ProofInfo {
        proof,
        y: PublicKey::from_slice(&y)?,
        mint_url: MintUrl::from_str(&row_mint_url)?,
        state: State::from_str(&row_state)?,
        spending_condition: row_spending_condition.and_then(|r| serde_json::from_str(&r).ok()),
        unit: CurrencyUnit::from_str(&row_unit).map_err(Error::from)?,
    })
}

fn sqlite_row_to_transaction(row: &SqliteRow) -> Result<Transaction, Error> {
    let mint_url: String = row.try_get("mint_url").map_err(Error::from)?;
    let direction: String = row.try_get("direction").map_err(Error::from)?;
    let unit: String = row.try_get("unit").map_err(Error::from)?;
    let amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let fee: i64 = row.try_get("fee").map_err(Error::from)?;
    let ys: Vec<u8> = row.try_get("ys").map_err(Error::from)?;
    let timestamp: i64 = row.try_get("timestamp").map_err(Error::from)?;
    let memo: Option<String> = row.try_get("memo").map_err(Error::from)?;
    let row_metadata: Option<String> = row.try_get("metadata").map_err(Error::from)?;

    let metadata: HashMap<String, String> = row_metadata
        .and_then(|m| serde_json::from_str(&m).ok())
        .unwrap_or_default();

    let ys: Result<Vec<PublicKey>, nut01::Error> =
        ys.chunks(33).map(PublicKey::from_slice).collect();

    Ok(Transaction {
        mint_url: MintUrl::from_str(&mint_url)?,
        direction: TransactionDirection::from_str(&direction)?,
        unit: CurrencyUnit::from_str(&unit)?,
        amount: Amount::from(amount as u64),
        fee: Amount::from(fee as u64),
        ys: ys?,
        timestamp: timestamp as u64,
        memo,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use cdk_common::database::WalletDatabase;
    use cdk_common::nuts::{ProofDleq, State};
    use cdk_common::secret::Secret;

    use crate::WalletSqliteDatabase;

    #[tokio::test]
    #[cfg(feature = "sqlcipher")]
    async fn test_sqlcipher() {
        use cdk_common::mint_url::MintUrl;
        use cdk_common::MintInfo;

        use super::*;
        let path = std::env::temp_dir()
            .to_path_buf()
            .join(format!("cdk-test-{}.sqlite", uuid::Uuid::new_v4()));
        let db = WalletSqliteDatabase::new(path, "password".to_string())
            .await
            .unwrap();

        db.migrate().await;

        let mint_info = MintInfo::new().description("test");
        let mint_url = MintUrl::from_str("https://mint.xyz").unwrap();

        db.add_mint(mint_url.clone(), Some(mint_info.clone()))
            .await
            .unwrap();

        let res = db.get_mint(mint_url).await.unwrap();
        assert_eq!(mint_info, res.clone().unwrap());
        assert_eq!("test", &res.unwrap().description.unwrap());
    }

    #[tokio::test]
    async fn test_proof_with_dleq() {
        use std::str::FromStr;

        use cdk_common::common::ProofInfo;
        use cdk_common::mint_url::MintUrl;
        use cdk_common::nuts::{CurrencyUnit, Id, Proof, PublicKey, SecretKey};
        use cdk_common::Amount;

        // Create a temporary database
        let path = std::env::temp_dir()
            .to_path_buf()
            .join(format!("cdk-test-dleq-{}.sqlite", uuid::Uuid::new_v4()));

        #[cfg(feature = "sqlcipher")]
        let db = WalletSqliteDatabase::new(path, "password".to_string())
            .await
            .unwrap();

        #[cfg(not(feature = "sqlcipher"))]
        let db = WalletSqliteDatabase::new(path).await.unwrap();

        db.migrate().await;

        // Create a proof with DLEQ
        let keyset_id = Id::from_str("00deadbeef123456").unwrap();
        let mint_url = MintUrl::from_str("https://example.com").unwrap();
        let secret = Secret::new("test_secret_for_dleq");

        // Create DLEQ components
        let e = SecretKey::generate();
        let s = SecretKey::generate();
        let r = SecretKey::generate();

        let dleq = ProofDleq::new(e.clone(), s.clone(), r.clone());

        let mut proof = Proof::new(
            Amount::from(64),
            keyset_id,
            secret,
            PublicKey::from_hex(
                "02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        );

        // Add DLEQ to the proof
        proof.dleq = Some(dleq);

        // Create ProofInfo
        let proof_info =
            ProofInfo::new(proof, mint_url.clone(), State::Unspent, CurrencyUnit::Sat).unwrap();

        // Store the proof in the database
        db.update_proofs(vec![proof_info.clone()], vec![])
            .await
            .unwrap();

        // Retrieve the proof from the database
        let retrieved_proofs = db
            .get_proofs(
                Some(mint_url),
                Some(CurrencyUnit::Sat),
                Some(vec![State::Unspent]),
                None,
            )
            .await
            .unwrap();

        // Verify we got back exactly one proof
        assert_eq!(retrieved_proofs.len(), 1);

        // Verify the DLEQ data was preserved
        let retrieved_proof = &retrieved_proofs[0];
        assert!(retrieved_proof.proof.dleq.is_some());

        let retrieved_dleq = retrieved_proof.proof.dleq.as_ref().unwrap();

        // Verify DLEQ components match what we stored
        assert_eq!(retrieved_dleq.e.to_string(), e.to_string());
        assert_eq!(retrieved_dleq.s.to_string(), s.to_string());
        assert_eq!(retrieved_dleq.r.to_string(), r.to_string());
    }
}

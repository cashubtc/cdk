//! SQLite Wallet Database

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use cdk::amount::Amount;
use cdk::cdk_database::{self, WalletDatabase};
use cdk::mint_url::MintUrl;
use cdk::nuts::{
    CurrencyUnit, Id, KeySetInfo, Keys, MeltQuoteState, MintInfo, MintQuoteState, Proof, PublicKey,
    SpendingConditions, State,
};
use cdk::secret::Secret;
use cdk::types::ProofInfo;
use cdk::wallet;
use cdk::wallet::MintQuote;
use error::Error;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqliteRow};
use sqlx::{ConnectOptions, Row};
use tracing::instrument;

pub mod error;

/// Wallet SQLite Database
#[derive(Debug, Clone)]
pub struct WalletSqliteDatabase {
    pool: SqlitePool,
}

impl WalletSqliteDatabase {
    /// Create new [`WalletSqliteDatabase`]
    pub async fn new(path: &Path) -> Result<Self, Error> {
        let path = path.to_str().ok_or(Error::InvalidDbPath)?;
        let _conn = SqliteConnectOptions::from_str(path)?
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .read_only(false)
            .create_if_missing(true)
            .auto_vacuum(sqlx::sqlite::SqliteAutoVacuum::Full)
            .connect()
            .await?;

        let pool = SqlitePool::connect(path).await?;

        Ok(Self { pool })
    }

    /// Migrate [`WalletSqliteDatabase`]
    pub async fn migrate(&self) {
        sqlx::migrate!("./src/wallet/migrations")
            .run(&self.pool)
            .await
            .expect("Could not run migrations");
    }

    async fn set_proof_state(&self, y: PublicKey, state: State) -> Result<(), cdk_database::Error> {
        sqlx::query(
            r#"
    UPDATE proof
    SET state=?
    WHERE y IS ?;
            "#,
        )
        .bind(state.to_string())
        .bind(y.to_bytes().to_vec())
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }
}

#[async_trait]
impl WalletDatabase for WalletSqliteDatabase {
    type Err = cdk_database::Error;

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
                )
            }
            None => (
                None, None, None, None, None, None, None, None, None, None, None,
            ),
        };

        sqlx::query(
            r#"
INSERT OR REPLACE INTO mint
(mint_url, name, pubkey, version, description, description_long, contact, nuts, icon_url, urls, motd, mint_time)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
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
INSERT OR REPLACE INTO mint_quote
(id, mint_url, amount, unit, request, state, expiry)
VALUES (?, ?, ?, ?, ?, ?, ?);
        "#,
        )
        .bind(quote.id.to_string())
        .bind(quote.mint_url.to_string())
        .bind(u64::from(quote.amount) as i64)
        .bind(quote.unit.to_string())
        .bind(quote.request)
        .bind(quote.state.to_string())
        .bind(quote.expiry as i64)
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
INSERT OR REPLACE INTO melt_quote
(id, unit, amount, request, fee_reserve, state, expiry)
VALUES (?, ?, ?, ?, ?, ?, ?);
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
INSERT OR REPLACE INTO key
(id, keys)
VALUES (?, ?);
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
    INSERT OR REPLACE INTO proof
    (y, mint_url, state, spending_condition, unit, amount, keyset_id, secret, c, witness)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
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

    async fn set_pending_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Self::Err> {
        for y in ys {
            self.set_proof_state(y, State::Pending).await?;
        }

        Ok(())
    }

    async fn reserve_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Self::Err> {
        for y in ys {
            self.set_proof_state(y, State::Reserved).await?;
        }

        Ok(())
    }

    async fn set_unspent_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Self::Err> {
        for y in ys {
            self.set_proof_state(y, State::Unspent).await?;
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
        .execute(&mut transaction)
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

    #[instrument(skip_all)]
    async fn get_nostr_last_checked(
        &self,
        verifying_key: &PublicKey,
    ) -> Result<Option<u32>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT last_check
FROM nostr_last_checked
WHERE key=?;
        "#,
        )
        .bind(verifying_key.to_bytes().to_vec())
        .fetch_one(&self.pool)
        .await;

        let count = match rec {
            Ok(rec) => {
                let count: Option<u32> = rec.try_get("last_check").map_err(Error::from)?;
                count
            }
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(count)
    }

    #[instrument(skip_all)]
    async fn add_nostr_last_checked(
        &self,
        verifying_key: PublicKey,
        last_checked: u32,
    ) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
INSERT OR REPLACE INTO nostr_last_checked
(key, last_check)
VALUES (?, ?);
        "#,
        )
        .bind(verifying_key.to_bytes().to_vec())
        .bind(last_checked)
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

    let state = MintQuoteState::from_str(&row_state)?;

    Ok(MintQuote {
        id: row_id,
        mint_url: MintUrl::from_str(&row_mint_url)?,
        amount: Amount::from(row_amount as u64),
        unit: CurrencyUnit::from_str(&row_unit).map_err(Error::from)?,
        request: row_request,
        state,
        expiry: row_expiry as u64,
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

    let y: Vec<u8> = row.try_get("y").map_err(Error::from)?;
    let row_mint_url: String = row.try_get("mint_url").map_err(Error::from)?;
    let row_state: String = row.try_get("state").map_err(Error::from)?;
    let row_spending_condition: Option<String> =
        row.try_get("spending_condition").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;

    let proof = Proof {
        amount: Amount::from(row_amount as u64),
        keyset_id: Id::from_str(&keyset_id)?,
        secret: Secret::from_str(&row_secret)?,
        c: PublicKey::from_slice(&row_c)?,
        witness: row_witness.and_then(|w| serde_json::from_str(&w).ok()),
        dleq: None,
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

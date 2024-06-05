//! SQLite Wallet Database

use std::collections::HashMap;
use std::str::FromStr;

use async_trait::async_trait;
use cdk::cdk_database::{self, WalletDatabase};
use cdk::nuts::{
    CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, Proof, Proofs, PublicKey, SpendingConditions,
    State,
};
use cdk::secret::Secret;
use cdk::types::{MeltQuote, MintQuote, ProofInfo};
use cdk::url::UncheckedUrl;
use cdk::Amount;
use error::Error;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqliteRow};
use sqlx::{ConnectOptions, Row};

use self::migration::init_migration;

pub mod error;
mod migration;

#[derive(Debug, Clone)]
pub struct WalletSQLiteDatabase {
    pool: SqlitePool,
}

impl WalletSQLiteDatabase {
    pub async fn new(path: &str) -> Result<Self, Error> {
        let _conn = SqliteConnectOptions::from_str(path)?
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .read_only(false)
            .create_if_missing(true)
            .auto_vacuum(sqlx::sqlite::SqliteAutoVacuum::Full)
            .connect()
            .await?;

        let pool = SqlitePool::connect(path).await?;

        init_migration(&pool).await?;

        Ok(Self { pool })
    }
}

#[async_trait]
impl WalletDatabase for WalletSQLiteDatabase {
    type Err = cdk_database::Error;

    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err> {
        let (name, pubkey, version, description, description_long, contact, nuts, motd) =
            match mint_info {
                Some(mint_info) => {
                    let MintInfo {
                        name,
                        pubkey,
                        version,
                        description,
                        description_long,
                        contact,
                        nuts,
                        motd,
                    } = mint_info;

                    (
                        name,
                        pubkey.map(|p| p.to_bytes().to_vec()),
                        version.map(|v| serde_json::to_string(&v).ok()),
                        description,
                        description_long,
                        contact.map(|c| serde_json::to_string(&c).ok()),
                        serde_json::to_string(&nuts).ok(),
                        motd,
                    )
                }
                None => (None, None, None, None, None, None, None, None),
            };

        sqlx::query(
            r#"
INSERT OR REPLACE INTO mint
(mint_url, name, pubkey, version, description, description_long, contact, nuts, motd)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);
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
        .bind(motd)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }
    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Self::Err> {
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
    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Self::Err> {
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
            .map(|row| {
                let mint_url: String = row.get("mint_url");

                let mint_info = sqlite_row_to_mint_info(&row).ok();

                (mint_url.into(), mint_info)
            })
            .collect();

        Ok(mints)
    }

    async fn add_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Self::Err> {
        for keyset in keysets {
            sqlx::query(
                r#"
INSERT OR REPLACE INTO keyset
(mint_url, id, unit, active)
VALUES (?, ?, ?, ?);
        "#,
            )
            .bind(mint_url.to_string())
            .bind(keyset.id.to_string())
            .bind(keyset.unit.to_string())
            .bind(keyset.active)
            .execute(&self.pool)
            .await
            .map_err(Error::from)?;
        }

        Ok(())
    }
    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
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

        let keysets: Vec<KeySetInfo> = recs.iter().flat_map(sqlite_row_to_keyset).collect();

        match keysets.is_empty() {
            false => Ok(Some(keysets)),
            true => Ok(None),
        }
    }
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

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
INSERT OR REPLACE INTO mint_quote
(id, mint_url, amount, unit, request, paid, expiry)
VALUES (?, ?, ?, ?, ?, ?, ?);
        "#,
        )
        .bind(quote.id.to_string())
        .bind(quote.mint_url.to_string())
        .bind(u64::from(quote.amount) as i64)
        .bind(quote.unit.to_string())
        .bind(quote.request)
        .bind(quote.paid)
        .bind(quote.expiry as i64)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }
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

        let mint_quotes = rec.iter().flat_map(sqlite_row_to_mint_quote).collect();

        Ok(mint_quotes)
    }
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

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
INSERT OR REPLACE INTO melt_quote
(id, unit, amount, request, fee_reserve, paid, expiry)
VALUES (?, ?, ?, ?, ?, ?, ?);
        "#,
        )
        .bind(quote.id.to_string())
        .bind(quote.unit.to_string())
        .bind(u64::from(quote.amount) as i64)
        .bind(quote.request)
        .bind(u64::from(quote.fee_reserve) as i64)
        .bind(quote.paid)
        .bind(quote.expiry as i64)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err> {
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
    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM key
WHERE id=?;
        "#,
        )
        .bind(id.to_string())
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

    async fn add_proofs(&self, proof_info: Vec<ProofInfo>) -> Result<(), Self::Err> {
        for proof in proof_info {
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

        Ok(())
    }
    async fn get_proofs(
        &self,
        mint_url: Option<UncheckedUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Option<Vec<ProofInfo>>, Self::Err> {
        tracing::debug!("{:?}", mint_url);
        tracing::debug!("{:?}", unit);
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
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        tracing::debug!("{}", recs.len());

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
        tracing::debug!("{}", proofs.len());

        match proofs.is_empty() {
            false => Ok(Some(proofs)),
            true => return Ok(None),
        }
    }
    async fn remove_proofs(&self, proofs: &Proofs) -> Result<(), Self::Err> {
        // TODO: Generate a IN clause
        for proof in proofs {
            sqlx::query(
                r#"
DELETE FROM proof
WHERE y = ?
        "#,
            )
            .bind(proof.y()?.to_bytes().to_vec())
            .execute(&self.pool)
            .await
            .map_err(Error::from)?;
        }

        Ok(())
    }

    async fn set_proof_state(&self, y: PublicKey, state: State) -> Result<(), Self::Err> {
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

    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
UPDATE keyset
SET counter = counter + ?
WHERE id IS ?;
        "#,
        )
        .bind(count)
        .bind(keyset_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }
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

    #[cfg(feature = "nostr")]
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
    #[cfg(feature = "nostr")]
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
    let motd: Option<String> = row.try_get("motd").map_err(Error::from)?;

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
        motd,
    })
}

fn sqlite_row_to_keyset(row: &SqliteRow) -> Result<KeySetInfo, Error> {
    let row_id: String = row.try_get("id").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let active: bool = row.try_get("active").map_err(Error::from)?;

    Ok(KeySetInfo {
        id: Id::from_str(&row_id)?,
        unit: CurrencyUnit::from(row_unit),
        active,
    })
}

fn sqlite_row_to_mint_quote(row: &SqliteRow) -> Result<MintQuote, Error> {
    let row_id: String = row.try_get("id").map_err(Error::from)?;
    let row_mint_url: String = row.try_get("mint_url").map_err(Error::from)?;
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let row_request: String = row.try_get("request").map_err(Error::from)?;
    let row_paid: bool = row.try_get("paid").map_err(Error::from)?;
    let row_expiry: i64 = row.try_get("expiry").map_err(Error::from)?;

    Ok(MintQuote {
        id: row_id,
        mint_url: row_mint_url.into(),
        amount: Amount::from(row_amount as u64),
        unit: CurrencyUnit::from(row_unit),
        request: row_request,
        paid: row_paid,
        expiry: row_expiry as u64,
    })
}

fn sqlite_row_to_melt_quote(row: &SqliteRow) -> Result<MeltQuote, Error> {
    let row_id: String = row.try_get("id").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let row_request: String = row.try_get("request").map_err(Error::from)?;
    let row_fee_reserve: i64 = row.try_get("fee_reserve").map_err(Error::from)?;
    let row_paid: bool = row.try_get("paid").map_err(Error::from)?;
    let row_expiry: i64 = row.try_get("expiry").map_err(Error::from)?;

    Ok(MeltQuote {
        id: row_id,
        amount: Amount::from(row_amount as u64),
        unit: CurrencyUnit::from(row_unit),
        request: row_request,
        fee_reserve: Amount::from(row_fee_reserve as u64),
        paid: row_paid,
        expiry: row_expiry as u64,
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
        mint_url: row_mint_url.into(),
        state: State::from_str(&row_state)?,
        spending_condition: row_spending_condition.and_then(|r| serde_json::from_str(&r).ok()),
        unit: CurrencyUnit::from(row_unit),
    })
}

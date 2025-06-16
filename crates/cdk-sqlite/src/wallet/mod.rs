//! SQLite Wallet Database

use std::collections::HashMap;
use std::ops::DerefMut;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::common::ProofInfo;
use cdk_common::database::WalletDatabase;
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{MeltQuoteState, MintQuoteState};
use cdk_common::secret::Secret;
use cdk_common::wallet::{self, MintQuote, Transaction, TransactionDirection, TransactionId};
use cdk_common::{
    database, Amount, CurrencyUnit, Id, KeySet, KeySetInfo, Keys, MintInfo, Proof, ProofDleq,
    PublicKey, SecretKey, SpendingConditions, State,
};
use error::Error;
use tracing::instrument;

use crate::common::{create_sqlite_pool, migrate, SqliteConnectionManager};
use crate::pool::Pool;
use crate::stmt::{Column, Statement};
use crate::{
    column_as_binary, column_as_nullable_binary, column_as_nullable_number,
    column_as_nullable_string, column_as_number, column_as_string, unpack_into,
};

pub mod error;
pub mod memory;

#[rustfmt::skip]
mod migrations;

/// Wallet SQLite Database
#[derive(Debug, Clone)]
pub struct WalletSqliteDatabase {
    pool: Arc<Pool<SqliteConnectionManager>>,
}

impl WalletSqliteDatabase {
    /// Create new [`WalletSqliteDatabase`]
    #[cfg(not(feature = "sqlcipher"))]
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let db = Self {
            pool: create_sqlite_pool(path.as_ref().to_str().ok_or(Error::InvalidDbPath)?),
        };
        db.migrate()?;
        Ok(db)
    }

    /// Create new [`WalletSqliteDatabase`]
    #[cfg(feature = "sqlcipher")]
    pub async fn new<P: AsRef<Path>>(path: P, password: String) -> Result<Self, Error> {
        let db = Self {
            pool: create_sqlite_pool(
                path.as_ref().to_str().ok_or(Error::InvalidDbPath)?,
                password,
            ),
        };
        db.migrate()?;
        Ok(db)
    }

    /// Migrate [`WalletSqliteDatabase`]
    fn migrate(&self) -> Result<(), Error> {
        migrate(self.pool.get()?.deref_mut(), migrations::MIGRATIONS)?;
        Ok(())
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

        Statement::new(
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
        )
        .bind(":mint_url", mint_url.to_string())
        .bind(":name", name)
        .bind(":pubkey", pubkey)
        .bind(":version", version)
        .bind(":description", description)
        .bind(":description_long", description_long)
        .bind(":contact", contact)
        .bind(":nuts", nuts)
        .bind(":icon_url", icon_url)
        .bind(":urls", urls)
        .bind(":motd", motd)
        .bind(":mint_time", time.map(|v| v as i64))
        .bind(":tos_url", tos_url)
        .execute(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), Self::Err> {
        let conn = self.pool.get().map_err(Error::Pool)?;

        Statement::new(r#"DELETE FROM mint WHERE mint_url=:mint_url"#)
            .bind(":mint_url", mint_url.to_string())
            .execute(&conn)
            .map_err(Error::Sqlite)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, Self::Err> {
        Ok(Statement::new(
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
        )
        .bind(":mint_url", mint_url.to_string())
        .fetch_one(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?
        .map(sqlite_row_to_mint_info)
        .transpose()?)
    }

    #[instrument(skip(self))]
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, Self::Err> {
        Ok(Statement::new(
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
        )
        .fetch_all(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?
        .into_iter()
        .map(|mut row| {
            let url = column_as_string!(
                row.pop().ok_or(Error::MissingColumn(0, 1))?,
                MintUrl::from_str
            );

            Ok((url, sqlite_row_to_mint_info(row).ok()))
        })
        .collect::<Result<HashMap<_, _>, Error>>()?)
    }

    #[instrument(skip(self))]
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Self::Err> {
        let tables = ["mint_quote", "proof"];
        let conn = self.pool.get().map_err(Error::Pool)?;

        for table in &tables {
            let query = format!(
                r#"
                UPDATE {table}
                SET mint_url = :new_mint_url
                WHERE mint_url = :old_mint_url
            "#
            );

            Statement::new(query)
                .bind(":new_mint_url", new_mint_url.to_string())
                .bind(":old_mint_url", old_mint_url.to_string())
                .execute(&conn)
                .map_err(Error::Sqlite)?;
        }

        Ok(())
    }

    #[instrument(skip(self, keysets))]
    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Self::Err> {
        let conn = self.pool.get().map_err(Error::Pool)?;
        for keyset in keysets {
            Statement::new(
                r#"
    INSERT INTO keyset
    (mint_url, id, unit, active, input_fee_ppk, final_expiry)
    VALUES
    (:mint_url, :id, :unit, :active, :input_fee_ppk, :final_expiry)
    ON CONFLICT(id) DO UPDATE SET
        mint_url = excluded.mint_url,
        unit = excluded.unit,
        active = excluded.active,
        input_fee_ppk = excluded.input_fee_ppk,
        final_expiry = excluded.final_expiry;
    "#,
            )
            .bind(":mint_url", mint_url.to_string())
            .bind(":id", keyset.id.to_string())
            .bind(":unit", keyset.unit.to_string())
            .bind(":active", keyset.active)
            .bind(":input_fee_ppk", keyset.input_fee_ppk as i64)
            .bind(":final_expiry", keyset.final_expiry.map(|v| v as i64))
            .execute(&conn)
            .map_err(Error::Sqlite)?;
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err> {
        let keysets = Statement::new(
            r#"
            SELECT
                id,
                unit,
                active,
                input_fee_ppk
            FROM
                keyset
            WHERE mint_url = :mint_url
            "#,
        )
        .bind(":mint_url", mint_url.to_string())
        .fetch_all(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?
        .into_iter()
        .map(sqlite_row_to_keyset)
        .collect::<Result<Vec<_>, Error>>()?;

        match keysets.is_empty() {
            false => Ok(Some(keysets)),
            true => Ok(None),
        }
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Self::Err> {
        Ok(Statement::new(
            r#"
            SELECT
                id,
                unit,
                active,
                input_fee_ppk
            FROM
                keyset
            WHERE id = :id
            "#,
        )
        .bind(":id", keyset_id.to_string())
        .fetch_one(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?
        .map(sqlite_row_to_keyset)
        .transpose()?)
    }

    #[instrument(skip_all)]
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        Statement::new(
            r#"
INSERT INTO mint_quote
(id, mint_url, amount, unit, request, state, expiry, secret_key)
VALUES
(:id, :mint_url, :amount, :unit, :request, :state, :expiry, :secret_key)
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
        .bind(":id", quote.id.to_string())
        .bind(":mint_url", quote.mint_url.to_string())
        .bind(":amount", u64::from(quote.amount) as i64)
        .bind(":unit", quote.unit.to_string())
        .bind(":request", quote.request)
        .bind(":state", quote.state.to_string())
        .bind(":expiry", quote.expiry as i64)
        .bind(":secret_key", quote.secret_key.map(|p| p.to_string()))
        .execute(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        Ok(Statement::new(
            r#"
            SELECT
                id,
                mint_url,
                amount,
                unit,
                request,
                state,
                expiry,
                secret_key
            FROM
                mint_quote
            WHERE
                id = :id
            "#,
        )
        .bind(":id", quote_id.to_string())
        .fetch_one(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?
        .map(sqlite_row_to_mint_quote)
        .transpose()?)
    }

    #[instrument(skip(self))]
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        Ok(Statement::new(
            r#"
            SELECT
                id,
                mint_url,
                amount,
                unit,
                request,
                state,
                expiry,
                secret_key
            FROM
                mint_quote
            "#,
        )
        .fetch_all(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?
        .into_iter()
        .map(sqlite_row_to_mint_quote)
        .collect::<Result<_, _>>()?)
    }

    #[instrument(skip(self))]
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        Statement::new(r#"DELETE FROM mint_quote WHERE id=:id"#)
            .bind(":id", quote_id.to_string())
            .execute(&self.pool.get().map_err(Error::Pool)?)
            .map_err(Error::Sqlite)?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_melt_quote(&self, quote: wallet::MeltQuote) -> Result<(), Self::Err> {
        Statement::new(
            r#"
INSERT INTO melt_quote
(id, unit, amount, request, fee_reserve, state, expiry)
VALUES
(:id, :unit, :amount, :request, :fee_reserve, :state, :expiry)
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
        .bind(":id", quote.id.to_string())
        .bind(":unit", quote.unit.to_string())
        .bind(":amount", u64::from(quote.amount) as i64)
        .bind(":request", quote.request)
        .bind(":fee_reserve", u64::from(quote.fee_reserve) as i64)
        .bind(":state", quote.state.to_string())
        .bind(":expiry", quote.expiry as i64)
        .execute(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<wallet::MeltQuote>, Self::Err> {
        Ok(Statement::new(
            r#"
            SELECT
                id,
                unit,
                amount,
                request,
                fee_reserve,
                state,
                expiry,
                payment_preimage
            FROM
                melt_quote
            WHERE
                id=:id
            "#,
        )
        .bind(":id", quote_id.to_owned())
        .fetch_one(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?
        .map(sqlite_row_to_melt_quote)
        .transpose()?)
    }

    #[instrument(skip(self))]
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        Statement::new(r#"DELETE FROM melt_quote WHERE id=:id"#)
            .bind(":id", quote_id.to_owned())
            .execute(&self.pool.get().map_err(Error::Pool)?)
            .map_err(Error::Sqlite)?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_keys(&self, keyset: KeySet) -> Result<(), Self::Err> {
        // Recompute ID for verification
        keyset.verify_id()?;

        Statement::new(
            r#"
            INSERT INTO key
            (id, keys)
            VALUES
            (:id, :keys)
            ON CONFLICT(id) DO UPDATE SET
                keys = excluded.keys
        "#,
        )
        .bind(":id", keyset.id.to_string())
        .bind(
            ":keys",
            serde_json::to_string(&keyset.keys).map_err(Error::from)?,
        )
        .execute(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?;

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keys(&self, keyset_id: &Id) -> Result<Option<Keys>, Self::Err> {
        Ok(Statement::new(
            r#"
            SELECT
                keys
            FROM key
            WHERE id = :id
            "#,
        )
        .bind(":id", keyset_id.to_string())
        .plunk(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?
        .map(|keys| {
            let keys = column_as_string!(keys);
            serde_json::from_str(&keys).map_err(Error::from)
        })
        .transpose()?)
    }

    #[instrument(skip(self))]
    async fn remove_keys(&self, id: &Id) -> Result<(), Self::Err> {
        Statement::new(r#"DELETE FROM key WHERE id = :id"#)
            .bind(":id", id.to_string())
            .plunk(&self.pool.get().map_err(Error::Pool)?)
            .map_err(Error::Sqlite)?;

        Ok(())
    }

    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Self::Err> {
        // TODO: Use a transaction for all these operations
        for proof in added {
            Statement::new(
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
            )
            .bind(":y", proof.y.to_bytes().to_vec())
            .bind(":mint_url", proof.mint_url.to_string())
            .bind(":state",proof.state.to_string())
            .bind(
                ":spending_condition",
                proof
                    .spending_condition
                    .map(|s| serde_json::to_string(&s).ok()),
            )
            .bind(":unit", proof.unit.to_string())
            .bind(":amount", u64::from(proof.proof.amount) as i64)
            .bind(":keyset_id", proof.proof.keyset_id.to_string())
            .bind(":secret", proof.proof.secret.to_string())
            .bind(":c", proof.proof.c.to_bytes().to_vec())
            .bind(
                ":witness",
                proof
                    .proof
                    .witness
                    .map(|w| serde_json::to_string(&w).unwrap()),
            )
            .bind(
                ":dleq_e",
                proof.proof.dleq.as_ref().map(|dleq| dleq.e.to_secret_bytes().to_vec()),
            )
            .bind(
                ":dleq_s",
                proof.proof.dleq.as_ref().map(|dleq| dleq.s.to_secret_bytes().to_vec()),
            )
            .bind(
                ":dleq_r",
                proof.proof.dleq.as_ref().map(|dleq| dleq.r.to_secret_bytes().to_vec()),
            )
            .execute(&self.pool.get().map_err(Error::Pool)?)
            .map_err(Error::Sqlite)?;
        }

        Statement::new(r#"DELETE FROM proof WHERE y IN (:ys)"#)
            .bind_vec(
                ":ys",
                removed_ys.iter().map(|y| y.to_bytes().to_vec()).collect(),
            )
            .execute(&self.pool.get().map_err(Error::Pool)?)
            .map_err(Error::Sqlite)?;

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
        Ok(Statement::new(
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
        )
        .fetch_all(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?
        .into_iter()
        .filter_map(|row| {
            let row = sqlite_row_to_proof_info(row).ok()?;

            if row.matches_conditions(&mint_url, &unit, &state, &spending_conditions) {
                Some(row)
            } else {
                None
            }
        })
        .collect::<Vec<_>>())
    }

    async fn update_proofs_state(&self, ys: Vec<PublicKey>, state: State) -> Result<(), Self::Err> {
        Statement::new("UPDATE proof SET state = :state WHERE y IN (:ys)")
            .bind_vec(":ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
            .bind(":state", state.to_string())
            .execute(&self.pool.get().map_err(Error::Pool)?)
            .map_err(Error::Sqlite)?;

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Self::Err> {
        Statement::new(
            r#"
            UPDATE keyset
            SET counter=counter+:count
            WHERE id=:id
            "#,
        )
        .bind(":count", count)
        .bind(":id", keyset_id.to_string())
        .execute(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?;

        Ok(())
    }

    #[instrument(skip(self), fields(keyset_id = %keyset_id))]
    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u32>, Self::Err> {
        Ok(Statement::new(
            r#"
            SELECT
                counter
            FROM
                keyset
            WHERE
                id=:id
            "#,
        )
        .bind(":id", keyset_id.to_string())
        .plunk(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?
        .map(|n| Ok::<_, Error>(column_as_number!(n)))
        .transpose()?)
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

        Statement::new(
            r#"
INSERT INTO transactions
(id, mint_url, direction, unit, amount, fee, ys, timestamp, memo, metadata)
VALUES
(:id, :mint_url, :direction, :unit, :amount, :fee, :ys, :timestamp, :memo, :metadata)
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
        .bind(":id", transaction.id().as_slice().to_vec())
        .bind(":mint_url", mint_url)
        .bind(":direction", direction)
        .bind(":unit", unit)
        .bind(":amount", amount)
        .bind(":fee", fee)
        .bind(":ys", ys)
        .bind(":timestamp", transaction.timestamp as i64)
        .bind(":memo", transaction.memo)
        .bind(
            ":metadata",
            serde_json::to_string(&transaction.metadata).map_err(Error::from)?,
        )
        .execute(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, Self::Err> {
        Ok(Statement::new(
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
                metadata
            FROM
                transactions
            WHERE
                id = :id
            "#,
        )
        .bind(":id", transaction_id.as_slice().to_vec())
        .fetch_one(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?
        .map(sqlite_row_to_transaction)
        .transpose()?)
    }

    #[instrument(skip(self))]
    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, Self::Err> {
        Ok(Statement::new(
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
                metadata
            FROM
                transactions
            "#,
        )
        .fetch_all(&self.pool.get().map_err(Error::Pool)?)
        .map_err(Error::Sqlite)?
        .into_iter()
        .filter_map(|row| {
            // TODO: Avoid a table scan by passing the heavy lifting of checking to the DB engine
            let transaction = sqlite_row_to_transaction(row).ok()?;
            if transaction.matches_conditions(&mint_url, &direction, &unit) {
                Some(transaction)
            } else {
                None
            }
        })
        .collect::<Vec<_>>())
    }

    #[instrument(skip(self))]
    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), Self::Err> {
        Statement::new(r#"DELETE FROM transactions WHERE id=:id"#)
            .bind(":id", transaction_id.as_slice().to_vec())
            .execute(&self.pool.get().map_err(Error::Pool)?)
            .map_err(Error::Sqlite)?;

        Ok(())
    }
}

fn sqlite_row_to_mint_info(row: Vec<Column>) -> Result<MintInfo, Error> {
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

fn sqlite_row_to_keyset(row: Vec<Column>) -> Result<KeySetInfo, Error> {
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

fn sqlite_row_to_mint_quote(row: Vec<Column>) -> Result<MintQuote, Error> {
    unpack_into!(
        let (
            id,
            mint_url,
            amount,
            unit,
            request,
            state,
            expiry,
            secret_key
        ) = row
    );

    let amount: u64 = column_as_number!(amount);

    Ok(MintQuote {
        id: column_as_string!(id),
        mint_url: column_as_string!(mint_url, MintUrl::from_str),
        amount: Amount::from(amount),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        request: column_as_string!(request),
        state: column_as_string!(state, MintQuoteState::from_str),
        expiry: column_as_number!(expiry),
        secret_key: column_as_nullable_string!(secret_key)
            .map(|v| SecretKey::from_str(&v))
            .transpose()?,
    })
}

fn sqlite_row_to_melt_quote(row: Vec<Column>) -> Result<wallet::MeltQuote, Error> {
    unpack_into!(
        let (
            id,
            unit,
            amount,
            request,
            fee_reserve,
            state,
            expiry,
            payment_preimage
        ) = row
    );

    let amount: u64 = column_as_number!(amount);
    let fee_reserve: u64 = column_as_number!(fee_reserve);

    Ok(wallet::MeltQuote {
        id: column_as_string!(id),
        amount: Amount::from(amount),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        request: column_as_string!(request),
        fee_reserve: Amount::from(fee_reserve),
        state: column_as_string!(state, MeltQuoteState::from_str),
        expiry: column_as_number!(expiry),
        payment_preimage: column_as_nullable_string!(payment_preimage),
    })
}

fn sqlite_row_to_proof_info(row: Vec<Column>) -> Result<ProofInfo, Error> {
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

fn sqlite_row_to_transaction(row: Vec<Column>) -> Result<Transaction, Error> {
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
            metadata
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

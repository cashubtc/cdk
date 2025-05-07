//! SQLite Mint Auth

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use cdk_common::database::{self, MintAuthDatabase};
use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::{AuthProof, BlindSignature, Id, PublicKey, State};
use cdk_common::{AuthRequired, ProtectedEndpoint};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use tracing::instrument;

use super::{sqlite_row_to_blind_signature, sqlite_row_to_keyset_info};
use crate::mint::Error;

/// Mint SQLite Database
#[derive(Debug, Clone)]
pub struct MintSqliteAuthDatabase {
    pool: SqlitePool,
}

impl MintSqliteAuthDatabase {
    /// Create new [`MintSqliteAuthDatabase`]
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

    /// Migrate [`MintSqliteAuthDatabase`]
    pub async fn migrate(&self) {
        sqlx::migrate!("./src/mint/auth/migrations")
            .run(&self.pool)
            .await
            .expect("Could not run migrations");
    }
}

#[async_trait]
impl MintAuthDatabase for MintSqliteAuthDatabase {
    type Err = database::Error;

    #[instrument(skip(self))]
    async fn set_active_keyset(&self, id: Id) -> Result<(), Self::Err> {
        tracing::info!("Setting auth keyset {id} active");
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let update_res = sqlx::query(
            r#"
    UPDATE keyset 
    SET active = CASE 
        WHEN id = ? THEN TRUE
        ELSE FALSE
    END;
    "#,
        )
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await;

        match update_res {
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

    async fn get_active_keyset_id(&self) -> Result<Option<Id>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT id
FROM keyset
WHERE active = 1;
        "#,
        )
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

    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        let res = sqlx::query(
            r#"
INSERT OR REPLACE INTO keyset
(id, unit, active, valid_from, valid_to, derivation_path, max_order, derivation_path_index)
VALUES (?, ?, ?, ?, ?, ?, ?, ?);
        "#,
        )
        .bind(keyset.id.to_string())
        .bind(keyset.unit.to_string())
        .bind(keyset.active)
        .bind(keyset.valid_from as i64)
        .bind(keyset.final_expiry.map(|v| v as i64))
        .bind(keyset.derivation_path.to_string())
        .bind(keyset.max_order)
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

    async fn add_proof(&self, proof: AuthProof) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        if let Err(err) = sqlx::query(
            r#"
INSERT INTO proof
(y, keyset_id, secret, c, state)
VALUES (?, ?, ?, ?, ?);
        "#,
        )
        .bind(proof.y()?.to_bytes().to_vec())
        .bind(proof.keyset_id.to_string())
        .bind(proof.secret.to_string())
        .bind(proof.c.to_bytes().to_vec())
        .bind("UNSPENT")
        .execute(&mut *transaction)
        .await
        .map_err(Error::from)
        {
            tracing::debug!("Attempting to add known proof. Skipping.... {:?}", err);
        }
        transaction.commit().await.map_err(Error::from)?;

        Ok(())
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

    async fn update_proof_state(
        &self,
        y: &PublicKey,
        proofs_state: State,
    ) -> Result<Option<State>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        // Get current state for single y
        let current_state = sqlx::query("SELECT state FROM proof WHERE y = ?")
            .bind(y.to_bytes().to_vec())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|err| {
                tracing::error!("SQLite could not get state of proof: {err:?}");
                Error::SQLX(err)
            })?
            .map(|row| {
                let state: String = row.get("state");
                State::from_str(&state).map_err(Error::from)
            })
            .transpose()?;

        // Update state for single y
        sqlx::query("UPDATE proof SET state = ? WHERE state != ? AND y = ?")
            .bind(proofs_state.to_string())
            .bind(State::Spent.to_string())
            .bind(y.to_bytes().to_vec())
            .execute(&mut *transaction)
            .await
            .map_err(|err| {
                tracing::error!("SQLite could not update proof state: {err:?}");
                Error::SQLX(err)
            })?;

        transaction.commit().await.map_err(Error::from)?;
        Ok(current_state)
    }

    async fn add_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
        blind_signatures: &[BlindSignature],
    ) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        for (message, signature) in blinded_messages.iter().zip(blind_signatures) {
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

    async fn add_protected_endpoints(
        &self,
        protected_endpoints: HashMap<ProtectedEndpoint, AuthRequired>,
    ) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        for (endpoint, auth) in protected_endpoints.iter() {
            if let Err(err) = sqlx::query(
                r#"
INSERT OR REPLACE INTO protected_endpoints
(endpoint, auth)
VALUES (?, ?);
        "#,
            )
            .bind(serde_json::to_string(endpoint)?)
            .bind(serde_json::to_string(auth)?)
            .execute(&mut *transaction)
            .await
            .map_err(Error::from)
            {
                tracing::debug!(
                    "Attempting to add protected endpoint. Skipping.... {:?}",
                    err
                );
            }
        }

        transaction.commit().await.map_err(Error::from)?;

        Ok(())
    }
    async fn remove_protected_endpoints(
        &self,
        protected_endpoints: Vec<ProtectedEndpoint>,
    ) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let sql = format!(
            "DELETE FROM protected_endpoints WHERE endpoint IN ({})",
            std::iter::repeat("?")
                .take(protected_endpoints.len())
                .collect::<Vec<_>>()
                .join(",")
        );

        let endpoints = protected_endpoints
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?;

        endpoints
            .iter()
            .fold(sqlx::query(&sql), |query, endpoint| query.bind(endpoint))
            .execute(&mut *transaction)
            .await
            .map_err(Error::from)?;

        transaction.commit().await.map_err(Error::from)?;
        Ok(())
    }
    async fn get_auth_for_endpoint(
        &self,
        protected_endpoint: ProtectedEndpoint,
    ) -> Result<Option<AuthRequired>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT *
FROM protected_endpoints
WHERE endpoint=?;
        "#,
        )
        .bind(serde_json::to_string(&protected_endpoint)?)
        .fetch_one(&mut *transaction)
        .await;

        match rec {
            Ok(rec) => {
                transaction.commit().await.map_err(Error::from)?;

                let auth: String = rec.try_get("auth").map_err(Error::from)?;

                Ok(Some(serde_json::from_str(&auth)?))
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
    async fn get_auth_for_endpoints(
        &self,
    ) -> Result<HashMap<ProtectedEndpoint, Option<AuthRequired>>, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let recs = sqlx::query(
            r#"
SELECT *
FROM protected_endpoints
        "#,
        )
        .fetch_all(&mut *transaction)
        .await;

        match recs {
            Ok(recs) => {
                transaction.commit().await.map_err(Error::from)?;

                let mut endpoints = HashMap::new();

                for rec in recs {
                    let auth: String = rec.try_get("auth").map_err(Error::from)?;
                    let endpoint: String = rec.try_get("endpoint").map_err(Error::from)?;

                    let endpoint: ProtectedEndpoint = serde_json::from_str(&endpoint)?;
                    let auth: AuthRequired = serde_json::from_str(&auth)?;

                    endpoints.insert(endpoint, Some(auth));
                }

                Ok(endpoints)
            }
            Err(err) => {
                tracing::error!("SQLite could not get protected endpoints");
                if let Err(err) = transaction.rollback().await {
                    tracing::error!("Could not rollback sql transaction: {}", err);
                }
                Err(Error::from(err).into())
            }
        }
    }
}

//! SQLite Mint Auth

use std::collections::HashMap;
use std::ops::DerefMut;
use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::database::{self, MintAuthDatabase, MintAuthTransaction};
use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::{AuthProof, BlindSignature, Id, PublicKey, State};
use cdk_common::{AuthRequired, ProtectedEndpoint};
use tracing::instrument;

use super::async_rusqlite::AsyncRusqlite;
use super::{sqlite_row_to_blind_signature, sqlite_row_to_keyset_info, SqliteTransaction};
use crate::column_as_string;
use crate::common::{create_sqlite_pool, migrate};
use crate::mint::async_rusqlite::query;
use crate::mint::Error;

/// Mint SQLite Database
#[derive(Debug, Clone)]
pub struct MintSqliteAuthDatabase {
    pool: AsyncRusqlite,
}

#[rustfmt::skip]
mod migrations;

impl MintSqliteAuthDatabase {
    /// Create new [`MintSqliteAuthDatabase`]
    #[cfg(not(feature = "sqlcipher"))]
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let pool = create_sqlite_pool(path.as_ref().to_str().ok_or(Error::InvalidDbPath)?);
        migrate(pool.get()?.deref_mut(), migrations::MIGRATIONS)?;

        Ok(Self {
            pool: AsyncRusqlite::new(pool),
        })
    }

    /// Create new [`MintSqliteAuthDatabase`]
    #[cfg(feature = "sqlcipher")]
    pub async fn new<P: AsRef<Path>>(path: P, password: String) -> Result<Self, Error> {
        let pool = create_sqlite_pool(
            path.as_ref().to_str().ok_or(Error::InvalidDbPath)?,
            password,
        );
        migrate(pool.get()?.deref_mut(), migrations::MIGRATIONS)?;

        Ok(Self {
            pool: AsyncRusqlite::new(pool),
        })
    }
}

#[async_trait]
impl MintAuthTransaction<database::Error> for SqliteTransaction<'_> {
    #[instrument(skip(self))]
    async fn set_active_keyset(&mut self, id: Id) -> Result<(), database::Error> {
        tracing::info!("Setting auth keyset {id} active");
        query(
            r#"
            UPDATE keyset
            SET active = CASE
                WHEN id = :id THEN TRUE
                ELSE FALSE
            END;
            "#,
        )
        .bind(":id", id.to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_keyset_info(&mut self, keyset: MintKeySetInfo) -> Result<(), database::Error> {
        query(
            r#"
        INSERT INTO
            keyset (
                id, unit, active, valid_from, valid_to, derivation_path,
                max_order, derivation_path_index
            )
        VALUES (
            :id, :unit, :active, :valid_from, :valid_to, :derivation_path,
            :max_order, :derivation_path_index
        )
        ON CONFLICT(id) DO UPDATE SET
            unit = excluded.unit,
            active = excluded.active,
            valid_from = excluded.valid_from,
            valid_to = excluded.valid_to,
            derivation_path = excluded.derivation_path,
            max_order = excluded.max_order,
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
        .bind(":derivation_path_index", keyset.derivation_path_index)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_proof(&mut self, proof: AuthProof) -> Result<(), database::Error> {
        if let Err(err) = query(
            r#"
                INSERT INTO proof
                (y, keyset_id, secret, c, state)
                VALUES
                (:y, :keyset_id, :secret, :c, :state)
                "#,
        )
        .bind(":y", proof.y()?.to_bytes().to_vec())
        .bind(":keyset_id", proof.keyset_id.to_string())
        .bind(":secret", proof.secret.to_string())
        .bind(":c", proof.c.to_bytes().to_vec())
        .bind(":state", "UNSPENT".to_string())
        .execute(&self.inner)
        .await
        {
            tracing::debug!("Attempting to add known proof. Skipping.... {:?}", err);
        }
        Ok(())
    }

    async fn update_proof_state(
        &mut self,
        y: &PublicKey,
        proofs_state: State,
    ) -> Result<Option<State>, Self::Err> {
        let current_state = query(r#"SELECT state FROM proof WHERE y = :y"#)
            .bind(":y", y.to_bytes().to_vec())
            .pluck(&self.inner)
            .await?
            .map(|state| Ok::<_, Error>(column_as_string!(state, State::from_str)))
            .transpose()?;

        query(r#"UPDATE proof SET state = :new_state WHERE state = :state AND y = :y"#)
            .bind(":y", y.to_bytes().to_vec())
            .bind(
                ":state",
                current_state.as_ref().map(|state| state.to_string()),
            )
            .bind(":new_state", proofs_state.to_string())
            .execute(&self.inner)
            .await?;

        Ok(current_state)
    }

    async fn add_blind_signatures(
        &mut self,
        blinded_messages: &[PublicKey],
        blind_signatures: &[BlindSignature],
    ) -> Result<(), database::Error> {
        for (message, signature) in blinded_messages.iter().zip(blind_signatures) {
            query(
                r#"
                       INSERT
                       INTO blind_signature
                       (y, amount, keyset_id, c)
                       VALUES
                       (:y, :amount, :keyset_id, :c)
                   "#,
            )
            .bind(":y", message.to_bytes().to_vec())
            .bind(":amount", u64::from(signature.amount) as i64)
            .bind(":keyset_id", signature.keyset_id.to_string())
            .bind(":c", signature.c.to_bytes().to_vec())
            .execute(&self.inner)
            .await?;
        }

        Ok(())
    }

    async fn add_protected_endpoints(
        &mut self,
        protected_endpoints: HashMap<ProtectedEndpoint, AuthRequired>,
    ) -> Result<(), database::Error> {
        for (endpoint, auth) in protected_endpoints.iter() {
            if let Err(err) = query(
                r#"
                 INSERT OR REPLACE INTO protected_endpoints
                 (endpoint, auth)
                 VALUES (:endpoint, :auth);
                 "#,
            )
            .bind(":endpoint", serde_json::to_string(endpoint)?)
            .bind(":auth", serde_json::to_string(auth)?)
            .execute(&self.inner)
            .await
            {
                tracing::debug!(
                    "Attempting to add protected endpoint. Skipping.... {:?}",
                    err
                );
            }
        }

        Ok(())
    }
    async fn remove_protected_endpoints(
        &mut self,
        protected_endpoints: Vec<ProtectedEndpoint>,
    ) -> Result<(), database::Error> {
        query(r#"DELETE FROM protected_endpoints WHERE endpoint IN (:endpoints)"#)
            .bind_vec(
                ":endpoints",
                protected_endpoints
                    .iter()
                    .map(serde_json::to_string)
                    .collect::<Result<_, _>>()?,
            )
            .execute(&self.inner)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl MintAuthDatabase for MintSqliteAuthDatabase {
    type Err = database::Error;

    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn MintAuthTransaction<database::Error> + Send + Sync + 'a>, database::Error>
    {
        Ok(Box::new(SqliteTransaction {
            inner: self.pool.begin().await?,
        }))
    }

    async fn get_active_keyset_id(&self) -> Result<Option<Id>, Self::Err> {
        Ok(query(
            r#"
            SELECT
                id
            FROM
                keyset
            WHERE
                active = 1;
            "#,
        )
        .pluck(&self.pool)
        .await?
        .map(|id| Ok::<_, Error>(column_as_string!(id, Id::from_str, Id::from_bytes)))
        .transpose()?)
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
                WHERE id=:id"#,
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(sqlite_row_to_keyset_info)
        .collect::<Result<Vec<_>, _>>()?)
    }

    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err> {
        let mut current_states = query(r#"SELECT y, state FROM proof WHERE y IN (:ys)"#)
            .bind_vec(":ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| {
                Ok((
                    column_as_string!(&row[0], PublicKey::from_hex, PublicKey::from_slice),
                    column_as_string!(&row[1], State::from_str),
                ))
            })
            .collect::<Result<HashMap<_, _>, Error>>()?;

        Ok(ys.iter().map(|y| current_states.remove(y)).collect())
    }

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
                y
            FROM
                blind_signature
            WHERE y IN (:y)
            "#,
        )
        .bind_vec(
            ":y",
            blinded_messages
                .iter()
                .map(|y| y.to_bytes().to_vec())
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

    async fn get_auth_for_endpoint(
        &self,
        protected_endpoint: ProtectedEndpoint,
    ) -> Result<Option<AuthRequired>, Self::Err> {
        Ok(
            query(r#"SELECT auth FROM protected_endpoints WHERE endpoint = :endpoint"#)
                .bind(":endpoint", serde_json::to_string(&protected_endpoint)?)
                .pluck(&self.pool)
                .await?
                .map(|auth| {
                    Ok::<_, Error>(column_as_string!(
                        auth,
                        serde_json::from_str,
                        serde_json::from_slice
                    ))
                })
                .transpose()?,
        )
    }

    async fn get_auth_for_endpoints(
        &self,
    ) -> Result<HashMap<ProtectedEndpoint, Option<AuthRequired>>, Self::Err> {
        Ok(query(r#"SELECT endpoint, auth FROM protected_endpoints"#)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| {
                let endpoint =
                    column_as_string!(&row[0], serde_json::from_str, serde_json::from_slice);
                let auth = column_as_string!(&row[1], serde_json::from_str, serde_json::from_slice);
                Ok((endpoint, Some(auth)))
            })
            .collect::<Result<HashMap<_, _>, Error>>()?)
    }
}

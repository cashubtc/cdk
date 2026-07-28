//! Keys database implementation

use std::collections::HashMap;
use std::str::FromStr;

use async_trait::async_trait;
use bitcoin::bip32::DerivationPath;
use cdk_common::common::IssuerVersion;
use cdk_common::database::{Error, MintKeyDatabaseTransaction, MintKeysDatabase};
use cdk_common::mint::MintKeySetInfo;
use cdk_common::{CurrencyUnit, Id};

use super::{SQLMintDatabase, SQLTransaction};
use crate::database::{ConnectionWithTransaction, DatabaseExecutor};
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{
    column_as_nullable_number, column_as_nullable_string, column_as_number, column_as_string,
    unpack_into,
};

pub(crate) fn sql_row_to_keyset_info(row: Vec<Column>) -> Result<MintKeySetInfo, Error> {
    unpack_into!(
        let (
            id,
            unit,
            active,
            valid_from,
            valid_to,
            derivation_path,
            derivation_path_index,
            amounts,
            row_keyset_ppk,
            issuer_version
        ) = row
    );

    let amounts = column_as_nullable_string!(amounts)
        .and_then(|str| serde_json::from_str(&str).ok())
        .ok_or_else(|| Error::Database("amounts field is required".to_string().into()))?;

    Ok(MintKeySetInfo {
        id: column_as_string!(id, Id::from_str, Id::from_bytes),
        unit: column_as_string!(unit, CurrencyUnit::from_str),
        active: matches!(active, Column::Integer(1)),
        valid_from: column_as_number!(valid_from),
        derivation_path: column_as_string!(derivation_path, DerivationPath::from_str),
        derivation_path_index: column_as_nullable_number!(derivation_path_index),
        amounts,
        input_fee_ppk: column_as_nullable_number!(row_keyset_ppk).unwrap_or(0),
        final_expiry: column_as_nullable_number!(valid_to),
        issuer_version: column_as_nullable_string!(issuer_version).and_then(|v| {
            match IssuerVersion::from_str(&v) {
                Ok(ver) => Some(ver),
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse issuer_version from database: {}. Error: {}",
                        v,
                        e
                    );
                    None
                }
            }
        }),
    })
}

#[async_trait]
impl<RM> MintKeyDatabaseTransaction<'_, Error> for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    async fn add_keyset_info(&mut self, keyset: MintKeySetInfo) -> Result<(), Error> {
        query(
            r#"
        INSERT INTO
            keyset (
                id, unit, active, valid_from, valid_to, derivation_path,
                amounts, input_fee_ppk, derivation_path_index, issuer_version
            )
        VALUES (
            :id, :unit, :active, :valid_from, :valid_to, :derivation_path,
            :amounts, :input_fee_ppk, :derivation_path_index, :issuer_version
        )
        ON CONFLICT(id) DO UPDATE SET
            unit = excluded.unit,
            active = excluded.active,
            valid_from = excluded.valid_from,
            valid_to = excluded.valid_to,
            derivation_path = excluded.derivation_path,
            amounts = excluded.amounts,
            input_fee_ppk = excluded.input_fee_ppk,
            derivation_path_index = excluded.derivation_path_index,
            issuer_version = excluded.issuer_version
        "#,
        )?
        .bind("id", keyset.id.to_string())
        .bind("unit", keyset.unit.to_string())
        .bind("active", keyset.active)
        .bind("valid_from", keyset.valid_from as i64)
        .bind("valid_to", keyset.final_expiry.map(|v| v as i64))
        .bind("derivation_path", keyset.derivation_path.to_string())
        .bind("amounts", serde_json::to_string(&keyset.amounts).ok())
        .bind("input_fee_ppk", keyset.input_fee_ppk as i64)
        .bind("derivation_path_index", keyset.derivation_path_index)
        .bind(
            "issuer_version",
            keyset.issuer_version.map(|v| v.to_string()),
        )
        .execute(&self.inner)
        .await?;

        self.bump_keyset_epoch().await?;

        Ok(())
    }

    async fn set_active_keyset(&mut self, unit: CurrencyUnit, id: Id) -> Result<(), Error> {
        query(r#"UPDATE keyset SET active=FALSE WHERE unit = :unit"#)?
            .bind("unit", unit.to_string())
            .execute(&self.inner)
            .await?;

        query(r#"UPDATE keyset SET active=TRUE WHERE unit = :unit AND id = :id"#)?
            .bind("unit", unit.to_string())
            .bind("id", id.to_string())
            .execute(&self.inner)
            .await?;

        self.bump_keyset_epoch().await?;

        Ok(())
    }

    async fn next_derivation_index(&mut self, unit: &CurrencyUnit) -> Result<u32, Error> {
        // Take the per-unit advisory lock (if the backend needs one) so two
        // rotations of the same unit serialize and cannot read the same MAX
        // index below. Held until the transaction commits.
        self.advisory_lock(unit).await?;

        let next = match query(
            r#"SELECT COALESCE(MAX(derivation_path_index), 0) + 1 FROM keyset WHERE unit = :unit"#,
        )?
        .bind("unit", unit.to_string())
        .pluck(&self.inner)
        .await?
        {
            Some(column) => column_as_number!(column),
            None => 1,
        };

        Ok(next)
    }

    async fn get_keyset_infos_by_unit(
        &mut self,
        unit: &CurrencyUnit,
    ) -> Result<Vec<MintKeySetInfo>, Error> {
        // Same per-unit lock rotations take, held to commit, so a concurrent
        // rotation cannot slip a higher keyset in between this read and the
        // caller's active-pointer reassignment.
        self.advisory_lock(unit).await?;

        Ok(query(
            r#"SELECT
                id,
                unit,
                active,
                valid_from,
                valid_to,
                derivation_path,
                derivation_path_index,
                amounts,
                input_fee_ppk,
                issuer_version
            FROM
                keyset
                WHERE unit = :unit"#,
        )?
        .bind("unit", unit.to_string())
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .map(sql_row_to_keyset_info)
        .collect::<Result<Vec<_>, _>>()?)
    }
}

impl<RM> SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    /// Take a transaction-scoped advisory lock keyed by `key`, serializing
    /// concurrent writers across processes and held until commit.
    ///
    /// No-op on backends that already serialize writers (SQLite's
    /// `BEGIN IMMEDIATE`). Postgres runs at `START TRANSACTION` isolation, which
    /// does not serialize concurrent reads, so it takes an explicit,
    /// non-standard lock. Dispatched by driver name, the same way migrations
    /// are.
    async fn advisory_lock<T: ToString + Send>(&self, key: T) -> Result<(), Error> {
        if RM::Connection::name() == "postgres" {
            query(r#"SELECT pg_advisory_xact_lock(hashtext(:key))"#)?
                .bind("key", key.to_string())
                .execute(&self.inner)
                .await?;
        }

        Ok(())
    }

    /// Bump the persisted keyset epoch so any keyset change (insert or
    /// active-pointer reassignment) is observable by peers, which reload when
    /// the epoch they loaded no longer matches.
    ///
    /// Upsert rather than a bare `UPDATE`: a plain update would silently affect
    /// zero rows if row 0 were ever absent, freezing the epoch at its fallback
    /// and stalling every peer's reload. The insert path makes the row
    /// self-healing.
    async fn bump_keyset_epoch(&self) -> Result<(), Error> {
        // Qualify the existing value with the table name: on Postgres a bare
        // `epoch` in the update expression is ambiguous between the target row
        // and `excluded`. Matches the upsert style used elsewhere in this crate.
        query(
            r#"
            INSERT INTO keyset_epoch (id, epoch) VALUES (0, 1)
            ON CONFLICT (id) DO UPDATE SET epoch = keyset_epoch.epoch + 1
            "#,
        )?
        .execute(&self.inner)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl<RM> MintKeysDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn MintKeyDatabaseTransaction<'a, Error> + Send + Sync + 'a>, Error> {
        let tx = SQLTransaction {
            inner: ConnectionWithTransaction::new(
                self.pool
                    .get()
                    .await
                    .map_err(|e| Error::Database(Box::new(e)))?,
            )
            .await?,
        };

        Ok(Box::new(tx))
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        Ok(
            query(r#" SELECT id FROM keyset WHERE active = :active AND unit = :unit"#)?
                .bind("active", true)
                .bind("unit", unit.to_string())
                .pluck(&*conn)
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
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        Ok(
            query(r#"SELECT id, unit FROM keyset WHERE active = :active"#)?
                .bind("active", true)
                .fetch_all(&*conn)
                .await?
                .into_iter()
                .map(|row| {
                    Ok((
                        column_as_string!(&row[1], CurrencyUnit::from_str),
                        column_as_string!(&row[0], Id::from_str, Id::from_bytes),
                    ))
                })
                .collect::<Result<HashMap<_, _>, Error>>()?,
        )
    }

    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"SELECT
                id,
                unit,
                active,
                valid_from,
                valid_to,
                derivation_path,
                derivation_path_index,
                amounts,
                input_fee_ppk,
                issuer_version
            FROM
                keyset
                WHERE id=:id"#,
        )?
        .bind("id", id.to_string())
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_keyset_info)
        .transpose()?)
    }

    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"SELECT
                id,
                unit,
                active,
                valid_from,
                valid_to,
                derivation_path,
                derivation_path_index,
                amounts,
                input_fee_ppk,
                issuer_version
            FROM
                keyset
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_keyset_info)
        .collect::<Result<Vec<_>, _>>()?)
    }

    async fn keysets_epoch(&self) -> Result<u64, Self::Err> {
        // A single-row counter bumped inside every keyset-writing transaction,
        // so it moves on any change (insert or active-pointer reassignment). One
        // row to read, far cheaper than reading every keyset.
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        Ok(
            match query(r#"SELECT epoch FROM keyset_epoch WHERE id = 0"#)?
                .pluck(&*conn)
                .await?
            {
                Some(column) => column_as_number!(column),
                None => 0,
            },
        )
    }
}

#[cfg(test)]
mod test {
    use super::*;

    mod keyset_amounts_tests {
        use super::*;

        #[test]
        fn keyset_with_amounts() {
            let amounts = (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>();
            let result = sql_row_to_keyset_info(vec![
                Column::Text("0083a60439303340".to_owned()),
                Column::Text("sat".to_owned()),
                Column::Integer(1),
                Column::Integer(1749844864),
                Column::Null,
                Column::Text("0'/0'/0'".to_owned()),
                Column::Integer(0),
                Column::Text(serde_json::to_string(&amounts).expect("valid json")),
                Column::Integer(0),
                Column::Text("cdk/0.1.0".to_owned()),
            ]);
            assert!(result.is_ok());
            let keyset = result.unwrap();
            assert_eq!(keyset.amounts.len(), 32);
            assert_eq!(
                keyset.issuer_version,
                Some(IssuerVersion::from_str("cdk/0.1.0").unwrap())
            );
        }
    }
}

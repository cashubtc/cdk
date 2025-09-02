use crate::column_as_string;
use crate::common::migrate;
use crate::database::{ConnectionWithTransaction, DatabaseExecutor};
use async_trait::async_trait;
use cdk_common::database::Error;
use lightning::util::persist::KVStore;
use migrations::MIGRATIONS;
use std::sync::Arc;
mod migrations;
use crate::pool::{DatabasePool, Pool, PooledResource};
use crate::stmt::query;

/// Mint SQL Database
#[derive(Debug, Clone)]
pub struct SQLLdkDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    pool: Arc<Pool<RM>>,
}
#[async_trait]
impl<D> KVStore for SQLLdkDatabase<D>
where
    D: DatabasePool + Send + Sync,
    D::Connection: DatabaseExecutor,
{
    fn read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Vec<u8>, bitcoin::io::Error> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let conn = self.pool.get()
                    .map_err(|e| bitcoin::io::Error::new(bitcoin::io::ErrorKind::Other, e))?;

                let stmt = query(
                    "SELECT value FROM ldk_kv_store WHERE primary_namespace = :primary_namespace AND secondary_namespace = :secondary_namespace AND key = :key"
                )
                    .map_err(|e| bitcoin::io::Error::new(bitcoin::io::ErrorKind::Other, e))?
                    .bind("primary_namespace", primary_namespace)
                    .bind("secondary_namespace", secondary_namespace)
                    .bind("key", key);

                let row = stmt.fetch_one(&*conn).await
                    .map_err(|e| bitcoin::io::Error::new(bitcoin::io::ErrorKind::Other, e))?;

                match row {
                    Some(mut columns) if !columns.is_empty() => {
                        let value = columns.remove(0);
                        match value {
                            crate::value::Value::Blob(bytes) => Ok(bytes),
                            crate::value::Value::Null => Err(bitcoin::io::Error::new(
                                bitcoin::io::ErrorKind::NotFound,
                                "Key not found"
                            )),
                            _ => Err(bitcoin::io::Error::new(
                                bitcoin::io::ErrorKind::InvalidData,
                                "Invalid data type for value"
                            )),
                        }
                    }
                    _ => Err(bitcoin::io::Error::new(
                        bitcoin::io::ErrorKind::NotFound,
                        "Key not found"
                    )),
                }
            })
        })
    }

    fn write(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        buf: &[u8],
    ) -> Result<(), bitcoin::io::Error> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let conn = self
                    .pool
                    .get()
                    .map_err(|e| bitcoin::io::Error::new(bitcoin::io::ErrorKind::Other, e))?;

                let stmt = query(
                    "INSERT INTO ldk_kv_store (primary_namespace, secondary_namespace, key, value)
                     VALUES (:primary_namespace, :secondary_namespace, :key, :value)
                     ON CONFLICT (primary_namespace, secondary_namespace, key)
                     DO UPDATE SET value = :value",
                )
                .map_err(|e| bitcoin::io::Error::new(bitcoin::io::ErrorKind::Other, e))?
                .bind("primary_namespace", primary_namespace)
                .bind("secondary_namespace", secondary_namespace)
                .bind("key", key)
                .bind("value", buf.to_vec());

                stmt.execute(&*conn)
                    .await
                    .map_err(|e| bitcoin::io::Error::new(bitcoin::io::ErrorKind::Other, e))?;

                Ok(())
            })
        })
    }

    fn remove(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        _lazy: bool,
    ) -> Result<(), bitcoin::io::Error> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let conn = self.pool.get()
                    .map_err(|e| bitcoin::io::Error::new(bitcoin::io::ErrorKind::Other, e))?;

                let stmt = query(
                    "DELETE FROM ldk_kv_store WHERE primary_namespace = :primary_namespace AND secondary_namespace = :secondary_namespace AND key = :key"
                )
                    .map_err(|e| bitcoin::io::Error::new(bitcoin::io::ErrorKind::Other, e))?
                    .bind("primary_namespace", primary_namespace)
                    .bind("secondary_namespace", secondary_namespace)
                    .bind("key", key);

                stmt.execute(&*conn).await
                    .map_err(|e| bitcoin::io::Error::new(bitcoin::io::ErrorKind::Other, e))?;

                Ok(())
            })
        })
    }

    fn list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, bitcoin::io::Error> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let conn = self.pool.get()
                    .map_err(|e| bitcoin::io::Error::new(bitcoin::io::ErrorKind::Other, e))?;

                let stmt = query(
                    "SELECT key FROM ldk_kv_store WHERE primary_namespace = :primary_namespace AND secondary_namespace = :secondary_namespace"
                )
                    .map_err(|e| bitcoin::io::Error::new(bitcoin::io::ErrorKind::Other, e))?
                    .bind("primary_namespace", primary_namespace)
                    .bind("secondary_namespace", secondary_namespace);

                let rows = stmt.fetch_all(&*conn).await
                    .map_err(|e| bitcoin::io::Error::new(bitcoin::io::ErrorKind::Other, e))?;

                let mut keys = Vec::new();
                for mut row in rows {
                    if !row.is_empty() {
                        let key_value = row.remove(0);
                        match key_value {
                            crate::value::Value::Text(key) => keys.push(key),
                            _ => return Err(bitcoin::io::Error::new(
                                bitcoin::io::ErrorKind::InvalidData,
                                "Invalid data type for key"
                            )),
                        }
                    }
                }

                Ok(keys)
            })
        })
    }
}

impl<RM> SQLLdkDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    /// Creates a new instance
    pub async fn new<X>(db: X) -> Result<Self, Error>
    where
        X: Into<RM::Config>,
    {
        let pool = Pool::new(db.into());

        Self::migrate(pool.get().map_err(|e| Error::Database(Box::new(e)))?).await?;

        Ok(Self { pool })
    }

    /// Migrate
    async fn migrate(conn: PooledResource<RM>) -> Result<(), Error> {
        let tx = ConnectionWithTransaction::new(conn).await?;
        migrate(&tx, RM::Connection::name(), crate::mint::ldk::MIGRATIONS).await?;
        tx.commit().await?;
        Ok(())
    }

    #[inline(always)]
    async fn fetch_from_config<R>(&self, id: &str) -> Result<R, Error>
    where
        R: serde::de::DeserializeOwned,
    {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        let value = column_as_string!(query(r#"SELECT value FROM config WHERE id = :id LIMIT 1"#)?
            .bind("id", id.to_owned())
            .pluck(&*conn)
            .await?
            .ok_or(Error::UnknownQuoteTTL)?);

        Ok(serde_json::from_str(&value)?)
    }
}

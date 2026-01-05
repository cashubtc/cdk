//! Key-Value store database implementation

use async_trait::async_trait;
use cdk_common::database::Error;

use super::{SQLMintDatabase, SQLTransaction};
use crate::database::ConnectionWithTransaction;
use crate::pool::DatabasePool;

#[async_trait]
impl<RM> cdk_common::database::KVStoreTransaction<Error> for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    async fn kv_read(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Error> {
        crate::keyvalue::kv_read_in_transaction(
            &self.inner,
            primary_namespace,
            secondary_namespace,
            key,
        )
        .await
    }

    async fn kv_write(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), Error> {
        crate::keyvalue::kv_write_in_transaction(
            &self.inner,
            primary_namespace,
            secondary_namespace,
            key,
            value,
        )
        .await
    }

    async fn kv_remove(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), Error> {
        crate::keyvalue::kv_remove_in_transaction(
            &self.inner,
            primary_namespace,
            secondary_namespace,
            key,
        )
        .await
    }

    async fn kv_list(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Error> {
        crate::keyvalue::kv_list_in_transaction(&self.inner, primary_namespace, secondary_namespace)
            .await
    }
}

#[async_trait]
impl<RM> cdk_common::database::KVStoreDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Error> {
        crate::keyvalue::kv_read(&self.pool, primary_namespace, secondary_namespace, key).await
    }

    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Error> {
        crate::keyvalue::kv_list(&self.pool, primary_namespace, secondary_namespace).await
    }
}

#[async_trait]
impl<RM> cdk_common::database::KVStore for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    async fn begin_transaction(
        &self,
    ) -> Result<Box<dyn cdk_common::database::KVStoreTransaction<Self::Err> + Send + Sync>, Error>
    {
        Ok(Box::new(SQLTransaction {
            inner: ConnectionWithTransaction::new(
                self.pool.get().map_err(|e| Error::Database(Box::new(e)))?,
            )
            .await?,
        }))
    }
}

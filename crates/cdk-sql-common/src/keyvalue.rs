//! Generic KV Store implementations for SQL databases
//!
//! This module provides generic implementations of KVStore traits that can be
//! used by both mint and wallet database implementations.

use std::sync::Arc;

use cdk_common::database::{validate_kvstore_params, Error};
use cdk_common::util::unix_time;

use crate::column_as_string;
#[cfg(feature = "mint")]
use crate::database::ConnectionWithTransaction;
#[cfg(feature = "mint")]
use crate::pool::PooledResource;
use crate::pool::{DatabasePool, Pool};
use crate::stmt::{query, Column};

/// Generic implementation of KVStoreTransaction for SQL databases
#[cfg(feature = "mint")]
pub(crate) async fn kv_read_in_transaction<RM>(
    conn: &ConnectionWithTransaction<RM::Connection, PooledResource<RM>>,
    primary_namespace: &str,
    secondary_namespace: &str,
    key: &str,
) -> Result<Option<Vec<u8>>, Error>
where
    RM: DatabasePool,
{
    // Validate parameters according to KV store requirements
    validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;
    Ok(query(
        r#"
        SELECT value
        FROM kv_store
        WHERE primary_namespace = :primary_namespace
        AND secondary_namespace = :secondary_namespace
        AND key = :key
        "#,
    )?
    .bind("primary_namespace", primary_namespace.to_owned())
    .bind("secondary_namespace", secondary_namespace.to_owned())
    .bind("key", key.to_owned())
    .pluck(conn)
    .await?
    .and_then(|col| match col {
        Column::Blob(data) => Some(data),
        _ => None,
    }))
}

/// Generic implementation of kv_write for transactions
#[cfg(feature = "mint")]
pub(crate) async fn kv_write_in_transaction<RM>(
    conn: &ConnectionWithTransaction<RM::Connection, PooledResource<RM>>,
    primary_namespace: &str,
    secondary_namespace: &str,
    key: &str,
    value: &[u8],
) -> Result<(), Error>
where
    RM: DatabasePool,
{
    // Validate parameters according to KV store requirements
    validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;

    let current_time = unix_time();

    query(
        r#"
        INSERT INTO kv_store
        (primary_namespace, secondary_namespace, key, value, created_time, updated_time)
        VALUES (:primary_namespace, :secondary_namespace, :key, :value, :created_time, :updated_time)
        ON CONFLICT(primary_namespace, secondary_namespace, key)
        DO UPDATE SET
            value = excluded.value,
            updated_time = excluded.updated_time
        "#,
    )?
    .bind("primary_namespace", primary_namespace.to_owned())
    .bind("secondary_namespace", secondary_namespace.to_owned())
    .bind("key", key.to_owned())
    .bind("value", value.to_vec())
    .bind("created_time", current_time as i64)
    .bind("updated_time", current_time as i64)
    .execute(conn)
    .await?;

    Ok(())
}

/// Generic implementation of kv_remove for transactions
#[cfg(feature = "mint")]
pub(crate) async fn kv_remove_in_transaction<RM>(
    conn: &ConnectionWithTransaction<RM::Connection, PooledResource<RM>>,
    primary_namespace: &str,
    secondary_namespace: &str,
    key: &str,
) -> Result<(), Error>
where
    RM: DatabasePool,
{
    // Validate parameters according to KV store requirements
    validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;
    query(
        r#"
        DELETE FROM kv_store
        WHERE primary_namespace = :primary_namespace
        AND secondary_namespace = :secondary_namespace
        AND key = :key
        "#,
    )?
    .bind("primary_namespace", primary_namespace.to_owned())
    .bind("secondary_namespace", secondary_namespace.to_owned())
    .bind("key", key.to_owned())
    .execute(conn)
    .await?;

    Ok(())
}

/// Generic implementation of kv_list for transactions
#[cfg(feature = "mint")]
pub(crate) async fn kv_list_in_transaction<RM>(
    conn: &ConnectionWithTransaction<RM::Connection, PooledResource<RM>>,
    primary_namespace: &str,
    secondary_namespace: &str,
) -> Result<Vec<String>, Error>
where
    RM: DatabasePool,
{
    // Validate namespace parameters according to KV store requirements
    validate_kvstore_params(primary_namespace, secondary_namespace, None)?;
    query(
        r#"
        SELECT key
        FROM kv_store
        WHERE primary_namespace = :primary_namespace
        AND secondary_namespace = :secondary_namespace
        ORDER BY key
        "#,
    )?
    .bind("primary_namespace", primary_namespace.to_owned())
    .bind("secondary_namespace", secondary_namespace.to_owned())
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|row| Ok(column_as_string!(&row[0])))
    .collect::<Result<Vec<_>, Error>>()
}

/// Generic implementation of kv_read for database (non-transactional)
pub(crate) async fn kv_read<RM>(
    pool: &Arc<Pool<RM>>,
    primary_namespace: &str,
    secondary_namespace: &str,
    key: &str,
) -> Result<Option<Vec<u8>>, Error>
where
    RM: DatabasePool + 'static,
{
    // Validate parameters according to KV store requirements
    validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;

    let conn = pool.get().map_err(|e| Error::Database(Box::new(e)))?;
    Ok(query(
        r#"
        SELECT value
        FROM kv_store
        WHERE primary_namespace = :primary_namespace
        AND secondary_namespace = :secondary_namespace
        AND key = :key
        "#,
    )?
    .bind("primary_namespace", primary_namespace.to_owned())
    .bind("secondary_namespace", secondary_namespace.to_owned())
    .bind("key", key.to_owned())
    .pluck(&*conn)
    .await?
    .and_then(|col| match col {
        Column::Blob(data) => Some(data),
        _ => None,
    }))
}

/// Generic implementation of kv_list for database (non-transactional)
pub(crate) async fn kv_list<RM>(
    pool: &Arc<Pool<RM>>,
    primary_namespace: &str,
    secondary_namespace: &str,
) -> Result<Vec<String>, Error>
where
    RM: DatabasePool + 'static,
{
    // Validate namespace parameters according to KV store requirements
    validate_kvstore_params(primary_namespace, secondary_namespace, None)?;

    let conn = pool.get().map_err(|e| Error::Database(Box::new(e)))?;
    query(
        r#"
        SELECT key
        FROM kv_store
        WHERE primary_namespace = :primary_namespace
        AND secondary_namespace = :secondary_namespace
        ORDER BY key
        "#,
    )?
    .bind("primary_namespace", primary_namespace.to_owned())
    .bind("secondary_namespace", secondary_namespace.to_owned())
    .fetch_all(&*conn)
    .await?
    .into_iter()
    .map(|row| Ok(column_as_string!(&row[0])))
    .collect::<Result<Vec<_>, Error>>()
}

/// Generic implementation of kv_write for database (non-transactional, standalone)
#[cfg(feature = "wallet")]
pub(crate) async fn kv_write_standalone<C>(
    conn: &C,
    primary_namespace: &str,
    secondary_namespace: &str,
    key: &str,
    value: &[u8],
) -> Result<(), Error>
where
    C: crate::database::DatabaseExecutor,
{
    // Validate parameters according to KV store requirements
    validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;

    let current_time = unix_time();

    query(
        r#"
        INSERT INTO kv_store
        (primary_namespace, secondary_namespace, key, value, created_time, updated_time)
        VALUES (:primary_namespace, :secondary_namespace, :key, :value, :created_time, :updated_time)
        ON CONFLICT(primary_namespace, secondary_namespace, key)
        DO UPDATE SET
            value = excluded.value,
            updated_time = excluded.updated_time
        "#,
    )?
    .bind("primary_namespace", primary_namespace.to_owned())
    .bind("secondary_namespace", secondary_namespace.to_owned())
    .bind("key", key.to_owned())
    .bind("value", value.to_vec())
    .bind("created_time", current_time as i64)
    .bind("updated_time", current_time as i64)
    .execute(conn)
    .await?;

    Ok(())
}

/// Generic implementation of kv_remove for database (non-transactional, standalone)
#[cfg(feature = "wallet")]
pub(crate) async fn kv_remove_standalone<C>(
    conn: &C,
    primary_namespace: &str,
    secondary_namespace: &str,
    key: &str,
) -> Result<(), Error>
where
    C: crate::database::DatabaseExecutor,
{
    // Validate parameters according to KV store requirements
    validate_kvstore_params(primary_namespace, secondary_namespace, Some(key))?;
    query(
        r#"
        DELETE FROM kv_store
        WHERE primary_namespace = :primary_namespace
        AND secondary_namespace = :secondary_namespace
        AND key = :key
        "#,
    )?
    .bind("primary_namespace", primary_namespace.to_owned())
    .bind("secondary_namespace", secondary_namespace.to_owned())
    .bind("key", key.to_owned())
    .execute(conn)
    .await?;

    Ok(())
}

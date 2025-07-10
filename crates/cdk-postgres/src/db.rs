use cdk_common::database::Error;
use cdk_sql_base::pool::PooledResource;
use cdk_sql_base::stmt::{Column, Statement};
use futures_util::{pin_mut, TryStreamExt};

use crate::value::PgValue;
use crate::PgConnectionPool;

#[inline(always)]
pub async fn pg_batch(
    conn: &PooledResource<PgConnectionPool>,
    statement: Statement,
) -> Result<(), Error> {
    let (sql, _placeholder_values) = statement.to_sql()?;

    conn.batch_execute(&sql)
        .await
        .map_err(|e| Error::Database(Box::new(e)))
}

#[inline(always)]
pub async fn pg_execute(
    conn: &PooledResource<PgConnectionPool>,
    statement: Statement,
) -> Result<usize, Error> {
    let (sql, placeholder_values) = statement.to_sql()?;
    let prepared_statement = conn
        .prepare(&sql)
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;

    conn.execute_raw(
        &prepared_statement,
        placeholder_values
            .iter()
            .map(|x| x.into())
            .collect::<Vec<PgValue>>(),
    )
    .await
    .map_err(|e| Error::Database(Box::new(e)))
    .map(|x| x as usize)
}

#[inline(always)]
pub async fn pg_fetch_one(
    conn: &PooledResource<PgConnectionPool>,
    statement: Statement,
) -> Result<Option<Vec<Column>>, Error> {
    let (sql, placeholder_values) = statement.to_sql()?;
    let prepared_statement = conn
        .prepare(&sql)
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;

    let stream = conn
        .query_raw(
            &prepared_statement,
            placeholder_values
                .iter()
                .map(|x| x.into())
                .collect::<Vec<PgValue>>(),
        )
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;

    pin_mut!(stream);

    Ok(stream
        .try_next()
        .await
        .map_err(|e| Error::Database(Box::new(e)))?
        .map(|row| {
            (0..row.len())
                .map(|i| row.get::<_, PgValue>(i).into())
                .collect::<Vec<_>>()
        }))
}

#[inline(always)]
pub async fn pg_fetch_all(
    conn: &PooledResource<PgConnectionPool>,
    statement: Statement,
) -> Result<Vec<Vec<Column>>, Error> {
    let (sql, placeholder_values) = statement.to_sql()?;
    let prepared_statement = conn
        .prepare(&sql)
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;

    let stream = conn
        .query_raw(
            &prepared_statement,
            placeholder_values
                .iter()
                .map(|x| x.into())
                .collect::<Vec<PgValue>>(),
        )
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;

    pin_mut!(stream);

    let mut rows = vec![];
    while let Some(row) = stream
        .try_next()
        .await
        .map_err(|e| Error::Database(Box::new(e)))?
    {
        rows.push(
            (0..row.len())
                .map(|i| row.get::<_, PgValue>(i).into())
                .collect::<Vec<_>>(),
        );
    }

    Ok(rows)
}

#[inline(always)]
pub async fn gn_pluck(
    conn: &PooledResource<PgConnectionPool>,
    statement: Statement,
) -> Result<Option<Column>, Error> {
    let (sql, placeholder_values) = statement.to_sql()?;
    let prepared_statement = conn
        .prepare(&sql)
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;

    let stream = conn
        .query_raw(
            &prepared_statement,
            placeholder_values
                .iter()
                .map(|x| x.into())
                .collect::<Vec<PgValue>>(),
        )
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;

    pin_mut!(stream);

    Ok(stream
        .try_next()
        .await
        .map_err(|e| Error::Database(Box::new(e)))?
        .map(|row| row.get::<_, PgValue>(0).into()))
}

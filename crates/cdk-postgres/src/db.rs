use cdk_common::database::Error;
use cdk_sql_common::stmt::{Column, Statement};
use futures_util::{pin_mut, TryStreamExt};
use tokio_postgres::Client;

use crate::value::PgValue;

#[inline(always)]
pub async fn pg_batch(conn: &Client, statement: Statement) -> Result<(), Error> {
    let (sql, _placeholder_values) = statement.to_sql()?;

    conn.batch_execute(&sql)
        .await
        .map_err(|e| Error::Database(Box::new(e)))
}

#[inline(always)]
pub async fn pg_execute(conn: &Client, statement: Statement) -> Result<usize, Error> {
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
    conn: &Client,
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

    stream
        .try_next()
        .await
        .map_err(|e| Error::Database(Box::new(e)))?
        .map(|row| {
            (0..row.len())
                .map(|i| row.try_get::<_, PgValue>(i).map(|value| value.into()))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
        .map_err(|e| Error::Database(Box::new(e)))
}

#[inline(always)]
pub async fn pg_fetch_all(conn: &Client, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
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
                .map(|i| row.try_get::<_, PgValue>(i).map(|value| value.into()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| Error::Database(Box::new(e)))?,
        );
    }

    Ok(rows)
}

#[inline(always)]
pub async fn gn_pluck(conn: &Client, statement: Statement) -> Result<Option<Column>, Error> {
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

    stream
        .try_next()
        .await
        .map_err(|e| Error::Database(Box::new(e)))?
        .map(|row| row.try_get::<_, PgValue>(0).map(|value| value.into()))
        .transpose()
        .map_err(|e| Error::Database(Box::new(e)))
}

use cdk_common::database::Error;
use cdk_sql_common::stmt::{Column, Statement};
use futures_util::{pin_mut, TryStreamExt};
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Error as PgError};

use crate::value::PgValue;

#[inline(always)]
fn to_pgsql_error(err: PgError) -> Error {
    if let Some(err) = err.as_db_error() {
        if *err.code() == SqlState::INTEGRITY_CONSTRAINT_VIOLATION {
            return Error::Duplicate;
        }
    }

    Error::Database(Box::new(err))
}

#[inline(always)]
pub async fn pg_batch(conn: &Client, statement: Statement) -> Result<(), Error> {
    let (sql, _placeholder_values) = statement.to_sql()?;

    conn.batch_execute(&sql).await.map_err(to_pgsql_error)
}

#[inline(always)]
pub async fn pg_execute(conn: &Client, statement: Statement) -> Result<usize, Error> {
    let (sql, placeholder_values) = statement.to_sql()?;
    let prepared_statement = conn.prepare(&sql).await.map_err(to_pgsql_error)?;

    conn.execute_raw(
        &prepared_statement,
        placeholder_values
            .iter()
            .map(|x| x.into())
            .collect::<Vec<PgValue>>(),
    )
    .await
    .map_err(to_pgsql_error)
    .map(|x| x as usize)
}

#[inline(always)]
pub async fn pg_fetch_one(
    conn: &Client,
    statement: Statement,
) -> Result<Option<Vec<Column>>, Error> {
    let (sql, placeholder_values) = statement.to_sql()?;
    let prepared_statement = conn.prepare(&sql).await.map_err(to_pgsql_error)?;

    let stream = conn
        .query_raw(
            &prepared_statement,
            placeholder_values
                .iter()
                .map(|x| x.into())
                .collect::<Vec<PgValue>>(),
        )
        .await
        .map_err(to_pgsql_error)?;

    pin_mut!(stream);

    stream
        .try_next()
        .await
        .map_err(to_pgsql_error)?
        .map(|row| {
            (0..row.len())
                .map(|i| row.try_get::<_, PgValue>(i).map(|value| value.into()))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
        .map_err(to_pgsql_error)
}

#[inline(always)]
pub async fn pg_fetch_all(conn: &Client, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
    let (sql, placeholder_values) = statement.to_sql()?;
    let prepared_statement = conn.prepare(&sql).await.map_err(to_pgsql_error)?;

    let stream = conn
        .query_raw(
            &prepared_statement,
            placeholder_values
                .iter()
                .map(|x| x.into())
                .collect::<Vec<PgValue>>(),
        )
        .await
        .map_err(to_pgsql_error)?;

    pin_mut!(stream);

    let mut rows = vec![];
    while let Some(row) = stream.try_next().await.map_err(to_pgsql_error)? {
        rows.push(
            (0..row.len())
                .map(|i| row.try_get::<_, PgValue>(i).map(|value| value.into()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(to_pgsql_error)?,
        );
    }

    Ok(rows)
}

#[inline(always)]
pub async fn gn_pluck(conn: &Client, statement: Statement) -> Result<Option<Column>, Error> {
    let (sql, placeholder_values) = statement.to_sql()?;
    let prepared_statement = conn.prepare(&sql).await.map_err(to_pgsql_error)?;

    let stream = conn
        .query_raw(
            &prepared_statement,
            placeholder_values
                .iter()
                .map(|x| x.into())
                .collect::<Vec<PgValue>>(),
        )
        .await
        .map_err(to_pgsql_error)?;

    pin_mut!(stream);

    stream
        .try_next()
        .await
        .map_err(to_pgsql_error)?
        .map(|row| row.try_get::<_, PgValue>(0).map(|value| value.into()))
        .transpose()
        .map_err(to_pgsql_error)
}

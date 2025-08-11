use cdk_common::database::Error;
use cdk_sql_common::run_db_operation;
use cdk_sql_common::stmt::{Column, Statement};
use futures_util::{pin_mut, TryStreamExt};
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Error as PgError};

use crate::value::PgValue;

#[inline(always)]
fn to_pgsql_error(err: PgError) -> Error {
    if let Some(err) = err.as_db_error() {
        let code = err.code().to_owned();
        if code == SqlState::INTEGRITY_CONSTRAINT_VIOLATION || code == SqlState::UNIQUE_VIOLATION {
            return Error::Duplicate;
        }
    }

    Error::Database(Box::new(err))
}

#[inline(always)]
pub async fn pg_batch(conn: &Client, statement: Statement) -> Result<(), Error> {
    let (sql, _placeholder_values) = statement.to_sql()?;

    run_db_operation(&sql, conn.batch_execute(&sql), to_pgsql_error).await
}

#[inline(always)]
pub async fn pg_execute(conn: &Client, statement: Statement) -> Result<usize, Error> {
    let (sql, placeholder_values) = statement.to_sql()?;
    let prepared_statement = conn.prepare(&sql).await.map_err(to_pgsql_error)?;

    run_db_operation(
        &sql,
        async {
            conn.execute_raw(
                &prepared_statement,
                placeholder_values
                    .iter()
                    .map(|x| x.into())
                    .collect::<Vec<PgValue>>(),
            )
            .await
            .map(|x| x as usize)
        },
        to_pgsql_error,
    )
    .await
}

#[inline(always)]
pub async fn pg_fetch_one(
    conn: &Client,
    statement: Statement,
) -> Result<Option<Vec<Column>>, Error> {
    let (sql, placeholder_values) = statement.to_sql()?;
    let prepared_statement = conn.prepare(&sql).await.map_err(to_pgsql_error)?;

    run_db_operation(
        &sql,
        async {
            let stream = conn
                .query_raw(
                    &prepared_statement,
                    placeholder_values
                        .iter()
                        .map(|x| x.into())
                        .collect::<Vec<PgValue>>(),
                )
                .await?;

            pin_mut!(stream);

            stream
                .try_next()
                .await?
                .map(|row| {
                    (0..row.len())
                        .map(|i| row.try_get::<_, PgValue>(i).map(|value| value.into()))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()
        },
        to_pgsql_error,
    )
    .await
}

#[inline(always)]
pub async fn pg_fetch_all(conn: &Client, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
    let (sql, placeholder_values) = statement.to_sql()?;
    let prepared_statement = conn.prepare(&sql).await.map_err(to_pgsql_error)?;

    run_db_operation(
        &sql,
        async {
            let stream = conn
                .query_raw(
                    &prepared_statement,
                    placeholder_values
                        .iter()
                        .map(|x| x.into())
                        .collect::<Vec<PgValue>>(),
                )
                .await?;

            pin_mut!(stream);

            let mut rows = vec![];
            while let Some(row) = stream.try_next().await? {
                rows.push(
                    (0..row.len())
                        .map(|i| row.try_get::<_, PgValue>(i).map(|value| value.into()))
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }

            Ok(rows)
        },
        to_pgsql_error,
    )
    .await
}

#[inline(always)]
pub async fn pg_pluck(conn: &Client, statement: Statement) -> Result<Option<Column>, Error> {
    let (sql, placeholder_values) = statement.to_sql()?;
    let prepared_statement = conn.prepare(&sql).await.map_err(to_pgsql_error)?;

    run_db_operation(
        &sql,
        async {
            let stream = conn
                .query_raw(
                    &prepared_statement,
                    placeholder_values
                        .iter()
                        .map(|x| x.into())
                        .collect::<Vec<PgValue>>(),
                )
                .await?;

            pin_mut!(stream);

            stream
                .try_next()
                .await?
                .map(|row| row.try_get::<_, PgValue>(0).map(|value| value.into()))
                .transpose()
        },
        to_pgsql_error,
    )
    .await
}

use std::future::Future;

use cdk_common::database::Error;
use cdk_sql_common::run_db_operation;
use cdk_sql_common::stmt::{Column, Statement};
use cdk_sql_common::value::Value;
use futures_util::{pin_mut, TryStreamExt};
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Error as PgError, Statement as PgStatement};

use crate::statement_cache::StatementCache;
use crate::value::PgValue;

#[inline(always)]
fn to_pgsql_error(err: PgError) -> Error {
    if let Some(err) = err.as_db_error() {
        let code = err.code().to_owned();
        if code == SqlState::INTEGRITY_CONSTRAINT_VIOLATION || code == SqlState::UNIQUE_VIOLATION {
            return Error::Duplicate;
        }

        if code == SqlState::T_R_DEADLOCK_DETECTED {
            return Error::Locked;
        }
    }

    Error::Database(Box::new(err))
}

/// A plan cached before a schema change, or a statement name lost by a connection pooler in
/// transaction mode, both recover by parsing the statement once more.
#[inline(always)]
fn should_reparse(err: &PgError) -> bool {
    matches!(
        err.code(),
        Some(code)
            if *code == SqlState::FEATURE_NOT_SUPPORTED
                || *code == SqlState::INVALID_SQL_STATEMENT_NAME
    )
}

#[inline(always)]
fn pg_values(values: &[Value]) -> Vec<PgValue<'_>> {
    values.iter().map(PgValue::from).collect()
}

/// A client paired with the statements already parsed on that same connection.
#[derive(Clone, Copy, Debug)]
pub struct PgSession<'a> {
    client: &'a Client,
    statements: &'a StatementCache,
}

impl<'a> PgSession<'a> {
    /// Pairs a client with the cache of statements parsed on its connection.
    pub fn new(client: &'a Client, statements: &'a StatementCache) -> Self {
        Self { client, statements }
    }

    /// Reuses the parsed statement when `cacheable`, which a set placeholder rules out because
    /// it renders a different number of parameters per call.
    async fn prepare(&self, sql: &str, cacheable: bool) -> Result<PgStatement, PgError> {
        if !cacheable {
            return self.client.prepare(sql).await;
        }

        if let Some(statement) = self.statements.get(sql) {
            return Ok(statement);
        }

        let statement = self.client.prepare(sql).await?;
        self.statements.insert(sql, &statement);
        Ok(statement)
    }
}

/// Runs `operation` against the parsed form of `sql`, retrying once on the errors that a
/// re-parse fixes.
///
/// PostgreSQL raises those before executing, so the retry cannot duplicate a write. Inside a
/// transaction the first error has already aborted it and the retry fails too, but the cache
/// entry is gone either way, so the caller's next transaction succeeds.
async fn with_statement<T, F, Fut>(
    session: PgSession<'_>,
    sql: &str,
    cacheable: bool,
    operation: F,
) -> Result<T, Error>
where
    T: Send,
    F: Fn(PgStatement) -> Fut + Send,
    Fut: Future<Output = Result<T, PgError>> + Send,
{
    run_db_operation(
        sql,
        async {
            let statement = session.prepare(sql, cacheable).await?;

            match operation(statement).await {
                Err(err) if should_reparse(&err) => {
                    session.statements.remove(sql);
                    operation(session.prepare(sql, cacheable).await?).await
                }
                result => result,
            }
        },
        to_pgsql_error,
    )
    .await
}

/// Multi-statement DDL invalidates the plans cached on this connection, so it starts over.
#[inline(always)]
pub async fn pg_batch(session: PgSession<'_>, statement: Statement) -> Result<(), Error> {
    let (sql, _placeholder_values) = statement.to_sql()?;

    run_db_operation(&sql, session.client.batch_execute(&sql), to_pgsql_error).await?;
    session.statements.clear();

    Ok(())
}

#[inline(always)]
pub async fn pg_execute(session: PgSession<'_>, statement: Statement) -> Result<usize, Error> {
    let cacheable = statement.is_cacheable();
    let (sql, placeholder_values) = statement.to_sql()?;
    let values = &placeholder_values;

    with_statement(session, &sql, cacheable, move |prepared| async move {
        session
            .client
            .execute_raw(&prepared, pg_values(values))
            .await
            .map(|affected| affected as usize)
    })
    .await
}

#[inline(always)]
pub async fn pg_fetch_one(
    session: PgSession<'_>,
    statement: Statement,
) -> Result<Option<Vec<Column>>, Error> {
    let cacheable = statement.is_cacheable();
    let (sql, placeholder_values) = statement.to_sql()?;
    let values = &placeholder_values;

    with_statement(session, &sql, cacheable, move |prepared| async move {
        let stream = session
            .client
            .query_raw(&prepared, pg_values(values))
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
    })
    .await
}

#[inline(always)]
pub async fn pg_fetch_all(
    session: PgSession<'_>,
    statement: Statement,
) -> Result<Vec<Vec<Column>>, Error> {
    let cacheable = statement.is_cacheable();
    let (sql, placeholder_values) = statement.to_sql()?;
    let values = &placeholder_values;

    with_statement(session, &sql, cacheable, move |prepared| async move {
        let stream = session
            .client
            .query_raw(&prepared, pg_values(values))
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
    })
    .await
}

#[inline(always)]
pub async fn pg_pluck(
    session: PgSession<'_>,
    statement: Statement,
) -> Result<Option<Column>, Error> {
    let cacheable = statement.is_cacheable();
    let (sql, placeholder_values) = statement.to_sql()?;
    let values = &placeholder_values;

    with_statement(session, &sql, cacheable, move |prepared| async move {
        let stream = session
            .client
            .query_raw(&prepared, pg_values(values))
            .await?;

        pin_mut!(stream);

        stream
            .try_next()
            .await?
            .map(|row| row.try_get::<_, PgValue>(0).map(|value| value.into()))
            .transpose()
    })
    .await
}

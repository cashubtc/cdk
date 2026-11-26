//! Stataments mod
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use cdk_common::database::Error;

use crate::database::DatabaseExecutor;
use crate::value::Value;

/// The Column type
pub type Column = Value;

/// Expected response type for a given SQL statement
#[derive(Debug, Clone, Copy, Default)]
pub enum ExpectedSqlResponse {
    /// A single row
    SingleRow,
    /// All the rows that matches a query
    #[default]
    ManyRows,
    /// How many rows were affected by the query
    AffectedRows,
    /// Return the first column of the first row
    Pluck,
    /// Batch
    Batch,
}

/// Part value
#[derive(Debug, Clone)]
pub enum PlaceholderValue {
    /// Value
    Value(Value),
    /// Set
    Set(Vec<Value>),
    /// A `VALUES` row list for a multi-row `INSERT`, rendered as `($1, $2), ($3, $4)`
    Rows(Vec<Vec<Value>>),
}

impl From<Value> for PlaceholderValue {
    fn from(value: Value) -> Self {
        PlaceholderValue::Value(value)
    }
}

impl From<Vec<Value>> for PlaceholderValue {
    fn from(value: Vec<Value>) -> Self {
        PlaceholderValue::Set(value)
    }
}

/// SQL Part
#[derive(Debug, Clone)]
pub enum SqlPart {
    /// Raw SQL statement
    Raw(Arc<str>),
    /// Placeholder
    Placeholder(Arc<str>, Option<PlaceholderValue>),
}

/// SQL parser error
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum SqlParseError {
    /// Invalid SQL
    #[error("Unterminated String literal")]
    UnterminatedStringLiteral,
    /// Invalid placeholder name
    #[error("Invalid placeholder name")]
    InvalidPlaceholder,
}

/// Rudimentary SQL parser.
///
/// This function does not validate the SQL statement, it only extracts the placeholder to be
/// database agnostic.
pub fn split_sql_parts(input: &str) -> Result<Vec<SqlPart>, SqlParseError> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            '\'' | '"' => {
                // Start of string literal
                let quote = c;
                current.push(
                    chars
                        .next()
                        .ok_or(SqlParseError::UnterminatedStringLiteral)?,
                );

                let mut closed = false;
                while let Some(&next) = chars.peek() {
                    current.push(
                        chars
                            .next()
                            .ok_or(SqlParseError::UnterminatedStringLiteral)?,
                    );

                    if next == quote {
                        if chars.peek() == Some(&quote) {
                            // Escaped quote (e.g. '' inside strings)
                            current.push(
                                chars
                                    .next()
                                    .ok_or(SqlParseError::UnterminatedStringLiteral)?,
                            );
                        } else {
                            closed = true;
                            break;
                        }
                    }
                }

                if !closed {
                    return Err(SqlParseError::UnterminatedStringLiteral);
                }
            }

            '-' => {
                current.push(
                    chars
                        .next()
                        .ok_or(SqlParseError::UnterminatedStringLiteral)?,
                );

                if chars.peek() == Some(&'-') {
                    while let Some(&next) = chars.peek() {
                        current.push(
                            chars
                                .next()
                                .ok_or(SqlParseError::UnterminatedStringLiteral)?,
                        );
                        if next == '\n' {
                            break;
                        }
                    }
                }
            }

            ':' => {
                chars.next(); // consume ':'

                if chars.peek() == Some(&':') {
                    current.push(':');
                    current.push(
                        chars
                            .next()
                            .ok_or(SqlParseError::UnterminatedStringLiteral)?,
                    );
                    continue;
                }

                // Flush current raw SQL
                if !current.is_empty() {
                    parts.push(SqlPart::Raw(current.clone().into()));
                    current.clear();
                }

                let mut name = String::new();

                while let Some(&next) = chars.peek() {
                    if next.is_alphanumeric() || next == '_' {
                        name.push(
                            chars
                                .next()
                                .ok_or(SqlParseError::UnterminatedStringLiteral)?,
                        );
                    } else {
                        break;
                    }
                }

                if name.is_empty() {
                    return Err(SqlParseError::InvalidPlaceholder);
                }

                parts.push(SqlPart::Placeholder(name.into(), None));
            }

            _ => {
                current.push(
                    chars
                        .next()
                        .ok_or(SqlParseError::UnterminatedStringLiteral)?,
                );
            }
        }
    }

    if !current.is_empty() {
        parts.push(SqlPart::Raw(current.into()));
    }

    Ok(parts)
}

type Cache = HashMap<String, (Vec<SqlPart>, Option<Arc<str>>)>;

/// Sql message
#[derive(Debug, Clone, Default)]
pub struct Statement {
    cache: Arc<RwLock<Cache>>,
    cached_sql: Option<Arc<str>>,
    sql: Option<String>,
    /// The SQL statement
    pub parts: Vec<SqlPart>,
    /// The expected response type
    pub expected_response: ExpectedSqlResponse,
}

impl Statement {
    /// Creates a new statement
    fn new(sql: &str, cache: Arc<RwLock<Cache>>) -> Result<Self, SqlParseError> {
        let parsed = cache
            .read()
            .map(|cache| cache.get(sql).cloned())
            .ok()
            .flatten();

        if let Some((parts, cached_sql)) = parsed {
            Ok(Self {
                parts,
                cached_sql,
                sql: None,
                cache,
                ..Default::default()
            })
        } else {
            let parts = split_sql_parts(sql)?;

            if let Ok(mut cache) = cache.write() {
                cache.insert(sql.to_owned(), (parts.clone(), None));
            } else {
                tracing::warn!("Failed to acquire write lock for SQL statement cache");
            }

            Ok(Self {
                parts,
                sql: Some(sql.to_owned()),
                cache,
                ..Default::default()
            })
        }
    }

    /// Convert Statement into a SQL statement and the list of placeholders
    ///
    /// By default it converts the statement into placeholder using $1..$n placeholders which seems
    /// to be more widely supported, although it can be reimplemented with other formats since part
    /// is public
    pub fn to_sql(self) -> Result<(String, Vec<Value>), Error> {
        let has_expanded_placeholder = self.parts.iter().any(|part| {
            matches!(
                part,
                SqlPart::Placeholder(
                    _,
                    Some(PlaceholderValue::Set(_) | PlaceholderValue::Rows(_))
                )
            )
        });

        if let (false, Some(cached_sql)) = (has_expanded_placeholder, self.cached_sql) {
            let sql = cached_sql.to_string();
            let values = self
                .parts
                .into_iter()
                .map(|x| match x {
                    SqlPart::Placeholder(name, value) => {
                        match value.ok_or(Error::MissingPlaceholder(name.to_string()))? {
                            PlaceholderValue::Value(value) => Ok(vec![value]),
                            PlaceholderValue::Set(values) => Ok(values),
                            PlaceholderValue::Rows(rows) => {
                                Ok(rows.into_iter().flatten().collect())
                            }
                        }
                    }
                    SqlPart::Raw(_) => Ok(vec![]),
                })
                .collect::<Result<Vec<_>, Error>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            return Ok((sql, values));
        }

        let mut placeholder_values = Vec::new();
        let mut can_be_cached = true;
        let sql = self
            .parts
            .into_iter()
            .map(|x| match x {
                SqlPart::Placeholder(name, value) => {
                    match value.ok_or(Error::MissingPlaceholder(name.to_string()))? {
                        PlaceholderValue::Value(value) => {
                            placeholder_values.push(value);
                            Ok::<_, Error>(format!("${}", placeholder_values.len()))
                        }
                        PlaceholderValue::Set(mut values) => {
                            can_be_cached = false;
                            let start_size = placeholder_values.len();
                            placeholder_values.append(&mut values);
                            let placeholders = (start_size + 1..=placeholder_values.len())
                                .map(|i| format!("${i}"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            Ok(placeholders)
                        }
                        PlaceholderValue::Rows(rows) => {
                            can_be_cached = false;
                            let mut tuples = Vec::with_capacity(rows.len());
                            for row in rows {
                                let start_size = placeholder_values.len();
                                placeholder_values.extend(row);
                                tuples.push(format!(
                                    "({})",
                                    (start_size + 1..=placeholder_values.len())
                                        .map(|i| format!("${i}"))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ));
                            }
                            Ok(tuples.join(", "))
                        }
                    }
                }
                SqlPart::Raw(raw) => Ok(raw.trim().to_string()),
            })
            .collect::<Result<Vec<String>, _>>()?
            .join(" ");

        if can_be_cached {
            if let Some(original_sql) = self.sql {
                let _ = self.cache.write().map(|mut cache| {
                    if let Some((_, cached_sql)) = cache.get_mut(&original_sql) {
                        *cached_sql = Some(sql.clone().into());
                    }
                });
            }
        }

        Ok((sql, placeholder_values))
    }

    /// Binds a given placeholder to a value.
    #[inline]
    pub fn bind<C, V>(mut self, name: C, value: V) -> Self
    where
        C: ToString,
        V: Into<Value>,
    {
        let name = name.to_string();
        let value = value.into();
        let value: PlaceholderValue = value.into();

        for part in self.parts.iter_mut() {
            if let SqlPart::Placeholder(part_name, part_value) = part {
                if **part_name == *name.as_str() {
                    *part_value = Some(value.clone());
                }
            }
        }

        self
    }

    /// Binds a single variable with a vector.
    ///
    /// This will rewrite the function from `:foo` (where value is vec![1, 2, 3]) to `:foo0, :foo1,
    /// :foo2` and binds each value from the value vector accordingly.
    ///
    /// Returns an error if the vector is empty, as empty `IN` clauses produce invalid SQL.
    #[inline]
    pub fn bind_vec<C, V>(mut self, name: C, value: Vec<V>) -> Result<Self, Error>
    where
        C: ToString,
        V: Into<Value>,
    {
        let name = name.to_string();

        if value.is_empty() {
            return Err(Error::EmptyInClause(name));
        }

        let value: PlaceholderValue = value
            .into_iter()
            .map(|x| x.into())
            .collect::<Vec<Value>>()
            .into();

        for part in self.parts.iter_mut() {
            if let SqlPart::Placeholder(part_name, part_value) = part {
                if **part_name == *name.as_str() {
                    *part_value = Some(value.clone());
                }
            }
        }

        Ok(self)
    }

    /// Binds a placeholder to a `VALUES` row list for a multi-row `INSERT`.
    ///
    /// `INSERT INTO t (a, b) VALUES :rows` with two rows renders as
    /// `INSERT INTO t (a, b) VALUES ($1, $2), ($3, $4)`.
    ///
    /// Returns [`Error::EmptyInClause`] for no rows, since an empty `VALUES` list is invalid SQL,
    /// and [`Error::Internal`] for rows of differing width, which would otherwise reach the
    /// database as a column-count mismatch.
    #[inline]
    pub fn bind_rows<C, V>(mut self, name: C, rows: Vec<Vec<V>>) -> Result<Self, Error>
    where
        C: ToString,
        V: Into<Value>,
    {
        let name = name.to_string();

        let rows = rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|value| value.into())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        let Some(width) = rows.first().map(|row| row.len()) else {
            return Err(Error::EmptyInClause(name));
        };

        if let Some(bad) = rows.iter().find(|row| row.len() != width) {
            return Err(Error::Internal(format!(
                "row list {name} mixes widths {width} and {}",
                bad.len()
            )));
        }

        let value = PlaceholderValue::Rows(rows);

        for part in self.parts.iter_mut() {
            if let SqlPart::Placeholder(part_name, part_value) = part {
                if **part_name == *name.as_str() {
                    *part_value = Some(value.clone());
                }
            }
        }

        Ok(self)
    }

    /// Executes a query and returns the affected rows
    pub async fn pluck<C>(self, conn: &C) -> Result<Option<Value>, Error>
    where
        C: DatabaseExecutor,
    {
        conn.pluck(self).await
    }

    /// Executes a query and returns the affected rows
    pub async fn batch<C>(self, conn: &C) -> Result<(), Error>
    where
        C: DatabaseExecutor,
    {
        conn.batch(self).await
    }

    /// Executes a query and returns the affected rows
    pub async fn execute<C>(self, conn: &C) -> Result<usize, Error>
    where
        C: DatabaseExecutor,
    {
        conn.execute(self).await
    }

    /// Runs the query and returns the first row or None
    pub async fn fetch_one<C>(self, conn: &C) -> Result<Option<Vec<Column>>, Error>
    where
        C: DatabaseExecutor,
    {
        conn.fetch_one(self).await
    }

    /// Runs the query and returns the first row or None
    pub async fn fetch_all<C>(self, conn: &C) -> Result<Vec<Vec<Column>>, Error>
    where
        C: DatabaseExecutor,
    {
        conn.fetch_all(self).await
    }
}

/// Maximum bound parameters allowed in a single statement.
///
/// SQLite's `SQLITE_MAX_VARIABLE_NUMBER` defaults to 32766 since 3.32 but to 999 in older builds,
/// and this crate cannot know how a backend's SQLite was compiled, so the legacy limit binds.
/// PostgreSQL's wire protocol caps parameters at 65535, well above it. The headroom below 999
/// leaves room for scalar placeholders bound alongside a row list.
pub const MAX_SQL_PARAMETERS: usize = 900;

/// How many rows of `columns_per_row` parameters fit in one statement, at least one.
pub fn max_rows_per_statement(columns_per_row: usize) -> usize {
    (MAX_SQL_PARAMETERS / columns_per_row.max(1)).max(1)
}

/// Executes `sql` once per parameter-safe chunk of `rows`, binding each chunk to `placeholder`,
/// and returns the total number of affected rows.
///
/// Chunks are submitted in order, so a caller that sorted `rows` by primary key keeps the lock
/// ordering that avoids deadlocks between concurrent writers.
pub async fn execute_rows<C>(
    conn: &C,
    sql: &str,
    placeholder: &str,
    rows: Vec<Vec<Value>>,
) -> Result<usize, Error>
where
    C: DatabaseExecutor,
{
    let Some(width) = rows.first().map(|row| row.len()) else {
        return Ok(0);
    };

    let mut affected = 0;

    for chunk in rows.chunks(max_rows_per_statement(width)) {
        affected += query(sql)?
            .bind_rows(placeholder, chunk.to_vec())?
            .execute(conn)
            .await?;
    }

    Ok(affected)
}

/// Runs `sql` once per parameter-safe chunk of `keys`, binding each chunk to `placeholder`, and
/// returns every row the chunks matched.
///
/// A key list is one parameter per key, hence the row budget for a single-column row.
///
/// `sql` must bind no placeholder other than `placeholder`, since the chunk consumes the whole
/// parameter budget. A statement that takes row locks needs `keys` sorted by the locked column,
/// or the locks are ordered within a chunk but not across them, which is how two concurrent
/// callers deadlock.
pub async fn fetch_in_chunks<C, V>(
    conn: &C,
    sql: &str,
    placeholder: &str,
    keys: Vec<V>,
) -> Result<Vec<Vec<Column>>, Error>
where
    C: DatabaseExecutor,
    V: Into<Value>,
{
    let keys = keys.into_iter().map(Into::into).collect::<Vec<Value>>();

    if keys.is_empty() {
        return Err(Error::EmptyInClause(placeholder.to_owned()));
    }

    let mut rows = Vec::new();

    for chunk in keys.chunks(max_rows_per_statement(1)) {
        rows.extend(
            query(sql)?
                .bind_vec(placeholder, chunk.to_vec())?
                .fetch_all(conn)
                .await?,
        );
    }

    Ok(rows)
}

/// Executes `sql` once per parameter-safe chunk of `keys`, binding each chunk to `placeholder`,
/// and returns the total number of affected rows.
///
/// Carries the same constraints as [`fetch_in_chunks`], and one more: the chunks are separate
/// statements, so a write whose row order matters is ordered inside a chunk and not across them.
/// A statement whose effect depends on what the other keys matched cannot be split at all.
pub async fn execute_in_chunks<C, V>(
    conn: &C,
    sql: &str,
    placeholder: &str,
    keys: Vec<V>,
) -> Result<usize, Error>
where
    C: DatabaseExecutor,
    V: Into<Value>,
{
    let keys = keys.into_iter().map(Into::into).collect::<Vec<Value>>();

    if keys.is_empty() {
        return Err(Error::EmptyInClause(placeholder.to_owned()));
    }

    let mut affected = 0;

    for chunk in keys.chunks(max_rows_per_statement(1)) {
        affected += query(sql)?
            .bind_vec(placeholder, chunk.to_vec())?
            .execute(conn)
            .await?;
    }

    Ok(affected)
}

/// Creates a new query statement
#[inline(always)]
pub fn query(sql: &str) -> Result<Statement, Error> {
    static CACHE: LazyLock<Arc<RwLock<Cache>>> =
        LazyLock::new(|| Arc::new(RwLock::new(HashMap::new())));
    Statement::new(sql, CACHE.clone()).map_err(|e| Error::Database(Box::new(e)))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Records what each statement would bind, so a test can assert the parameter budget without
    /// a database.
    #[derive(Debug, Default)]
    struct RecordingExecutor {
        bound: Mutex<Vec<usize>>,
        rows_per_statement: usize,
    }

    #[async_trait::async_trait]
    impl DatabaseExecutor for RecordingExecutor {
        fn name() -> &'static str {
            "recording"
        }

        async fn execute(&self, statement: Statement) -> Result<usize, Error> {
            let (_, values) = statement.to_sql()?;
            self.bound.lock().expect("poisoned").push(values.len());
            Ok(values.len())
        }

        async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
            Ok(self.fetch_all(statement).await?.into_iter().next())
        }

        async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
            let (_, values) = statement.to_sql()?;
            self.bound.lock().expect("poisoned").push(values.len());
            Ok((0..self.rows_per_statement)
                .map(|row| vec![Value::from(row as i64)])
                .collect())
        }

        async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
            Ok(self
                .fetch_all(statement)
                .await?
                .into_iter()
                .next()
                .and_then(|mut row| row.drain(..).next()))
        }

        async fn batch(&self, _statement: Statement) -> Result<(), Error> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn fetch_in_chunks_keeps_every_statement_inside_the_budget() {
        let conn = RecordingExecutor {
            rows_per_statement: 2,
            ..Default::default()
        };
        let keys = (0..MAX_SQL_PARAMETERS * 2 + 1)
            .map(|key| key as i64)
            .collect::<Vec<_>>();

        let rows = fetch_in_chunks(&conn, "SELECT id FROM t WHERE id IN (:ids)", "ids", keys)
            .await
            .expect("fetch");

        let bound = conn.bound.lock().expect("poisoned").clone();
        assert_eq!(bound.len(), 3, "one statement per chunk");
        assert!(
            bound.iter().all(|count| *count <= MAX_SQL_PARAMETERS),
            "a statement bound more than the budget: {bound:?}"
        );
        assert_eq!(
            bound.iter().sum::<usize>(),
            MAX_SQL_PARAMETERS * 2 + 1,
            "every key reaches a statement exactly once"
        );
        assert_eq!(rows.len(), 6, "rows from every chunk are returned");
    }

    #[tokio::test]
    async fn execute_in_chunks_sums_what_each_chunk_affected() {
        let conn = RecordingExecutor::default();
        let keys = (0..MAX_SQL_PARAMETERS + 1)
            .map(|key| key as i64)
            .collect::<Vec<_>>();

        let affected = execute_in_chunks(&conn, "DELETE FROM t WHERE id IN (:ids)", "ids", keys)
            .await
            .expect("execute");

        assert_eq!(affected, MAX_SQL_PARAMETERS + 1);
        assert_eq!(conn.bound.lock().expect("poisoned").len(), 2);
    }

    #[tokio::test]
    async fn chunked_helpers_reject_an_empty_key_list() {
        let conn = RecordingExecutor::default();

        let fetched = fetch_in_chunks(
            &conn,
            "SELECT 1 WHERE id IN (:ids)",
            "ids",
            Vec::<i64>::new(),
        )
        .await;
        assert!(matches!(fetched, Err(Error::EmptyInClause(name)) if name == "ids"));

        let executed = execute_in_chunks(
            &conn,
            "DELETE FROM t WHERE id IN (:ids)",
            "ids",
            Vec::<i64>::new(),
        )
        .await;
        assert!(matches!(executed, Err(Error::EmptyInClause(name)) if name == "ids"));
    }

    #[test]
    fn bind_vec_errors_on_empty_vec() {
        let stmt = query("SELECT * FROM foo WHERE id IN (:ids)").unwrap();
        let result = stmt.bind_vec("ids", Vec::<Vec<u8>>::new());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::EmptyInClause(name) if name == "ids"));
    }

    #[test]
    fn parser_preserves_postgres_cast_operator() {
        let stmt = query("SELECT (ord - 1)::int AS matched WHERE id = :id")
            .unwrap()
            .bind("id", "quote-id");

        let (sql, values) = stmt.to_sql().unwrap();

        assert_eq!(sql, "SELECT (ord - 1)::int AS matched WHERE id = $1");
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn bind_vec_ignores_cached_sql_for_same_query_string() {
        let raw_sql = "SELECT * FROM cached_sql_vec_bug WHERE id IN (:ids)";

        let (cached_sql, cached_values) =
            query(raw_sql).unwrap().bind("ids", 1_i64).to_sql().unwrap();
        assert!(cached_sql.contains("$1"));
        assert_eq!(cached_values.len(), 1);

        let (sql, values) = query(raw_sql)
            .unwrap()
            .bind_vec("ids", vec![1_i64, 2, 3])
            .unwrap()
            .to_sql()
            .unwrap();

        assert!(sql.contains("$1, $2, $3"));
        assert_eq!(values.len(), 3);
    }

    #[test]
    fn bind_rows_renders_value_tuples() {
        let (sql, values) = query("INSERT INTO foo (a, b) VALUES :rows")
            .unwrap()
            .bind_rows("rows", vec![vec![1_i64, 2], vec![3, 4], vec![5, 6]])
            .unwrap()
            .to_sql()
            .unwrap();

        assert_eq!(
            sql,
            "INSERT INTO foo (a, b) VALUES ($1, $2), ($3, $4), ($5, $6)"
        );
        assert_eq!(values.len(), 6);
        assert_eq!(values[5], Value::Integer(6));
    }

    #[test]
    fn bind_rows_numbers_after_scalar_binds() {
        let (sql, values) = query("INSERT INTO foo (a, b) VALUES :rows ON CONFLICT DO NOTHING")
            .unwrap()
            .bind("ignored", 9_i64)
            .bind_rows("rows", vec![vec![1_i64, 2]])
            .unwrap()
            .to_sql()
            .unwrap();

        assert!(sql.contains("($1, $2)"));
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn bind_rows_errors_on_no_rows() {
        let result = query("INSERT INTO foo (a) VALUES :rows")
            .unwrap()
            .bind_rows("rows", Vec::<Vec<i64>>::new());

        assert!(matches!(result.unwrap_err(), Error::EmptyInClause(name) if name == "rows"));
    }

    #[test]
    fn bind_rows_errors_on_ragged_rows() {
        let result = query("INSERT INTO foo (a, b) VALUES :rows")
            .unwrap()
            .bind_rows("rows", vec![vec![1_i64, 2], vec![3]]);

        assert!(matches!(result.unwrap_err(), Error::Internal(_)));
    }

    #[test]
    fn bind_rows_ignores_cached_sql_for_same_query_string() {
        let raw_sql = "INSERT INTO cached_sql_rows (a) VALUES :rows";

        let (cached_sql, _) = query(raw_sql)
            .unwrap()
            .bind("rows", 1_i64)
            .to_sql()
            .unwrap();
        assert!(cached_sql.ends_with("$1"));

        let (sql, values) = query(raw_sql)
            .unwrap()
            .bind_rows("rows", vec![vec![1_i64], vec![2]])
            .unwrap()
            .to_sql()
            .unwrap();

        assert!(sql.ends_with("($1), ($2)"));
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn max_rows_per_statement_fits_the_parameter_budget() {
        assert_eq!(max_rows_per_statement(11), 81);
        assert_eq!(max_rows_per_statement(2), 450);
        assert!(max_rows_per_statement(11) * 11 <= MAX_SQL_PARAMETERS);
        assert_eq!(max_rows_per_statement(0), MAX_SQL_PARAMETERS);
        assert_eq!(max_rows_per_statement(MAX_SQL_PARAMETERS * 2), 1);
    }
}

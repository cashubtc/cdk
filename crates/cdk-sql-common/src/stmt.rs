//! Stataments mod
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use cdk_common::database::{ConversionError, Error};

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
#[derive(Debug, Default)]
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
        let has_set_placeholder = self.parts.iter().any(|part| {
            matches!(
                part,
                SqlPart::Placeholder(_, Some(PlaceholderValue::Set(_)))
            )
        });

        if let (false, Some(cached_sql)) = (has_set_placeholder, self.cached_sql) {
            let sql = cached_sql.to_string();
            let values = self
                .parts
                .into_iter()
                .map(|x| match x {
                    SqlPart::Placeholder(name, value) => {
                        match value.ok_or(Error::MissingPlaceholder(name.to_string()))? {
                            PlaceholderValue::Value(value) => Ok(vec![value]),
                            PlaceholderValue::Set(values) => Ok(values),
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

    /// Binds a given placeholder to a `u64`, or to NULL when it is absent.
    ///
    /// Every integer column is signed, so a value past `i64::MAX` used to wrap
    /// negative and commit a row that no later read could decode, which failed
    /// every read of that table rather than only its own row. Refusing the
    /// write keeps the table readable.
    #[inline]
    pub fn bind_u64<C, V>(self, name: C, value: V) -> Result<Self, Error>
    where
        C: ToString,
        V: Into<Option<u64>>,
    {
        let name = name.to_string();
        let value = match value.into() {
            Some(value) => Value::Integer(
                i64::try_from(value)
                    .map_err(|_| ConversionError::ValueOutOfRange(name.clone(), value))?,
            ),
            None => Value::Null,
        };

        Ok(self.bind(name, value))
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

/// Creates a new query statement
#[inline(always)]
pub fn query(sql: &str) -> Result<Statement, Error> {
    static CACHE: LazyLock<Arc<RwLock<Cache>>> =
        LazyLock::new(|| Arc::new(RwLock::new(HashMap::new())));
    Statement::new(sql, CACHE.clone()).map_err(|e| Error::Database(Box::new(e)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn bind_u64_accepts_up_to_i64_max() {
        let largest = u64::try_from(i64::MAX).expect("i64::MAX is not negative");
        let (_, values) = query("SELECT :a, :b, :c")
            .unwrap()
            .bind_u64("a", 0u64)
            .unwrap()
            .bind_u64("b", largest)
            .unwrap()
            .bind_u64("c", Some(7u64))
            .unwrap()
            .to_sql()
            .unwrap();

        assert_eq!(
            values,
            vec![
                Value::Integer(0),
                Value::Integer(i64::MAX),
                Value::Integer(7)
            ]
        );
    }

    #[test]
    fn bind_u64_binds_null_when_absent() {
        let (_, values) = query("SELECT :a")
            .unwrap()
            .bind_u64("a", None::<u64>)
            .unwrap()
            .to_sql()
            .unwrap();

        assert_eq!(values, vec![Value::Null]);
    }

    #[test]
    fn bind_u64_rejects_past_i64_max() {
        let oversized = u64::try_from(i64::MAX).expect("i64::MAX is not negative") + 1;
        let err = query("SELECT :expiry")
            .unwrap()
            .bind_u64("expiry", oversized)
            .expect_err("value does not fit");

        assert!(matches!(
            err,
            Error::Conversion(ConversionError::ValueOutOfRange(field, value))
                if field == "expiry" && value == oversized
        ));
    }
}

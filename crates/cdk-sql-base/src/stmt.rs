//! Stataments mod
use std::collections::HashMap;

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

/// Sql message
#[derive(Debug, Default)]
pub struct Statement {
    /// The SQL statement
    pub sql: String,
    /// The list of arguments for the placeholders. It only supports named arguments for simplicity
    /// sake
    pub args: HashMap<String, Value>,
    /// The expected response type
    pub expected_response: ExpectedSqlResponse,
}

impl Statement {
    /// Creates a new statement
    pub fn new<T>(sql: T) -> Self
    where
        T: ToString,
    {
        Self {
            sql: sql.to_string(),
            ..Default::default()
        }
    }

    /// Binds a given placeholder to a value.
    #[inline]
    pub fn bind<C, V>(mut self, name: C, value: V) -> Self
    where
        C: ToString,
        V: Into<Value>,
    {
        self.args.insert(name.to_string(), value.into());
        self
    }

    /// Binds a single variable with a vector.
    ///
    /// This will rewrite the function from `:foo` (where value is vec![1, 2, 3]) to `:foo0, :foo1,
    /// :foo2` and binds each value from the value vector accordingly.
    #[inline]
    pub fn bind_vec<C, V>(mut self, name: C, value: Vec<V>) -> Self
    where
        C: ToString,
        V: Into<Value>,
    {
        let mut new_sql = String::with_capacity(self.sql.len());
        let target = name.to_string();
        let mut i = 0;

        let placeholders = value
            .into_iter()
            .enumerate()
            .map(|(key, value)| {
                let key = format!("{target}{key}");
                self.args.insert(key.clone(), value.into());
                key
            })
            .collect::<Vec<_>>()
            .join(",");

        while let Some(pos) = self.sql[i..].find(&target) {
            let abs_pos = i + pos;
            let after = abs_pos + target.len();
            let is_word_boundary = self.sql[after..]
                .chars()
                .next()
                .map_or(true, |c| !c.is_alphanumeric() && c != '_');

            if is_word_boundary {
                new_sql.push_str(&self.sql[i..abs_pos]);
                new_sql.push_str(&placeholders);
                i = after;
            } else {
                new_sql.push_str(&self.sql[i..=abs_pos]);
                i = abs_pos + 1;
            }
        }

        new_sql.push_str(&self.sql[i..]);

        self.sql = new_sql;
        self
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
pub fn query<T>(sql: T) -> Statement
where
    T: ToString,
{
    Statement::new(sql)
}

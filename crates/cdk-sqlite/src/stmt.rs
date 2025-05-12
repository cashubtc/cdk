use r2d2::PooledConnection;
use r2d2_sqlite::rusqlite::{self, CachedStatement};
use r2d2_sqlite::SqliteConnectionManager;

pub type Value = r2d2_sqlite::rusqlite::types::Value;

/// The Column type
pub type Column = r2d2_sqlite::rusqlite::types::Value;

/// Expected Sql response
#[derive(Debug, Clone, Copy, Default)]
pub enum ExpectedSqlResponse {
    SingleRow,
    #[default]
    ManyRows,
    AffectedRows,
    Pluck,
}

/// Sql message
#[derive(Default, Debug)]
pub struct Statement {
    pub sql: String,
    pub args: Vec<(String, Value)>,
    pub expected_response: ExpectedSqlResponse,
}

impl Statement {
    pub fn new<T: ToString>(sql: T) -> Self {
        Self {
            sql: sql.to_string(),
            ..Default::default()
        }
    }

    #[inline]
    pub fn bind<C: ToString, V: Into<Value>>(mut self, name: C, value: V) -> Self {
        self.args.push((name.to_string(), value.into()));
        self
    }

    /// Binds a single variable with a vector.
    ///
    /// This will rewrite the function from `:foo` (where value is vec![1, 2, 3]) to `:foo0, :foo1,
    /// :foo2` and binds each value from the value vector accordingly.
    #[inline]
    pub fn bind_vec<C: ToString, V: Into<Value>>(mut self, name: C, value: Vec<V>) -> Self {
        let mut new_sql = String::with_capacity(self.sql.len());
        let target = name.to_string();
        let mut i = 0;

        let placeholders = value
            .into_iter()
            .enumerate()
            .map(|(key, value)| {
                let key = format!("{target}{key}");
                self.args.push((key.clone(), value.into()));
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

    fn get_stmt(
        self,
        conn: &PooledConnection<SqliteConnectionManager>,
    ) -> rusqlite::Result<CachedStatement<'_>> {
        let mut stmt = conn.prepare_cached(&self.sql)?;
        for (name, value) in self.args {
            let index = stmt
                .parameter_index(&name)
                .map_err(|_| rusqlite::Error::InvalidColumnName(name.clone()))?
                .ok_or(rusqlite::Error::InvalidColumnName(name))?;

            stmt.raw_bind_parameter(index, value)?;
        }

        Ok(stmt)
    }
    ///
    /// Executes a query and returns the affected rows
    pub fn plunk(
        self,
        conn: &PooledConnection<SqliteConnectionManager>,
    ) -> rusqlite::Result<Option<Value>> {
        let mut stmt = self.get_stmt(conn)?;
        let mut rows = stmt.raw_query();
        rows.next()?.map(|row| row.get(0)).transpose()
    }

    /// Executes a query and returns the affected rows
    pub fn execute(
        self,
        conn: &PooledConnection<SqliteConnectionManager>,
    ) -> rusqlite::Result<usize> {
        self.get_stmt(conn)?.raw_execute()
    }

    /// Runs the query and returns the first row or None
    pub fn fetch_one(
        self,
        conn: &PooledConnection<SqliteConnectionManager>,
    ) -> rusqlite::Result<Option<Vec<Column>>> {
        let mut stmt = self.get_stmt(conn)?;
        let columns = stmt.column_count();
        let mut rows = stmt.raw_query();
        rows.next()?
            .map(|row| {
                (0..columns)
                    .map(|i| row.get(i))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
    }

    /// Runs the query and returns the first row or None
    pub fn fetch_all(
        self,
        conn: &PooledConnection<SqliteConnectionManager>,
    ) -> rusqlite::Result<Vec<Vec<Column>>> {
        let mut stmt = self.get_stmt(conn)?;
        let columns = stmt.column_count();
        let mut rows = stmt.raw_query();
        let mut results = vec![];

        while let Some(row) = rows.next()? {
            results.push(
                (0..columns)
                    .map(|i| row.get(i))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }

        Ok(results)
    }
}

//! Prepared statements.

use std::fmt;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::params::Params;
use crate::raw::RawStatement;
use crate::row::{MappedRows, Row, Rows};
use crate::types::ToSql;

/// A prepared statement, borrowed from its [`Connection`].
pub struct Statement<'conn> {
    conn: &'conn Connection,
    raw: RawStatement,
}

impl fmt::Debug for Statement<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Statement")
            .field("column_count", &self.column_count())
            .field("parameter_count", &self.parameter_count())
            .finish_non_exhaustive()
    }
}

impl<'conn> Statement<'conn> {
    pub(crate) fn new(conn: &'conn Connection, raw: RawStatement) -> Self {
        Self { conn, raw }
    }

    pub(crate) fn connection(&self) -> &'conn Connection {
        self.conn
    }

    pub(crate) fn into_raw(self) -> RawStatement {
        self.raw
    }

    /// Binds `params`, runs the statement and returns the number of changed
    /// rows.
    ///
    /// Fails with [`Error::ExecuteReturnedResults`] if the statement returns
    /// rows; use [`query`](Self::query) for those.
    pub fn execute<P>(&mut self, params: P) -> Result<usize>
    where
        P: Params,
    {
        params.bind_in(self)?;
        self.raw_execute()
    }

    /// Runs the statement with its current bindings. See
    /// [`execute`](Self::execute).
    pub fn raw_execute(&mut self) -> Result<usize> {
        let stepped = self.raw.step();
        self.raw.reset();
        match stepped? {
            true => Err(Error::ExecuteReturnedResults),
            false => Ok(usize::try_from(self.conn.changes()).unwrap_or(usize::MAX)),
        }
    }

    /// Binds `params` and starts the query.
    pub fn query<P>(&mut self, params: P) -> Result<Rows<'_>>
    where
        P: Params,
    {
        params.bind_in(self)?;
        Ok(self.raw_query())
    }

    /// Starts the query with the current bindings.
    pub fn raw_query(&mut self) -> Rows<'_> {
        Rows::new(&mut self.raw)
    }

    /// Binds `params` and maps each row with `f`.
    pub fn query_map<T, P, F>(&mut self, params: P, f: F) -> Result<MappedRows<'_, F>>
    where
        P: Params,
        F: FnMut(&Row<'_>) -> Result<T>,
    {
        self.query(params).map(|rows| rows.mapped(f))
    }

    /// Binds `params` and maps the first row with `f`.
    ///
    /// Fails with [`Error::QueryReturnedNoRows`] when there is no row.
    pub fn query_row<T, P, F>(&mut self, params: P, f: F) -> Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        let mut rows = self.query(params)?;
        let row = rows.next()?.ok_or(Error::QueryReturnedNoRows)?;
        f(&row)
    }

    /// Returns whether the query yields at least one row.
    pub fn exists<P>(&mut self, params: P) -> Result<bool>
    where
        P: Params,
    {
        let mut rows = self.query(params)?;
        Ok(rows.next()?.is_some())
    }

    /// Number of result columns.
    pub fn column_count(&self) -> usize {
        self.raw.column_count()
    }

    /// Name of the result column at `index`.
    pub fn column_name(&self, index: usize) -> Result<&str> {
        self.raw
            .column_name(index)
            .ok_or(Error::InvalidColumnIndex(index))
    }

    /// Names of all result columns.
    pub fn column_names(&self) -> Vec<&str> {
        (0..self.column_count())
            .filter_map(|index| self.raw.column_name(index))
            .collect()
    }

    /// Index of the result column called `name`, ignoring ASCII case.
    pub fn column_index(&self, name: &str) -> Result<usize> {
        column_index(&self.raw, name)
    }

    /// Number of SQL parameters.
    pub fn parameter_count(&self) -> usize {
        self.raw.bind_parameter_count()
    }

    /// One-based index of the named parameter, including its prefix.
    pub fn parameter_index(&self, name: &str) -> Result<Option<usize>> {
        self.raw.bind_parameter_index(name)
    }

    /// Binds one parameter by its one-based index.
    pub fn raw_bind_parameter<T>(&mut self, one_based_index: usize, value: T) -> Result<()>
    where
        T: ToSql,
    {
        let value = value.to_sql()?;
        self.raw.bind(one_based_index, value.as_value_ref())
    }

    /// Sets every parameter back to NULL.
    pub fn clear_bindings(&mut self) {
        self.raw.clear_bindings();
    }

    pub(crate) fn bind_positional<I>(&mut self, params: I) -> Result<()>
    where
        I: IntoIterator,
        I::Item: ToSql,
    {
        let expected = self.parameter_count();
        let mut given = 0;
        for value in params {
            given += 1;
            if given > expected {
                break;
            }
            self.raw_bind_parameter(given, value)?;
        }
        if given == expected {
            Ok(())
        } else {
            Err(Error::InvalidParameterCount(given, expected))
        }
    }

    pub(crate) fn bind_named<T>(&mut self, params: &[(&str, T)]) -> Result<()>
    where
        T: ToSql,
    {
        for (name, value) in params {
            let index = self
                .parameter_index(name)?
                .ok_or_else(|| Error::InvalidParameterName((*name).to_owned()))?;
            self.raw_bind_parameter(index, value)?;
        }
        Ok(())
    }

    pub(crate) fn ensure_parameter_count(&self, given: usize) -> Result<()> {
        let expected = self.parameter_count();
        if given == expected {
            Ok(())
        } else {
            Err(Error::InvalidParameterCount(given, expected))
        }
    }
}

pub(crate) fn column_index(raw: &RawStatement, name: &str) -> Result<usize> {
    (0..raw.column_count())
        .find(|index| {
            raw.column_name(*index)
                .is_some_and(|column| column.eq_ignore_ascii_case(name))
        })
        .ok_or_else(|| Error::InvalidColumnName(name.to_owned()))
}

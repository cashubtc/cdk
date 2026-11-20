//! Query results.

use std::fmt;

use crate::error::{Error, Result};
use crate::raw::RawStatement;
use crate::statement::column_index;
use crate::types::{FromSql, FromSqlError, ValueRef};

/// The rows of a running query. Dropping it resets the statement.
pub struct Rows<'stmt> {
    raw: Option<&'stmt mut RawStatement>,
}

impl fmt::Debug for Rows<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rows")
            .field("finished", &self.raw.is_none())
            .finish()
    }
}

impl<'stmt> Rows<'stmt> {
    pub(crate) fn new(raw: &'stmt mut RawStatement) -> Self {
        Self { raw: Some(raw) }
    }

    /// Advances to the next row.
    ///
    /// The returned row borrows `self`, so it must be dropped before the next
    /// call. After `Ok(None)` or an error, every later call returns `Ok(None)`.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Row<'_>>> {
        let stepped = match self.raw.as_deref_mut() {
            Some(raw) => raw.step(),
            None => return Ok(None),
        };
        match stepped {
            Ok(true) => Ok(self.raw.as_deref().map(|raw| Row { raw })),
            Ok(false) => {
                self.finish();
                Ok(None)
            }
            Err(error) => {
                self.finish();
                Err(error)
            }
        }
    }

    /// Maps each row with `f`, as an iterator.
    pub fn mapped<T, F>(self, f: F) -> MappedRows<'stmt, F>
    where
        F: FnMut(&Row<'_>) -> Result<T>,
    {
        MappedRows { rows: self, f }
    }

    fn finish(&mut self) {
        if let Some(raw) = self.raw.take() {
            raw.reset();
        }
    }
}

impl Drop for Rows<'_> {
    fn drop(&mut self) {
        self.finish();
    }
}

/// An iterator that maps each row of a query.
pub struct MappedRows<'stmt, F> {
    rows: Rows<'stmt>,
    f: F,
}

impl<F> fmt::Debug for MappedRows<'_, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MappedRows")
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

impl<T, F> Iterator for MappedRows<'_, F>
where
    F: FnMut(&Row<'_>) -> Result<T>,
{
    type Item = Result<T>;

    fn next(&mut self) -> Option<Self::Item> {
        let f = &mut self.f;
        self.rows
            .next()
            .transpose()
            .map(|row| row.and_then(|row| f(&row)))
    }
}

/// The current row of a query.
pub struct Row<'stmt> {
    raw: &'stmt RawStatement,
}

impl fmt::Debug for Row<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Row")
            .field("column_count", &self.raw.column_count())
            .finish_non_exhaustive()
    }
}

impl Row<'_> {
    /// Converts a column to `T`.
    pub fn get<I, T>(&self, index: I) -> Result<T>
    where
        I: RowIndex,
        T: FromSql,
    {
        let index = index.index(self)?;
        let value = self.raw.column_value(index)?;
        T::column_result(value).map_err(|error| match error {
            FromSqlError::InvalidType => Error::InvalidColumnType(
                index,
                self.raw.column_name(index).unwrap_or_default().to_owned(),
                value.data_type(),
            ),
            error => Error::from_sql(index, value.data_type(), error),
        })
    }

    /// Borrows a column without converting it.
    pub fn get_ref<I>(&self, index: I) -> Result<ValueRef<'_>>
    where
        I: RowIndex,
    {
        let index = index.index(self)?;
        self.raw.column_value(index)
    }

    /// Number of columns in the row.
    pub fn column_count(&self) -> usize {
        self.raw.column_count()
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for usize {}
    impl Sealed for &str {}
}

/// A column position (`usize`) or name (`&str`).
pub trait RowIndex: sealed::Sealed {
    /// Resolves to a zero-based column index.
    fn index(&self, row: &Row<'_>) -> Result<usize>;
}

impl RowIndex for usize {
    fn index(&self, row: &Row<'_>) -> Result<usize> {
        if *self < row.column_count() {
            Ok(*self)
        } else {
            Err(Error::InvalidColumnIndex(*self))
        }
    }
}

impl RowIndex for &str {
    fn index(&self, row: &Row<'_>) -> Result<usize> {
        column_index(row.raw, self)
    }
}

//! Error types.

use std::error::Error as StdError;
use std::ffi::{c_int, NulError};
use std::fmt;
use std::path::PathBuf;
use std::str::Utf8Error;

use crate::ffi;
use crate::types::{FromSqlError, Type};

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Primary SQLite result code category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorCode {
    /// Internal logic error in SQLite.
    InternalMalfunction,
    /// Access permission denied.
    PermissionDenied,
    /// Callback routine requested an abort.
    OperationAborted,
    /// The database file is locked.
    DatabaseBusy,
    /// A table in the database is locked.
    DatabaseLocked,
    /// A memory allocation failed.
    OutOfMemory,
    /// Attempt to write a read-only database.
    ReadOnly,
    /// Operation terminated by `sqlite3_interrupt`.
    OperationInterrupted,
    /// Disk I/O error.
    SystemIoFailure,
    /// The database disk image is malformed.
    DatabaseCorrupt,
    /// Unknown opcode or file not found.
    NotFound,
    /// Insertion failed because the database is full.
    DiskFull,
    /// Unable to open the database file.
    CannotOpen,
    /// Database lock protocol error.
    FileLockingProtocolFailed,
    /// The database schema changed.
    SchemaChanged,
    /// String or blob exceeds the size limit.
    TooBig,
    /// Abort due to a constraint violation.
    ConstraintViolation,
    /// Data type mismatch.
    TypeMismatch,
    /// Library used incorrectly.
    ApiMisuse,
    /// Uses OS features not supported on the host.
    NoLargeFileSupport,
    /// Authorization denied.
    AuthorizationForStatementDenied,
    /// Bind parameter index out of range.
    ParameterOutOfRange,
    /// File opened that is not a database file.
    NotADatabase,
    /// Any other result code.
    Unknown,
}

impl ErrorCode {
    fn from_primary(code: c_int) -> Self {
        match code & 0xff {
            ffi::SQLITE_INTERNAL => Self::InternalMalfunction,
            ffi::SQLITE_PERM => Self::PermissionDenied,
            ffi::SQLITE_ABORT => Self::OperationAborted,
            ffi::SQLITE_BUSY => Self::DatabaseBusy,
            ffi::SQLITE_LOCKED => Self::DatabaseLocked,
            ffi::SQLITE_NOMEM => Self::OutOfMemory,
            ffi::SQLITE_READONLY => Self::ReadOnly,
            ffi::SQLITE_INTERRUPT => Self::OperationInterrupted,
            ffi::SQLITE_IOERR => Self::SystemIoFailure,
            ffi::SQLITE_CORRUPT => Self::DatabaseCorrupt,
            ffi::SQLITE_NOTFOUND => Self::NotFound,
            ffi::SQLITE_FULL => Self::DiskFull,
            ffi::SQLITE_CANTOPEN => Self::CannotOpen,
            ffi::SQLITE_PROTOCOL => Self::FileLockingProtocolFailed,
            ffi::SQLITE_SCHEMA => Self::SchemaChanged,
            ffi::SQLITE_TOOBIG => Self::TooBig,
            ffi::SQLITE_CONSTRAINT => Self::ConstraintViolation,
            ffi::SQLITE_MISMATCH => Self::TypeMismatch,
            ffi::SQLITE_MISUSE => Self::ApiMisuse,
            ffi::SQLITE_NOLFS => Self::NoLargeFileSupport,
            ffi::SQLITE_AUTH => Self::AuthorizationForStatementDenied,
            ffi::SQLITE_RANGE => Self::ParameterOutOfRange,
            ffi::SQLITE_NOTADB => Self::NotADatabase,
            _ => Self::Unknown,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::InternalMalfunction => "internal logic error in SQLite",
            Self::PermissionDenied => "access permission denied",
            Self::OperationAborted => "callback requested query abort",
            Self::DatabaseBusy => "database is locked",
            Self::DatabaseLocked => "database table is locked",
            Self::OutOfMemory => "out of memory",
            Self::ReadOnly => "attempt to write a readonly database",
            Self::OperationInterrupted => "interrupted",
            Self::SystemIoFailure => "disk I/O error",
            Self::DatabaseCorrupt => "database disk image is malformed",
            Self::NotFound => "unknown operation",
            Self::DiskFull => "database or disk is full",
            Self::CannotOpen => "unable to open database file",
            Self::FileLockingProtocolFailed => "locking protocol",
            Self::SchemaChanged => "database schema has changed",
            Self::TooBig => "string or blob too big",
            Self::ConstraintViolation => "constraint failed",
            Self::TypeMismatch => "datatype mismatch",
            Self::ApiMisuse => "bad parameter or other API misuse",
            Self::NoLargeFileSupport => "large file support is disabled",
            Self::AuthorizationForStatementDenied => "authorization denied",
            Self::ParameterOutOfRange => "column index out of range",
            Self::NotADatabase => "file is not a database",
            Self::Unknown => "unknown error",
        }
    }
}

/// A SQLite result code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SqliteError {
    /// The primary result code category.
    pub code: ErrorCode,
    /// The extended result code, such as `ffi::SQLITE_CONSTRAINT_UNIQUE`.
    pub extended_code: c_int,
}

impl SqliteError {
    /// Builds an error from a primary or extended SQLite result code.
    pub fn new(result_code: c_int) -> Self {
        Self {
            code: ErrorCode::from_primary(result_code),
            extended_code: result_code,
        }
    }
}

impl fmt::Display for SqliteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (code {})",
            self.code.description(),
            self.extended_code
        )
    }
}

impl StdError for SqliteError {}

/// Errors returned by this crate.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// SQLite returned an error code, with its message when one was available.
    SqliteFailure(SqliteError, Option<String>),
    /// SQLite was compiled without thread safety.
    SqliteSingleThreadedMode,
    /// A [`FromSql`](crate::types::FromSql) conversion failed.
    FromSqlConversionFailure(usize, Type, Box<dyn StdError + Send + Sync + 'static>),
    /// An integer column did not fit in the requested type.
    IntegralValueOutOfRange(usize, i64),
    /// Text was not valid UTF-8.
    Utf8Error(Utf8Error),
    /// A string passed to SQLite contained a NUL byte.
    NulError(NulError),
    /// A named parameter does not exist in the statement.
    InvalidParameterName(String),
    /// A database path could not be used.
    InvalidPath(PathBuf),
    /// [`execute`](crate::Statement::execute) was used on a statement that
    /// returned rows.
    ExecuteReturnedResults,
    /// A single-row query returned no rows.
    QueryReturnedNoRows,
    /// A column index was out of range.
    InvalidColumnIndex(usize),
    /// A column name does not exist in the result.
    InvalidColumnName(String),
    /// A column's type could not be converted to the requested Rust type.
    InvalidColumnType(usize, String, Type),
    /// Wrong number of positional parameters: `(given, expected)`.
    InvalidParameterCount(usize, usize),
    /// A [`ToSql`](crate::ToSql) conversion failed.
    ToSqlConversionFailure(Box<dyn StdError + Send + Sync + 'static>),
    /// The SQL passed to `prepare` contained more than one statement.
    MultipleStatement,
}

impl Error {
    /// The primary SQLite error category, when this is a SQLite failure.
    pub fn sqlite_error_code(&self) -> Option<ErrorCode> {
        match self {
            Self::SqliteFailure(error, _) => Some(error.code),
            _ => None,
        }
    }

    /// The SQLite result code, when this is a SQLite failure.
    pub fn sqlite_error(&self) -> Option<&SqliteError> {
        match self {
            Self::SqliteFailure(error, _) => Some(error),
            _ => None,
        }
    }

    pub(crate) fn from_sql(column: usize, data_type: Type, error: FromSqlError) -> Self {
        match error {
            FromSqlError::OutOfRange(value) => Self::IntegralValueOutOfRange(column, value),
            FromSqlError::Other(error) => Self::FromSqlConversionFailure(column, data_type, error),
            error => Self::FromSqlConversionFailure(column, data_type, Box::new(error)),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SqliteFailure(error, Some(message)) => write!(f, "{message} ({error})"),
            Self::SqliteFailure(error, None) => error.fmt(f),
            Self::SqliteSingleThreadedMode => {
                f.write_str("SQLite was compiled without thread safety")
            }
            Self::FromSqlConversionFailure(column, data_type, error) => {
                write!(f, "cannot convert {data_type} column {column}: {error}")
            }
            Self::IntegralValueOutOfRange(column, value) => {
                write!(f, "integer {value} in column {column} is out of range")
            }
            Self::Utf8Error(error) => error.fmt(f),
            Self::NulError(error) => error.fmt(f),
            Self::InvalidParameterName(name) => write!(f, "invalid parameter name: {name}"),
            Self::InvalidPath(path) => write!(f, "invalid database path: {}", path.display()),
            Self::ExecuteReturnedResults => f.write_str("execute returned results"),
            Self::QueryReturnedNoRows => f.write_str("query returned no rows"),
            Self::InvalidColumnIndex(column) => write!(f, "invalid column index: {column}"),
            Self::InvalidColumnName(name) => write!(f, "invalid column name: {name}"),
            Self::InvalidColumnType(column, name, data_type) => write!(
                f,
                "invalid column type {data_type} at index {column}, name {name}"
            ),
            Self::InvalidParameterCount(given, expected) => write!(
                f,
                "wrong number of parameters: {given} given, {expected} expected"
            ),
            Self::ToSqlConversionFailure(error) => error.fmt(f),
            Self::MultipleStatement => f.write_str("multiple statements provided"),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::SqliteFailure(error, _) => Some(error),
            Self::Utf8Error(error) => Some(error),
            Self::NulError(error) => Some(error),
            Self::FromSqlConversionFailure(_, _, error) | Self::ToSqlConversionFailure(error) => {
                Some(&**error)
            }
            _ => None,
        }
    }
}

impl From<Utf8Error> for Error {
    fn from(error: Utf8Error) -> Self {
        Self::Utf8Error(error)
    }
}

impl From<NulError> for Error {
    fn from(error: NulError) -> Self {
        Self::NulError(error)
    }
}

/// Turns [`Error::QueryReturnedNoRows`] into `Ok(None)`.
pub trait OptionalExtension<T> {
    /// Returns `Ok(None)` when the query returned no rows.
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

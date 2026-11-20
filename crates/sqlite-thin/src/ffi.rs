//! SQLite constants, as defined in `sqlite3.h`.
//!
//! These values are part of SQLite's stable ABI and never change. A test checks
//! them against the `libsqlite3-sys` in the build.

use std::ffi::c_int;

/// Successful result.
pub const SQLITE_OK: c_int = 0;
/// Generic error.
pub const SQLITE_ERROR: c_int = 1;
/// Internal logic error in SQLite.
pub const SQLITE_INTERNAL: c_int = 2;
/// Access permission denied.
pub const SQLITE_PERM: c_int = 3;
/// Callback routine requested an abort.
pub const SQLITE_ABORT: c_int = 4;
/// The database file is locked.
pub const SQLITE_BUSY: c_int = 5;
/// A table in the database is locked.
pub const SQLITE_LOCKED: c_int = 6;
/// A memory allocation failed.
pub const SQLITE_NOMEM: c_int = 7;
/// Attempt to write a read-only database.
pub const SQLITE_READONLY: c_int = 8;
/// Operation terminated by `sqlite3_interrupt`.
pub const SQLITE_INTERRUPT: c_int = 9;
/// Disk I/O error.
pub const SQLITE_IOERR: c_int = 10;
/// The database disk image is malformed.
pub const SQLITE_CORRUPT: c_int = 11;
/// Unknown opcode or file not found.
pub const SQLITE_NOTFOUND: c_int = 12;
/// The database is full.
pub const SQLITE_FULL: c_int = 13;
/// Unable to open the database file.
pub const SQLITE_CANTOPEN: c_int = 14;
/// Database lock protocol error.
pub const SQLITE_PROTOCOL: c_int = 15;
/// The database schema changed.
pub const SQLITE_SCHEMA: c_int = 17;
/// String or blob exceeds the size limit.
pub const SQLITE_TOOBIG: c_int = 18;
/// Abort due to a constraint violation.
pub const SQLITE_CONSTRAINT: c_int = 19;
/// Data type mismatch.
pub const SQLITE_MISMATCH: c_int = 20;
/// Library used incorrectly.
pub const SQLITE_MISUSE: c_int = 21;
/// Uses OS features not supported on the host.
pub const SQLITE_NOLFS: c_int = 22;
/// Authorization denied.
pub const SQLITE_AUTH: c_int = 23;
/// Bind parameter index out of range.
pub const SQLITE_RANGE: c_int = 25;
/// File opened that is not a database file.
pub const SQLITE_NOTADB: c_int = 26;
/// `sqlite3_step` has another row ready.
pub const SQLITE_ROW: c_int = 100;
/// `sqlite3_step` has finished executing.
pub const SQLITE_DONE: c_int = 101;

/// Busy: another connection is recovering the WAL.
pub const SQLITE_BUSY_RECOVERY: c_int = SQLITE_BUSY | (1 << 8);
/// Busy: a WAL snapshot is stale.
pub const SQLITE_BUSY_SNAPSHOT: c_int = SQLITE_BUSY | (2 << 8);
/// Busy: a blocking lock timed out.
pub const SQLITE_BUSY_TIMEOUT: c_int = SQLITE_BUSY | (3 << 8);

/// CHECK constraint failed.
pub const SQLITE_CONSTRAINT_CHECK: c_int = SQLITE_CONSTRAINT | (1 << 8);
/// Commit hook caused a rollback.
pub const SQLITE_CONSTRAINT_COMMITHOOK: c_int = SQLITE_CONSTRAINT | (2 << 8);
/// FOREIGN KEY constraint failed.
pub const SQLITE_CONSTRAINT_FOREIGNKEY: c_int = SQLITE_CONSTRAINT | (3 << 8);
/// Constraint raised by a function.
pub const SQLITE_CONSTRAINT_FUNCTION: c_int = SQLITE_CONSTRAINT | (4 << 8);
/// NOT NULL constraint failed.
pub const SQLITE_CONSTRAINT_NOTNULL: c_int = SQLITE_CONSTRAINT | (5 << 8);
/// PRIMARY KEY constraint failed.
pub const SQLITE_CONSTRAINT_PRIMARYKEY: c_int = SQLITE_CONSTRAINT | (6 << 8);
/// RAISE in a trigger.
pub const SQLITE_CONSTRAINT_TRIGGER: c_int = SQLITE_CONSTRAINT | (7 << 8);
/// UNIQUE constraint failed.
pub const SQLITE_CONSTRAINT_UNIQUE: c_int = SQLITE_CONSTRAINT | (8 << 8);
/// Constraint raised by a virtual table.
pub const SQLITE_CONSTRAINT_VTAB: c_int = SQLITE_CONSTRAINT | (9 << 8);
/// Rowid is not unique.
pub const SQLITE_CONSTRAINT_ROWID: c_int = SQLITE_CONSTRAINT | (10 << 8);

/// Open read-only.
pub const SQLITE_OPEN_READONLY: c_int = 0x0000_0001;
/// Open for reading and writing.
pub const SQLITE_OPEN_READWRITE: c_int = 0x0000_0002;
/// Create the database if missing.
pub const SQLITE_OPEN_CREATE: c_int = 0x0000_0004;
/// Interpret the filename as a URI.
pub const SQLITE_OPEN_URI: c_int = 0x0000_0040;
/// Open an in-memory database.
pub const SQLITE_OPEN_MEMORY: c_int = 0x0000_0080;
/// Multi-thread mode.
pub const SQLITE_OPEN_NOMUTEX: c_int = 0x0000_8000;
/// Serialized mode.
pub const SQLITE_OPEN_FULLMUTEX: c_int = 0x0001_0000;
/// Use the shared cache.
pub const SQLITE_OPEN_SHAREDCACHE: c_int = 0x0002_0000;
/// Use a private cache.
pub const SQLITE_OPEN_PRIVATECACHE: c_int = 0x0004_0000;

/// INTEGER storage class.
pub const SQLITE_INTEGER: c_int = 1;
/// REAL storage class.
pub const SQLITE_FLOAT: c_int = 2;
/// TEXT storage class.
pub const SQLITE_TEXT: c_int = 3;
/// BLOB storage class.
pub const SQLITE_BLOB: c_int = 4;
/// NULL storage class.
pub const SQLITE_NULL: c_int = 5;

/// UTF-8 text encoding.
pub const SQLITE_UTF8: c_int = 1;

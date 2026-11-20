//! The only module that calls into SQLite.
//!
//! Invariants upheld by the rest of the crate:
//!
//! - Every [`RawStatement`] is finalized before the [`RawConnection`] that
//!   prepared it is closed. `Statement<'conn>` borrows the connection, and
//!   `Connection` drops its statement cache before its `RawConnection`.
//!   `sqlite3_close` refuses to close a connection with live statements, so a
//!   mistake there leaks the connection rather than freeing memory in use.
//! - Neither type is `Sync`. A connection and its statements are used by one
//!   thread at a time, which SQLite's multi-thread mode permits.
//!
//! Pointer lifetimes inside a statement are enforced by the borrow checker
//! here: stepping, resetting and binding take `&mut self`, and column values
//! borrow `&self`, so no column value can outlive the row it was read from.
#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_uchar, CStr, CString};
use std::ptr::{self, NonNull};
use std::slice;

use crate::error::{Error, Result, SqliteError};
use crate::types::ValueRef;
use crate::{ffi, sys};

fn sqlite_failure(db: *mut sys::Sqlite3, result_code: c_int) -> Error {
    let message = if db.is_null() {
        None
    } else {
        // SAFETY: `db` is a live connection handle; `sqlite3_errmsg` returns a
        // NUL-terminated string owned by SQLite that stays valid until the next
        // API call on this connection, and we copy it out immediately.
        let message = unsafe { sys::sqlite3_errmsg(db) };
        if message.is_null() {
            None
        } else {
            // SAFETY: non-null and NUL-terminated, as above.
            Some(
                unsafe { CStr::from_ptr(message) }
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    };
    Error::SqliteFailure(SqliteError::new(result_code), message)
}

/// An open `sqlite3` connection handle.
pub(crate) struct RawConnection {
    db: NonNull<sys::Sqlite3>,
}

// SAFETY: `open` rejects SQLite builds without thread safety. In multi-thread
// mode a connection may move between threads as long as it is not used by two
// threads at once, which holds because `RawConnection` is not `Sync`.
unsafe impl Send for RawConnection {}

impl RawConnection {
    pub(crate) fn open(path: &CStr, flags: c_int) -> Result<Self> {
        // SAFETY: takes no arguments and only reads a compile-time setting.
        if unsafe { sys::sqlite3_threadsafe() } == 0 {
            return Err(Error::SqliteSingleThreadedMode);
        }
        let mut db = ptr::null_mut();
        // SAFETY: `path` is NUL-terminated, `db` is a valid out-pointer and a
        // null VFS name selects the default VFS.
        let rc = unsafe { sys::sqlite3_open_v2(path.as_ptr(), &mut db, flags, ptr::null()) };
        let Some(db) = NonNull::new(db) else {
            // SQLite could not even allocate the handle.
            return Err(Error::SqliteFailure(SqliteError::new(rc), None));
        };
        // From here on `Drop` closes the handle, including on the error path.
        let connection = Self { db };
        if rc != ffi::SQLITE_OK {
            // SAFETY: `db` is a handle returned by `sqlite3_open_v2`, which
            // remains valid for error queries even when opening failed.
            let extended = unsafe { sys::sqlite3_extended_errcode(db.as_ptr()) };
            return Err(sqlite_failure(db.as_ptr(), extended));
        }
        // SAFETY: `db` is an open connection.
        unsafe { sys::sqlite3_extended_result_codes(db.as_ptr(), 1) };
        Ok(connection)
    }

    fn check(&self, rc: c_int) -> Result<()> {
        if rc == ffi::SQLITE_OK {
            Ok(())
        } else {
            Err(sqlite_failure(self.db.as_ptr(), rc))
        }
    }

    /// Prepares the first statement in `sql`.
    ///
    /// Returns `None` when `sql` holds only whitespace or comments, along with
    /// the byte offset where the unparsed remainder begins.
    pub(crate) fn prepare(&self, sql: &str) -> Result<(Option<RawStatement>, usize)> {
        if sql.is_empty() {
            return Ok((None, 0));
        }
        let len = c_int::try_from(sql.len())
            .map_err(|_| Error::SqliteFailure(SqliteError::new(ffi::SQLITE_TOOBIG), None))?;
        let mut stmt = ptr::null_mut();
        let mut tail: *const c_char = ptr::null();
        // SAFETY: `sql` points to `len` readable bytes (no NUL terminator is
        // needed when the length is given), and both out-pointers are valid.
        let rc = unsafe {
            sys::sqlite3_prepare_v2(
                self.db.as_ptr(),
                sql.as_ptr().cast::<c_char>(),
                len,
                &mut stmt,
                &mut tail,
            )
        };
        self.check(rc)?;
        // `tail` points into `sql`, just past the parsed statement. Compare
        // addresses only; nothing is dereferenced.
        let consumed = if tail.is_null() {
            sql.len()
        } else {
            (tail as usize)
                .saturating_sub(sql.as_ptr() as usize)
                .min(sql.len())
        };
        Ok((
            NonNull::new(stmt).map(|stmt| RawStatement { stmt }),
            consumed,
        ))
    }

    pub(crate) fn busy_timeout(&self, milliseconds: c_int) -> Result<()> {
        // SAFETY: `self.db` is an open connection.
        let rc = unsafe { sys::sqlite3_busy_timeout(self.db.as_ptr(), milliseconds) };
        self.check(rc)
    }

    pub(crate) fn is_autocommit(&self) -> bool {
        // SAFETY: `self.db` is an open connection.
        unsafe { sys::sqlite3_get_autocommit(self.db.as_ptr()) != 0 }
    }

    pub(crate) fn changes(&self) -> u64 {
        // SAFETY: `self.db` is an open connection.
        let changes = unsafe { sys::sqlite3_changes(self.db.as_ptr()) };
        u64::try_from(changes).unwrap_or_default()
    }

    pub(crate) fn last_insert_rowid(&self) -> i64 {
        // SAFETY: `self.db` is an open connection.
        unsafe { sys::sqlite3_last_insert_rowid(self.db.as_ptr()) }
    }
}

impl Drop for RawConnection {
    fn drop(&mut self) {
        // SAFETY: `self.db` is open and is never used again. All statements
        // were finalized first (see the module invariants); if one were not,
        // `sqlite3_close` returns SQLITE_BUSY and leaves the handle open.
        unsafe { sys::sqlite3_close(self.db.as_ptr()) };
    }
}

/// A prepared `sqlite3_stmt`.
pub(crate) struct RawStatement {
    stmt: NonNull<sys::Sqlite3Stmt>,
}

// SAFETY: a statement is only used through its owning connection, which is
// `Send` but not `Sync`; see `RawConnection`. `RawStatement` is not `Sync`.
unsafe impl Send for RawStatement {}

impl RawStatement {
    fn db(&self) -> *mut sys::Sqlite3 {
        // SAFETY: `self.stmt` is a live statement.
        unsafe { sys::sqlite3_db_handle(self.stmt.as_ptr()) }
    }

    fn check(&self, rc: c_int) -> Result<()> {
        if rc == ffi::SQLITE_OK {
            Ok(())
        } else {
            Err(sqlite_failure(self.db(), rc))
        }
    }

    /// Advances to the next row. Returns `true` when a row is available.
    pub(crate) fn step(&mut self) -> Result<bool> {
        // SAFETY: `self.stmt` is a live statement; `&mut self` guarantees no
        // column value borrowed from the previous row is still alive.
        match unsafe { sys::sqlite3_step(self.stmt.as_ptr()) } {
            ffi::SQLITE_ROW => Ok(true),
            ffi::SQLITE_DONE => Ok(false),
            rc => Err(sqlite_failure(self.db(), rc)),
        }
    }

    /// Resets the statement so it can run again. Bindings are kept.
    pub(crate) fn reset(&mut self) {
        // SAFETY: `self.stmt` is a live statement. The return value repeats
        // the error of the last step, which was already reported.
        unsafe { sys::sqlite3_reset(self.stmt.as_ptr()) };
    }

    /// Sets every bound parameter back to NULL.
    pub(crate) fn clear_bindings(&mut self) {
        // SAFETY: `self.stmt` is a live statement.
        unsafe { sys::sqlite3_clear_bindings(self.stmt.as_ptr()) };
    }

    pub(crate) fn bind_parameter_count(&self) -> usize {
        // SAFETY: `self.stmt` is a live statement.
        let count = unsafe { sys::sqlite3_bind_parameter_count(self.stmt.as_ptr()) };
        usize::try_from(count).unwrap_or_default()
    }

    /// Returns the one-based index of a named parameter.
    pub(crate) fn bind_parameter_index(&self, name: &str) -> Result<Option<usize>> {
        let name = CString::new(name)?;
        // SAFETY: `self.stmt` is a live statement and `name` is NUL-terminated.
        let index = unsafe { sys::sqlite3_bind_parameter_index(self.stmt.as_ptr(), name.as_ptr()) };
        Ok(usize::try_from(index).ok().filter(|index| *index > 0))
    }

    /// Binds `value` to the one-based parameter `index`. SQLite copies text
    /// and blobs, so `value` need not outlive the call.
    pub(crate) fn bind(&mut self, index: usize, value: ValueRef<'_>) -> Result<()> {
        let index = c_int::try_from(index)
            .map_err(|_| Error::SqliteFailure(SqliteError::new(ffi::SQLITE_RANGE), None))?;
        let stmt = self.stmt.as_ptr();
        let too_big = || Error::SqliteFailure(SqliteError::new(ffi::SQLITE_TOOBIG), None);
        let rc = match value {
            ValueRef::Null => {
                // SAFETY: `stmt` is a live statement; SQLite validates `index`.
                unsafe { sys::sqlite3_bind_null(stmt, index) }
            }
            ValueRef::Integer(value) => {
                // SAFETY: as for `Null`.
                unsafe { sys::sqlite3_bind_int64(stmt, index, value) }
            }
            ValueRef::Real(value) => {
                // SAFETY: as for `Null`.
                unsafe { sys::sqlite3_bind_double(stmt, index, value) }
            }
            ValueRef::Text([]) => {
                // SAFETY: as for `Null`; a static empty C string needs no copy.
                unsafe { sys::sqlite3_bind_text(stmt, index, c"".as_ptr(), 0, sys::SQLITE_STATIC) }
            }
            ValueRef::Text(text) => {
                let len = u64::try_from(text.len()).map_err(|_| too_big())?;
                // SAFETY: as for `Null`; `text` is readable for `len` bytes and
                // SQLITE_TRANSIENT makes SQLite copy it before returning.
                unsafe {
                    sys::sqlite3_bind_text64(
                        stmt,
                        index,
                        text.as_ptr().cast::<c_char>(),
                        len,
                        sys::SQLITE_TRANSIENT,
                        ffi::SQLITE_UTF8 as c_uchar,
                    )
                }
            }
            ValueRef::Blob([]) => {
                // A null blob pointer would bind NULL, so bind a zero-length
                // blob explicitly.
                // SAFETY: as for `Null`.
                unsafe { sys::sqlite3_bind_zeroblob(stmt, index, 0) }
            }
            ValueRef::Blob(blob) => {
                let len = u64::try_from(blob.len()).map_err(|_| too_big())?;
                // SAFETY: as for `Null`; `blob` is readable for `len` bytes and
                // SQLITE_TRANSIENT makes SQLite copy it before returning.
                unsafe {
                    sys::sqlite3_bind_blob64(
                        stmt,
                        index,
                        blob.as_ptr().cast(),
                        len,
                        sys::SQLITE_TRANSIENT,
                    )
                }
            }
        };
        self.check(rc)
    }

    pub(crate) fn column_count(&self) -> usize {
        // SAFETY: `self.stmt` is a live statement.
        let count = unsafe { sys::sqlite3_column_count(self.stmt.as_ptr()) };
        usize::try_from(count).unwrap_or_default()
    }

    /// The name of a result column, or `None` when out of range.
    pub(crate) fn column_name(&self, index: usize) -> Option<&str> {
        if index >= self.column_count() {
            return None;
        }
        let index = c_int::try_from(index).ok()?;
        // SAFETY: `self.stmt` is a live statement and `index` is in range.
        let name = unsafe { sys::sqlite3_column_name(self.stmt.as_ptr(), index) };
        if name.is_null() {
            return None;
        }
        // SAFETY: the name is NUL-terminated and stays valid until the
        // statement is finalized or re-prepared. Re-preparing only happens in
        // `step`, which needs `&mut self` and so cannot overlap this borrow.
        unsafe { CStr::from_ptr(name) }.to_str().ok()
    }

    /// Reads a column of the current row.
    ///
    /// Callers must only use this after `step` returned `true`; `Row` is the
    /// only caller and only exists in that state.
    pub(crate) fn column_value(&self, index: usize) -> Result<ValueRef<'_>> {
        if index >= self.column_count() {
            return Err(Error::InvalidColumnIndex(index));
        }
        let column = c_int::try_from(index).map_err(|_| Error::InvalidColumnIndex(index))?;
        let stmt = self.stmt.as_ptr();
        // SAFETY: `stmt` is live, positioned on a row, and `column` is in range.
        let value = match unsafe { sys::sqlite3_column_type(stmt, column) } {
            ffi::SQLITE_INTEGER => {
                // SAFETY: as above.
                let value = unsafe { sys::sqlite3_column_int64(stmt, column) };
                ValueRef::Integer(value)
            }
            ffi::SQLITE_FLOAT => {
                // SAFETY: as above.
                let value = unsafe { sys::sqlite3_column_double(stmt, column) };
                ValueRef::Real(value)
            }
            ffi::SQLITE_TEXT => {
                // Read the pointer before the length, as SQLite documents.
                // SAFETY: as above.
                let text = unsafe { sys::sqlite3_column_text(stmt, column) };
                if text.is_null() {
                    // Text columns always have a buffer, so this is an
                    // allocation failure.
                    return Err(sqlite_failure(self.db(), ffi::SQLITE_NOMEM));
                }
                // SAFETY: as above.
                let len = unsafe { sys::sqlite3_column_bytes(stmt, column) };
                let len = usize::try_from(len).unwrap_or_default();
                // SAFETY: SQLite guarantees `len` readable bytes at `text`
                // until the next step, reset or finalize, all of which need
                // `&mut self`. This crate never asks SQLite to convert a
                // column's type, which would also invalidate the buffer.
                ValueRef::Text(unsafe { slice::from_raw_parts(text, len) })
            }
            ffi::SQLITE_BLOB => {
                // SAFETY: as above.
                let blob = unsafe { sys::sqlite3_column_blob(stmt, column) };
                // SAFETY: as above.
                let len = unsafe { sys::sqlite3_column_bytes(stmt, column) };
                let len = usize::try_from(len).unwrap_or_default();
                if len == 0 {
                    // Zero-length blobs come back as a null pointer.
                    ValueRef::Blob(&[])
                } else if blob.is_null() {
                    return Err(sqlite_failure(self.db(), ffi::SQLITE_NOMEM));
                } else {
                    // SAFETY: as for text above.
                    ValueRef::Blob(unsafe { slice::from_raw_parts(blob.cast::<u8>(), len) })
                }
            }
            _ => ValueRef::Null,
        };
        Ok(value)
    }
}

impl Drop for RawStatement {
    fn drop(&mut self) {
        // SAFETY: `self.stmt` is live and never used again.
        unsafe { sys::sqlite3_finalize(self.stmt.as_ptr()) };
    }
}

//! Hand-written declarations of the SQLite C functions this crate calls.
//!
//! Every function here has been part of SQLite's stable C API since 3.8.7
//! (2014). Declaring them here, rather than using `libsqlite3-sys`'s generated
//! bindings, means changes to that crate's Rust API never affect this one;
//! `libsqlite3-sys` is only used to build and link the library itself.
//!
//! Signatures follow `sqlite3.h`. Destructor arguments are declared as data
//! pointers: SQLite only compares them against the sentinel values
//! [`SQLITE_STATIC`] and [`SQLITE_TRANSIENT`] and never calls them, and data
//! and function pointers share a representation on every supported target.
#![allow(unsafe_code)]

use std::ffi::{c_char, c_double, c_int, c_uchar, c_void};
use std::marker::{PhantomData, PhantomPinned};

/// Opaque `sqlite3` connection handle.
#[repr(C)]
pub(crate) struct Sqlite3 {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// Opaque `sqlite3_stmt` prepared statement handle.
#[repr(C)]
pub(crate) struct Sqlite3Stmt {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// `void (*)(void *)` destructor argument; see the module docs.
pub(crate) type Destructor = *const c_void;

/// The buffer outlives the binding, so SQLite may use it without copying.
pub(crate) const SQLITE_STATIC: Destructor = std::ptr::null();

/// SQLite copies the buffer before the bind call returns.
pub(crate) const SQLITE_TRANSIENT: Destructor = std::ptr::without_provenance(usize::MAX);

unsafe extern "C" {
    pub(crate) fn sqlite3_threadsafe() -> c_int;

    pub(crate) fn sqlite3_open_v2(
        filename: *const c_char,
        db: *mut *mut Sqlite3,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    pub(crate) fn sqlite3_close(db: *mut Sqlite3) -> c_int;

    pub(crate) fn sqlite3_errmsg(db: *mut Sqlite3) -> *const c_char;
    pub(crate) fn sqlite3_extended_errcode(db: *mut Sqlite3) -> c_int;
    pub(crate) fn sqlite3_extended_result_codes(db: *mut Sqlite3, onoff: c_int) -> c_int;

    pub(crate) fn sqlite3_busy_timeout(db: *mut Sqlite3, ms: c_int) -> c_int;
    pub(crate) fn sqlite3_get_autocommit(db: *mut Sqlite3) -> c_int;
    pub(crate) fn sqlite3_changes(db: *mut Sqlite3) -> c_int;
    pub(crate) fn sqlite3_last_insert_rowid(db: *mut Sqlite3) -> i64;

    pub(crate) fn sqlite3_prepare_v2(
        db: *mut Sqlite3,
        sql: *const c_char,
        n_byte: c_int,
        stmt: *mut *mut Sqlite3Stmt,
        tail: *mut *const c_char,
    ) -> c_int;
    pub(crate) fn sqlite3_db_handle(stmt: *mut Sqlite3Stmt) -> *mut Sqlite3;
    pub(crate) fn sqlite3_step(stmt: *mut Sqlite3Stmt) -> c_int;
    pub(crate) fn sqlite3_reset(stmt: *mut Sqlite3Stmt) -> c_int;
    pub(crate) fn sqlite3_clear_bindings(stmt: *mut Sqlite3Stmt) -> c_int;
    pub(crate) fn sqlite3_finalize(stmt: *mut Sqlite3Stmt) -> c_int;

    pub(crate) fn sqlite3_bind_parameter_count(stmt: *mut Sqlite3Stmt) -> c_int;
    pub(crate) fn sqlite3_bind_parameter_index(
        stmt: *mut Sqlite3Stmt,
        name: *const c_char,
    ) -> c_int;
    pub(crate) fn sqlite3_bind_null(stmt: *mut Sqlite3Stmt, index: c_int) -> c_int;
    pub(crate) fn sqlite3_bind_int64(stmt: *mut Sqlite3Stmt, index: c_int, value: i64) -> c_int;
    pub(crate) fn sqlite3_bind_double(
        stmt: *mut Sqlite3Stmt,
        index: c_int,
        value: c_double,
    ) -> c_int;
    pub(crate) fn sqlite3_bind_text(
        stmt: *mut Sqlite3Stmt,
        index: c_int,
        value: *const c_char,
        n_byte: c_int,
        destructor: Destructor,
    ) -> c_int;
    pub(crate) fn sqlite3_bind_text64(
        stmt: *mut Sqlite3Stmt,
        index: c_int,
        value: *const c_char,
        n_byte: u64,
        destructor: Destructor,
        encoding: c_uchar,
    ) -> c_int;
    pub(crate) fn sqlite3_bind_blob64(
        stmt: *mut Sqlite3Stmt,
        index: c_int,
        value: *const c_void,
        n_byte: u64,
        destructor: Destructor,
    ) -> c_int;
    pub(crate) fn sqlite3_bind_zeroblob(stmt: *mut Sqlite3Stmt, index: c_int, n: c_int) -> c_int;

    pub(crate) fn sqlite3_column_count(stmt: *mut Sqlite3Stmt) -> c_int;
    pub(crate) fn sqlite3_column_name(stmt: *mut Sqlite3Stmt, column: c_int) -> *const c_char;
    pub(crate) fn sqlite3_column_type(stmt: *mut Sqlite3Stmt, column: c_int) -> c_int;
    pub(crate) fn sqlite3_column_int64(stmt: *mut Sqlite3Stmt, column: c_int) -> i64;
    pub(crate) fn sqlite3_column_double(stmt: *mut Sqlite3Stmt, column: c_int) -> c_double;
    pub(crate) fn sqlite3_column_text(stmt: *mut Sqlite3Stmt, column: c_int) -> *const c_uchar;
    pub(crate) fn sqlite3_column_blob(stmt: *mut Sqlite3Stmt, column: c_int) -> *const c_void;
    pub(crate) fn sqlite3_column_bytes(stmt: *mut Sqlite3Stmt, column: c_int) -> c_int;
}

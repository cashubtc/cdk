use std::time::Duration;

use crate::types::{Type, Value, ValueRef};
use crate::{
    ffi, params_from_iter, Connection, DatabaseName, Error, ErrorCode, OptionalExtension, Result,
    TransactionBehavior,
};

fn memory() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory database");
    conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v)")
        .expect("create table");
    conn
}

fn stored<T>(conn: &Connection, value: T) -> Value
where
    T: crate::ToSql,
{
    conn.execute("DELETE FROM t", []).expect("clear");
    conn.execute("INSERT INTO t (id, v) VALUES (1, ?1)", [value])
        .expect("insert");
    conn.query_row("SELECT v FROM t WHERE id = 1", [], |row| row.get(0))
        .expect("select")
}

#[test]
fn values_round_trip() {
    let conn = memory();
    assert_eq!(stored(&conn, Value::Null), Value::Null);
    assert_eq!(stored(&conn, i64::MIN), Value::Integer(i64::MIN));
    assert_eq!(stored(&conn, i64::MAX), Value::Integer(i64::MAX));
    assert_eq!(stored(&conn, 1.5f64), Value::Real(1.5));
    assert_eq!(stored(&conn, "héllo"), Value::Text("héllo".to_owned()));
    assert_eq!(stored(&conn, "a\0b"), Value::Text("a\0b".to_owned()));
    assert_eq!(
        stored(&conn, vec![0u8, 1, 255]),
        Value::Blob(vec![0, 1, 255])
    );
    assert_eq!(stored(&conn, true), Value::Integer(1));
    assert_eq!(stored(&conn, None::<i64>), Value::Null);
    let big = vec![7u8; 1 << 20];
    assert_eq!(stored(&conn, big.clone()), Value::Blob(big));
}

#[test]
fn empty_text_and_blob_are_not_null() {
    let conn = memory();
    assert_eq!(stored(&conn, ""), Value::Text(String::new()));
    assert_eq!(stored(&conn, Vec::<u8>::new()), Value::Blob(Vec::new()));
    let kind: String = conn
        .query_row("SELECT typeof(v) FROM t", [], |row| row.get(0))
        .expect("typeof");
    assert_eq!(kind, "blob");
}

#[test]
fn typed_reads_and_conversion_errors() {
    let conn = memory();
    conn.execute("INSERT INTO t (id, v) VALUES (1, 300)", [])
        .expect("insert");
    let row = |sql: &str| -> Result<u8> { conn.query_row(sql, [], |row| row.get(0)) };
    assert!(matches!(
        row("SELECT v FROM t"),
        Err(Error::IntegralValueOutOfRange(0, 300))
    ));
    assert!(matches!(
        conn.query_row("SELECT v FROM t", [], |row| row.get::<_, String>(0)),
        Err(Error::InvalidColumnType(0, _, Type::Integer))
    ));
    assert!(matches!(
        conn.query_row("SELECT v FROM t", [], |row| row.get::<_, i64>(1)),
        Err(Error::InvalidColumnIndex(1))
    ));
    let by_name: i64 = conn
        .query_row("SELECT v AS Value FROM t", [], |row| row.get("value"))
        .expect("by name");
    assert_eq!(by_name, 300);
    let fixed: [u8; 2] = conn
        .query_row("SELECT X'0102'", [], |row| row.get(0))
        .expect("fixed array");
    assert_eq!(fixed, [1, 2]);
    assert!(conn
        .query_row("SELECT X'010203'", [], |row| row.get::<_, [u8; 2]>(0))
        .is_err());
}

#[test]
fn invalid_utf8_text_is_an_error_not_a_panic() {
    let conn = memory();
    let result = conn.query_row("SELECT CAST(X'FF' AS TEXT)", [], |row| {
        row.get::<_, String>(0)
    });
    assert!(matches!(result, Err(Error::FromSqlConversionFailure(..))));
    let raw = conn
        .query_row("SELECT CAST(X'FF' AS TEXT)", [], |row| {
            row.get_ref(0)
                .map(|value| value.as_bytes().map(<[u8]>::to_vec))
        })
        .expect("query")
        .expect("bytes");
    assert_eq!(raw, [0xff]);
}

#[test]
fn parameter_forms() {
    let conn = memory();
    conn.execute("INSERT INTO t (id, v) VALUES (?1, ?2)", params![1, "a"])
        .expect("params!");
    conn.execute("INSERT INTO t (id, v) VALUES (?1, ?2)", (2, "b"))
        .expect("tuple");
    conn.execute(
        "INSERT INTO t (id, v) VALUES (:id, :v)",
        named_params! { ":id": 3, ":v": "c" },
    )
    .expect("named");
    conn.execute(
        "INSERT INTO t (id, v) VALUES (?1, ?2)",
        params_from_iter(["4", "d"]),
    )
    .expect("iter");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM t", (), |row| row.get(0))
        .expect("count");
    assert_eq!(count, 4);

    assert!(matches!(
        conn.execute("INSERT INTO t (id, v) VALUES (?1, ?2)", [1]),
        Err(Error::InvalidParameterCount(1, 2))
    ));
    assert!(matches!(
        conn.execute("INSERT INTO t (id) VALUES (?1)", [1, 2]),
        Err(Error::InvalidParameterCount(2, 1))
    ));
    assert!(matches!(
        conn.execute(
            "INSERT INTO t (id) VALUES (:id)",
            named_params! { ":nope": 1 }
        ),
        Err(Error::InvalidParameterName(_))
    ));
}

#[test]
fn query_map_and_rows() {
    let conn = memory();
    for id in 1..=3 {
        conn.execute("INSERT INTO t (id, v) VALUES (?1, ?1 * 10)", [id])
            .expect("insert");
    }
    let mut stmt = conn
        .prepare("SELECT v FROM t WHERE id >= ?1 ORDER BY id")
        .expect("prepare");
    let values = stmt
        .query_map([2], |row| row.get::<_, i64>(0))
        .expect("query")
        .collect::<Result<Vec<_>>>()
        .expect("rows");
    assert_eq!(values, [20, 30]);

    // The statement is reusable after the rows are dropped.
    let mut rows = stmt.query([1]).expect("query");
    let mut seen = Vec::new();
    while let Some(row) = rows.next().expect("step") {
        seen.push(row.get::<_, i64>(0).expect("value"));
    }
    assert_eq!(seen, [10, 20, 30]);
    assert!(rows.next().expect("finished").is_none());
    drop(rows);
    assert!(stmt.exists([3]).expect("exists"));
    assert!(!stmt.exists([4]).expect("exists"));
    assert_eq!(stmt.column_names(), ["v"]);
}

#[test]
fn no_rows_and_optional() {
    let conn = memory();
    let missing = conn.query_row("SELECT v FROM t", [], |row| row.get::<_, i64>(0));
    assert!(matches!(missing, Err(Error::QueryReturnedNoRows)));
    let optional = conn
        .query_row("SELECT v FROM t", [], |row| row.get::<_, i64>(0))
        .optional()
        .expect("optional");
    assert_eq!(optional, None);
}

#[test]
fn statement_misuse_is_rejected() {
    let conn = memory();
    assert!(matches!(
        conn.prepare("SELECT 1; SELECT 2"),
        Err(Error::MultipleStatement)
    ));
    conn.prepare("SELECT 1; -- trailing comment\n")
        .expect("comment after statement");
    assert!(conn.prepare("  -- only a comment").is_err());
    assert!(matches!(
        conn.execute("SELECT 1", []),
        Err(Error::ExecuteReturnedResults)
    ));
    let error = conn.prepare("SELECT FROM").expect_err("syntax error");
    assert_eq!(error.sqlite_error_code(), Some(ErrorCode::Unknown));
    assert!(error.to_string().contains("syntax error"));
}

#[test]
fn constraint_errors_carry_extended_codes() {
    let conn = memory();
    conn.execute_batch("CREATE TABLE u (k TEXT UNIQUE)")
        .expect("create");
    conn.execute("INSERT INTO t (id) VALUES (1)", [])
        .expect("insert");
    let error = conn
        .execute("INSERT INTO t (id) VALUES (1)", [])
        .expect_err("duplicate primary key");
    let code = error.sqlite_error().expect("sqlite error");
    assert_eq!(code.code, ErrorCode::ConstraintViolation);
    assert_eq!(code.extended_code, ffi::SQLITE_CONSTRAINT_PRIMARYKEY);

    conn.execute("INSERT INTO u (k) VALUES ('a')", [])
        .expect("insert");
    let error = conn
        .execute("INSERT INTO u (k) VALUES ('a')", [])
        .expect_err("duplicate unique");
    assert_eq!(
        error.sqlite_error().map(|code| code.extended_code),
        Some(ffi::SQLITE_CONSTRAINT_UNIQUE)
    );
}

#[test]
fn execute_batch_runs_everything() {
    let conn = memory();
    conn.execute_batch(
        "-- leading comment
        INSERT INTO t (id, v) VALUES (1, 'a');
        PRAGMA journal_mode;
        INSERT INTO t (id, v) VALUES (2, 'b'); -- trailing comment",
    )
    .expect("batch");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 2);
    assert!(conn
        .execute_batch("INSERT INTO t (id) VALUES (3); BROKEN")
        .is_err());
    conn.execute_batch("").expect("empty batch");
}

#[test]
fn cached_statements_are_reused_and_reset() {
    let conn = memory();
    for id in 1..=3 {
        let mut stmt = conn
            .prepare_cached("INSERT INTO t (id, v) VALUES (?1, ?2)")
            .expect("prepare");
        stmt.execute(params![id, "x"]).expect("insert");
    }
    {
        let mut stmt = conn
            .prepare_cached("SELECT id FROM t ORDER BY id")
            .expect("prepare");
        let mut rows = stmt.query([]).expect("query");
        // Abandon the query part-way; returning to the cache must reset it.
        assert!(rows.next().expect("row").is_some());
    }
    let mut stmt = conn
        .prepare_cached("  SELECT id FROM t ORDER BY id  ")
        .expect("prepare");
    let first: i64 = stmt.query_row([], |row| row.get(0)).expect("first row");
    assert_eq!(first, 1);

    // Two simultaneous uses of the same SQL both work.
    let mut other = conn
        .prepare_cached("SELECT id FROM t ORDER BY id")
        .expect("prepare second copy");
    assert_eq!(
        other
            .query_row([], |row| row.get::<_, i64>(0))
            .expect("row"),
        1
    );
    drop(other);
    drop(stmt);

    conn.set_prepared_statement_cache_capacity(0);
    conn.flush_prepared_statement_cache();
    conn.prepare_cached("SELECT 1").expect("uncached").discard();
}

#[test]
fn transactions_commit_and_roll_back() {
    let mut conn = memory();
    {
        let tx = conn.transaction().expect("begin");
        tx.execute("INSERT INTO t (id) VALUES (1)", [])
            .expect("insert");
        // Dropped without commit.
    }
    assert!(conn.is_autocommit());
    {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("begin");
        tx.execute("INSERT INTO t (id) VALUES (2)", [])
            .expect("insert");
        tx.commit().expect("commit");
    }
    {
        let tx = conn.unchecked_transaction().expect("begin");
        tx.execute("INSERT INTO t (id) VALUES (3)", [])
            .expect("insert");
        tx.rollback().expect("rollback");
    }
    let ids = conn
        .prepare("SELECT id FROM t")
        .expect("prepare")
        .query_map([], |row| row.get::<_, i64>(0))
        .expect("query")
        .collect::<Result<Vec<_>>>()
        .expect("rows");
    assert_eq!(ids, [2]);
}

#[test]
fn pragmas() {
    let conn = memory();
    conn.pragma_update(Some(DatabaseName::Main), "user_version", 7)
        .expect("update");
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("query");
    assert_eq!(version, 7);
    let mut columns = Vec::new();
    conn.pragma(None, "table_info", "t", |row| {
        columns.push(row.get::<_, String>("name")?);
        Ok(())
    })
    .expect("table_info");
    assert_eq!(columns, ["id", "v"]);
    assert!(conn
        .pragma_update(None, "user_version; DROP TABLE t", 1)
        .is_err());
    assert!(conn.pragma_update(None, "user_version", f64::NAN).is_err());
}

#[test]
fn files_persist_and_open_errors_report() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("db.sqlite");
    {
        let conn = Connection::open(&path).expect("create");
        conn.busy_timeout(Duration::from_secs(1)).expect("timeout");
        conn.execute_batch("CREATE TABLE kv (k TEXT); INSERT INTO kv VALUES ('kept')")
            .expect("write");
    }
    let conn = Connection::open(&path).expect("reopen");
    let kept: String = conn
        .query_row("SELECT k FROM kv", [], |row| row.get(0))
        .expect("read");
    assert_eq!(kept, "kept");
    assert_eq!(conn.last_insert_rowid(), 0);

    let missing = dir.path().join("no-such-dir").join("db.sqlite");
    let error = Connection::open(&missing).expect_err("missing directory");
    assert_eq!(error.sqlite_error_code(), Some(ErrorCode::CannotOpen));
}

#[test]
fn connection_moves_between_threads() {
    fn assert_send<T: Send>() {}
    assert_send::<Connection>();
    let conn = memory();
    let conn = std::thread::spawn(move || {
        conn.execute("INSERT INTO t (id) VALUES (1)", [])
            .expect("insert");
        conn
    })
    .join()
    .expect("thread");
    assert_eq!(conn.last_insert_rowid(), 1);
}

#[test]
fn value_ref_accessors() {
    assert_eq!(ValueRef::from("x").as_str().expect("text"), "x");
    assert!(ValueRef::Integer(1).as_str().is_err());
    assert_eq!(ValueRef::Blob(b"ab").as_bytes().expect("bytes"), b"ab");
    assert_eq!(ValueRef::Real(1.0).data_type(), Type::Real);
}

#[test]
fn constants_match_libsqlite3_sys() {
    macro_rules! same {
        ($($name:ident),+ $(,)?) => {
            $(assert_eq!(ffi::$name, libsqlite3_sys::$name, stringify!($name));)+
        };
    }
    same!(
        SQLITE_OK,
        SQLITE_ERROR,
        SQLITE_INTERNAL,
        SQLITE_PERM,
        SQLITE_ABORT,
        SQLITE_BUSY,
        SQLITE_LOCKED,
        SQLITE_NOMEM,
        SQLITE_READONLY,
        SQLITE_INTERRUPT,
        SQLITE_IOERR,
        SQLITE_CORRUPT,
        SQLITE_NOTFOUND,
        SQLITE_FULL,
        SQLITE_CANTOPEN,
        SQLITE_PROTOCOL,
        SQLITE_SCHEMA,
        SQLITE_TOOBIG,
        SQLITE_CONSTRAINT,
        SQLITE_MISMATCH,
        SQLITE_MISUSE,
        SQLITE_NOLFS,
        SQLITE_AUTH,
        SQLITE_RANGE,
        SQLITE_NOTADB,
        SQLITE_ROW,
        SQLITE_DONE,
        SQLITE_BUSY_RECOVERY,
        SQLITE_BUSY_SNAPSHOT,
        SQLITE_BUSY_TIMEOUT,
        SQLITE_CONSTRAINT_CHECK,
        SQLITE_CONSTRAINT_COMMITHOOK,
        SQLITE_CONSTRAINT_FOREIGNKEY,
        SQLITE_CONSTRAINT_FUNCTION,
        SQLITE_CONSTRAINT_NOTNULL,
        SQLITE_CONSTRAINT_PRIMARYKEY,
        SQLITE_CONSTRAINT_TRIGGER,
        SQLITE_CONSTRAINT_UNIQUE,
        SQLITE_CONSTRAINT_VTAB,
        SQLITE_CONSTRAINT_ROWID,
        SQLITE_OPEN_READONLY,
        SQLITE_OPEN_READWRITE,
        SQLITE_OPEN_CREATE,
        SQLITE_OPEN_URI,
        SQLITE_OPEN_MEMORY,
        SQLITE_OPEN_NOMUTEX,
        SQLITE_OPEN_FULLMUTEX,
        SQLITE_OPEN_SHAREDCACHE,
        SQLITE_OPEN_PRIVATECACHE,
        SQLITE_INTEGER,
        SQLITE_FLOAT,
        SQLITE_TEXT,
        SQLITE_BLOB,
        SQLITE_NULL,
        SQLITE_UTF8,
    );
}

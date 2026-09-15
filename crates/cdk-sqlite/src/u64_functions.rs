//! Checked `u64` arithmetic and the encoding the amount columns use.
//!
//! SQLite has no unsigned 64 bit type and its own arithmetic promotes an
//! integer overflow to a float, so an amount is stored as its eight big endian
//! bytes and added by these functions instead of by `+` and `SUM`. Big endian
//! is what makes SQLite's `memcmp` over equal length blobs the same order as
//! the numeric one, so equality, ordering and indexes need no cast.
//!
//! `u64` converts a value SQL names, an integer literal or a column that still
//! holds a pre-migration integer, into that encoding. Postgres declares the
//! same three names as aliases, so one statement is written once and is correct
//! on either backend.
//!
//! One limit on that: SQLite parses a decimal literal past `i64::MAX` as a
//! float, so a value that large reaches SQL as a bind or a blob literal, never
//! as digits. `u64` refuses the float rather than rounding it.
//!
//! None of them belongs in a `CHECK`, a `DEFAULT` or an index: a schema
//! referring to one could not be read by a connection that had not registered
//! it, `sqlite3` included.

use cdk_sql_common::value::{amount_from_blob, amount_to_blob};
use rusqlite::functions::{Context, FunctionFlags};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, Error, Result};

/// Every one of these is a pure function of its arguments.
///
/// `SQLITE_DIRECTONLY` is what makes the schema restriction above an error
/// rather than a convention: SQLite refuses one of these in a `CHECK`, a
/// `DEFAULT`, an index, a view or a trigger, so no schema can come to depend on
/// a function a plain `sqlite3` connection would not have.
const FLAGS: FunctionFlags = FunctionFlags::SQLITE_UTF8
    .union(FunctionFlags::SQLITE_DETERMINISTIC)
    .union(FunctionFlags::SQLITE_DIRECTONLY);

/// A stored amount that is not a `u64`, or a sum that leaves the range.
#[derive(Debug, thiserror::Error)]
enum AmountError {
    /// The stored value is not a non-negative integer within `u64`
    #[error("{0} is not a u64 amount")]
    NotAnAmount(String),

    /// The addition leaves the `u64` range
    #[error("Amount sum overflows u64")]
    Overflow,
}

fn user_error(err: AmountError) -> Error {
    Error::UserFunctionError(Box::new(err))
}

/// Reads one argument as an amount, or `None` when it is SQL `NULL`.
///
/// Integers are accepted so the migration can convert a column that still holds
/// one and so a query can pass a literal. Text is not: it was the previous
/// encoding, and one arriving now means a table the migration missed.
fn amount_arg(ctx: &Context<'_>, index: usize) -> Result<Option<u64>> {
    match ctx.get_raw(index) {
        ValueRef::Null => Ok(None),
        ValueRef::Integer(value) => u64::try_from(value)
            .map(Some)
            .map_err(|_| user_error(AmountError::NotAnAmount(value.to_string()))),
        ValueRef::Blob(bytes) => amount_from_blob(bytes).map(Some).ok_or_else(|| {
            user_error(AmountError::NotAnAmount(format!(
                "<{} byte blob>",
                bytes.len()
            )))
        }),
        ValueRef::Real(value) => Err(user_error(AmountError::NotAnAmount(value.to_string()))),
        ValueRef::Text(_) => Err(user_error(AmountError::NotAnAmount("<text>".to_owned()))),
    }
}

/// Sums amounts, refusing a total that leaves the `u64` range.
struct AmountSum;

impl rusqlite::functions::Aggregate<u64, Value> for AmountSum {
    fn init(&self, _: &mut Context<'_>) -> Result<u64> {
        Ok(0)
    }

    fn step(&self, ctx: &mut Context<'_>, total: &mut u64) -> Result<()> {
        if let Some(amount) = amount_arg(ctx, 0)? {
            *total = total
                .checked_add(amount)
                .ok_or_else(|| user_error(AmountError::Overflow))?;
        }

        Ok(())
    }

    /// Unlike `SUM`, no rows sums to zero rather than to `NULL`: the callers
    /// all want a total, and a `COALESCE` fallback would have to spell out the
    /// encoding to stay comparable.
    fn finalize(&self, _: &mut Context<'_>, total: Option<u64>) -> Result<Value> {
        Ok(Value::Blob(amount_to_blob(total.unwrap_or_default())))
    }
}

/// Registers the amount encoding and arithmetic every connection needs.
pub fn register(conn: &Connection) -> Result<()> {
    conn.create_scalar_function("u64", 1, FLAGS, |ctx| {
        Ok(match amount_arg(ctx, 0)? {
            Some(value) => Value::Blob(amount_to_blob(value)),
            None => Value::Null,
        })
    })?;

    conn.create_scalar_function("u64_add", 2, FLAGS, |ctx| {
        let (Some(left), Some(right)) = (amount_arg(ctx, 0)?, amount_arg(ctx, 1)?) else {
            return Ok(Value::Null);
        };

        let total = left
            .checked_add(right)
            .ok_or_else(|| user_error(AmountError::Overflow))?;

        Ok(Value::Blob(amount_to_blob(total)))
    })?;

    conn.create_aggregate_function("u64_sum", 1, FLAGS, AmountSum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().expect("in memory database");
        register(&conn).expect("registers");
        conn
    }

    fn blob(conn: &Connection, sql: &str) -> Vec<u8> {
        conn.query_row(sql, [], |row| row.get::<_, Vec<u8>>(0))
            .expect("query")
    }

    fn fails(conn: &Connection, sql: &str) -> bool {
        conn.query_row(sql, [], |row| row.get::<_, Vec<u8>>(0))
            .is_err()
    }

    #[test]
    fn encodes_an_integer_big_endian() {
        let conn = connection();
        assert_eq!(blob(&conn, "SELECT u64(0)"), amount_to_blob(0));
        assert_eq!(blob(&conn, "SELECT u64(1)"), amount_to_blob(1));
        assert_eq!(
            blob(&conn, "SELECT u64(9223372036854775807)"),
            amount_to_blob(i64::MAX as u64)
        );
    }

    /// `printf('%016X', -1)` renders `FFFFFFFFFFFFFFFF`, so a pure SQL
    /// conversion would turn a negative row into `u64::MAX` without failing.
    #[test]
    fn refuses_a_negative_integer() {
        assert!(fails(&connection(), "SELECT u64(-1)"));
    }

    #[test]
    fn passes_an_encoded_amount_through() {
        let conn = connection();
        assert_eq!(
            blob(&conn, "SELECT u64(x'FFFFFFFFFFFFFFFF')"),
            amount_to_blob(u64::MAX)
        );
    }

    #[test]
    fn refuses_text_a_real_and_a_short_blob() {
        let conn = connection();
        assert!(fails(&conn, "SELECT u64('42')"));
        assert!(fails(&conn, "SELECT u64(1.5)"));
        assert!(fails(&conn, "SELECT u64(x'01')"));
    }

    #[test]
    fn encodes_null_to_null() {
        let value: Option<Vec<u8>> = connection()
            .query_row("SELECT u64(NULL)", [], |row| row.get(0))
            .expect("query");
        assert!(value.is_none());
    }

    /// The order `idx_mint_quote_pending` and every amount comparison rely on.
    #[test]
    fn encoded_amounts_sort_in_numeric_order() {
        let conn = connection();
        conn.execute_batch(
            "CREATE TABLE proof (amount BLOB NOT NULL);
             INSERT INTO proof VALUES (x'FFFFFFFFFFFFFFFF'), (x'8000000000000000'),
                                      (x'7FFFFFFFFFFFFFFF'), (u64(1));",
        )
        .expect("fixture");

        let mut stmt = conn
            .prepare("SELECT amount FROM proof ORDER BY amount")
            .expect("prepare");
        let sorted = stmt
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .expect("query")
            .collect::<Result<Vec<_>>>()
            .expect("rows");

        assert_eq!(
            sorted,
            vec![
                amount_to_blob(1),
                amount_to_blob(i64::MAX as u64),
                amount_to_blob(i64::MAX as u64 + 1),
                amount_to_blob(u64::MAX),
            ]
        );
    }

    /// The same text has to run on postgres, where these three names are
    /// aliases of arithmetic the server already does, so a statement touching
    /// an amount is written once rather than once per backend.
    #[test]
    fn runs_the_text_postgres_also_accepts() {
        let conn = connection();
        assert_eq!(
            blob(&conn, "SELECT u64_add(u64(9223372036854775807), u64(2))"),
            amount_to_blob(i64::MAX as u64 + 2)
        );
        assert_eq!(
            blob(&conn, "SELECT u64_sum(x) FROM (SELECT u64(1) AS x)"),
            amount_to_blob(1)
        );
    }

    /// SQLite parses a decimal literal past `i64::MAX` as a float, so a value
    /// that large has to reach SQL as a bind or a blob literal rather than as
    /// digits. Refusing it is the point: silently rounding one is the loss the
    /// amount columns exist to stop.
    #[test]
    fn refuses_a_decimal_literal_past_the_signed_range() {
        let conn = connection();
        assert!(fails(&conn, "SELECT u64(18446744073709551615)"));
        assert_eq!(
            blob(&conn, "SELECT u64(x'FFFFFFFFFFFFFFFF')"),
            amount_to_blob(u64::MAX)
        );
    }

    #[test]
    fn adds_across_the_whole_range() {
        let conn = connection();
        assert_eq!(blob(&conn, "SELECT u64_add(1, 2)"), amount_to_blob(3));
        assert_eq!(
            blob(&conn, "SELECT u64_add(x'FFFFFFFFFFFFFFFE', u64(1))"),
            amount_to_blob(u64::MAX)
        );
    }

    #[test]
    fn refuses_an_addition_that_overflows() {
        let conn = connection();
        assert!(fails(&conn, "SELECT u64_add(x'FFFFFFFFFFFFFFFF', u64(1))"));
    }

    #[test]
    fn sums_past_the_signed_range() {
        let conn = connection();
        conn.execute_batch(
            "CREATE TABLE proof (amount BLOB NOT NULL);
             INSERT INTO proof VALUES (x'7FFFFFFFFFFFFFFF'), (x'8000000000000000');",
        )
        .expect("fixture");

        assert_eq!(
            blob(&conn, "SELECT u64_sum(amount) FROM proof"),
            amount_to_blob(u64::MAX)
        );
    }

    #[test]
    fn sums_no_rows_to_zero() {
        let conn = connection();
        conn.execute_batch("CREATE TABLE proof (amount BLOB NOT NULL);")
            .expect("fixture");

        assert_eq!(
            blob(&conn, "SELECT u64_sum(amount) FROM proof"),
            amount_to_blob(0)
        );
    }

    #[test]
    fn refuses_a_sum_that_overflows() {
        let conn = connection();
        conn.execute_batch(
            "CREATE TABLE proof (amount BLOB NOT NULL);
             INSERT INTO proof VALUES (x'FFFFFFFFFFFFFFFF'), (u64(1));",
        )
        .expect("fixture");

        assert!(fails(&conn, "SELECT u64_sum(amount) FROM proof"));
    }

    #[test]
    fn refuses_a_negative_stored_value() {
        assert!(fails(&connection(), "SELECT u64_add(-1, 1)"));
    }

    #[test]
    fn a_null_operand_adds_to_null() {
        let value: Option<Vec<u8>> = connection()
            .query_row("SELECT u64_add(NULL, 1)", [], |row| row.get(0))
            .expect("query");
        assert!(value.is_none());
    }

    /// The shape every table rebuild in the v0.18 migration takes, over a table
    /// that already holds rows: the amounts convert, a NULL stays NULL, the
    /// column's new CHECK accepts what `u64` wrote, and the indexes the rebuild
    /// recreates come back. A fresh database never exercises this, because it
    /// runs the rebuild against empty tables.
    #[test]
    fn rebuilds_a_populated_table() {
        let conn = connection();
        conn.execute_batch(
            "CREATE TABLE mint_quote (id TEXT PRIMARY KEY, amount INTEGER, amount_paid INTEGER NOT NULL DEFAULT 0);
             CREATE INDEX idx_mint_quote_amount ON mint_quote(amount);
             INSERT INTO mint_quote VALUES ('largest', 9223372036854775807, 5),
                                           ('unamounted', NULL, 0),
                                           ('zero', 0, 0);",
        )
        .expect("pre-migration fixture");

        conn.execute_batch(
            "CREATE TABLE mint_quote_new (
                 id TEXT PRIMARY KEY,
                 amount BLOB
                     CHECK (amount IS NULL
                            OR (typeof(amount) = 'blob' AND length(amount) = 8)),
                 amount_paid BLOB NOT NULL DEFAULT x'0000000000000000'
                     CHECK (typeof(amount_paid) = 'blob' AND length(amount_paid) = 8)
             );
             INSERT INTO mint_quote_new SELECT id, u64(amount), u64(amount_paid) FROM mint_quote;
             DROP TABLE mint_quote;
             ALTER TABLE mint_quote_new RENAME TO mint_quote;
             CREATE INDEX idx_mint_quote_amount ON mint_quote(amount);",
        )
        .expect("rebuild");

        assert_eq!(
            blob(&conn, "SELECT amount FROM mint_quote WHERE id = 'largest'"),
            amount_to_blob(i64::MAX as u64)
        );
        assert_eq!(
            blob(
                &conn,
                "SELECT amount_paid FROM mint_quote WHERE id = 'largest'"
            ),
            amount_to_blob(5)
        );
        assert_eq!(
            blob(&conn, "SELECT amount FROM mint_quote WHERE id = 'zero'"),
            amount_to_blob(0)
        );

        let unamounted: Option<Vec<u8>> = conn
            .query_row(
                "SELECT amount FROM mint_quote WHERE id = 'unamounted'",
                [],
                |row| row.get(0),
            )
            .expect("query");
        assert!(unamounted.is_none());

        let indexes: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type = 'index' AND tbl_name = 'mint_quote' AND name = 'idx_mint_quote_amount'",
                [],
                |row| row.get(0),
            )
            .expect("query");
        assert_eq!(indexes, 1);
    }

    /// Re-running a rebuild that already happened has to be a no-op rather than
    /// a second conversion, since the batch is not one transaction and a
    /// migration that failed part way is retried from the top.
    #[test]
    fn rebuilding_an_already_rebuilt_table_is_idempotent() {
        let conn = connection();
        conn.execute_batch(
            "CREATE TABLE proof (amount BLOB NOT NULL);
             INSERT INTO proof VALUES (u64(9223372036854775807));",
        )
        .expect("fixture");

        conn.execute_batch(
            "CREATE TABLE proof_new (amount BLOB NOT NULL);
             INSERT INTO proof_new SELECT u64(amount) FROM proof;
             DROP TABLE proof;
             ALTER TABLE proof_new RENAME TO proof;",
        )
        .expect("rebuild");

        assert_eq!(
            blob(&conn, "SELECT amount FROM proof"),
            amount_to_blob(i64::MAX as u64)
        );
    }

    /// A schema naming one of these could not be read by a connection that had
    /// not registered it, `sqlite3` included, so registering them direct-only
    /// makes that an error at the point the schema is written.
    #[test]
    fn refuses_to_be_named_by_a_schema() {
        let conn = connection();
        conn.execute_batch("CREATE TABLE proof (amount BLOB NOT NULL);")
            .expect("fixture");

        assert!(conn
            .execute_batch("CREATE TABLE bad (amount BLOB CHECK (amount = u64(1)));")
            .is_err());
        assert!(conn
            .execute_batch("CREATE INDEX idx_proof_amount ON proof(u64(amount));")
            .is_err());
        assert!(conn
            .execute_batch("CREATE VIEW totals AS SELECT u64_sum(amount) FROM proof;")
            .and_then(|()| conn.execute_batch("SELECT * FROM totals;"))
            .is_err());
    }
}

//! Helpers for tests that have to look at a stored column rather than the value
//! the database trait hands back.

use cdk_sql_common::pool::Pool;
use cdk_sql_common::stmt::query;
use cdk_sql_common::value::Value;

use crate::common::{Config, SqliteConnectionManager};

/// Runs `sql` with a connection of its own and returns its first column, so a
/// `NULL` stays distinguishable from the zero a read path maps it to.
pub(crate) async fn stored_value(path: &str, sql: &str) -> Value {
    #[cfg(not(feature = "sqlcipher"))]
    let config: Config = path.into();
    #[cfg(feature = "sqlcipher")]
    let config: Config = (path, "test").into();

    let pool = Pool::<SqliteConnectionManager>::new(config);
    let conn = pool.get().await.expect("valid connection");

    query(sql)
        .expect("query")
        .fetch_one(&*conn)
        .await
        .expect("fetch")
        .expect("one row")
        .into_iter()
        .next()
        .expect("one column")
}

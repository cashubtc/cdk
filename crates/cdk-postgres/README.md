# CDK PostgreSQL storage

CDK uses `deadpool-postgres` to manage client connections. Mint, authentication,
and wallet constructors keep their existing signatures and run migrations before
returning. Existing database contents and SQL migration files are unchanged.

## PostgreSQL and PgBouncer

| Endpoint | Supported configuration |
| --- | --- |
| PostgreSQL directly | Normal PostgreSQL connections |
| PgBouncer session pooling | `pool_mode = session` |
| PgBouncer transaction pooling | Named protocol prepared-statement tracking enabled |
| PgBouncer statement pooling | Unsupported: CDK uses multi-statement transactions |

Use a maintained PgBouncer release with protocol-level prepared-statement
tracking (introduced in 1.21), and explicitly configure:

```ini
[pgbouncer]
pool_mode = transaction
max_prepared_statements = 200
```

The nonzero value enables tracking. `200` is a starting value; size it for your
workload and monitor proxy resource use. Tracking disabled is unsupported in
transaction mode. CDK continues to use parameterized, uncached driver `prepare()`
operations. SQL `PREPARE`/`DEALLOCATE` and session-reset recycling are not required.
See [PgBouncer's prepared-statement documentation](https://www.pgbouncer.org/config.html#max_prepared_statements).

Point the ordinary CDK PostgreSQL URL at PgBouncer, including for migrations. No
separate direct migration URL is needed. Configure TLS for the CDK-to-proxy and
proxy-to-PostgreSQL connections as appropriate for your deployment. Explicit
`PgConfig` TLS mode overrides URL `sslmode`. TLS setup errors fail construction;
there is no fallback on TLS construction failure.

The default client pool limit remains 20 **per backend instance**, with a default
10-second timeout applied separately to waiting, connection creation, recycling,
and transaction cleanup. These phase limits are not a total acquisition deadline.
Zero capacity is rejected. Deadpool uses verified health checks on checkout.
Size PgBouncer's server pools separately from CDK's client pools. Adding pools
does not make multiple active mint replicas safe.

## Schemas and migrations

The existing `schema=name` connection-string extension selects one schema
identifier. CDK uses parameterized transaction-local `search_path`; it never sets
session-level schema state. Explicit transactions set the path immediately after
`BEGIN`. Standalone reads, writes, and batches with a configured schema use a
short transaction, adding begin/schema/commit round trips. Connections without a
configured schema retain the database's default search path.

Migration startup acquires one connection, begins, takes a transaction-scoped
advisory lock keyed by database and schema, creates the schema if necessary, sets
the local path, runs the existing migrations, and commits. Concurrent startup of
the same schema is serialized. Failed bootstrap and migrations roll back together.
Quote and keyset advisory locks remain transaction-scoped.

Coordinate application deployments that change result types of prepared queries.
Future DDL changes can invalidate statements tracked by PgBouncer; after the
migration, use PgBouncer's administrative `RECONNECT` and wait for old server
connections to drain before resuming incompatible traffic. Follow the
[upstream migration guidance](https://www.pgbouncer.org/config.html#max_prepared_statements).
This pool replacement itself requires no data migration.

## Cancellation and errors

Owned transactions retain their Deadpool checkout through commit, rollback, or
asynchronous rollback on drop. Cleanup is armed before starting a transaction.
Failed or timed-out cleanup removes and disposes of the connection. Runtime
shutdown and cancellation of cleanup use that same disposal path. An adapter
whose query was cancelled cannot be used again, even if another reference to it
survives. Acquire a new connection for subsequent work.

Writes and commits are never automatically retried. A lost commit response can
mean the commit succeeded; higher-level recovery must resolve that uncertainty.


## Low-level API migration

`cdk_sql_common::pool`, `Pool`, `PooledResource`, `DatabaseConfig`,
`DatabasePool`, `DatabaseConnector`, `DatabaseTransaction`,
`ConnectionWithTransaction`, and `GenericTransactionHandler` have been removed.
`PgConnectionPool`, `SqliteConnectionManager`, the old
`PostgresConnection::new`, and the old `SslMode` enum are also removed. TLS
configuration continues through `PgConfig::new`.

Use `PostgresBackend` or `cdk_sqlite::SqliteBackend` through
`cdk_sql_common::database::SqlBackend`:

```rust,no_run
use cdk_postgres::{PgConfig, PostgresBackend};
use cdk_sql_common::database::{SqlBackend, SqlTransaction};
use cdk_sql_common::stmt::query;

# async fn example() -> Result<(), cdk_common::database::Error> {
let backend = PostgresBackend::new(PgConfig::from(
    "host=localhost user=cdk dbname=cdk schema=mint",
))?;
let tx = backend.begin_migration().await?;
query("CREATE TABLE IF NOT EXISTS example (id BIGINT PRIMARY KEY)")?
    .batch(&tx).await?;
tx.commit().await?;
let conn = backend.acquire().await?;
let _rows = query("SELECT id FROM example")?.fetch_all(&conn).await?;
# Ok(())
# }
```

`SqlBackend::new` constructs the pool without connecting. Normal mint/wallet/auth
constructors additionally acquire and migrate before succeeding. Use
`SqlConnection::begin_transaction` to consume an existing checkout without
acquiring another. `SqlTransaction` commits and rolls back by consuming itself.
Do not issue manual transaction-control SQL through `DatabaseExecutor`.

The normal `MintPgDatabase`, `MintPgAuthDatabase`, `WalletPgDatabase`, SQLite
aliases, `new_wallet_pg_database`, `PgConfig::new`, and FFI constructors remain
available. No wallet API or FFI wallet method changes are needed.

## Testing

From the repository root:

```sh
nix develop .#regtest
bash misc/pgbouncer/test.sh
```

The helper starts PostgreSQL and two isolated PgBouncer listeners: transaction
pooling on 6432 (two server connections, round robin) and session pooling on 6433.
It runs the suites through all three endpoints and separately runs the test that
pins one backend and proves a prepared statement executes on a different backend
PID. `start-pgbouncer` and `stop-pgbouncer` can also be used independently with
`start-postgres`. Port and upstream overrides are documented in
`misc/pgbouncer/start.sh`; the generated authentication configuration is for local
testing only and must not be deployed.

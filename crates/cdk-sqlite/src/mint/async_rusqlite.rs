//! Async, pipelined rusqlite client
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread::spawn;
use std::time::Instant;

use cdk_common::database::Error;
use cdk_sql_common::database::{DatabaseConnector, DatabaseExecutor, DatabaseTransaction};
use cdk_sql_common::pool::{self, Pool, PooledResource};
use cdk_sql_common::stmt::{Column, ExpectedSqlResponse, Statement as InnerStatement};
use cdk_sql_common::ConversionError;
use rusqlite::{ffi, Connection, ErrorCode, TransactionBehavior};
use tokio::sync::{mpsc, oneshot};

use crate::common::{create_sqlite_pool, from_sqlite, to_sqlite, SqliteConnectionManager};

/// The number of queued SQL statements before it start failing
const SQL_QUEUE_SIZE: usize = 10_000;
/// How many ms is considered a slow query, and it'd be logged for further debugging
const SLOW_QUERY_THRESHOLD_MS: u128 = 20;
/// How many SQLite parallel connections can be used to read things in parallel
const WORKING_THREAD_POOL_SIZE: usize = 5;

#[derive(Debug, Clone)]
pub struct AsyncRusqlite {
    sender: mpsc::Sender<DbRequest>,
    inflight_requests: Arc<AtomicUsize>,
}

impl From<PathBuf> for AsyncRusqlite {
    fn from(value: PathBuf) -> Self {
        AsyncRusqlite::new(create_sqlite_pool(value.to_str().unwrap_or_default(), None))
    }
}

impl From<&str> for AsyncRusqlite {
    fn from(value: &str) -> Self {
        AsyncRusqlite::new(create_sqlite_pool(value, None))
    }
}

impl From<(&str, &str)> for AsyncRusqlite {
    fn from((value, pass): (&str, &str)) -> Self {
        AsyncRusqlite::new(create_sqlite_pool(value, Some(pass.to_owned())))
    }
}

impl From<(PathBuf, &str)> for AsyncRusqlite {
    fn from((value, pass): (PathBuf, &str)) -> Self {
        AsyncRusqlite::new(create_sqlite_pool(
            value.to_str().unwrap_or_default(),
            Some(pass.to_owned()),
        ))
    }
}

impl From<(&str, String)> for AsyncRusqlite {
    fn from((value, pass): (&str, String)) -> Self {
        AsyncRusqlite::new(create_sqlite_pool(value, Some(pass)))
    }
}

impl From<(PathBuf, String)> for AsyncRusqlite {
    fn from((value, pass): (PathBuf, String)) -> Self {
        AsyncRusqlite::new(create_sqlite_pool(
            value.to_str().unwrap_or_default(),
            Some(pass),
        ))
    }
}

impl From<&PathBuf> for AsyncRusqlite {
    fn from(value: &PathBuf) -> Self {
        AsyncRusqlite::new(create_sqlite_pool(value.to_str().unwrap_or_default(), None))
    }
}

/// Internal request for the database thread
#[derive(Debug)]
enum DbRequest {
    Sql(InnerStatement, oneshot::Sender<DbResponse>),
    Begin(oneshot::Sender<DbResponse>),
    Commit(oneshot::Sender<DbResponse>),
    Rollback(oneshot::Sender<DbResponse>),
}

#[derive(Debug)]
enum DbResponse {
    Transaction(mpsc::Sender<DbRequest>),
    AffectedRows(usize),
    Pluck(Option<Column>),
    Row(Option<Vec<Column>>),
    Rows(Vec<Vec<Column>>),
    Error(SqliteError),
    Unexpected,
    Ok,
}

#[derive(thiserror::Error, Debug)]
enum SqliteError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    #[error(transparent)]
    Inner(#[from] Error),

    #[error(transparent)]
    Pool(#[from] pool::Error<rusqlite::Error>),

    /// Duplicate entry
    #[error("Duplicate")]
    Duplicate,

    #[error(transparent)]
    Conversion(#[from] ConversionError),
}

impl From<SqliteError> for Error {
    fn from(val: SqliteError) -> Self {
        match val {
            SqliteError::Duplicate => Error::Duplicate,
            SqliteError::Conversion(e) => e.into(),
            o => Error::Internal(o.to_string()),
        }
    }
}

/// Process a query
#[inline(always)]
fn process_query(conn: &Connection, statement: InnerStatement) -> Result<DbResponse, SqliteError> {
    let start = Instant::now();
    let expected_response = statement.expected_response;
    let (sql, placeholder_values) = statement.to_sql()?;
    let sql = sql.trim_end_matches("FOR UPDATE");

    let mut stmt = conn.prepare_cached(sql)?;
    for (i, value) in placeholder_values.into_iter().enumerate() {
        stmt.raw_bind_parameter(i + 1, to_sqlite(value))?;
    }

    let columns = stmt.column_count();

    let to_return = match expected_response {
        ExpectedSqlResponse::AffectedRows => DbResponse::AffectedRows(stmt.raw_execute()?),
        ExpectedSqlResponse::Batch => {
            conn.execute_batch(sql)?;
            DbResponse::Ok
        }
        ExpectedSqlResponse::ManyRows => {
            let mut rows = stmt.raw_query();
            let mut results = vec![];

            while let Some(row) = rows.next()? {
                results.push(
                    (0..columns)
                        .map(|i| row.get(i).map(from_sqlite))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }

            DbResponse::Rows(results)
        }
        ExpectedSqlResponse::Pluck => {
            let mut rows = stmt.raw_query();
            DbResponse::Pluck(
                rows.next()?
                    .map(|row| row.get(0usize).map(from_sqlite))
                    .transpose()?,
            )
        }
        ExpectedSqlResponse::SingleRow => {
            let mut rows = stmt.raw_query();
            let row = rows
                .next()?
                .map(|row| {
                    (0..columns)
                        .map(|i| row.get(i).map(from_sqlite))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?;
            DbResponse::Row(row)
        }
    };

    let duration = start.elapsed();

    if duration.as_millis() > SLOW_QUERY_THRESHOLD_MS {
        tracing::warn!("[SLOW QUERY] Took {} ms: {}", duration.as_millis(), sql);
    }

    Ok(to_return)
}

/// Spawns N number of threads to execute SQL statements
///
/// Enable parallelism with a pool of threads.
///
/// There is a main thread, which receives SQL requests and routes them to a worker thread from a
/// fixed-size pool.
///
/// By doing so, SQLite does synchronization, and Rust will only intervene when a transaction is
/// executed. Transactions are executed in the main thread.
fn rusqlite_spawn_worker_threads(
    inflight_requests: Arc<AtomicUsize>,
    threads: usize,
) -> std_mpsc::Sender<(
    PooledResource<SqliteConnectionManager>,
    InnerStatement,
    oneshot::Sender<DbResponse>,
)> {
    let (sender, receiver) = std_mpsc::channel::<(
        PooledResource<SqliteConnectionManager>,
        InnerStatement,
        oneshot::Sender<DbResponse>,
    )>();
    let receiver = Arc::new(Mutex::new(receiver));

    for _ in 0..threads {
        let rx = receiver.clone();
        let inflight_requests = inflight_requests.clone();
        spawn(move || loop {
            while let Ok((conn, sql, reply_to)) = rx.lock().expect("failed to acquire").recv() {
                let result = process_query(&conn, sql);
                let _ = match result {
                    Ok(ok) => reply_to.send(ok),
                    Err(err) => {
                        tracing::error!("Failed query with error {:?}", err);
                        let err = if let SqliteError::Sqlite(rusqlite::Error::SqliteFailure(
                            ffi::Error {
                                code,
                                extended_code,
                            },
                            _,
                        )) = &err
                        {
                            if *code == ErrorCode::ConstraintViolation
                                && (*extended_code == ffi::SQLITE_CONSTRAINT_PRIMARYKEY
                                    || *extended_code == ffi::SQLITE_CONSTRAINT_UNIQUE)
                            {
                                SqliteError::Duplicate
                            } else {
                                err
                            }
                        } else {
                            err
                        };

                        reply_to.send(DbResponse::Error(err))
                    }
                };
                drop(conn);
                inflight_requests.fetch_sub(1, Ordering::Relaxed);
            }
        });
    }
    sender
}

/// # Rusqlite main worker
///
/// This function takes ownership of a pool of connections to SQLite, executes SQL statements, and
/// returns the results or number of affected rows to the caller. All communications are done
/// through channels. This function is synchronous, but a thread pool exists to execute queries, and
/// SQLite will coordinate data access. Transactions are executed in the main and it takes ownership
/// of the main thread until it is finalized
///
/// This is meant to be called in their thread, as it will not exit the loop until the communication
/// channel is closed.
fn rusqlite_worker_manager(
    mut receiver: mpsc::Receiver<DbRequest>,
    pool: Arc<Pool<SqliteConnectionManager>>,
    inflight_requests: Arc<AtomicUsize>,
) {
    let send_sql_to_thread =
        rusqlite_spawn_worker_threads(inflight_requests.clone(), WORKING_THREAD_POOL_SIZE);

    let mut tx_id: usize = 0;

    while let Some(request) = receiver.blocking_recv() {
        inflight_requests.fetch_add(1, Ordering::Relaxed);
        match request {
            DbRequest::Sql(statement, reply_to) => {
                let conn = match pool.get() {
                    Ok(conn) => conn,
                    Err(err) => {
                        tracing::error!("Failed to acquire a pool connection: {:?}", err);
                        inflight_requests.fetch_sub(1, Ordering::Relaxed);
                        let _ = reply_to.send(DbResponse::Error(err.into()));
                        continue;
                    }
                };

                let _ = send_sql_to_thread.send((conn, statement, reply_to));
                continue;
            }
            DbRequest::Begin(reply_to) => {
                let (sender, mut receiver) = mpsc::channel(SQL_QUEUE_SIZE);
                let mut conn = match pool.get() {
                    Ok(conn) => conn,
                    Err(err) => {
                        tracing::error!("Failed to acquire a pool connection: {:?}", err);
                        inflight_requests.fetch_sub(1, Ordering::Relaxed);
                        let _ = reply_to.send(DbResponse::Error(err.into()));
                        continue;
                    }
                };

                let tx = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
                    Ok(tx) => tx,
                    Err(err) => {
                        tracing::error!("Failed to begin a transaction: {:?}", err);
                        inflight_requests.fetch_sub(1, Ordering::Relaxed);
                        let _ = reply_to.send(DbResponse::Error(err.into()));
                        continue;
                    }
                };

                // Transaction has begun successfully, send the `sender` back to the caller
                // and wait for statements to execute. On `Drop` the wrapper transaction
                // should send a `rollback`.
                let _ = reply_to.send(DbResponse::Transaction(sender));

                tx_id += 1;

                // We intentionally handle the transaction hijacking the main loop, there is
                // no point is queueing more operations for SQLite, since transaction have
                // exclusive access. In other database implementation this block of code
                // should be sent to their own thread to allow concurrency
                loop {
                    let request = if let Some(request) = receiver.blocking_recv() {
                        request
                    } else {
                        // If the receiver loop is broken (i.e no more `senders` are active) and no
                        // `Commit` statement has been sent, this will trigger a `Rollback`
                        // automatically
                        tracing::trace!("Tx {}: Transaction rollback on drop", tx_id);
                        let _ = tx.rollback();
                        break;
                    };

                    match request {
                        DbRequest::Commit(reply_to) => {
                            tracing::trace!("Tx {}: Commit", tx_id);
                            let _ = reply_to.send(match tx.commit() {
                                Ok(()) => DbResponse::Ok,
                                Err(err) => {
                                    tracing::error!("Failed commit {:?}", err);
                                    DbResponse::Error(err.into())
                                }
                            });
                            break;
                        }
                        DbRequest::Rollback(reply_to) => {
                            tracing::trace!("Tx {}: Rollback", tx_id);
                            let _ = reply_to.send(match tx.rollback() {
                                Ok(()) => DbResponse::Ok,
                                Err(err) => {
                                    tracing::error!("Failed rollback {:?}", err);
                                    DbResponse::Error(err.into())
                                }
                            });
                            break;
                        }
                        DbRequest::Begin(reply_to) => {
                            let _ = reply_to.send(DbResponse::Unexpected);
                        }
                        DbRequest::Sql(statement, reply_to) => {
                            tracing::trace!("Tx {}: SQL {:?}", tx_id, statement);
                            let _ = match process_query(&tx, statement) {
                                Ok(ok) => reply_to.send(ok),
                                Err(err) => {
                                    tracing::error!(
                                        "Tx {}: Failed query with error {:?}",
                                        tx_id,
                                        err
                                    );
                                    let err = if let SqliteError::Sqlite(
                                        rusqlite::Error::SqliteFailure(
                                            ffi::Error {
                                                code,
                                                extended_code,
                                            },
                                            _,
                                        ),
                                    ) = &err
                                    {
                                        if *code == ErrorCode::ConstraintViolation
                                            && (*extended_code == ffi::SQLITE_CONSTRAINT_PRIMARYKEY
                                                || *extended_code == ffi::SQLITE_CONSTRAINT_UNIQUE)
                                        {
                                            SqliteError::Duplicate
                                        } else {
                                            err
                                        }
                                    } else {
                                        err
                                    };
                                    reply_to.send(DbResponse::Error(err))
                                }
                            };
                        }
                    }
                }

                drop(conn);
            }
            DbRequest::Commit(reply_to) => {
                let _ = reply_to.send(DbResponse::Unexpected);
            }
            DbRequest::Rollback(reply_to) => {
                let _ = reply_to.send(DbResponse::Unexpected);
            }
        }

        // If wasn't a `continue` the transaction is done by reaching this code, and we should
        // decrease the inflight_request counter
        inflight_requests.fetch_sub(1, Ordering::Relaxed);
    }
}

impl AsyncRusqlite {
    /// Creates a new Async Rusqlite wrapper.
    pub fn new(pool: Arc<Pool<SqliteConnectionManager>>) -> Self {
        let (sender, receiver) = mpsc::channel(SQL_QUEUE_SIZE);
        let inflight_requests = Arc::new(AtomicUsize::new(0));
        let inflight_requests_for_thread = inflight_requests.clone();
        spawn(move || {
            rusqlite_worker_manager(receiver, pool, inflight_requests_for_thread);
        });

        Self {
            sender,
            inflight_requests,
        }
    }

    fn get_queue_sender(&self) -> &mpsc::Sender<DbRequest> {
        &self.sender
    }

    /// Show how many inflight requests
    #[allow(dead_code)]
    pub fn inflight_requests(&self) -> usize {
        self.inflight_requests.load(Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl DatabaseConnector for AsyncRusqlite {
    type Transaction<'a> = Transaction<'a>;

    /// Begins a transaction
    ///
    /// If the transaction is Drop it will trigger a rollback operation
    async fn begin(&self) -> Result<Self::Transaction<'_>, Error> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(DbRequest::Begin(sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::Transaction(db_sender) => Ok(Transaction {
                db_sender,
                _marker: PhantomData,
            }),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }
}

#[async_trait::async_trait]
impl DatabaseExecutor for AsyncRusqlite {
    fn name() -> &'static str {
        "sqlite"
    }

    async fn fetch_one(&self, mut statement: InnerStatement) -> Result<Option<Vec<Column>>, Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::SingleRow;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::Row(row) => Ok(row),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    async fn batch(&self, mut statement: InnerStatement) -> Result<(), Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::Batch;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::Ok => Ok(()),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    async fn fetch_all(&self, mut statement: InnerStatement) -> Result<Vec<Vec<Column>>, Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::ManyRows;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::Rows(row) => Ok(row),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    async fn execute(&self, mut statement: InnerStatement) -> Result<usize, Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::AffectedRows;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::AffectedRows(total) => Ok(total),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    async fn pluck(&self, mut statement: InnerStatement) -> Result<Option<Column>, Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::Pluck;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::Pluck(value) => Ok(value),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }
}

/// Database transaction
#[derive(Debug)]
pub struct Transaction<'conn> {
    db_sender: mpsc::Sender<DbRequest>,
    _marker: PhantomData<&'conn ()>,
}

impl Transaction<'_> {
    fn get_queue_sender(&self) -> &mpsc::Sender<DbRequest> {
        &self.db_sender
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        let (sender, _) = oneshot::channel();
        let _ = self.db_sender.try_send(DbRequest::Rollback(sender));
    }
}

#[async_trait::async_trait]
impl<'a> DatabaseTransaction<'a> for Transaction<'a> {
    async fn commit(self) -> Result<(), Error> {
        let (sender, receiver) = oneshot::channel();
        self.db_sender
            .send(DbRequest::Commit(sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::Ok => Ok(()),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    async fn rollback(self) -> Result<(), Error> {
        let (sender, receiver) = oneshot::channel();
        self.db_sender
            .send(DbRequest::Rollback(sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::Ok => Ok(()),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }
}

#[async_trait::async_trait]
impl DatabaseExecutor for Transaction<'_> {
    fn name() -> &'static str {
        "sqlite"
    }

    async fn fetch_one(&self, mut statement: InnerStatement) -> Result<Option<Vec<Column>>, Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::SingleRow;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::Row(row) => Ok(row),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    async fn batch(&self, mut statement: InnerStatement) -> Result<(), Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::Batch;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::Ok => Ok(()),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    async fn fetch_all(&self, mut statement: InnerStatement) -> Result<Vec<Vec<Column>>, Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::ManyRows;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::Rows(row) => Ok(row),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    async fn execute(&self, mut statement: InnerStatement) -> Result<usize, Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::AffectedRows;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::AffectedRows(total) => Ok(total),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    async fn pluck(&self, mut statement: InnerStatement) -> Result<Option<Column>, Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::Pluck;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?;

        match receiver
            .await
            .map_err(|_| Error::Internal("Communication".to_owned()))?
        {
            DbResponse::Pluck(value) => Ok(value),
            DbResponse::Error(err) => Err(err.into()),
            _ => Err(Error::InvalidDbResponse),
        }
    }
}

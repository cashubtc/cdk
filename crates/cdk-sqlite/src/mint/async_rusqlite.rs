use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread::spawn;
use std::time::Instant;

use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};

use crate::common::SqliteConnectionManager;
use crate::mint::Error;
use crate::pool::{Pool, PooledResource};
use crate::stmt::{Column, ExpectedSqlResponse, Statement as InnerStatement, Value};

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

/// Internal request for the database thread
#[derive(Debug)]
pub enum DbRequest {
    Sql(InnerStatement, oneshot::Sender<DbResponse>),
    Begin(oneshot::Sender<DbResponse>),
    Commit(oneshot::Sender<DbResponse>),
    Rollback(oneshot::Sender<DbResponse>),
}

#[derive(Debug)]
pub enum DbResponse {
    Transaction(mpsc::Sender<DbRequest>),
    AffectedRows(usize),
    Pluck(Option<Column>),
    Row(Option<Vec<Column>>),
    Rows(Vec<Vec<Column>>),
    Error(Error),
    Unexpected,
    Ok,
}

/// Statement for the async_rusqlite wrapper
pub struct Statement(InnerStatement);

impl Statement {
    /// Bind a variable
    pub fn bind<C, V>(self, name: C, value: V) -> Self
    where
        C: ToString,
        V: Into<Value>,
    {
        Self(self.0.bind(name, value))
    }

    /// Bind vec
    pub fn bind_vec<C, V>(self, name: C, value: Vec<V>) -> Self
    where
        C: ToString,
        V: Into<Value>,
    {
        Self(self.0.bind_vec(name, value))
    }

    /// Executes a query and return the number of affected rows
    pub async fn execute<C>(self, conn: &C) -> Result<usize, Error>
    where
        C: DatabaseExecutor + Send + Sync,
    {
        conn.execute(self.0).await
    }

    /// Returns the first column of the first row of the query result
    pub async fn pluck<C>(self, conn: &C) -> Result<Option<Column>, Error>
    where
        C: DatabaseExecutor + Send + Sync,
    {
        conn.pluck(self.0).await
    }

    /// Returns the first row of the query result
    pub async fn fetch_one<C>(self, conn: &C) -> Result<Option<Vec<Column>>, Error>
    where
        C: DatabaseExecutor + Send + Sync,
    {
        conn.fetch_one(self.0).await
    }

    /// Returns all rows of the query result
    pub async fn fetch_all<C>(self, conn: &C) -> Result<Vec<Vec<Column>>, Error>
    where
        C: DatabaseExecutor + Send + Sync,
    {
        conn.fetch_all(self.0).await
    }
}

/// Process a query
#[inline(always)]
fn process_query(conn: &Connection, sql: InnerStatement) -> Result<DbResponse, Error> {
    let start = Instant::now();
    let mut args = sql.args;
    let mut stmt = conn.prepare_cached(&sql.sql)?;
    let total_parameters = stmt.parameter_count();

    for index in 1..=total_parameters {
        let value = if let Some(value) = stmt.parameter_name(index).map(|name| {
            args.remove(name)
                .ok_or(Error::MissingParameter(name.to_owned()))
        }) {
            value?
        } else {
            continue;
        };

        stmt.raw_bind_parameter(index, value)?;
    }

    let columns = stmt.column_count();

    let to_return = match sql.expected_response {
        ExpectedSqlResponse::AffectedRows => DbResponse::AffectedRows(stmt.raw_execute()?),
        ExpectedSqlResponse::ManyRows => {
            let mut rows = stmt.raw_query();
            let mut results = vec![];

            while let Some(row) = rows.next()? {
                results.push(
                    (0..columns)
                        .map(|i| row.get(i))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }

            DbResponse::Rows(results)
        }
        ExpectedSqlResponse::Pluck => {
            let mut rows = stmt.raw_query();
            DbResponse::Pluck(rows.next()?.map(|row| row.get(0usize)).transpose()?)
        }
        ExpectedSqlResponse::SingleRow => {
            let mut rows = stmt.raw_query();
            let row = rows
                .next()?
                .map(|row| {
                    (0..columns)
                        .map(|i| row.get(i))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?;
            DbResponse::Row(row)
        }
    };

    let duration = start.elapsed();

    if duration.as_millis() > SLOW_QUERY_THRESHOLD_MS {
        tracing::error!("[SLOW QUERY] Took {} ms: {}", duration.as_millis(), sql.sql);
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
            while let Ok((conn, sql, reply_to)) = rx.lock().unwrap().recv() {
                tracing::info!("Execute query: {}", sql.sql);
                let result = process_query(&conn, sql);
                let _ = match result {
                    Ok(ok) => reply_to.send(ok),
                    Err(err) => {
                        tracing::error!("Failed query with error {:?}", err);
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
            DbRequest::Sql(sql, reply_to) => {
                let conn = match pool.get() {
                    Ok(conn) => conn,
                    Err(err) => {
                        tracing::error!("Failed to acquire a pool connection: {:?}", err);
                        inflight_requests.fetch_sub(1, Ordering::Relaxed);
                        let _ = reply_to.send(DbResponse::Error(err.into()));
                        continue;
                    }
                };

                let _ = send_sql_to_thread.send((conn, sql, reply_to));
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

                let tx = match conn.transaction() {
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
                        tracing::info!("Tx {}: Transaction rollback on drop", tx_id);
                        let _ = tx.rollback();
                        break;
                    };

                    match request {
                        DbRequest::Commit(reply_to) => {
                            tracing::info!("Tx {}: Commit", tx_id);
                            let _ = reply_to.send(match tx.commit() {
                                Ok(()) => DbResponse::Ok,
                                Err(err) => DbResponse::Error(err.into()),
                            });
                            break;
                        }
                        DbRequest::Rollback(reply_to) => {
                            tracing::info!("Tx {}: Rollback", tx_id);
                            let _ = reply_to.send(match tx.rollback() {
                                Ok(()) => DbResponse::Ok,
                                Err(err) => DbResponse::Error(err.into()),
                            });
                            break;
                        }
                        DbRequest::Begin(reply_to) => {
                            let _ = reply_to.send(DbResponse::Unexpected);
                        }
                        DbRequest::Sql(sql, reply_to) => {
                            tracing::info!("Tx {}: SQL {}", tx_id, sql.sql);
                            let _ = match process_query(&tx, sql) {
                                Ok(ok) => reply_to.send(ok),
                                Err(err) => reply_to.send(DbResponse::Error(err)),
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

#[async_trait::async_trait]
pub trait DatabaseExecutor {
    /// Returns the connection to the database thread (or the on-going transaction)
    fn get_queue_sender(&self) -> mpsc::Sender<DbRequest>;

    /// Executes a query and returns the affected rows
    async fn execute(&self, mut statement: InnerStatement) -> Result<usize, Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::AffectedRows;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Communication)?;

        match receiver.await.map_err(|_| Error::Communication)? {
            DbResponse::AffectedRows(n) => Ok(n),
            DbResponse::Error(err) => Err(err),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    /// Runs the query and returns the first row or None
    async fn fetch_one(&self, mut statement: InnerStatement) -> Result<Option<Vec<Column>>, Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::SingleRow;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Communication)?;

        match receiver.await.map_err(|_| Error::Communication)? {
            DbResponse::Row(row) => Ok(row),
            DbResponse::Error(err) => Err(err),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    /// Runs the query and returns the first row or None
    async fn fetch_all(&self, mut statement: InnerStatement) -> Result<Vec<Vec<Column>>, Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::ManyRows;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Communication)?;

        match receiver.await.map_err(|_| Error::Communication)? {
            DbResponse::Rows(rows) => Ok(rows),
            DbResponse::Error(err) => Err(err),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    async fn pluck(&self, mut statement: InnerStatement) -> Result<Option<Column>, Error> {
        let (sender, receiver) = oneshot::channel();
        statement.expected_response = ExpectedSqlResponse::Pluck;
        self.get_queue_sender()
            .send(DbRequest::Sql(statement, sender))
            .await
            .map_err(|_| Error::Communication)?;

        match receiver.await.map_err(|_| Error::Communication)? {
            DbResponse::Pluck(value) => Ok(value),
            DbResponse::Error(err) => Err(err),
            _ => Err(Error::InvalidDbResponse),
        }
    }
}

#[inline(always)]
pub fn query<T>(sql: T) -> Statement
where
    T: ToString,
{
    Statement(crate::stmt::Statement::new(sql))
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

    /// Show how many inflight requests
    #[allow(dead_code)]
    pub fn inflight_requests(&self) -> usize {
        self.inflight_requests.load(Ordering::Relaxed)
    }

    /// Begins a transaction
    ///
    /// If the transaction is Drop it will trigger a rollback operation
    pub async fn begin(&self) -> Result<Transaction<'_>, Error> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(DbRequest::Begin(sender))
            .await
            .map_err(|_| Error::Communication)?;

        match receiver.await.map_err(|_| Error::Communication)? {
            DbResponse::Transaction(db_sender) => Ok(Transaction {
                db_sender,
                _marker: PhantomData,
            }),
            DbResponse::Error(err) => Err(err),
            _ => Err(Error::InvalidDbResponse),
        }
    }
}

impl DatabaseExecutor for AsyncRusqlite {
    #[inline(always)]
    fn get_queue_sender(&self) -> mpsc::Sender<DbRequest> {
        self.sender.clone()
    }
}

pub struct Transaction<'conn> {
    db_sender: mpsc::Sender<DbRequest>,
    _marker: PhantomData<&'conn ()>,
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        let (sender, _) = oneshot::channel();
        let _ = self.db_sender.try_send(DbRequest::Rollback(sender));
    }
}

impl Transaction<'_> {
    pub async fn commit(self) -> Result<(), Error> {
        let (sender, receiver) = oneshot::channel();
        self.db_sender
            .send(DbRequest::Commit(sender))
            .await
            .map_err(|_| Error::Communication)?;

        match receiver.await.map_err(|_| Error::Communication)? {
            DbResponse::Ok => Ok(()),
            DbResponse::Error(err) => Err(err),
            _ => Err(Error::InvalidDbResponse),
        }
    }

    pub async fn rollback(self) -> Result<(), Error> {
        let (sender, receiver) = oneshot::channel();
        self.db_sender
            .send(DbRequest::Rollback(sender))
            .await
            .map_err(|_| Error::Communication)?;

        match receiver.await.map_err(|_| Error::Communication)? {
            DbResponse::Ok => Ok(()),
            DbResponse::Error(err) => Err(err),
            _ => Err(Error::InvalidDbResponse),
        }
    }
}

impl DatabaseExecutor for Transaction<'_> {
    /// Get the internal sender to the SQL queue
    #[inline(always)]
    fn get_queue_sender(&self) -> mpsc::Sender<DbRequest> {
        self.db_sender.clone()
    }
}

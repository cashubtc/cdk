use std::marker::PhantomData;
use std::sync::Arc;
//use std::sync::atomic::AtomicUsize;
//use std::sync::Arc;
use std::thread::spawn;

use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};

use crate::common::SqliteConnectionManager;
use crate::mint::Error;
use crate::pool::Pool;
use crate::stmt::{Column, ExpectedSqlResponse, Statement as InnerStatement, Value};

const BUFFER_REQUEST_SIZE: usize = 10_000;

#[derive(Debug, Clone)]
pub struct AsyncRusqlite {
    sender: mpsc::Sender<DbRequest>,
    //inflight_requests: Arc<AtomicUsize>,
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
    pub fn bind<C: ToString, V: Into<Value>>(self, name: C, value: V) -> Self {
        Self(self.0.bind(name, value))
    }

    /// Bind vec
    pub fn bind_vec<C: ToString, V: Into<Value>>(self, name: C, value: Vec<V>) -> Self {
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
    let mut stmt = conn.prepare_cached(&sql.sql)?;
    for (name, value) in sql.args {
        let index = stmt
            .parameter_index(&name)
            .map_err(|_| Error::MissingParameter(name.clone()))?
            .ok_or(Error::MissingParameter(name))?;

        stmt.raw_bind_parameter(index, value)?;
    }

    let columns = stmt.column_count();

    Ok(match sql.expected_response {
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
    })
}

fn rusqlite_worker(
    mut receiver: mpsc::Receiver<DbRequest>,
    pool: Arc<Pool<SqliteConnectionManager>>,
) {
    while let Some(request) = receiver.blocking_recv() {
        match request {
            DbRequest::Sql(sql, reply_to) => {
                let conn = match pool.get() {
                    Ok(conn) => conn,
                    Err(err) => {
                        let _ = reply_to.send(DbResponse::Error(err.into()));
                        continue;
                    }
                };

                let result = process_query(&conn, sql);
                let _ = match result {
                    Ok(ok) => reply_to.send(ok),
                    Err(err) => reply_to.send(DbResponse::Error(err)),
                };
                drop(conn);
            }
            DbRequest::Begin(reply_to) => {
                let (sender, mut receiver) = mpsc::channel(BUFFER_REQUEST_SIZE);
                let mut conn = match pool.get() {
                    Ok(conn) => conn,
                    Err(err) => {
                        let _ = reply_to.send(DbResponse::Error(err.into()));
                        continue;
                    }
                };

                let tx = match conn.transaction() {
                    Ok(tx) => tx,
                    Err(err) => {
                        let _ = reply_to.send(DbResponse::Error(err.into()));
                        continue;
                    }
                };

                // Transaction has begun successfully, send the `sender` back to the caller
                // and wait for statements to execute. On `Drop` the wrapper transaction
                // should send a `rollback`.
                //
                let _ = reply_to.send(DbResponse::Transaction(sender));

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
                        let _ = tx.rollback();
                        break;
                    };

                    match request {
                        DbRequest::Commit(reply_to) => {
                            let _ = reply_to.send(match tx.commit() {
                                Ok(()) => DbResponse::Ok,
                                Err(err) => DbResponse::Error(err.into()),
                            });
                            break;
                        }
                        DbRequest::Rollback(reply_to) => {
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
    }
}

#[async_trait::async_trait]
pub trait DatabaseExecutor {
    fn get_queue_sender(&self) -> mpsc::Sender<DbRequest>;

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
pub fn query<T: ToString>(sql: T) -> Statement {
    Statement(crate::stmt::Statement::new(sql))
}

impl AsyncRusqlite {
    pub fn new(pool: Arc<Pool<SqliteConnectionManager>>) -> Self {
        let (sender, receiver) = mpsc::channel(BUFFER_REQUEST_SIZE);
        spawn(move || {
            rusqlite_worker(receiver, pool);
        });

        Self {
            sender,
            //inflight_requests: Arc::new(0.into()),
        }
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

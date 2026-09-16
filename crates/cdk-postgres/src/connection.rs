//! Shared Postgres connection setup.
//!
//! Both the database pool ([`crate::PostgresConnection`]) and the pub/sub bus
//! ([`crate::bus`]) resolve the [`SslMode`] and call [`tokio_postgres::connect`]
//! the same way, and differ only in what drives the returned connection. The
//! pool spawns a task that awaits it to detect staleness
//! ([`connect_and_drive`] with [`AwaitDrive`]); the bus reads notifications off
//! it, so [`connect_listening`] hands the connection back inside a stream of
//! payloads instead of spawning anything.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::Poll;

use cdk_common::database::Error;
use cdk_common::task::spawn;
use futures_util::{stream, Stream};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::task::JoinHandle;
use tokio_postgres::{AsyncMessage, Client, Connection};

use crate::{PgConfig, SslMode};

/// Stream of `LISTEN` payloads read off a live connection. A connection error
/// is reported as the stream's error so the caller can reconnect.
pub(crate) type Payloads = Pin<Box<dyn Stream<Item = Result<String, String>> + Send>>;

/// Connect using the config's TLS policy and return the client together with
/// the connection's notification payloads.
///
/// The connection lives inside the stream rather than a spawned task, so
/// dropping the stream closes it and there is no handle to abort. It also means
/// the stream has to be polled for any request on `client` to make progress.
pub(crate) async fn connect_listening(config: &PgConfig) -> Result<(Client, Payloads), Error> {
    let (connection_config, tls) = config.connection()?;

    match tls {
        SslMode::NoTls(tls) => {
            let (client, connection) = connection_config
                .connect(tls)
                .await
                .map_err(|err| Error::Database(Box::new(err)))?;
            Ok((client, payloads(connection)))
        }
        SslMode::NativeTls(tls) => {
            let (client, connection) = connection_config
                .connect(tls)
                .await
                .map_err(|err| Error::Database(Box::new(err)))?;
            Ok((client, payloads(connection)))
        }
    }
}

/// Turn a connection into the stream of its notification payloads, dropping the
/// other async messages it carries.
fn payloads<S, T>(connection: Connection<S, T>) -> Payloads
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut connection = Box::pin(connection);

    Box::pin(stream::poll_fn(move |cx| loop {
        return match connection.as_mut().poll_message(cx) {
            Poll::Ready(Some(Ok(AsyncMessage::Notification(notification)))) => {
                Poll::Ready(Some(Ok(notification.payload().to_string())))
            }
            Poll::Ready(Some(Ok(_))) => continue,
            Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err.to_string()))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        };
    }))
}

/// How to drive a live `tokio-postgres` connection.
///
/// The method is generic over the connection's stream types so a single
/// implementation works for both the plain and the TLS connection produced by
/// the two [`SslMode`] variants.
pub(crate) trait DriveConnection: Send + 'static {
    /// Consume the connection and return the future that drives it to
    /// completion.
    fn drive<S, T>(self, connection: Connection<S, T>) -> Pin<Box<dyn Future<Output = ()> + Send>>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static;
}

/// Connect using the config's TLS policy and spawn a task that drives the
/// connection with `drive`.
///
/// Returns the client and the driver's join handle. Callers that only need the
/// client (fire-and-forget driving) may ignore the handle; the bus awaits it to
/// detect a dropped connection.
pub(crate) async fn connect_and_drive<D>(
    config: &PgConfig,
    drive: D,
) -> Result<(Client, JoinHandle<()>), Error>
where
    D: DriveConnection,
{
    let (connection_config, tls) = config.connection()?;

    match tls {
        SslMode::NoTls(tls) => {
            let (client, connection) = connection_config
                .connect(tls)
                .await
                .map_err(|err| Error::Database(Box::new(err)))?;
            Ok((client, spawn(drive.drive(connection))))
        }
        SslMode::NativeTls(tls) => {
            let (client, connection) = connection_config
                .connect(tls)
                .await
                .map_err(|err| Error::Database(Box::new(err)))?;
            Ok((client, spawn(drive.drive(connection))))
        }
    }
}

/// Driver used by the connection pool: await the connection to completion and
/// mark the resource stale when it ends, so the pool discards it.
pub(crate) struct AwaitDrive {
    stale: Arc<AtomicBool>,
}

impl AwaitDrive {
    /// Create a driver that flips `stale` to `true` when the connection ends.
    pub(crate) fn new(stale: Arc<AtomicBool>) -> Self {
        Self { stale }
    }
}

impl DriveConnection for AwaitDrive {
    fn drive<S, T>(self, connection: Connection<S, T>) -> Pin<Box<dyn Future<Output = ()> + Send>>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Box::pin(async move {
            let _ = connection.await;
            self.stale.store(true, Ordering::Release);
        })
    }
}

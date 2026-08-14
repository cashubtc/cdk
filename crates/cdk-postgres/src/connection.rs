//! Shared Postgres connection setup.
//!
//! Both the database pool ([`crate::PostgresConnection`]) and the pub/sub bus
//! ([`crate::bus`]) open a `tokio-postgres` connection the same way: resolve the
//! [`SslMode`], call [`tokio_postgres::connect`], and spawn a task to drive the
//! returned connection future. They differ only in how the connection is
//! driven: the pool awaits it to detect staleness, while the bus polls it for
//! notifications. [`connect_and_drive`] captures the shared part; the caller
//! supplies the driving behavior through [`DriveConnection`].

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cdk_common::task::spawn;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::task::JoinHandle;
use tokio_postgres::{connect, Client, Connection, Error as PgError};

use crate::{PgConfig, SslMode};

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

/// Connect using the config's SSL mode and spawn a task that drives the
/// connection with `drive`.
///
/// Returns the client and the driver's join handle. Callers that only need the
/// client (fire-and-forget driving) may ignore the handle; the bus awaits it to
/// detect a dropped connection.
pub(crate) async fn connect_and_drive<D>(
    config: &PgConfig,
    drive: D,
) -> Result<(Client, JoinHandle<()>), PgError>
where
    D: DriveConnection,
{
    match config.tls() {
        SslMode::NoTls(tls) => {
            let (client, connection) = connect(config.url(), tls).await?;
            Ok((client, spawn(drive.drive(connection))))
        }
        SslMode::NativeTls(tls) => {
            let (client, connection) = connect(config.url(), tls).await?;
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

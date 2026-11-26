//! PostgreSQL's committed mint state feed.

use std::future::poll_fn;

use cdk_common::database::mint::{MintStateChange, MintStateChanges};
use cdk_common::database::Error;
use cdk_common::Amount;
use futures_util::stream;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_postgres::{AsyncMessage, Client, Connection};

use super::{tls, PgConfig, SslMode};

pub(super) async fn subscribe(config: &PgConfig) -> Result<MintStateChanges, Error> {
    let (connection_config, tls) = tls::configure(&config.url, config.tls_mode.as_deref())?;
    let connect = async {
        match tls {
            SslMode::NoTls(tls) => {
                let (client, connection) =
                    connection_config.connect(tls).await.map_err(db_error)?;
                listen(client, connection, config.schema.as_deref()).await
            }
            SslMode::NativeTls(tls) => {
                let (client, connection) =
                    connection_config.connect(tls).await.map_err(db_error)?;
                listen(client, connection, config.schema.as_deref()).await
            }
        }
    };
    tokio::time::timeout(config.connection_timeout, connect)
        .await
        .map_err(|err| Error::Database(Box::new(err)))?
}

async fn listen<S, T>(
    client: Client,
    mut connection: Connection<S, T>,
    schema: Option<&str>,
) -> Result<MintStateChanges, Error>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    {
        let setup = async {
            let channel: String = client
                .query_one(
                    "SELECT 'cdk_mint_' || pg_catalog.md5(COALESCE($1::text, current_schema()))",
                    &[&schema],
                )
                .await
                .map_err(db_error)?
                .try_get(0)
                .map_err(db_error)?;
            if channel.len() != 41
                || !channel
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(Error::Internal(
                    "Invalid PostgreSQL mint notification channel".to_string(),
                ));
            }
            client
                .batch_execute(&format!("LISTEN {channel}"))
                .await
                .map_err(db_error)
        };
        tokio::pin!(setup);
        loop {
            tokio::select! {
                result = &mut setup => {
                    result?;
                    break;
                }
                message = poll_fn(|cx| connection.poll_message(cx)) => match message {
                    Some(Ok(_)) => {}
                    Some(Err(err)) => return Err(db_error(err)),
                    None => return Err(Error::Internal("PostgreSQL notification connection closed".to_string())),
                }
            }
        }
    }

    // The feed owns its dedicated session. Dropping it closes the session
    // without leaving a connection driver or an unbounded queue behind.
    Ok(Box::pin(stream::unfold(
        (client, connection),
        |(client, mut connection)| async {
            loop {
                let message = poll_fn(|cx| connection.poll_message(cx)).await?;
                let item = match message {
                    Ok(AsyncMessage::Notification(notification)) => decode(notification.payload()),
                    Ok(_) => continue,
                    Err(err) => Err(db_error(err)),
                };
                return Some((item, (client, connection)));
            }
        },
    )))
}

fn db_error(error: tokio_postgres::Error) -> Error {
    Error::Database(Box::new(error))
}

fn decode(payload: &str) -> Result<MintStateChange, Error> {
    let invalid = || Error::Internal("Invalid PostgreSQL mint notification".to_string());
    let mut fields = payload.split(':');
    let change = match fields.next() {
        Some("proof") => MintStateChange::Proof(
            fields
                .next()
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?,
            fields
                .next()
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?,
        ),
        Some("mint") => MintStateChange::MintQuote(
            fields
                .next()
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?,
            Amount::from(
                fields
                    .next()
                    .ok_or_else(invalid)?
                    .parse::<u64>()
                    .map_err(|_| invalid())?,
            ),
            Amount::from(
                fields
                    .next()
                    .ok_or_else(invalid)?
                    .parse::<u64>()
                    .map_err(|_| invalid())?,
            ),
        ),
        Some("melt") => MintStateChange::MeltQuote(
            fields
                .next()
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?,
            fields
                .next()
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?,
        ),
        _ => return Err(invalid()),
    };
    if fields.next().is_some() {
        return Err(invalid());
    }
    Ok(change)
}

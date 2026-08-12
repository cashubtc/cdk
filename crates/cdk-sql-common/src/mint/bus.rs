//! Cross-instance pub/sub bus backed by a shared SQL table (polling outbox).
//!
//! The mint's NUT-17 notifications are in-process by default: an event
//! published on one instance only reaches subscribers connected to that same
//! instance. [`SqlBus`] implements [`cdk_common::pub_sub::Bus`] so several mint
//! instances sharing a database also share notifications, using nothing but
//! plain `INSERT`/`SELECT` statements.
//!
//! This is the portable alternative to the Postgres `LISTEN`/`NOTIFY` bus. It
//! works on any backend the SQL layer supports (SQLite, Postgres) and, because
//! it never holds a session-pinned connection, it also works when Postgres is
//! reached through a transaction-pooling proxy such as PgBouncer, where
//! `LISTEN` cannot.
//!
//! How it works:
//!
//! * On publish the event is delivered to local subscribers immediately and, in
//!   the background, appended as a row to the `pubsub_outbox` table.
//! * Each instance polls the table on a fixed interval, reading rows newer than
//!   its cursor that were not written by itself (filtered by `origin`), and
//!   injects them into its own local fan-out.
//! * Old rows are pruned on a coarser cadence so the table does not grow without
//!   bound.
//!
//! Compared to `LISTEN`/`NOTIFY`:
//!
//! * Latency is bounded by the poll interval rather than being near-instant.
//! * There is no 8000-byte payload cap: the event lives in a row, not a
//!   notification argument.
//! * A restart or missed poll does not silently drop events; the cursor resumes
//!   and catches up, unless the rows have already aged past the retention
//!   window, in which case those subscribers recover on their next
//!   `fetch_events` backfill (the same guarantee the mint already relies on).
//!
//! Ordering caveat: rows are read in `id` order and the cursor advances to the
//! highest id seen. Under concurrent writers a row can commit after a
//! higher-id row is already visible, so it may be skipped by the cursor. This
//! matches the best-effort contract of the live push: correctness comes from
//! the database-backed backfill, not from the bus, so a skipped live event is
//! recovered on the subscriber's next read.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::database::Error;
use cdk_common::pub_sub::{Bus, LocalDelivery, Spec};
use cdk_common::util::unix_time;
use tokio::sync::watch;

use crate::pool::{DatabasePool, Pool};
use crate::stmt::query;
use crate::value::Value;

/// Default interval between polls of the outbox table.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Default age after which delivered outbox rows are pruned.
const DEFAULT_RETENTION: Duration = Duration::from_secs(3600);

/// Maximum rows read in a single poll, to bound memory and query cost.
const POLL_BATCH: i64 = 512;

/// Smallest accepted poll interval. Anything shorter turns the poll loop into a
/// tight query loop against the mint database.
pub const MIN_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Retention must cover at least this many poll intervals, so a row is still
/// there when every instance next polls.
pub const MIN_RETENTION_POLL_INTERVALS: u32 = 10;

/// Rejected [`SqlBusOptions`] timings.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidOptions {
    /// Poll interval below [`MIN_POLL_INTERVAL`].
    #[error("poll interval {0:?} is below the {MIN_POLL_INTERVAL:?} minimum")]
    PollInterval(Duration),
    /// Retention too small for the configured poll interval, which would prune
    /// outbox rows before peers have read them.
    #[error(
        "retention {retention:?} is too short for a {poll_interval:?} poll interval; \
         use at least {minimum:?}"
    )]
    Retention {
        /// Configured retention.
        retention: Duration,
        /// Configured poll interval.
        poll_interval: Duration,
        /// Smallest retention accepted for that poll interval.
        minimum: Duration,
    },
}

/// Configuration for a [`SqlBus`].
#[derive(Clone, Debug)]
pub struct SqlBusOptions {
    /// Interval between polls of the outbox table. Lower means fresher live
    /// events at the cost of more queries.
    pub poll_interval: Duration,
    /// Age after which an outbox row is eligible for pruning. Must comfortably
    /// exceed `poll_interval` so no instance misses a row before it is deleted.
    pub retention: Duration,
}

impl Default for SqlBusOptions {
    fn default() -> Self {
        Self {
            poll_interval: DEFAULT_POLL_INTERVAL,
            retention: DEFAULT_RETENTION,
        }
    }
}

impl SqlBusOptions {
    /// Reject timings that would busy-poll the database or drop outbox rows
    /// before peers have read them.
    pub fn validate(&self) -> Result<(), InvalidOptions> {
        if self.poll_interval < MIN_POLL_INTERVAL {
            return Err(InvalidOptions::PollInterval(self.poll_interval));
        }

        let minimum = self
            .poll_interval
            .saturating_mul(MIN_RETENTION_POLL_INTERVALS);
        if self.retention < minimum {
            return Err(InvalidOptions::Retention {
                retention: self.retention,
                poll_interval: self.poll_interval,
                minimum,
            });
        }

        Ok(())
    }
}

/// A connected SQL bus, not yet bound to a subscriber set.
///
/// [`SqlBusConnector::connect`] records the current outbox high-water mark as
/// the starting cursor (so history is not replayed); [`SqlBusConnector::build`]
/// then attaches it to a [`Pubsub`](cdk_common::pub_sub::Pubsub) via its
/// [`LocalDelivery`] handle and starts the background poll loop.
#[allow(missing_debug_implementations)]
pub struct SqlBusConnector<RM>
where
    RM: DatabasePool + 'static,
{
    pool: Arc<Pool<RM>>,
    origin: String,
    cursor: i64,
    options: SqlBusOptions,
}

impl<RM> SqlBusConnector<RM>
where
    RM: DatabasePool + 'static,
{
    /// Prepare a bus over `pool`, reusing the caller's connection pool.
    ///
    /// Reads the current maximum outbox id so the poll loop starts from "now"
    /// and only delivers events published after this instance came up, matching
    /// `LISTEN`/`NOTIFY` semantics.
    pub async fn connect(pool: Arc<Pool<RM>>, options: SqlBusOptions) -> Result<Self, Error> {
        options
            .validate()
            .map_err(|err| Error::Database(Box::new(err)))?;

        let cursor = {
            let conn = pool.get().await.map_err(|e| Error::Database(Box::new(e)))?;
            query("SELECT COALESCE(MAX(id), 0) FROM pubsub_outbox")?
                .pluck(&*conn)
                .await?
                .and_then(value_as_i64)
                .unwrap_or(0)
        };

        Ok(Self {
            pool,
            origin: uuid::Uuid::new_v4().to_string(),
            cursor,
            options,
        })
    }

    /// Attach the bus to a subscriber set and return it as a [`Bus`].
    ///
    /// Spawns the poll loop that reads peer events from the outbox and delivers
    /// them through `local`. The loop stops when the returned bus is dropped.
    pub fn build<S>(self, local: LocalDelivery<S>) -> Arc<dyn Bus<S>>
    where
        S: Spec + 'static,
    {
        let SqlBusConnector {
            pool,
            origin,
            cursor,
            options,
        } = self;

        let (shutdown, shutdown_rx) = watch::channel(());

        let poll_pool = pool.clone();
        let poll_origin = origin.clone();
        cdk_common::task::spawn(poll_loop::<S, RM>(
            poll_pool,
            local.clone(),
            poll_origin,
            cursor,
            options,
            shutdown_rx,
        ));

        Arc::new(SqlBus {
            local,
            pool,
            origin,
            _shutdown: shutdown,
            _spec: PhantomData,
        })
    }
}

/// SQL-backed [`Bus`]. See the module docs.
#[allow(missing_debug_implementations)]
pub struct SqlBus<S, RM>
where
    S: Spec + 'static,
    RM: DatabasePool + 'static,
{
    local: LocalDelivery<S>,
    pool: Arc<Pool<RM>>,
    origin: String,
    /// Dropped when the bus is dropped, which stops the poll loop.
    _shutdown: watch::Sender<()>,
    _spec: PhantomData<fn() -> S>,
}

impl<S, RM> Bus<S> for SqlBus<S, RM>
where
    S: Spec + 'static,
    RM: DatabasePool + 'static,
{
    fn publish(&self, event: S::Event) {
        // Serialize before delivering locally, since delivery consumes the event.
        let payload = match serde_json::to_string(&event) {
            Ok(payload) => payload,
            Err(err) => {
                tracing::warn!("sql bus: failed to serialize event: {err}");
                self.local.deliver(event);
                return;
            }
        };

        self.local.deliver(event);

        let pool = self.pool.clone();
        let origin = self.origin.clone();
        cdk_common::task::spawn(async move {
            let conn = match pool.get().await {
                Ok(conn) => conn,
                Err(err) => {
                    tracing::warn!("sql bus: no connection to append event: {err}");
                    return;
                }
            };

            let stmt = match query(
                "INSERT INTO pubsub_outbox (origin, payload, created_time) \
                 VALUES (:origin, :payload, :created_time)",
            ) {
                Ok(stmt) => stmt,
                Err(err) => {
                    tracing::warn!("sql bus: failed to build insert: {err}");
                    return;
                }
            };

            if let Err(err) = stmt
                .bind("origin", origin)
                .bind("payload", payload)
                .bind("created_time", unix_time() as i64)
                .execute(&*conn)
                .await
            {
                tracing::warn!("sql bus: failed to append event: {err}");
            }
        });
    }
}

/// Background task: poll the outbox for peer events and prune stale rows until
/// the bus is dropped.
async fn poll_loop<S, RM>(
    pool: Arc<Pool<RM>>,
    local: LocalDelivery<S>,
    origin: String,
    mut cursor: i64,
    options: SqlBusOptions,
    mut shutdown: watch::Receiver<()>,
) where
    S: Spec + 'static,
    RM: DatabasePool + 'static,
{
    let retention_secs = options.retention.as_secs() as i64;
    // Prune at most once per retention window: pruning is idempotent and every
    // instance runs it, so a coarse cadence keeps the churn low.
    let prune_cadence = retention_secs.max(1) as u64;
    let mut last_prune = unix_time();

    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            _ = tokio::time::sleep(options.poll_interval) => {}
        }

        match poll_once::<S, RM>(&pool, &local, &origin, cursor).await {
            Ok(new_cursor) => cursor = new_cursor,
            Err(err) => tracing::warn!("sql bus: poll failed: {err}"),
        }

        let now = unix_time();
        if now.saturating_sub(last_prune) >= prune_cadence {
            let threshold = now.saturating_sub(retention_secs.max(0) as u64) as i64;
            if let Err(err) = prune::<RM>(&pool, threshold).await {
                tracing::warn!("sql bus: prune failed: {err}");
            }
            last_prune = now;
        }
    }
}

/// Read one batch of peer events newer than `cursor`, deliver each locally, and
/// return the new cursor (the highest id seen, or `cursor` if none).
async fn poll_once<S, RM>(
    pool: &Arc<Pool<RM>>,
    local: &LocalDelivery<S>,
    origin: &str,
    cursor: i64,
) -> Result<i64, Error>
where
    S: Spec + 'static,
    RM: DatabasePool + 'static,
{
    let conn = pool.get().await.map_err(|e| Error::Database(Box::new(e)))?;
    let rows = query(
        "SELECT id, payload FROM pubsub_outbox \
         WHERE id > :cursor AND origin <> :origin \
         ORDER BY id ASC LIMIT :limit",
    )?
    .bind("cursor", cursor)
    .bind("origin", origin.to_owned())
    .bind("limit", POLL_BATCH)
    .fetch_all(&*conn)
    .await?;

    let mut max = cursor;
    for row in rows {
        let id = match row.first().and_then(value_as_i64_ref) {
            Some(id) => id,
            None => continue,
        };
        max = max.max(id);

        let payload = match row.get(1) {
            Some(Value::Text(payload)) => payload,
            _ => continue,
        };

        match serde_json::from_str::<S::Event>(payload) {
            Ok(event) => local.deliver(event),
            Err(_) => tracing::warn!("sql bus: dropping malformed payload"),
        }
    }

    Ok(max)
}

/// Delete outbox rows older than `threshold` (a unix timestamp in seconds).
async fn prune<RM>(pool: &Arc<Pool<RM>>, threshold: i64) -> Result<(), Error>
where
    RM: DatabasePool + 'static,
{
    let conn = pool.get().await.map_err(|e| Error::Database(Box::new(e)))?;
    query("DELETE FROM pubsub_outbox WHERE created_time < :threshold")?
        .bind("threshold", threshold)
        .execute(&*conn)
        .await?;
    Ok(())
}

fn value_as_i64(value: Value) -> Option<i64> {
    match value {
        Value::Integer(i) => Some(i),
        _ => None,
    }
}

fn value_as_i64_ref(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(i) => Some(*i),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert!(SqlBusOptions::default().validate().is_ok());
    }

    #[test]
    fn zero_poll_interval_is_rejected() {
        let options = SqlBusOptions {
            poll_interval: Duration::ZERO,
            ..SqlBusOptions::default()
        };
        assert!(matches!(
            options.validate(),
            Err(InvalidOptions::PollInterval(_))
        ));
    }

    #[test]
    fn retention_must_outlast_several_poll_intervals() {
        let options = SqlBusOptions {
            poll_interval: Duration::from_millis(500),
            retention: Duration::from_secs(1),
        };
        assert!(matches!(
            options.validate(),
            Err(InvalidOptions::Retention { .. })
        ));

        let options = SqlBusOptions {
            retention: Duration::ZERO,
            ..SqlBusOptions::default()
        };
        assert!(matches!(
            options.validate(),
            Err(InvalidOptions::Retention { .. })
        ));
    }

    #[test]
    fn huge_poll_interval_does_not_overflow_the_retention_minimum() {
        let options = SqlBusOptions {
            poll_interval: Duration::from_millis(u64::MAX),
            retention: Duration::MAX,
        };
        assert!(options.validate().is_ok());
    }
}

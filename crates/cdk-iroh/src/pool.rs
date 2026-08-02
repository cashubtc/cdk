//! Concurrent connection pool keyed by authenticated peer.

use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use iroh::{endpoint::Connection, Endpoint, EndpointId};
use tokio::{sync::Mutex, time::Instant};

use crate::{
    address::peer_fingerprint, config::IrohTimeouts, metrics::MetricsInner, protocol, Error,
};

#[derive(Debug)]
struct PoolEntry {
    connection: Option<Connection>,
    retry_at: Option<Instant>,
    failures: u32,
    generation: u64,
}

/// A clone-shared pool which only serializes duplicate dials for one peer.
#[derive(Debug, Clone)]
pub(crate) struct ConnectionPool {
    entries: Arc<Mutex<HashMap<EndpointId, Arc<Mutex<PoolEntry>>>>>,
    timeouts: IrohTimeouts,
    max_entries: usize,
    metrics: Arc<MetricsInner>,
}

impl ConnectionPool {
    pub(crate) fn new(
        timeouts: IrohTimeouts,
        max_entries: usize,
        metrics: Arc<MetricsInner>,
    ) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            timeouts,
            max_entries,
            metrics,
        }
    }

    pub(crate) async fn connect(
        &self,
        endpoint: &Endpoint,
        endpoint_id: EndpointId,
    ) -> Result<Connection, Error> {
        let slot = {
            let mut entries = self.entries.lock().await;
            if !entries.contains_key(&endpoint_id) {
                let now = Instant::now();
                entries.retain(|_, slot| {
                    let Ok(entry) = slot.try_lock() else {
                        return true;
                    };
                    match &entry.connection {
                        Some(connection) => connection.close_reason().is_none(),
                        None => entry.retry_at.is_some_and(|retry_at| retry_at > now),
                    }
                });
                if entries.len() >= self.max_entries {
                    self.metrics
                        .admission_rejections
                        .fetch_add(1, Ordering::Relaxed);
                    return Err(Error::Admission {
                        resource: "outgoing connection pool",
                    });
                }
            }
            entries
                .entry(endpoint_id)
                .or_insert_with(|| {
                    Arc::new(Mutex::new(PoolEntry {
                        connection: None,
                        retry_at: None,
                        failures: 0,
                        generation: 0,
                    }))
                })
                .clone()
        };

        let mut entry = slot.lock().await;
        if let Some(connection) = entry
            .connection
            .as_ref()
            .filter(|connection| connection.close_reason().is_none())
        {
            self.metrics.pool_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(connection.clone());
        }

        entry.connection = None;
        if entry
            .retry_at
            .is_some_and(|retry_at| retry_at > Instant::now())
        {
            return Err(Error::ConnectBackoff {
                peer: peer_fingerprint(endpoint_id),
            });
        }

        self.metrics
            .connection_attempts
            .fetch_add(1, Ordering::Relaxed);
        tracing::debug!(peer = %endpoint_id.fmt_short(), "dialing Iroh peer");
        let connection = match tokio::time::timeout(
            self.timeouts.connect,
            endpoint.connect(endpoint_id, protocol::ALPN),
        )
        .await
        {
            Ok(Ok(connection)) => connection,
            Ok(Err(_)) => {
                self.metrics
                    .connection_failures
                    .fetch_add(1, Ordering::Relaxed);
                record_failure(&mut entry, endpoint_id);
                return Err(Error::Connect {
                    peer: peer_fingerprint(endpoint_id),
                });
            }
            Err(_) => {
                self.metrics
                    .connection_failures
                    .fetch_add(1, Ordering::Relaxed);
                self.metrics.timeouts.fetch_add(1, Ordering::Relaxed);
                record_failure(&mut entry, endpoint_id);
                return Err(Error::Timeout {
                    operation: "connect",
                });
            }
        };

        entry.failures = 0;
        entry.retry_at = None;
        entry.generation = entry.generation.wrapping_add(1);
        let generation = entry.generation;
        entry.connection = Some(connection.clone());
        drop(entry);

        let watched = connection.clone();
        let pool = self.clone();
        tokio::spawn(async move {
            let _ = watched.closed().await;
            pool.evict_generation(endpoint_id, generation).await;
        });
        Ok(connection)
    }

    pub(crate) async fn evict(&self, endpoint_id: EndpointId) {
        let slot = {
            let entries = self.entries.lock().await;
            entries.get(&endpoint_id).cloned()
        };
        let Some(slot) = slot else {
            return;
        };
        slot.lock().await.connection = None;
        self.remove_if_unshared(endpoint_id, &slot).await;
    }

    async fn evict_generation(&self, endpoint_id: EndpointId, generation: u64) {
        let slot = {
            let entries = self.entries.lock().await;
            entries.get(&endpoint_id).cloned()
        };
        let Some(slot) = slot else {
            return;
        };
        let mut entry = slot.lock().await;
        if entry.generation != generation {
            return;
        }
        entry.connection = None;
        drop(entry);
        self.remove_if_unshared(endpoint_id, &slot).await;
    }

    async fn remove_if_unshared(&self, endpoint_id: EndpointId, slot: &Arc<Mutex<PoolEntry>>) {
        let mut entries = self.entries.lock().await;
        let is_current = entries
            .get(&endpoint_id)
            .is_some_and(|current| Arc::ptr_eq(current, slot));
        if is_current && Arc::strong_count(slot) == 2 {
            entries.remove(&endpoint_id);
        }
    }
}

fn record_failure(entry: &mut PoolEntry, endpoint_id: EndpointId) {
    entry.failures = entry.failures.saturating_add(1);
    entry.retry_at = Some(Instant::now() + retry_delay(endpoint_id, entry.failures));
}

fn retry_delay(endpoint_id: EndpointId, failures: u32) -> Duration {
    static JITTER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let exponent = failures.saturating_sub(1).min(7);
    let base_millis = 250_u64.saturating_mul(1_u64 << exponent).min(30_000);
    let mut hasher = DefaultHasher::new();
    endpoint_id.hash(&mut hasher);
    failures.hash(&mut hasher);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    JITTER_SEQUENCE
        .fetch_add(1, Ordering::Relaxed)
        .hash(&mut hasher);
    let jitter = hasher.finish() % (base_millis / 4 + 1);
    Duration::from_millis(base_millis + jitter)
}

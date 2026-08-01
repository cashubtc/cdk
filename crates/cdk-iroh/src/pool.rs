//! Concurrent connection pool keyed by authenticated peer and ALPN.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::{atomic::Ordering, Arc},
};

use iroh::{endpoint::Connection, Endpoint, EndpointId};
use tokio::sync::Mutex;

use crate::{address::peer_fingerprint, config::IrohTimeouts, metrics::MetricsInner, Error};

#[derive(Debug, Clone, Eq)]
struct PoolKey {
    endpoint_id: EndpointId,
    alpn: Arc<[u8]>,
}

impl PartialEq for PoolKey {
    fn eq(&self, other: &Self) -> bool {
        self.endpoint_id == other.endpoint_id && self.alpn == other.alpn
    }
}

impl Hash for PoolKey {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.endpoint_id.hash(state);
        self.alpn.hash(state);
    }
}

#[derive(Debug)]
struct PoolEntry {
    connection: Option<Connection>,
}

/// A clone-shared pool which only serializes duplicate dials for one peer/ALPN.
#[derive(Debug, Clone)]
pub(crate) struct ConnectionPool {
    entries: Arc<Mutex<HashMap<PoolKey, Arc<Mutex<PoolEntry>>>>>,
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
        alpn: &[u8],
    ) -> Result<Connection, Error> {
        let key = PoolKey {
            endpoint_id,
            alpn: Arc::from(alpn),
        };
        let slot = {
            let mut entries = self.entries.lock().await;
            if !entries.contains_key(&key) {
                entries.retain(|_, slot| {
                    let Ok(entry) = slot.try_lock() else {
                        return true;
                    };
                    entry
                        .connection
                        .as_ref()
                        .is_none_or(|connection| connection.close_reason().is_none())
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
                .entry(key)
                .or_insert_with(|| Arc::new(Mutex::new(PoolEntry { connection: None })))
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
        self.metrics
            .connection_attempts
            .fetch_add(1, Ordering::Relaxed);
        tracing::debug!(peer = %endpoint_id.fmt_short(), "dialing Iroh peer");
        let connection =
            match tokio::time::timeout(self.timeouts.connect, endpoint.connect(endpoint_id, alpn))
                .await
            {
                Ok(Ok(connection)) => connection,
                Ok(Err(_)) => {
                    self.metrics
                        .connection_failures
                        .fetch_add(1, Ordering::Relaxed);
                    let error = Error::Connect {
                        peer: peer_fingerprint(endpoint_id),
                    };
                    drop(entry);
                    self.remove_if_unshared(endpoint_id, alpn, &slot).await;
                    return Err(error);
                }
                Err(_) => {
                    self.metrics
                        .connection_failures
                        .fetch_add(1, Ordering::Relaxed);
                    self.metrics.timeouts.fetch_add(1, Ordering::Relaxed);
                    let error = Error::Timeout {
                        operation: "connect",
                    };
                    drop(entry);
                    self.remove_if_unshared(endpoint_id, alpn, &slot).await;
                    return Err(error);
                }
            };
        entry.connection = Some(connection.clone());
        Ok(connection)
    }

    pub(crate) async fn evict(&self, endpoint_id: EndpointId, alpn: &[u8]) {
        let key = PoolKey {
            endpoint_id,
            alpn: Arc::from(alpn),
        };
        let slot = {
            let entries = self.entries.lock().await;
            entries.get(&key).cloned()
        };
        let Some(slot) = slot else {
            return;
        };
        slot.lock().await.connection = None;
        self.remove_if_unshared(endpoint_id, alpn, &slot).await;
    }

    async fn remove_if_unshared(
        &self,
        endpoint_id: EndpointId,
        alpn: &[u8],
        slot: &Arc<Mutex<PoolEntry>>,
    ) {
        let key = PoolKey {
            endpoint_id,
            alpn: Arc::from(alpn),
        };
        let mut entries = self.entries.lock().await;
        let is_current = entries
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, slot));
        if is_current && Arc::strong_count(slot) == 2 {
            entries.remove(&key);
        }
    }
}

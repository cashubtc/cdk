//! Bounded-cardinality transport metrics.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Point-in-time transport counters and gauges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricsSnapshot {
    /// Whether the local endpoint is online.
    pub endpoint_online: bool,
    /// Outgoing connection attempts.
    pub connection_attempts: u64,
    /// Failed outgoing connection attempts.
    pub connection_failures: u64,
    /// Reused pooled connections.
    pub pool_hits: u64,
    /// Currently admitted incoming connections.
    pub active_connections: usize,
    /// Currently admitted HTTP streams.
    pub active_streams: usize,
    /// Completed HTTP requests.
    pub requests: u64,
    /// Completed 2xx responses.
    pub responses_2xx: u64,
    /// Completed 4xx responses.
    pub responses_4xx: u64,
    /// Completed 5xx responses.
    pub responses_5xx: u64,
    /// Admission rejections.
    pub admission_rejections: u64,
    /// Transport timeouts.
    pub timeouts: u64,
}

#[derive(Debug, Default)]
pub(crate) struct MetricsInner {
    endpoint_online: AtomicUsize,
    pub(crate) connection_attempts: AtomicU64,
    pub(crate) connection_failures: AtomicU64,
    pub(crate) pool_hits: AtomicU64,
    pub(crate) active_connections: AtomicUsize,
    pub(crate) active_streams: AtomicUsize,
    pub(crate) requests: AtomicU64,
    pub(crate) responses_2xx: AtomicU64,
    pub(crate) responses_4xx: AtomicU64,
    pub(crate) responses_5xx: AtomicU64,
    pub(crate) admission_rejections: AtomicU64,
    pub(crate) timeouts: AtomicU64,
}

impl MetricsInner {
    pub(crate) fn set_online(&self, online: bool) {
        self.endpoint_online
            .store(usize::from(online), Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            endpoint_online: self.endpoint_online.load(Ordering::Relaxed) != 0,
            connection_attempts: self.connection_attempts.load(Ordering::Relaxed),
            connection_failures: self.connection_failures.load(Ordering::Relaxed),
            pool_hits: self.pool_hits.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
            active_streams: self.active_streams.load(Ordering::Relaxed),
            requests: self.requests.load(Ordering::Relaxed),
            responses_2xx: self.responses_2xx.load(Ordering::Relaxed),
            responses_4xx: self.responses_4xx.load(Ordering::Relaxed),
            responses_5xx: self.responses_5xx.load(Ordering::Relaxed),
            admission_rejections: self.admission_rejections.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn record_status(&self, status: u16) {
        match status {
            200..=299 => &self.responses_2xx,
            400..=499 => &self.responses_4xx,
            500..=599 => &self.responses_5xx,
            _ => return,
        }
        .fetch_add(1, Ordering::Relaxed);
    }
}

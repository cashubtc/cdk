//! Bounded-cardinality transport metrics.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Point-in-time transport counters and gauges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricsSnapshot {
    /// Configured discovery class (`n0`, `static`, or `custom`).
    pub discovery_mode: &'static str,
    /// Whether the local endpoint is online.
    pub endpoint_online: bool,
    /// Outgoing connection attempts.
    pub connection_attempts: u64,
    /// Failed outgoing connection attempts.
    pub connection_failures: u64,
    /// Successful outgoing connections initially selecting a direct path.
    pub direct_connections: u64,
    /// Successful outgoing connections initially selecting a relay path.
    pub relay_connections: u64,
    /// Successful outgoing connections without an observable selected path.
    pub unknown_path_connections: u64,
    /// Reconnects after a previously pooled connection closed.
    pub reconnects: u64,
    /// Reused pooled connections.
    pub pool_hits: u64,
    /// Currently admitted incoming connections.
    pub active_connections: usize,
    /// Currently admitted HTTP streams.
    pub active_streams: usize,
    /// Completed HTTP requests.
    pub requests: u64,
    /// Saturating sum of completed request durations in microseconds.
    pub request_duration_micros: u64,
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
    pub(crate) direct_connections: AtomicU64,
    pub(crate) relay_connections: AtomicU64,
    pub(crate) unknown_path_connections: AtomicU64,
    pub(crate) reconnects: AtomicU64,
    pub(crate) pool_hits: AtomicU64,
    pub(crate) active_connections: AtomicUsize,
    pub(crate) active_streams: AtomicUsize,
    pub(crate) requests: AtomicU64,
    pub(crate) request_duration_micros: AtomicU64,
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

    pub(crate) fn snapshot(&self, discovery_mode: &'static str) -> MetricsSnapshot {
        MetricsSnapshot {
            discovery_mode,
            endpoint_online: self.endpoint_online.load(Ordering::Relaxed) != 0,
            connection_attempts: self.connection_attempts.load(Ordering::Relaxed),
            connection_failures: self.connection_failures.load(Ordering::Relaxed),
            direct_connections: self.direct_connections.load(Ordering::Relaxed),
            relay_connections: self.relay_connections.load(Ordering::Relaxed),
            unknown_path_connections: self.unknown_path_connections.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
            pool_hits: self.pool_hits.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
            active_streams: self.active_streams.load(Ordering::Relaxed),
            requests: self.requests.load(Ordering::Relaxed),
            request_duration_micros: self.request_duration_micros.load(Ordering::Relaxed),
            responses_2xx: self.responses_2xx.load(Ordering::Relaxed),
            responses_4xx: self.responses_4xx.load(Ordering::Relaxed),
            responses_5xx: self.responses_5xx.load(Ordering::Relaxed),
            admission_rejections: self.admission_rejections.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn record_request_duration(&self, duration: std::time::Duration) {
        let micros = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
        let _ = self.request_duration_micros.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |current| Some(current.saturating_add(micros)),
        );
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

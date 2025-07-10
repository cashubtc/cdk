use std::time::Instant;

use axum::{
    body::Body,
    extract::{MatchedPath, State},
    http::Request,
    middleware::Next,
    response::Response,
};

use crate::MintState;
/// This module provides middleware for collecting HTTP metrics in the CDK Axum server.
///
/// The metrics collected include:
/// - Total number of HTTP requests by endpoint and status code
/// - HTTP request duration by endpoint
///
/// These metrics are exposed via the Prometheus endpoint provided by the cdk-prometheus crate.
///
/// To use this middleware, ensure the "prometheus" feature is enabled and the middleware
/// is applied to your router using `from_fn(metrics_middleware)`.
///
/// The middleware requires a MintState with a metrics field of type Option<Arc<CdkMetrics>>.

/// Middleware for recording HTTP metrics
#[cfg(feature = "prometheus")]
pub async fn metrics_middleware(
    State(state): State<MintState>,
    matched_path: Option<MatchedPath>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let start_time = Instant::now();

    let response = next.run(req).await;

    #[cfg(feature = "prometheus")]
    // Use the matched route pattern if available,
    // otherwise fall back to empty string to reduce the memory footprint
    let endpoint_path = matched_path
        .map(|mp| mp.as_str().to_string())
        .unwrap_or_else(|| "".to_string());

    #[cfg(feature = "prometheus")]
    {
        let status_code = response.status().as_u16().to_string();
        let request_duration = start_time.elapsed().as_secs_f64();
        let metrics = &state.mint.metrics;
        metrics.record_http_request(&endpoint_path, &status_code);
        metrics.record_http_request_duration(request_duration, &endpoint_path);
    }

    response
}

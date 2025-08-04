#[cfg(feature = "prometheus")]
use std::time::Instant;

#[cfg(feature = "prometheus")]
use axum::body::Body;
#[cfg(feature = "prometheus")]
use axum::extract::MatchedPath;
#[cfg(feature = "prometheus")]
use axum::http::Request;
#[cfg(feature = "prometheus")]
use axum::middleware::Next;
#[cfg(feature = "prometheus")]
use axum::response::Response;
#[cfg(feature = "prometheus")]
use cdk_prometheus::global;

/// Global metrics middleware that uses the singleton instance.
/// This version doesn't require access to MintState and can be used in any Axum application.
#[cfg(feature = "prometheus")]
pub async fn global_metrics_middleware(
    matched_path: Option<MatchedPath>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let start_time = Instant::now();

    let response = next.run(req).await;

    let endpoint_path = matched_path
        .map(|mp| mp.as_str().to_string())
        .unwrap_or_default();

    let status_code = response.status().as_u16().to_string();
    let request_duration = start_time.elapsed().as_secs_f64();

    // Always use global metrics
    global::record_http_request(&endpoint_path, &status_code);
    global::record_http_request_duration(request_duration, &endpoint_path);

    response
}

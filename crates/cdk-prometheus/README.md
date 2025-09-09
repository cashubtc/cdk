# CDK Prometheus

A small, focused crate that provides Prometheus metrics for CDK-based services. It bundles a ready-to-use metrics registry, a background HTTP server to expose metrics, helper functions for common CDK domains (HTTP, auth, Lightning, DB, mint operations), and an ergonomic macro for conditional metrics recording.

- Out-of-the-box metrics for HTTP, auth, Lightning payments, database, and mint operations
- Global, lazily-initialized metrics instance you can use anywhere
- Optional background server to expose metrics on /metrics
- Re-exports the prometheus crate for custom instrumentation
- Optional system metrics (feature-gated)

## Installation

Add the crate to your Cargo.toml (replace the version as needed):

```toml 
[dependencies] cdk-prometheus = { version = "0.1", features = ["system-metrics"] }
``` 

- Feature flags:
  - system-metrics: include basic process/system metrics collected periodically.

Note for downstream crates: the provided record_metrics! macro is gated at call-site by a feature named prometheus. If you use that macro, declare a prometheus feature in your application crate and enable it to compile the macro calls into real metrics (otherwise they no-op).

## Quick start
### Docker
Start Prometheus and Grafana with docker-compose:
```
docker compose up -d prometheus grafana
```
Start your mintd 
```
./mintd -w ~/.cdk-mintd
```
Check Prometheus and Grafana 
* `curl localhost:9000/metrics` for checking CDK metrics  
* `http://localhost:9090/targets?search=` checking the prometheus collector (you should see http://host.docker.internal:9000/metrics)
* `http://localhost:3011/d/cdk-mint-dashboard/cdk-mint-dashboard` Grafana dashboard (default login: admin/admin)

### Rust 
Expose a Prometheus endpoint with a default registry and CDK metrics:
```rust 
use cdk_prometheus::start_default_server_with_metrics;
#[tokio::main] async fn main() -> anyhow::Result<()> { // Starts an HTTP server (default bind and path) and registers CDK metrics into its registry start_default_server_with_metrics().await?; Ok(()) }
``` 

Or start it in the background (e.g., from your application bootstrap):
```rust
use cdk_prometheus::start_background_server_with_metrics;
fn main() -> anyhow::Result<()> { let _handle = start_background_server_with_metrics()?; // Continue bootstrapping your application... Ok(()) }
``` 

## Recording metrics

You can record metrics using:
- The global helpers (simple functions)
- The global singleton METRICS (direct methods)
- The record_metrics! macro (conditional recording with an optional instance)

### Global helpers
```rust 
use cdk_prometheus::global;
fn handle_request() { 
    global::record_http_request("/health", "200"); global::record_http_request_duration(0.003, "/health");
    global::record_auth_attempt();
    global::record_auth_success();
    
    // Lightning and DB
    global::record_lightning_payment(1500.0, 2.0); // amount, fee (both in base units you track)
    global::record_db_operation(0.015, "select_user");
    global::set_db_connections_active(8);
    
    // Mint operations
    global::inc_in_flight_requests("get_payment_quote");
    // ... do work ...
    global::record_mint_operation("get_payment_quote", true);
    global::record_mint_operation_histogram("get_payment_quote", true, 0.021);
    global::dec_in_flight_requests("get_payment_quote");
    
    // Errors
    global::record_error();
}
``` 

### Using the global METRICS instance directly
```rust 
use cdk_prometheus::METRICS;
fn do_db_work() { METRICS.record_db_operation(0.005, "update_user"); }
``` 

### Using the record_metrics! macro

The macro lets you write grouped calls concisely and optionally pass an instance to use; if no instance is present, it automatically falls back to the global helpers. At call-site, wrap your invocations with a prometheus feature so they can be disabled in minimal builds.
```rust 
use cdk_prometheus::record_metrics;
fn run_operation(metrics_opt: Option<cdk_prometheus::CdkMetrics>) { // Use instance if present, otherwise fallback to global record_metrics!(metrics_opt => { inc_in_flight_requests("make_payment"); record_mint_operation("make_payment", true); record_mint_operation_histogram("make_payment", true, 0.123); dec_in_flight_requests("make_payment"); });
    // Or call directly on the global helpers
    record_metrics!({
        record_error();
    });
}
``` 

## Exposing the /metrics endpoint

If you just need sane defaults, use the convenience starters shown above. If you want finer control (bind address, path, system metrics), build the server explicitly:
```rust 
use cdk_prometheus::{PrometheusBuilder, PrometheusServer, CdkMetrics, prometheus::Registry};
fn build_and_run() -> anyhow::Result<tokio::task::JoinHandle<anyhow::Result<()>>> { // Build a server wired up with the default CDK metrics let server = PrometheusBuilder::new().build_with_cdk_metrics()?; let handle = server.start_background(); Ok(handle) }
``` 

Notes:
- Default bind address and metrics path are set by the server configuration (commonly 127.0.0.1:9090 and /metrics).
- With system-metrics enabled, the server periodically updates process/system gauges.

## What’s included

The default CDK metrics instance (CdkMetrics) registers and maintains counters, histograms, and gauges for common areas:
- HTTP: request totals, durations
- Auth: attempts and successes
- Lightning: payment totals, amounts, fees
- Database: operation totals, latencies, active connections
- Mint: operation totals, in-flight gauges, per-operation latencies
- Errors: a general counter

You can use these immediately through the global helpers or the METRICS instance.

## Adding custom metrics

This crate re-exports the prometheus crate and exposes the underlying Registry so you can define and register your own metrics:
```rust 
use cdk_prometheus::{prometheus, global};
fn register_custom_metric() -> Result<(), prometheus::Error> { let my_counter = prometheus::IntCounter::new("my_counter", "A custom counter")?; let registry = global::registry(); // Arcregistry.register(Box::new(my_counter.clone()))?;
    my_counter.inc();
    Ok(())
}
``` 

If you prefer instance-level control:
```rust 
use std::sync::Arc; use cdk_prometheus::{create_cdk_metrics, prometheus};
fn with_instance() -> anyhow::Result<()> { let metrics = create_cdk_metrics()?; let registry: Arc[prometheus::Registry]() = metrics.registry();
    let hist = prometheus::Histogram::with_opts(
    prometheus::HistogramOpts::new("my_latency_seconds", "My op latency")
)?;
registry.register(Box::new(hist))?;
Ok(())
}
``` 

## Scraping with Prometheus

Example scrape_config:
```yaml 
scrape_configs:
- job_name: 'cdk' 
  scrape_interval: 15s 
  static_configs:
  - targets: ['127.0.0.1:9090']
``` 

If you changed the bind address or path, make sure to update targets or the metrics_path in your Prometheus configuration accordingly.

## System metrics (optional)

Enable the system-metrics feature to export basic process/system metrics. The server updates these at a configurable interval.
```toml 
cdk-prometheus = { version = "0.1", features = ["system-metrics"] }
``` 

## Error handling

Common error types surfaced by this crate include:
- Server bind failures
- Metrics collection/registry errors
- System metrics collection errors (when enabled)

Handle these at startup and monitor logs during runtime.

## Best practices

- Run the metrics server on localhost or a private interface and use a Prometheus agent/sidecar if needed.
- Register application-specific metrics early in your bootstrap so they are visible from the first scrape.
- Use histograms for latencies and size distributions; use counters for event totals; use gauges for in-flight or current-state values.
- Keep label cardinality bounded.

## License

MIT
```

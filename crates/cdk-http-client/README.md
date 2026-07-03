# cdk-http-client

HTTP client abstraction for the Cashu Development Kit (CDK).

This crate provides an HTTP client wrapper and transport trait abstraction that allows
other CDK crates to avoid direct dependencies on a specific backend.

## Features

- `bitreq` (default) - enables the bitreq backend and transport implementation
- `reqwest` - enables the reqwest backend and transport implementation
- `tor` - enables the Tor transport implementation

Native builds must enable at least one HTTP backend: `bitreq` or `reqwest`.
`bitreq` is the default. `cdk-common/http` enables it, and `cdk`'s `wallet` and
`mint` features depend on that, so any CDK build that uses HTTP gets a working
client out of the box — including `cdk --no-default-features --features wallet`.
Depending on this crate directly with `--no-default-features` and selecting no
backend is unsupported and fails at compile time with a clear error.

The backend features are additive. When both `bitreq` and `reqwest` are enabled,
`reqwest` takes precedence and is the single backend compiled in (it is a strict
superset, adding SOCKS proxy and invalid-certificate support). This means Cargo
feature unification across a dependency graph never produces a build conflict: a
crate that enables `reqwest` and another that enables `bitreq` resolve to
`reqwest`.

Use default bitreq backend:

```bash
cargo check -p cdk-http-client
```

Use reqwest backend (standalone):

```bash
cargo check -p cdk-http-client --no-default-features --features reqwest
```

To use the `reqwest` backend with CDK, add a direct `cdk-http-client` dependency
with the `reqwest` feature. It takes precedence wherever it is enabled, so there
is no need to disable default features:

```toml
[dependencies]
cdk = { version = "0.17.0", features = ["wallet"] }
cdk-http-client = { version = "0.17.0", features = ["reqwest"] }
```

## Usage

```rust
use cdk_http_client::{HttpClient, Response};
use serde::Deserialize;

#[derive(Deserialize)]
struct ApiResponse {
    message: String,
}

async fn example() -> Response<ApiResponse> {
    let client = HttpClient::new();
    client.fetch("https://api.example.com/data").await
}
```

## API

### Builder methods (return `RequestBuilder`):
- `get(url)` - GET request builder
- `post(url)` - POST request builder
- `patch(url)` - PATCH request builder

### Convenience methods (return deserialized JSON):
- `fetch<R>(url)` - simple GET returning JSON
- `post_json<B, R>(url, body)` - POST with JSON body
- `post_form<F, R>(url, form)` - POST with form data
- `patch_json<B, R>(url, body)` - PATCH with JSON body

### Transport types:
- `Transport` - trait consumed by higher-level CDK components
- `Async` - default transport using the selected backend
- `BitreqTransport` - alias for `Async` when `bitreq` is selected
- `ReqwestTransport` - alias for `Async` when `reqwest` is enabled
- `TorAsync` - Tor-specific transport (enabled by `tor` feature)

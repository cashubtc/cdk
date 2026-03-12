# cdk-http-client

HTTP client abstraction for the Cashu Development Kit (CDK).

This crate provides an HTTP client wrapper and transport trait abstraction that allows
other CDK crates to avoid direct dependencies on a specific backend.

## Features

- `bitreq` (default) - enables the bitreq backend and transport implementation
- `reqwest` - enables the reqwest backend and transport implementation
- `tor` - enables the Tor transport implementation

`bitreq` and `reqwest` are mutually exclusive.

Use reqwest backend:

```bash
cargo check -p cdk-http-client --no-default-features --features reqwest
```

Use default bitreq backend:

```bash
cargo check -p cdk-http-client
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
- `BitreqTransport` - alias for `Async` when `bitreq` is enabled
- `ReqwestTransport` - alias for `Async` when `reqwest` is enabled
- `TorAsync` - Tor-specific transport (enabled by `tor` feature)

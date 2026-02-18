# cdk-http-client

HTTP client abstraction for the Cashu Development Kit (CDK).

This crate provides an HTTP client wrapper that abstracts the underlying HTTP library
(reqwest or bitreq), allowing other CDK crates to avoid direct dependencies on a specific backend.

## Features

- `bitreq` (default) - use bitreq as the HTTP backend
- `reqwest` - use reqwest as the HTTP backend

These features are mutually exclusive. Enable only one backend at a time.

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

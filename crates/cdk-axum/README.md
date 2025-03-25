# CDK Axum

[![crates.io](https://img.shields.io/crates/v/cdk-axum.svg)](https://crates.io/crates/cdk-axum) [![Documentation](https://docs.rs/cdk-axum/badge.svg)](https://docs.rs/cdk-axum)

The CDK Axum crate is a component of the [Cashu Development Kit](https://github.com/cashubtc/cdk) that provides a web server implementation for Cashu mints using the [Axum](https://github.com/tokio-rs/axum) web framework.

## Overview

This crate implements the HTTP API for Cashu mints, providing endpoints for all the Cashu NUTs (Notation, Usage, and Terminology) specifications. It handles routing, request validation, response formatting, and includes features like WebSocket support and HTTP caching.

## Features

- Complete implementation of Cashu mint HTTP API
- WebSocket support for real-time notifications (NUT-17)
- HTTP response caching for improved performance (NUT-19)
- CORS support for browser-based clients
- Compression and decompression of HTTP payloads
- Configurable logging and tracing

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-axum = "0.8.1"
```

### Example

```rust
use cdk_axum::{MintServer, MintServerConfig};
use std::net::SocketAddr;

async fn start_mint_server(mint: impl MintTrait, db: impl MintDatabase) {
    let config = MintServerConfig {
        listen_addr: SocketAddr::from(([127, 0, 0, 1], 3338)),
        cors_allowed_origins: vec!["*".to_string()],
        cache_ttl: std::time::Duration::from_secs(60),
        cache_tti: std::time::Duration::from_secs(30),
        ..Default::default()
    };
    
    let server = MintServer::new(mint, db, config);
    server.start().await.unwrap();
}
```

## License

This project is licensed under the [MIT License](../../LICENSE).

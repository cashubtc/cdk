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
cdk-axum = "*"
```

## License

This project is licensed under the [MIT License](https://github.com/cashubtc/cdk/blob/main/LICENSE).

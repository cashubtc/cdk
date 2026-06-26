# CDK Common

[![crates.io](https://img.shields.io/crates/v/cdk-common.svg)](https://crates.io/crates/cdk-common)
[![Documentation](https://docs.rs/cdk-common/badge.svg)](https://docs.rs/cdk-common)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

Common types and utilities shared across the Cashu Development Kit (CDK) crates.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-common = "*"
```

## Features

This crate provides common functionality used across CDK crates including:

- Common data types and structures
- Shared traits and interfaces
- Utility functions
- Error types

The `http` feature enables CDK common's HTTP-facing helpers and re-exports,
including `cdk-http-client` types, the WebSocket client re-export, HTTP error
conversion, and OIDC auth helpers.

`cdk-common/http` selects the default `bitreq` backend so consumers get a working
HTTP client automatically. Applications that need the `reqwest` backend can add a
direct `cdk-http-client` dependency with the `reqwest` feature; backend features
are additive, and `reqwest` takes precedence when both are enabled.

## License

This project is licensed under the [MIT License](../../LICENSE).

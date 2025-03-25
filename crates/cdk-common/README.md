# CDK Common

[![crates.io](https://img.shields.io/crates/v/cdk-common.svg)](https://crates.io/crates/cdk-common)
[![Documentation](https://docs.rs/cdk-common/badge.svg)](https://docs.rs/cdk-common)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

Common types, traits, and utilities for the Cashu Development Kit (CDK).

## Overview

The `cdk-common` crate provides shared functionality used across the CDK crates. It contains core data structures, error types, and utility functions that are essential for implementing Cashu wallets and mints.

## Features

- **Core Data Types**: Implementations of fundamental Cashu types like `MintUrl`, `ProofInfo`, and `Melted`
- **Error Handling**: Comprehensive error types for Cashu operations
- **Database Abstractions**: Traits for database operations used by wallets and mints
- **NUT Implementations**: Common functionality for Cashu NUTs (Notation, Usage, and Terminology)

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-common = "*"
```

## Components

The crate includes several key modules:

- **common**: Core data structures used throughout the CDK
- **database**: Traits for database operations
- **error**: Error types and handling
- **mint_url**: Implementation of the MintUrl type
- **nuts**: Common functionality for Cashu NUTs

## License

This project is licensed under the [MIT License](https://github.com/cashubtc/cdk/blob/main/LICENSE).

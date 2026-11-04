# CDK Supabase

[![crates.io](https://img.shields.io/crates/v/cdk-supabase.svg)](https://crates.io/crates/cdk-supabase)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

[Supabase](https://github.com/supabase/supabase) storage backend implementation for the Cashu Development Kit (CDK) wallet.

## Features

This crate provides a Supabase-based storage implementation for wallet functionality, including:

- Saga Support: Persistence for multi-step wallet operations
- PostgREST-based queries
- Supabase Auth helpers 
- Supabase RPC for counters and atomic operations
- Automatic token refresh with support for Supabase GoTrue
- Client-side AES-256-GCM encryption before sending to Supabase


## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-supabase = "*"
```

## License

This project is licensed under the [MIT License](../../LICENSE).
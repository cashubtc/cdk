# CDK Mintd

[![crates.io](https://img.shields.io/crates/v/cdk-mintd.svg)](https://crates.io/crates/cdk-mintd)
[![Documentation](https://docs.rs/cdk-mintd/badge.svg)](https://docs.rs/cdk-mintd)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

Cashu mint daemon implementation for the Cashu Development Kit (CDK). This binary provides a complete Cashu mint server implementation with support for multiple database backends and Lightning Network integrations.

## Features

- **Multiple Database Backends**: SQLite and PostgreSQL
- **Lightning Network Integration**: Support for CLN, LND, LNbits, and test backends  
- **Authentication**: Optional user authentication with OpenID Connect
- **Management RPC**: gRPC interface for mint management
- **Docker Support**: Ready-to-use Docker configurations

## Installation

From crates.io:
```bash
cargo install cdk-mintd
```

From source:
```bash
cargo install --path .
```

## Quick Start

### Using SQLite (Default)
```bash
# Start with SQLite (no additional setup required)
cdk-mintd
```

### Using PostgreSQL
```bash
# Set environment variables
export CDK_MINTD_DATABASE=postgres
export CDK_MINTD_DATABASE_URL="postgresql://postgres:password@localhost:5432/cdk_mint"

# Start the mint
cdk-mintd
```

### Using Docker
```bash
# SQLite
docker-compose up

# PostgreSQL
docker-compose -f docker-compose.postgres.yaml up
```

## Configuration

The mint can be configured through environment variables or a configuration file. See `example.config.toml` for all available options.

### Database Configuration

#### SQLite (Default)
```toml
[database]
engine = "sqlite"
```

#### PostgreSQL  
```toml
[database]
engine = "postgres"
```
Set `CDK_MINTD_DATABASE_URL` environment variable for connection string.

#### ReDB
```toml
[database]
engine = "redb"
```

### Lightning Backend Configuration

```toml
[ln]
ln_backend = "fakewallet"  # Options: cln, lnd, lnbits, fakewallet
```

### Logging Configuration

You can configure where mintd outputs its logs using the `[info.logging]` section in your config file or environment variables:

**Config file:**
```toml
[info.logging]
# Where to output logs: "stdout", "file", or "both" (default: "both")
output = "both"
# Log level for console output (default: "info")
console_level = "info"  
# Log level for file output (default: "debug")
file_level = "debug"
```

**Environment variables:**
```bash
# Where to output logs: "stdout", "file", or "both" (default: "both")
export CDK_MINTD_LOGGING_OUTPUT="both"
# Log level for console output (default: "info")
export CDK_MINTD_LOGGING_CONSOLE_LEVEL="debug"
# Log level for file output (default: "debug")
export CDK_MINTD_LOGGING_FILE_LEVEL="debug"
```

**Available logging outputs:**
- `"stdout"` - Output logs only to console/terminal (stderr)
- `"file"` - Output logs only to rotating daily log files in `<work_dir>/logs/`
- `"both"` - Output logs to both console and files (default behavior)

**Available log levels:** `error`, `warn`, `info`, `debug`, `trace`

**Examples:**

Console only with debug level:
```bash
export CDK_MINTD_LOGGING_OUTPUT="stdout"
export CDK_MINTD_LOGGING_CONSOLE_LEVEL="debug"
```

File only with debug level:
```toml
[info.logging]
output = "file"
file_level = "debug"
```

Both console (warn) and file (debug):
```bash
export CDK_MINTD_LOGGING_OUTPUT="both"
export CDK_MINTD_LOGGING_CONSOLE_LEVEL="warn"
export CDK_MINTD_LOGGING_FILE_LEVEL="debug"
```

## Usage

```bash
# Start the mint with default configuration
cdk-mintd

# Start with custom config file
cdk-mintd --config /path/to/config.toml

# Start with specific work directory
cdk-mintd --work-dir /path/to/work/dir
# Disable logging entirely
cdk-mintd --enable-logging false

# Show help
cdk-mintd --help
```

## Environment Variables

Key environment variables:

- `CDK_MINTD_DATABASE`: Database engine (sqlite/postgres/redb)
- `CDK_MINTD_DATABASE_URL`: PostgreSQL connection string
- `CDK_MINTD_LN_BACKEND`: Lightning backend type
- `CDK_MINTD_LISTEN_HOST`: Host to bind to
- `CDK_MINTD_LISTEN_PORT`: Port to bind to

## Documentation

- [Configuration Examples](./example.config.toml)
- [PostgreSQL Setup Guide](../../POSTGRES.md)
- [Development Guide](../../DEVELOPMENT.md)

## License

This project is licensed under the [MIT License](../../LICENSE).

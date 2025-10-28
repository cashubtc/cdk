# CDK Mintd

[![crates.io](https://img.shields.io/crates/v/cdk-mintd.svg)](https://crates.io/crates/cdk-mintd)
[![Documentation](https://docs.rs/cdk-mintd/badge.svg)](https://docs.rs/cdk-mintd)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

> **Warning**
> This project is in early development, it does however work with real sats! Always use amounts you don't mind losing.

Cashu mint daemon implementation for the Cashu Development Kit (CDK). This binary provides a complete Cashu mint server implementation with support for multiple database backends and Lightning Network integrations.

## Features

- **Multiple Database Backends**: SQLite, PostgreSQL, and ReDB
- **Lightning Network Integration**: Support for CLN, LND, LNbits, LDK Node, and test backends  
- **Authentication**: Optional user authentication with OpenID Connect
- **Management RPC**: gRPC interface for mint management
- **Docker Support**: Ready-to-use Docker configurations

## Installation

### Option 1: Download Pre-built Binary
Download the latest release from the [GitHub releases page](https://github.com/cashubtc/cdk/releases).

### Option 2: Build from Source
```bash
git clone https://github.com/cashubtc/cdk.git
cd cdk
cargo build --bin cdk-mintd --release
# Binary will be at ./target/release/cdk-mintd
```

## Configuration

> **Important**: You must create the working directory and configuration file before starting the mint. The mint does not create them automatically.

### Setup Steps

1. **Create working directory**:
   ```bash
   mkdir -p ~/.cdk-mintd
   ```

2. **Create configuration file**:
   ```bash
   # Copy and customize the example config
   cp example.config.toml ~/.cdk-mintd/config.toml
   # Edit ~/.cdk-mintd/config.toml with your settings
   ```

3. **Start the mint**:
   ```bash
   cdk-mintd  # Uses ~/.cdk-mintd/config.toml automatically
   ```

### Configuration File Locations (in order of precedence)

1. **Explicit path**: `cdk-mintd --config /path/to/config.toml`
2. **Working directory**: `./config.toml` (in current directory) 
3. **Default location**: `~/.cdk-mintd/config.toml`
4. **Environment variables**: All config options can be set via environment variables

### Alternative Setup Methods

**Custom working directory**:
```bash
mkdir -p /my/custom/path
cp example.config.toml /my/custom/path/config.toml
cdk-mintd --work-dir /my/custom/path
```

**Environment variables only**:
```bash
export CDK_MINTD_LISTEN_PORT=3000
export CDK_MINTD_LN_BACKEND=fakewallet
export CDK_MINTD_DATABASE=sqlite
cdk-mintd
```

## Production Examples

### With LDK Node (Recommended for Testing)
```toml
[ln]
ln_backend = "ldk-node"

[ldk_node]
bitcoin_network = "signet"  # Use "mainnet" for production
esplora_url = "https://mutinynet.com/api"
rgs_url = "https://rgs.mutinynet.com/snapshot/0"
gossip_source_type = "rgs"
storage_dir_path = "/var/lib/cdk-mintd/ldk-node"
```


### With CLN Lightning Backend
```toml
[ln]
ln_backend = "cln"

[cln]
rpc_path = "/home/bitcoin/.lightning/bitcoin/lightning-rpc"
fee_percent = 0.01
reserve_fee_min = 10
```

### With LND Lightning Backend
```toml
[ln]
ln_backend = "lnd"

[lnd]
address = "https://localhost:10009"
macaroon_file = "/home/bitcoin/.lnd/data/chain/bitcoin/mainnet/admin.macaroon"
cert_file = "/home/bitcoin/.lnd/tls.cert"
fee_percent = 0.01
reserve_fee_min = 10
```

### With PostgreSQL Database
```toml
[database]
engine = "postgres"

[database.postgres]
url = "postgresql://mint_user:password@localhost:5432/cdk_mint"
```

## Directory Structure

After setup and first run, your directory will look like:

```
~/.cdk-mintd/                    # Working directory (create manually)
├── config.toml                  # Config file (create manually)
├── cdk-mintd.db                # SQLite database (created automatically)
├── logs/                       # Log files (created automatically if enabled)
│   ├── cdk-mintd.2024-01-01.log
│   └── cdk-mintd.2024-01-02.log
└── ldk-node/                   # LDK Node data (if using LDK backend)
    ├── wallet/
    └── graph/
```

**What you must create manually:**
- Working directory (e.g., `~/.cdk-mintd/`)
- Config file (`config.toml`)

**What gets created automatically:**
- Database files
- Log directories and files
- Lightning backend data directories

## Docker Usage

CDK Mintd provides ready-to-use Docker images with multiple Lightning backend options.

### Quick Start

#### Standard mint with fakewallet backend (testing only):
```bash
docker-compose up
```

#### Mint with LDK Node backend:
```bash
# Option 1: Use dedicated ldk-node compose file
docker-compose -f docker-compose.ldk-node.yaml up

# Option 2: Use main compose file with profile
docker-compose --profile ldk-node up
```

### Available Images

- **`cashubtc/mintd:latest`** - Standard mint with default features
- **`cashubtc/mintd-ldk-node:latest`** - Mint with LDK Node support

### Configuration via Environment Variables

All configuration can be done through environment variables:

```yaml
environment:
  - CDK_MINTD_LN_BACKEND=ldk-node
  - CDK_MINTD_DATABASE=sqlite
  - CDK_MINTD_LISTEN_HOST=0.0.0.0
  - CDK_MINTD_LISTEN_PORT=8085
  - CDK_MINTD_LDK_NODE_NETWORK=testnet
  - CDK_MINTD_LDK_NODE_ESPLORA_URL=https://blockstream.info/testnet/api
```

### Monitoring

Both Prometheus metrics and Grafana dashboards are included:
- Prometheus: http://localhost:9090
- Grafana: http://localhost:3011 (admin/admin)

For detailed Docker documentation, see [README-ldk-node.md](../../README-ldk-node.md).

## Testing Your Mint

1. **Verify the mint is running**:
   ```bash
   curl http://127.0.0.1:8085/v1/info
   ```

2. **Get mint keys**:
   ```bash
   curl http://127.0.0.1:8085/v1/keys
   ```

3. **Test with CDK CLI wallet**:
   ```bash
   # Download from: https://github.com/cashubtc/cdk/releases
   cdk-cli wallet add-mint http://127.0.0.1:8085
   cdk-cli wallet mint-quote 100
   ```

4. **For LDK Node backend**: Access the management interface at <http://127.0.0.1:8091>

## Command Line Usage

```bash
# Start with default configuration
cdk-mintd

# Start with custom config file
cdk-mintd --config /path/to/config.toml

# Start with custom working directory
cdk-mintd --work-dir /path/to/work/dir

# Disable logging
cdk-mintd --enable-logging false

# Show help
cdk-mintd --help
```

## Key Environment Variables

- `CDK_MINTD_DATABASE`: Database engine (`sqlite`/`postgres`/`redb`)
- `CDK_MINTD_DATABASE_URL`: PostgreSQL connection string
- `CDK_MINTD_LN_BACKEND`: Lightning backend (`cln`/`lnd`/`lnbits`/`ldk-node`/`fakewallet`)
- `CDK_MINTD_LISTEN_HOST`: Host to bind to (default: `127.0.0.1`)
- `CDK_MINTD_LISTEN_PORT`: Port to bind to (default: `8085`)

For complete configuration options, see the [example configuration file](./example.config.toml).

## Documentation

- **[Configuration Examples](./example.config.toml)** - Complete configuration reference
- **[PostgreSQL Setup Guide](../../docker-compose.postgres.yaml)** - Database setup with Docker Compose
- **[Development Guide](../../DEVELOPMENT.md)** - Contributing and development setup

## License

This project is licensed under the [MIT License](../../LICENSE).

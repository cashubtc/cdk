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

## Lightning Backend Documentation

For detailed configuration of each Lightning backend, see:

- **[LND](../cdk-lnd/README.md)** - Lightning Network Daemon
- **[CLN](../cdk-cln/README.md)** - Core Lightning
- **[LNbits](../cdk-lnbits/README.md)** - LNbits API integration

## Installation

### Option 1: Download Pre-built Binary

Statically-linked x86_64 Linux binaries are published to each [GitHub release](https://github.com/cashubtc/cdk/releases). These have zero runtime dependencies and run on any x86_64 Linux system.

Available binaries:
- **`cdk-mintd-{version}-x86_64`** -- standard mint with `postgres`, `prometheus`, and `redis` support
- **`cdk-mintd-ldk-{version}-x86_64`** -- mint with built-in `ldk-node` Lightning backend

Each release also includes a `SHA256SUMS` file to verify downloads:

```bash
# Download the binary and checksums
curl -LO https://github.com/cashubtc/cdk/releases/latest/download/cdk-mintd-{version}-x86_64
curl -LO https://github.com/cashubtc/cdk/releases/latest/download/SHA256SUMS

# Verify the checksum
sha256sum -c SHA256SUMS --ignore-missing

# Make executable and run
chmod +x cdk-mintd-*-x86_64
./cdk-mintd-*-x86_64 --help
```

To build static binaries locally, see the [Static Binaries](../../DEVELOPMENT.md#static-binaries) section in the Development Guide.

### Option 2: Build from Source

This project uses [Nix](https://nixos.org/) to manage development dependencies.

```bash
git clone https://github.com/cashubtc/cdk.git
cd cdk

# Enter lean development environment
nix develop

# OR enter full regtest environment (with bitcoind, cln, lnd, postgres)
nix develop .#regtest

# Build binary
cargo build --bin cdk-mintd --release
# Binary will be at ./target/release/cdk-mintd
```

## Configuration

The mint database is the source of truth for configuration. A TOML file is an
import/export document: it is read only by an explicit `cdk-mintd config`
command and is never reapplied by a normal `cdk-mintd` start. Operational
environment variables likewise do not override persisted configuration during
startup.

> Upgrading an existing mint requires a one-time import and careful preservation
> of RPC-managed values. Follow the
> [v0.17 cdk-mintd migration guide](../../docs/migrations/v0.17.md) before
> starting the new daemon.

### Setup Steps

```bash
mkdir -p ~/.cdk-mintd
cp example.config.toml ~/.cdk-mintd/config.toml
# Edit the document and provide any env: secrets it references.
cdk-mintd config validate --file ~/.cdk-mintd/config.toml
cdk-mintd config init --file ~/.cdk-mintd/config.toml
cdk-mintd
```

`config init` refuses to replace an existing record. On the first start, mintd
applies the imported mint metadata and quote TTL and marks that document
applied. Later starts preserve canonical database values changed through the
management RPC while loading the remaining daemon settings from the stored
document.

Changing or deleting the original TOML file after initialization has no effect
on the running mint or its next startup.

### Configuration Commands

```bash
# Validate locally; no database or RPC mutation
cdk-mintd config validate --file /path/to/config.toml

# Initialize the bootstrap-selected configuration database directly
cdk-mintd config init --file /path/to/config.toml

# Validate against the stored database and signer without writing
cdk-mintd config apply --file /path/to/config.toml --validate-only

# Atomically replace the document used by the next start
cdk-mintd config apply --file /path/to/config.toml

# Print or export the stored document
cdk-mintd config show
cdk-mintd config export --file /path/to/exported-config.toml
```

`config apply`, `show`, and `export` access the authoritative database directly.
Apply replaces one versioned record transactionally and sets it to unapplied. A
running daemon keeps its current in-memory snapshot; the replacement is used on
the next restart. If another apply wins while startup is consuming a document,
the newer document remains unapplied for the following restart.

`cdk-mintd` is not an RPC client. Immediate field-level mint management
(`get-info`, `update-motd`, `rotate-next-keyset`, and related commands) is
provided by the separate `cdk-mint-cli` binary. See
[`cdk-mint-rpc`](../cdk-mint-rpc/README.md).

### Bootstrap Settings

A small set of values cannot come solely from the database because mintd needs
them before it can open that database. These are bootstrap settings, not
competing operational configuration:

- Working directory: `--work-dir` or `CDK_MINTD_WORK_DIR`.
- Primary database engine and PostgreSQL connection settings:
  `CDK_MINTD_DATABASE`, `CDK_MINTD_POSTGRES_URL` (or the legacy
  `CDK_MINTD_DATABASE_URL`), and related PostgreSQL bootstrap variables.
- SQLCipher password when an invocation opens the local encrypted database.
  Encrypted SQLite startup and database commands therefore require
  `--password <password>`; `config validate` does not open the database.

`config validate` parses the supplied document, resolves its secret references,
and verifies its signer without opening the primary database. `config apply
--validate-only` additionally checks the stored database and signer constraints.

`config init` opens the database selected by the same bootstrap settings as
normal startup and rejects an import document whose primary database settings
do not match it. All other TOML and environment values are operational settings
and are loaded from the database during normal startup.

Primary database settings are immutable through `config apply`: moving the
authoritative database requires a separate data-migration procedure.

### Secret References

Secret fields must contain a reference, never a literal value:

```toml
[info]
mnemonic = "file:/run/secrets/mint-mnemonic"

[database.postgres]
url = "env:CDK_MINTD_POSTGRES_URL"

[lnbits]
admin_api_key = "env:CDK_MINTD_LNBITS_ADMIN_API_KEY"
invoice_api_key = "file:/run/secrets/lnbits-invoice-key"
```

`env:VARIABLE` reads the named variable and `file:/absolute/path` reads the
mounted file. Secret file paths must be absolute. Mintd validates and resolves
references when initializing, applying, and starting, but persists and exports
only the references. Resolved secret contents are never written to the
configuration store.

The same rule applies to mint seeds and mnemonics, PostgreSQL URLs, LNbits API
keys, BDK/LDK RPC passwords and mnemonics, and Redis connection values when
those sections are active.

At initialization, mintd binds the database to a fingerprint of the signer's
actual root public key. Applying a document or starting after an `env:`/`file:`
secret changes is rejected if that key differs, before local keyset state can be
mutated. Moving a secret to another reference or changing remote-signatory
connection details is allowed when the signer key is unchanged. Signer
migration is intentionally not part of ordinary configuration apply.

### Applying a Changed File

There is no configuration-file search path or implicit precedence order. To
replace configuration, edit a file and run the explicit apply command:

```bash
cdk-mintd config validate --file /path/to/changed-config.toml
cdk-mintd config apply --file /path/to/changed-config.toml
cdk-mintd config show
# Restart mintd to use the replacement.
```

### Fake Wallet Custom Payment Methods

The fake wallet backend can advertise custom payment methods for testing NUT-04
and NUT-05 custom payment flows. Configure methods in `config.toml` with one
entry per method and unit:

```toml
[[ln]]
ln_backend = "fakewallet"
unit = "sat"

[[ln]]
ln_backend = "fakewallet"
unit = "usd"

[fake_wallet]
custom_payment_methods = [
    { method = "paypal", unit = "sat" },
    { method = "venmo", unit = "usd" },
]
```

For a single fake wallet unit, the legacy `[ln]` table is still accepted and
defaults to `unit = "sat"`. For multiple fake wallet units, use one `[[ln]]`
entry per unit.

For Docker setups, put these operational values in the TOML import document and
run `config init` once against the persistent database. Setting the former
`CDK_MINTD_FAKE_WALLET_*` variables when starting mintd does not override the
database-backed configuration.

Bare method names are enabled for every fake wallet unit:

```toml
custom_payment_methods = ["paypal"]
```

Disable fake custom methods with:

```toml
custom_payment_methods = []
```

### Keyset Version Management

The mint supports rotating keysets to newer versions (e.g., migrating from V1 to V2).

**Policy Configuration:**
By default, the mint will use V2 (Version01) for *new* keysets but will preserve existing V1 (Version00) keysets to avoid unnecessary rotation. You can force a specific policy in an initialization or apply document:

- `use_keyset_v2 = true`: Forces V2. If the current active keyset is V1, it will be rotated to V2 on startup.
- `use_keyset_v2 = false`: Forces V1. If the current active keyset is V2, it will be rotated to V1 on startup.
- **Unset (Default)**: Preserves the current keyset version. If no keyset exists, V2 is created.

**Manual Rotation:**
You can manually trigger a rotation to a specific version using the CLI:

```bash
cdk-mint-cli rotate-next-keyset --use-keyset-v2 true  # Rotate to V2
cdk-mint-cli rotate-next-keyset --use-keyset-v2 false # Rotate to V1
```

## Production Examples

### With LDK Node (Recommended for Testing)
```toml
[ln]
ln_backend = "ldk-node"

[ldk_node]
bitcoin_network = "signet"  # Use "mainnet" for production
chain_source_type = "esplora"  # esplora, electrum, or bitcoinrpc
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
# fee_percent = 0.02      # Optional, defaults to 2%
# reserve_fee_min = 2     # Optional, defaults to 2 sats
```

### With LND Lightning Backend
```toml
[ln]
ln_backend = "lnd"

[lnd]
address = "https://localhost:10009"
macaroon_file = "/home/bitcoin/.lnd/data/chain/bitcoin/mainnet/admin.macaroon"
cert_file = "/home/bitcoin/.lnd/tls.cert"
# fee_percent = 0.02      # Optional, defaults to 2%
# reserve_fee_min = 2     # Optional, defaults to 2 sats
```

### With PostgreSQL Database
```toml
[database]
engine = "postgres"

[database.postgres]
url = "env:CDK_MINTD_POSTGRES_URL"
```

Set `CDK_MINTD_DATABASE=postgres` and `CDK_MINTD_POSTGRES_URL` for both
initialization and subsequent starts so mintd can locate the authoritative
database before reading its stored configuration.

### With Multiple Lightning Backends

A single mint can serve more than one currency unit by configuring a separate backend per unit. Replace the single `[ln]` block with one `[[ln]]` block per backend/unit, and keep the existing per-backend config sections (`[cln]`, `[lnbits]`, etc.) as-is.

```toml
[[ln]]
ln_backend = "cln"
unit = "sat"

[[ln]]
ln_backend = "lnbits"
unit = "msat"

[cln]
rpc_path = "/home/bitcoin/.lightning/bitcoin/lightning-rpc"

[lnbits]
admin_api_key = "env:CDK_MINTD_LNBITS_ADMIN_API_KEY"
invoice_api_key = "file:/run/secrets/lnbits-invoice-key"
lnbits_api = "https://lnbits.example.com"
```

Each `[[ln]]` block carries its own `min_mint`, `max_mint`, `min_melt`, `max_melt` if you want different limits per unit. The configured unit must match the backend's reported unit, except for the supported `sat`/`msat` conversion pair. If two configured backends expose the same `(unit, method)` pair, startup is rejected.

The legacy single `[ln]` form is still accepted; it is equivalent to one
`[[ln]]` entry with `unit = "sat"` (the default). Multi-backend topology is
imported from TOML and is not overridden by environment variables at startup.

## Directory Structure

After setup and first run, your directory will look like:

```
~/.cdk-mintd/                    # Working directory (create manually)
├── config.toml                  # Optional import/export document; not read at startup
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
- An initialization document, which may be stored anywhere and is no longer
  authoritative after `config init`

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

### Container Configuration

Operational configuration is initialized from a mounted TOML document and then
read from the persistent database. Environment variables on the normal mintd
container are limited to database/work-directory bootstrap and to values named
by `env:` secret references.

```yaml
environment:
  - CDK_MINTD_DATABASE=sqlite
  - CDK_MINTD_WORK_DIR=/data
volumes:
  - mint-data:/data
  - ./mint.toml:/config/mint.toml:ro
```

Run `cdk-mintd config init --file /config/mint.toml` once with the same
persistent volume before starting `cdk-mintd`. Later file changes are activated
only by an explicit `config apply` followed by a restart.

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
# Start using the active database-backed configuration
cdk-mintd

# Initialize once from a TOML import document
cdk-mintd config init --file /path/to/config.toml

# Validate or explicitly stage a changed document directly
cdk-mintd config validate --file /path/to/config.toml
cdk-mintd config apply --file /path/to/config.toml

# Select the bootstrap working directory
cdk-mintd --work-dir /path/to/work/dir

# Show help
cdk-mintd --help

# Immediate mint management uses the separate RPC client binary
cdk-mint-cli get-info --addr https://127.0.0.1:8086 --tls-dir /path/to/tls
```

## Bootstrap Environment Variables

- `CDK_MINTD_WORK_DIR`: Working directory used for SQLite and local files.
- `CDK_MINTD_DATABASE`: Primary database engine (`sqlite` or `postgres`).
- `CDK_MINTD_DATABASE_URL`: PostgreSQL connection string
- `CDK_MINTD_POSTGRES_URL`: Canonical PostgreSQL connection variable.

Other environment variables are read only when explicitly named by an
`env:VARIABLE` secret reference in the persisted document. They do not act as
automatic operational overrides. The legacy `--config` and `--seed-file` flags
are rejected for every command, with guidance to use `config init` or
`config apply`.

For complete configuration options, see the [example configuration file](./example.config.toml).

## Documentation

- **[Configuration Examples](./example.config.toml)** - Complete configuration reference
- **[PostgreSQL Setup Guide](../../docker-compose.postgres.yaml)** - Database setup with Docker Compose
- **[Development Guide](../../DEVELOPMENT.md)** - Contributing and development setup

## License

This project is licensed under the [MIT License](../../LICENSE).

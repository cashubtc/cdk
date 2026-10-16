# CDK Mintd

[![crates.io](https://img.shields.io/crates/v/cdk-mintd.svg)](https://crates.io/crates/cdk-mintd)
[![Documentation](https://docs.rs/cdk-mintd/badge.svg)](https://docs.rs/cdk-mintd)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

> **Warning**
> This project is in early development, it does however work with real sats! Always use amounts you don't mind losing.

Cashu mint daemon implementation for the Cashu Development Kit (CDK). This binary provides a complete Cashu mint server implementation with support for multiple database backends and pluggable payment backends (Lightning, on-chain, or custom processors).

## Features

- **Multiple Database Backends**: SQLite, PostgreSQL, and ReDB
- **Pluggable Payment Backends**: Support for CLN, LND, LDK Node, external payment processors, and test backends
- **Authentication**: Optional user authentication with OpenID Connect
- **Management RPC**: gRPC interface for mint management
- **Docker Support**: Ready-to-use Docker configurations

## Payment Backend Documentation

For detailed configuration of each payment backend, see:

- **[LND](../cdk-lnd/README.md)** - Lightning Network Daemon
- **[CLN](../cdk-cln/README.md)** - Core Lightning

LNbits is no longer provided as an embedded, first-class backend. Run LNbits integration as an external payment processor and connect it through the `grpc-processor` backend.

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
> [v0.18 cdk-mintd migration guide](../../docs/migrations/v0.18.md) before
> starting the new daemon.

### Setup Steps

```bash
mkdir -p ~/.cdk-mintd
cp example.config.toml ~/.cdk-mintd/config.toml
# Edit the document and provide any env: secrets it references.
cdk-mintd config validate --file ~/.cdk-mintd/config.toml
cdk-mintd config init --new-mint --file ~/.cdk-mintd/config.toml
cdk-mintd
```

`config init` requires `--new-mint` for a database that has never served a mint
or `--existing-mint` when importing configuration during an upgrade. Each mode
validates the selected database state and refuses to replace an existing
configuration record. On the first start, mintd
applies the imported mint metadata and quote TTL and marks that document
applied. Later starts preserve canonical database values changed through the
management RPC while loading the remaining daemon settings from the stored
document.

Changing or deleting the original TOML file after initialization has no effect
on the running mint or its next startup.

### Configuration Commands

```bash
# Convert a legacy file plus its active CDK_MINTD_* overrides
cdk-mintd config migrate \
  --file /path/to/legacy-config.toml \
  --output /path/to/migrated-config.toml

# Validate locally; no database or RPC mutation
cdk-mintd config validate --file /path/to/config.toml

# Initialize a database that has never served a mint
cdk-mintd config init --new-mint --file /path/to/config.toml

# Import configuration into an existing mint database
cdk-mintd config init --existing-mint --file /path/to/config.toml

# Add BDK during the first v0.18 import
cdk-mintd config init --existing-mint --allow-new-bdk-wallet \
  --file /path/to/config.toml

# Validate against the stored database and signer without writing
cdk-mintd config apply --file /path/to/config.toml --validate-only

# Atomically replace the document used by the next start
cdk-mintd config apply --file /path/to/config.toml

# Add BDK to an already initialized v0.18 mint
cdk-mintd config apply --allow-new-bdk-wallet --file /path/to/config.toml

# Discard a pending document or restore the previous applied document
cdk-mintd config rollback

# Print or export the stored document
cdk-mintd config show
cdk-mintd config export --file /path/to/exported-config.toml
# Explicitly replace an existing export
cdk-mintd config export --file /path/to/exported-config.toml --force
```

`config migrate` reproduces the legacy file-plus-environment precedence once and
writes a complete import document; it does not open the database or change the
source file. Environment-backed secrets become explicit `env:VARIABLE`
references. Literal secrets in the legacy TOML are copied into owner-only files
under `cdk-mintd-secrets/` beside the output document and become absolute
`file:` references. Use `--secrets-dir <path>` to choose another directory and
`--force` to replace files created by an earlier migration attempt. If the old
service actually used the global `--seed-file` BIP39 mnemonic option, pass the
same option to `config migrate`; the generated document references that existing
file directly. Do not pass it merely because an LDK storage directory contains
the unrelated binary `keys_seed` file.

The migrated document is normalized and includes effective defaults, so comments
and the original TOML layout are not preserved. Review it and run `config
validate` before `config init`.

`config apply`, `show`, and `export` access the authoritative database directly.
Export refuses to overwrite an existing file unless `--force` is passed.
Apply updates one versioned record transactionally, retains the last applied
document, and sets the replacement to unapplied. A running daemon keeps its
current in-memory snapshot; the replacement is used on the next restart. If
another apply wins while startup is consuming a document, the newer document
remains unapplied for the following restart. `config rollback` stages the
previous applied document and always requires another restart. This remains
true when rolling back a pending replacement because a failed startup may have
partially updated canonical database state. Only one previous applied document
is retained.

When BDK is selected for a new mint, `config init --new-mint` automatically
permits its first wallet creation. Existing-mint initialization and apply
instead require a persisted `<work-dir>/bdk_wallet/bdk_wallet.sqlite` whose
descriptors and network match the configured mnemonic and network. The check is
read-only and runs before the document is stored. Use
`--allow-new-bdk-wallet` only when intentionally adding BDK or performing
last-resort mnemonic recovery after confirming that the original wallet
database is lost. The flag permits an absent wallet but does not bypass an
invalid or mismatched one. Set `wallet_rescan_from_height` for Bitcoin RPC
mnemonic recovery so the fresh wallet does not start scanning at the current
chain tip. The creation permission is cleared after the first successful daemon
startup, so later restarts fail if the BDK wallet disappears. For an already
initialized v0.18 mint, use `config apply`, not `config init`, as described
below.

### Adding BDK to an Existing Mint

Use this procedure for a v0.18 mint whose database-backed configuration is
already initialized and whose Lightning backend is working. `config apply`
replaces the complete authoritative document, so retain every existing field,
especially `[payment_backend]` and the corresponding `[lnd]` or `[ldk_node]`
section.

First export the active unresolved document and record the current mint public
key, complete keyset list, and Lightning node ID:

```bash
cdk-mintd --work-dir /var/lib/cdk-mintd \
  config export --file /var/lib/cdk-mintd/config-with-bdk.toml
```

Stop the daemon and back up the primary database and complete working directory
before changing the document. An embedded LDK deployment must include its
entire configured storage directory. An LND deployment must be backed up using
LND's own node and channel-backup procedure because its identity and channel
state are external to `cdk-mintd`.

Add an on-chain backend and BDK section to the exported document:

```toml
[onchain]
onchain_backend = "bdk"

[bdk]
mnemonic = "file:/run/secrets/bdk-mnemonic"
network = "mainnet"
chain_source_type = "bitcoinrpc"
bitcoind_rpc_host = "127.0.0.1"
bitcoind_rpc_port = 8332
bitcoind_rpc_user = "cdk-mintd"
bitcoind_rpc_password = "file:/run/secrets/bitcoin-rpc-password"
```

Use the network served by the existing Lightning deployment. The BDK mnemonic
should normally be a new secret distinct from the mint signer and any embedded
LDK mnemonic. Protect and back it up before allowing the wallet to receive
funds.

Keep the existing Lightning selection unchanged. For LND, the relevant shape
remains:

```toml
[payment_backend]
backend = "lnd"
unit = "sat"

[lnd]
address = "https://127.0.0.1:10009"
cert_file = "/run/secrets/lnd-tls.cert"
macaroon_file = "/run/secrets/lnd-admin.macaroon"
```

For embedded LDK, retain its complete section, storage path, and original
identity source:

```toml
[payment_backend]
backend = "ldk-node"
unit = "sat"

[ldk_node]
bitcoin_network = "mainnet"
storage_dir_path = "/var/lib/cdk-mintd/ldk-node"
ldk_node_mnemonic = "file:/run/secrets/ldk-node-mnemonic"
```

Most nodes use a dedicated BIP39 mnemonic. Preserve its exact secret reference;
changing it changes the node identity and wallet descriptors. Only omit
`ldk_node_mnemonic` for a legacy node that was created from
`<storage_dir_path>/keys_seed`, and preserve that file with the complete storage
directory. LDK's binary `keys_seed` file is not the same as the legacy global
`cdk-mintd --seed-file` option, which contains a BIP39 mnemonic. Never replace
either LDK identity source with the mint mnemonic.

For a genuinely new BDK wallet, confirm that the expected path does not already
contain wallet data, then validate and stage the document with explicit
creation intent:

```bash
test ! -e /var/lib/cdk-mintd/bdk_wallet/bdk_wallet.sqlite

cdk-mintd --work-dir /var/lib/cdk-mintd \
  config apply --validate-only --allow-new-bdk-wallet \
  --file /var/lib/cdk-mintd/config-with-bdk.toml

cdk-mintd --work-dir /var/lib/cdk-mintd \
  config apply --allow-new-bdk-wallet \
  --file /var/lib/cdk-mintd/config-with-bdk.toml
```

Do not use `--allow-new-bdk-wallet` when the mint previously used BDK. Restore
the original `<work-dir>/bdk_wallet` directory and apply without the flag. If
the database has been lost and mnemonic recovery is unavoidable, use the same
BDK mnemonic and set `wallet_rescan_from_height` to a known wallet birthday for
Bitcoin RPC before explicitly allowing creation. Reconcile all previous BDK
quotes, transactions, and funds before serving users.

Start the daemon and verify all of the following before considering the change
complete:

- the mint public key and every active and inactive keyset are unchanged;
- the LND or LDK node ID is unchanged and Lightning mint/melt tests succeed;
- `<work-dir>/bdk_wallet/bdk_wallet.sqlite` exists with the service user's
  ownership;
- the BDK address network, wallet balance, and synchronization status are
  correct; and
- small on-chain mint and melt tests succeed at the configured confirmation
  threshold.

The new-wallet permission remains pending if startup fails and is cleared only
after all backends start and the daemon binds its listener. A retry validates
any BDK wallet created by the failed attempt. To abandon the pending document,
stop the daemon, inspect the failure, and run:

```bash
cdk-mintd --work-dir /var/lib/cdk-mintd config rollback
```

Restart to activate the restored document. Rollback does not delete a BDK
wallet created during a partial startup; retain it until any addresses, quotes,
or payments from the failed attempt have been assessed.

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
rejects unknown fields, and verifies its signer without opening the primary
database. `config apply
--validate-only` additionally checks the stored database and signer constraints.

`config init` opens the database selected by the same bootstrap settings as
normal startup and rejects an import document whose primary database settings
do not match it. `--new-mint` rejects existing mint identity or keyset state;
`--existing-mint` requires persisted identity and, for embedded signatories,
keyset history. Remote signatory keysets remain in the signatory database. All
other TOML and environment values are operational settings and are loaded from
the database during normal startup.

Primary database settings are immutable through `config apply`: moving the
authoritative database requires a separate data-migration procedure.

### Signing Modes

Configure exactly one signing mode:

- For an embedded signatory, set `[info].seed` or `[info].mnemonic` to a secret
  reference available on the mint host.
- For a remote signatory, set `[signatory].enabled = true` and omit both local
  fields. The private signing material remains on the signatory host.

Mintd rejects a remote-signatory configuration that also contains a non-empty
local seed or mnemonic. During database-backed startup it retains the validated
remote connection and checks its public identity again immediately before mint
construction and keyset operations.

### Secret References

Secret fields must contain a reference, never a literal value:

```toml
[info]
mnemonic = "file:/run/secrets/mint-mnemonic"

[database.postgres]
url = "env:CDK_MINTD_POSTGRES_URL"

[bdk]
bitcoind_rpc_password = "env:CDK_MINTD_BDK_BITCOIND_RPC_PASSWORD"
mnemonic = "file:/run/secrets/bdk-mnemonic"
```

`env:VARIABLE` reads the named variable and `file:/absolute/path` reads the
mounted file. Secret file paths must be absolute. Mintd validates and resolves
references when initializing, applying, and starting, but persists and exports
only the references. Resolved secret contents are never written to the
configuration store.

The same rule applies to mint seeds and mnemonics, PostgreSQL URLs, BDK/LDK RPC
passwords and mnemonics, and Redis connection values. Every
secret field present in the document must use a reference, including fields in
inactive sections. References in inactive sections are validated but not
resolved.

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
[[payment_backend]]
backend = "fakewallet"
unit = "sat"

[[payment_backend]]
backend = "fakewallet"
unit = "usd"

[fake_wallet]
custom_payment_methods = [
    { method = "paypal", unit = "sat" },
    { method = "venmo", unit = "usd" },
]
```

For a single fake wallet unit, the single `[payment_backend]` table is accepted
and defaults to `unit = "sat"`. For multiple fake wallet units, use one
`[[payment_backend]]` entry per unit.

For Docker setups, put these operational values in the TOML import document and
run `config init` once against the persistent database with the appropriate
new- or existing-mint mode. Setting the former
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
[payment_backend]
backend = "ldk-node"

[ldk_node]
bitcoin_network = "signet"  # Use "mainnet" for production
chain_source_type = "esplora"  # esplora, electrum, or bitcoinrpc
esplora_url = "https://mutinynet.com/api"
rgs_url = "https://rgs.mutinynet.com/snapshot/0"
gossip_source_type = "rgs"
storage_dir_path = "/var/lib/cdk-mintd/ldk-node"
ldk_node_mnemonic = "env:CDK_MINTD_LDK_NODE_MNEMONIC"
```

### With CLN Lightning Backend
```toml
[payment_backend]
backend = "cln"

[cln]
rpc_path = "/home/bitcoin/.lightning/bitcoin/lightning-rpc"
# fee_percent = 0.02      # Optional, defaults to 2%
# reserve_fee_min = 2     # Optional, defaults to 2 sats
```

### With LND Lightning Backend
```toml
[payment_backend]
backend = "lnd"

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

### With Multiple Payment Backends

A single mint can serve more than one currency unit by configuring a separate backend per unit. Replace the single `[payment_backend]` block with one `[[payment_backend]]` block per backend/unit, and keep the existing per-backend config sections (such as `[cln]`) as-is.

```toml
[[payment_backend]]
backend = "cln"
unit = "sat"

[[payment_backend]]
backend = "grpcprocessor"
unit = "msat"

[cln]
rpc_path = "/home/bitcoin/.lightning/bitcoin/lightning-rpc"

# An LNbits integration can be provided by an external payment processor.
[grpc_processor]
supported_units = ["msat"]
address = "127.0.0.1"
port = 50051
allow_insecure = true
```

Each `[[payment_backend]]` block carries its own `min_mint`, `max_mint`, `min_melt`, `max_melt` if you want different limits per unit. The configured unit must match the backend's reported unit, except for the supported `sat`/`msat` conversion pair. If two configured backends expose the same `(unit, method)` pair, startup is rejected.

### External payment processor security

An external payment processor can move funds from its backing wallet. Use mTLS
whenever it runs outside the mint's host. Configure mintd with a directory
containing all three client-side files:

```text
/run/cdk/payment-processor-tls/
├── ca.pem       # CA certificate used to verify the processor server
├── client.pem   # mintd client certificate accepted by the processor
└── client.key   # private key for client.pem
```

```toml
[grpc_processor]
supported_units = ["sat"]
address = "10.0.0.20"
port = 50051
tls_dir = "/run/cdk/payment-processor-tls"
allow_insecure = false
```

Setting `tls_dir` always enables mutual TLS; mintd refuses to connect when any
of these files is missing. TLS without `client.pem` and `client.key` is not
client authentication.

Plaintext connections require the explicit opt-in `allow_insecure = true`.
This includes non-loopback addresses, but it is unsafe on any untrusted network:
without TLS, mintd cannot authenticate the processor and traffic is not encrypted
or protected against modification. An attacker able to observe or alter the
connection could interfere with payment operations. Use this mode only on a
trusted, isolated network or through a separately authenticated and encrypted
tunnel; prefer mTLS even for internal deployments.

The single `[payment_backend]` form is equivalent to one `[[payment_backend]]`
entry with `unit = "sat"` (the default). Multi-backend topology is imported
from TOML and is not overridden by environment variables at startup.

## Directory Structure

After setup and first run, your directory will look like:

```
~/.cdk-mintd/                    # Working directory (create manually)
├── config.toml                  # Optional import/export document; not read at startup
├── cdk-mintd.sqlite            # Primary SQLite database (when selected)
├── bdk_wallet/                 # BDK state (when using the BDK backend)
│   └── bdk_wallet.sqlite
├── logs/                       # Log files (created automatically if enabled)
│   ├── cdk-mintd.2024-01-01.log
│   └── cdk-mintd.2024-01-02.log
└── ldk-node/                   # LDK Node data (if using LDK backend)
    ├── keys_seed                 # Legacy entropy source, when applicable
    └── ...                       # Wallet, channel, and graph state
```

**What you must create manually:**
- Working directory (e.g., `~/.cdk-mintd/`)
- An initialization document, which may be stored anywhere and is no longer
  authoritative after `config init`

**What gets created automatically:**
- Database files
- Log directories and files
- Payment backend data directories

## Docker Usage

CDK Mintd provides ready-to-use Docker images with multiple payment backend options.

### Quick Start

#### Standard mint with fakewallet backend (testing only):
```bash
export CDK_MINTD_MNEMONIC="your stable BIP39 mnemonic"
export CDK_MINTD_INIT_MODE=new
docker compose up
```

#### Mint with LDK Node backend:
```bash
export CDK_MINTD_MNEMONIC="your stable mint BIP39 mnemonic"
export CDK_MINTD_LDK_NODE_MNEMONIC="your distinct stable LDK Node BIP39 mnemonic"
export CDK_MINTD_INIT_MODE=new
docker compose -f docker-compose.ldk-node.yaml up
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
  - CDK_MINTD_INIT_MODE=new
  - CDK_MINTD_WORK_DIR=/data
volumes:
  - mint-data:/data
  - ./mint.toml:/config/mint.toml:ro
```

Run `cdk-mintd config init --new-mint --file /config/mint.toml` once with the
same persistent volume before starting a new mint. Use `--existing-mint` when
importing configuration into a database from an earlier version. Later file
changes are activated only by an explicit `config apply` followed by a restart.

The repository Compose files perform that one-shot initialization only when
`CDK_MINTD_INIT_MODE` is explicitly set to `new` or `existing`. With no mode,
an uninitialized or unreadable configuration fails closed. They never apply
later edits automatically.

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
cdk-mintd config init --new-mint --file /path/to/config.toml

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

Only one active `cdk-mintd` process may use a database. PostgreSQL is supported,
but it does not currently coordinate payment dispatch across multiple active
mint replicas.

Other environment variables are read only when explicitly named by an
`env:VARIABLE` secret reference in the persisted document. They do not act as
automatic operational overrides. The legacy `--config` and `--seed-file` flags
are rejected as normal startup inputs. `--seed-file` is accepted only by
`config migrate` to reproduce a deployment that actually used that legacy
mnemonic override.

For complete configuration options, see the [example configuration file](./example.config.toml).

## Documentation

- **[Configuration Examples](./example.config.toml)** - Complete configuration reference
- **[PostgreSQL Setup Guide](../../docker-compose.postgres.yaml)** - Database setup with Docker Compose
- **[Development Guide](../../DEVELOPMENT.md)** - Contributing and development setup

## License

This project is licensed under the [MIT License](../../LICENSE).

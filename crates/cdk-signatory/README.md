# CDK Signatory

[![crates.io](https://img.shields.io/crates/v/cdk-signatory.svg)](https://crates.io/crates/cdk-signatory)
[![Documentation](https://docs.rs/cdk-signatory/badge.svg)](https://docs.rs/cdk-signatory)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

Signing utilities and a standalone gRPC signatory service for the Cashu Development Kit (CDK).
The standalone service lets `cdk-mintd` use a remote signing process instead of keeping mint
signing keys in the mint daemon process.

## Components

This crate includes:
- A `Signatory` trait for blind signing, proof verification, and keyset rotation
- A database-backed signatory implementation
- A gRPC client and server for remote signing
- The `signatory` binary for running the standalone service

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-signatory = "*"
```

Or build the standalone binary from this workspace:

```bash
cargo build --release -p cdk-signatory --bin signatory
```

## Quick Start

The standalone binary uses SQLite by default. It stores its database and seed file in the work
directory. If `CDK_MINTD_MNEMONIC` is set, that mnemonic is used as the seed. Otherwise, the
binary reads `<work-dir>/seed` or creates a new mnemonic there on first start.

The gRPC server expects TLS files in the certs directory. The helper script creates the files
needed by both the signatory server and `cdk-mintd`.

```bash
mkdir -p ~/.cdk-signatory
bash crates/cdk-signatory/generate_certs.sh ~/.cdk-signatory

cargo run -p cdk-signatory --bin signatory -- \
  --work-dir ~/.cdk-signatory \
  --certs ~/.cdk-signatory \
  --listen-addr 127.0.0.1 \
  --listen-port 15060 \
  --enable-logging \
  --log-level info
```

For a built release binary:

```bash
./target/release/signatory \
  --work-dir ~/.cdk-signatory \
  --certs ~/.cdk-signatory
```

## Options

Show all CLI options:

```bash
cargo run -p cdk-signatory --bin signatory -- --help
```

Common options:

| Option | Description | Default |
|--------|-------------|---------|
| `--work-dir` | Directory for the SQLite database and seed file | `~/.cdk-signatory` |
| `--certs` | Directory containing `server.pem`, `server.key`, and `ca.pem` | Same as `--work-dir` |
| `--listen-addr` | gRPC bind address | `127.0.0.1` |
| `--listen-port` | gRPC bind port | `15060` |
| `--units` | Supported unit in `name,input_fee_ppk,max_order` format | `sat,0,32` |
| `--enable-logging` | Enable tracing output | `false` |
| `--log-level` | Log level when logging is enabled | `debug` |

`--units` can be repeated to support multiple units. `max_order` controls the generated powers-of-two
amounts, from `2^0` through `2^(max_order - 1)`.

## Configuration for cdk-mintd

### Config File

Point `cdk-mintd` at the remote signatory with `[signatory].enabled = true`:

```toml
[signatory]
enabled = true
address = "127.0.0.1"
port = 15060
tls_dir = "/home/user/.cdk-signatory"
allow_insecure = false
```

`tls_dir` must contain `ca.pem`, `client.pem`, and `client.key` for the `cdk-mintd` gRPC client.
The same directory created by `generate_certs.sh` can be used for both services.

### Import and Start

Add the section above to a complete `mint.toml`, then explicitly import it into
the mint database before the first start:

```bash
cdk-mintd config validate --file mint.toml
cdk-mintd config init --file mint.toml
cdk-mintd
```

Environment variables no longer override signatory settings at daemon startup.
To change them later, edit the complete file, run
`cdk-mintd config apply --file mint.toml` while the daemon is stopped, and
restart. Add `--rpc <endpoint>` to stage through a running daemon. See the
[`cdk-mintd` configuration guide](../cdk-mintd/README.md#configuration).

## Security Notes

- Back up the seed file or the stable secret referenced by `info.mnemonic`;
  losing the seed loses access to the mint signing keys.
- Keep `server.key`, `client.key`, and the seed file private.
- Use TLS for remote deployments. `allow_insecure = true` should only be used for local testing.

## License

This project is licensed under the [MIT License](../../LICENSE).

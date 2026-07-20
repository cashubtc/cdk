# LDK Node Network Configuration Guide

This guide provides configuration examples for running CDK LDK Node on different Bitcoin networks.

## Table of Contents

- [Mutinynet (Recommended for Testing)](#mutinynet-recommended-for-testing)
- [Bitcoin Testnet](#bitcoin-testnet)
- [Bitcoin Mainnet](#bitcoin-mainnet)
- [Electrum Chain Source](#electrum-chain-source)
- [Regtest (Development)](#regtest-development)
- [Docker Deployment](#docker-deployment)
- [Troubleshooting](#troubleshooting)

## Mutinynet (Recommended for Testing)

**Mutinynet** is a Bitcoin signet-based test network designed specifically for Lightning Network development with fast block times and reliable infrastructure.

### Configuration

```toml
[info]
url = "http://127.0.0.1:8085/"
listen_host = "127.0.0.1"
listen_port = 8085
mnemonic = "env:CDK_MINTD_MNEMONIC"

[database]
engine = "sqlite"

[ln]
ln_backend = "ldk-node"

[ldk_node]
bitcoin_network = "signet"
chain_source_type = "esplora"
esplora_url = "https://mutinynet.com/api"
gossip_source_type = "rgs"
rgs_url = "https://rgs.mutinynet.com/snapshot/0"
ldk_node_mnemonic = "env:CDK_MINTD_LDK_NODE_MNEMONIC"
storage_dir_path = "~/.cdk-ldk-node/mutinynet"
webserver_port = 8091
```

### Import and Start

Save the complete document as `mint.toml`, make both referenced mnemonic
variables available from your secret store, then initialize the authoritative
configuration explicitly:

```bash
cdk-mintd config validate --file mint.toml
cdk-mintd config init --file mint.toml
cdk-mintd
```

For later edits, run `cdk-mintd config apply --file mint.toml`, then restart.
Direct apply works beside a running daemon; use `--rpc <endpoint>` to select RPC
explicitly.

### Resources
- **Explorer/Faucet**: <https://mutinynet.com>
- **Esplora API**: `https://mutinynet.com/api`
- **RGS Endpoint**: `https://rgs.mutinynet.com/snapshot/0`

## Bitcoin Testnet

```toml
[ln]
ln_backend = "ldk-node"

[ldk_node]
bitcoin_network = "testnet"
esplora_url = "https://blockstream.info/testnet/api"
rgs_url = "https://rapidsync.lightningdevkit.org/snapshot"
gossip_source_type = "rgs"
storage_dir_path = "~/.cdk-ldk-node/testnet"
```

**Resources**: [Explorer](https://blockstream.info/testnet) | API: `https://blockstream.info/testnet/api`

## Bitcoin Mainnet

⚠️ **WARNING**: Uses real Bitcoin!

```toml
[ln]
ln_backend = "ldk-node"

[ldk_node]
bitcoin_network = "mainnet"
esplora_url = "https://blockstream.info/api"
rgs_url = "https://rapidsync.lightningdevkit.org/snapshot"
gossip_source_type = "rgs"
storage_dir_path = "/var/lib/cdk-ldk-node/mainnet"  # Use absolute path
webserver_host = "127.0.0.1"  # CRITICAL: Never bind to 0.0.0.0 in production
webserver_port = 8091
```

**Resources**: [Explorer](https://blockstream.info) | API: `https://blockstream.info/api`

### Production Security

🔒 **CRITICAL SECURITY CONSIDERATIONS**:

1. **Web Interface Security**: The LDK management interface has **NO AUTHENTICATION** and allows sending funds/managing channels. 
   - **NEVER** bind to `0.0.0.0` or expose publicly
   - Only use `127.0.0.1` (localhost) 
   - Use VPN, SSH tunneling, or reverse proxy with authentication for remote access
   - CSRF protection and non-permissive browser CORS reduce browser-based attack paths, but they do not authenticate users or replace access control

## Electrum Chain Source

Electrum can be used instead of Esplora or Bitcoin Core RPC on any supported Bitcoin network. The Electrum server must serve the network selected by `bitcoin_network`.

```toml
[ldk_node]
bitcoin_network = "regtest"
chain_source_type = "electrum"
electrum_url = "tcp://127.0.0.1:50001"
gossip_source_type = "p2p"
```

For an existing mint, update the complete configuration document and apply it
explicitly:

```bash
cdk-mintd config apply --file mint.toml
# Restart cdk-mintd to activate the staged configuration.
```

Direct database apply is the default and does not require management RPC. It can
run beside a steady daemon; use `--rpc <endpoint>` to select RPC explicitly.

## Regtest (Development)

```toml
[ln]
ln_backend = "ldk-node"

[ldk_node]
bitcoin_network = "regtest"
chain_source_type = "bitcoinrpc"
bitcoind_rpc_host = "127.0.0.1"
bitcoind_rpc_port = 18443
bitcoind_rpc_user = "testuser"
bitcoind_rpc_password = "env:CDK_MINTD_LDK_BITCOIND_RPC_PASSWORD"
gossip_source_type = "p2p"
```

For complete regtest environment: `just regtest` (see [REGTEST_GUIDE.md](../../REGTEST_GUIDE.md))

## Docker Deployment

⚠️ **SECURITY WARNING**: The examples below expose ports for testing. For production, **DO NOT expose port 8091** publicly as the web interface has no authentication and allows sending funds.

Use a complete mounted TOML document containing the appropriate `[ldk_node]`
section. Run `cdk-mintd config init --file ...` once against the persistent
database volume, then start the same image without a config-file argument.
Environment variables supplied to the running container are limited to database
bootstrap values and variables referenced by `env:` secret fields; they do not
override LDK settings.

See the [`cdk-mintd` Docker workflow](../cdk-mintd/README.md#docker-usage) for
the two-step initialization and startup commands. Set `info.listen_host =
"0.0.0.0"` in the imported document when publishing the mint API port.

## Troubleshooting

### Common Issues
- **RGS sync fails**: Try `gossip_source_type = "p2p"`
- **Connection errors**: Verify API endpoints with curl
- **Port conflicts**: Use `netstat -tuln` to check ports
- **Permissions**: Ensure storage directory is writable

### Debug Logging

Set logging in the imported configuration:

```toml
[info.logging]
console_level = "debug"
```

Apply the complete document and restart for the change to take effect. Direct
apply works beside a running daemon; use `--rpc <endpoint>` to select RPC
explicitly.

### Performance Tips
- Use RGS for faster gossip sync
- PostgreSQL for production
- Monitor initial sync resources

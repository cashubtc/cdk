# CDK LDK Node

CDK lightning backend for ldk-node, providing Lightning Network functionality for CDK with support for Cashu operations.

## Features

- Lightning Network payments (Bolt11 and Bolt12)
- Channel management
- Payment processing for Cashu mint operations
- Web management interface
- Support for multiple Bitcoin networks (Mainnet, Testnet, Signet/Mutinynet, Regtest)
- Esplora, Electrum, and Bitcoin Core RPC chain sources
- RGS (Rapid Gossip Sync) and P2P gossip support

## Quick Start

### Mutinynet (Recommended for Testing)

Add these settings to a complete `mint.toml`. New LDK nodes must provide their
mnemonic through an `env:` or `file:` secret reference.

```toml
[ln]
ln_backend = "ldk-node"

[ldk_node]
bitcoin_network = "signet"
chain_source_type = "esplora"
esplora_url = "https://mutinynet.com/api"
rgs_url = "https://rgs.mutinynet.com/snapshot/0"
gossip_source_type = "rgs"
ldk_node_mnemonic = "env:CDK_MINTD_LDK_NODE_MNEMONIC"
```

Make the referenced secret available, then explicitly initialize and start the
mint:

```bash
cdk-mintd config validate --file mint.toml
cdk-mintd config init --file mint.toml
cdk-mintd
```

After starting:
- Mint API: <http://127.0.0.1:8085>
- LDK management interface: <http://127.0.0.1:8091>
- Get test sats: [mutinynet.com](https://mutinynet.com)

**For complete network configuration examples, Docker setup, and production deployment, see [NETWORK_GUIDE.md](./NETWORK_GUIDE.md).**

## Web Management Interface

The CDK LDK Node includes a built-in web management interface accessible at `http://127.0.0.1:8091` by default.

⚠️ **SECURITY WARNING**: The web management interface has **NO AUTHENTICATION** and allows sending funds and managing channels. **NEVER expose it publicly** without proper authentication/authorization in front of it. Only bind to localhost (`127.0.0.1`) for security, or put it behind VPN, SSH tunneling, or authenticated reverse-proxy access. The dashboard includes CSRF protection and does not enable permissive browser CORS, but those browser hardening measures are not a substitute for access control.

### Key Features
- **Dashboard**: Node status, balance, and recent activity
- **Channel Management**: Open and close Lightning channels
- **Payment Management**: Create invoices, send payments, view history with pagination
- **On-chain Operations**: View balances and manage transactions

### Configuration

```toml
[ldk_node]
webserver_host = "127.0.0.1"  # IMPORTANT: Only localhost for security
webserver_port = 8091  # 0 = auto-assign port
```

For an existing mint, change these fields in the complete configuration, run
`cdk-mintd config apply --file mint.toml`, and restart. Direct apply works
beside a running daemon; use `--rpc <endpoint>` to select RPC explicitly.

## Basic Configuration

### Config File Example

```toml
[ln]
ln_backend = "ldk-node"

[ldk_node]
bitcoin_network = "signet"  # mainnet, testnet, signet, regtest
chain_source_type = "esplora"  # esplora, electrum, or bitcoinrpc
esplora_url = "https://mutinynet.com/api"
rgs_url = "https://rgs.mutinynet.com/snapshot/0"
gossip_source_type = "rgs"  # rgs or p2p
ldk_node_mnemonic = "env:CDK_MINTD_LDK_NODE_MNEMONIC"
webserver_port = 8091
```

### Applying Changes

LDK settings are part of the database-backed mint configuration; environment
variables do not override them when the daemon starts. Use `env:VARIABLE` only
for secret fields such as `ldk_node_mnemonic`, and use `config apply` plus a
restart for later configuration changes. Direct apply works beside a running
daemon; use `--rpc <endpoint>` to select RPC explicitly. See the
[`cdk-mintd` configuration guide](../cdk-mintd/README.md#configuration).

**For detailed network configurations, Docker setup, production deployment, and troubleshooting, see [NETWORK_GUIDE.md](./NETWORK_GUIDE.md).**

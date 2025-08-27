# CDK LDK Node

CDK lightning backend for ldk-node, providing Lightning Network functionality for CDK with support for Cashu operations.

## Features

- Lightning Network payments (Bolt11 and Bolt12)
- Channel management
- Payment processing for Cashu mint operations
- Web management interface
- Support for multiple Bitcoin networks (Mainnet, Testnet, Signet/Mutinynet, Regtest)
- RGS (Rapid Gossip Sync) and P2P gossip support

## Quick Start

### Mutinynet (Recommended for Testing)

```bash
# Using environment variables (simplest)
export CDK_MINTD_LN_BACKEND="ldk-node"
export CDK_MINTD_LDK_NODE_BITCOIN_NETWORK="signet"
export CDK_MINTD_LDK_NODE_ESPLORA_URL="https://mutinynet.com/api"
export CDK_MINTD_LDK_NODE_RGS_URL="https://rgs.mutinynet.com/snapshot/0"
export CDK_MINTD_LDK_NODE_GOSSIP_SOURCE_TYPE="rgs"

cdk-mintd
```

After starting:
- Mint API: <http://127.0.0.1:8085>
- LDK management interface: <http://127.0.0.1:8091>
- Get test sats: [mutinynet.com](https://mutinynet.com)

**For complete network configuration examples, Docker setup, and production deployment, see [NETWORK_GUIDE.md](./NETWORK_GUIDE.md).**

## Web Management Interface

The CDK LDK Node includes a built-in web management interface accessible at `http://127.0.0.1:8091` by default.

⚠️ **SECURITY WARNING**: The web management interface has **NO AUTHENTICATION** and allows sending funds and managing channels. **NEVER expose it publicly** without proper authentication/authorization in front of it. Only bind to localhost (`127.0.0.1`) for security.

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

Or via environment variables:
- `CDK_MINTD_LDK_NODE_WEBSERVER_HOST`
- `CDK_MINTD_LDK_NODE_WEBSERVER_PORT`

## Basic Configuration

### Config File Example

```toml
[ln]
ln_backend = "ldk-node"

[ldk_node]
bitcoin_network = "signet"  # mainnet, testnet, signet, regtest
esplora_url = "https://mutinynet.com/api"
rgs_url = "https://rgs.mutinynet.com/snapshot/0"
gossip_source_type = "rgs"  # rgs or p2p
webserver_port = 8091
```

### Environment Variables

All options can be set with `CDK_MINTD_LDK_NODE_` prefix:
- `CDK_MINTD_LDK_NODE_BITCOIN_NETWORK`
- `CDK_MINTD_LDK_NODE_ESPLORA_URL`
- `CDK_MINTD_LDK_NODE_RGS_URL`
- `CDK_MINTD_LDK_NODE_GOSSIP_SOURCE_TYPE`

**For detailed network configurations, Docker setup, production deployment, and troubleshooting, see [NETWORK_GUIDE.md](./NETWORK_GUIDE.md).**

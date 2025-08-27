# LDK Node Network Configuration Guide

This guide provides configuration examples for running CDK LDK Node on different Bitcoin networks.

## Table of Contents

- [Mutinynet (Recommended for Testing)](#mutinynet-recommended-for-testing)
- [Bitcoin Testnet](#bitcoin-testnet)
- [Bitcoin Mainnet](#bitcoin-mainnet)
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
storage_dir_path = "~/.cdk-ldk-node/mutinynet"
webserver_port = 8091
```

### Environment Variables

```bash
export CDK_MINTD_LN_BACKEND="ldk-node"
export CDK_MINTD_LDK_NODE_BITCOIN_NETWORK="signet"
export CDK_MINTD_LDK_NODE_ESPLORA_URL="https://mutinynet.com/api"
export CDK_MINTD_LDK_NODE_RGS_URL="https://rgs.mutinynet.com/snapshot/0"
export CDK_MINTD_LDK_NODE_GOSSIP_SOURCE_TYPE="rgs"

cdk-mintd
```

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
bitcoind_rpc_password = "testpass"
gossip_source_type = "p2p"
```

For complete regtest environment: `just regtest` (see [REGTEST_GUIDE.md](../../REGTEST_GUIDE.md))

## Docker Deployment

⚠️ **SECURITY WARNING**: The examples below expose ports for testing. For production, **DO NOT expose port 8091** publicly as the web interface has no authentication and allows sending funds.

```bash
# Mutinynet example (testing only - web interface exposed)
docker run -d \
  --name cdk-mintd \
  -p 8085:8085 -p 8091:8091 \
  -e CDK_MINTD_LN_BACKEND=ldk-node \
  -e CDK_MINTD_LDK_NODE_BITCOIN_NETWORK=signet \
  -e CDK_MINTD_LDK_NODE_ESPLORA_URL=https://mutinynet.com/api \
  -e CDK_MINTD_LDK_NODE_RGS_URL=https://rgs.mutinynet.com/snapshot/0 \
  -e CDK_MINTD_LDK_NODE_GOSSIP_SOURCE_TYPE=rgs \
  cashubtc/cdk-mintd:latest

# Production example (web interface not exposed)
docker run -d \
  --name cdk-mintd \
  -p 8085:8085 \
  --network host \
  -e CDK_MINTD_LN_BACKEND=ldk-node \
  -e CDK_MINTD_LDK_NODE_BITCOIN_NETWORK=mainnet \
  -e CDK_MINTD_LDK_NODE_WEBSERVER_HOST=127.0.0.1 \
  cashubtc/cdk-mintd:latest
```

## Troubleshooting

### Common Issues
- **RGS sync fails**: Try `gossip_source_type = "p2p"`
- **Connection errors**: Verify API endpoints with curl
- **Port conflicts**: Use `netstat -tuln` to check ports
- **Permissions**: Ensure storage directory is writable

### Debug Logging
```bash
export CDK_MINTD_LOGGING_CONSOLE_LEVEL="debug"
```

### Performance Tips
- Use RGS for faster gossip sync
- PostgreSQL for production
- Monitor initial sync resources

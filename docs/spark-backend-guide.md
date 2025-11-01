# Spark Backend Guide for CDK Mints

This guide explains how to set up and use the Spark Lightning backend with your CDK (Cashu Development Kit) mint.

## What is Spark?

Spark SDK is a nodeless Lightning implementation that enables self-custodial Lightning payments without the complexity of running and managing a full Lightning node. It's ideal for Cashu mints that want:

- **Simple setup**: No channel management or liquidity concerns
- **Lower operational costs**: No need to maintain Lightning node infrastructure
- **Self-custodial security**: Your keys never leave your application
- **Multi-protocol support**: Lightning, Spark native transfers, and on-chain Bitcoin

## Prerequisites

1. **Rust toolchain** (1.85.0 or later)
2. **BIP39 mnemonic** (12 or 24 words) for your wallet
3. **Storage directory** with appropriate permissions
4. **(Optional) Spark API key** for production use

### Getting a Spark API Key

While Spark works without an API key, getting one is recommended for production:

1. Visit [https://breez.technology/request-api-key/](https://breez.technology/request-api-key/)
2. Fill out the simple form (it's free!)
3. Receive your API key via email
4. Add it to your mint configuration

## Installation

### 1. Generate a Mnemonic

If you don't have a mnemonic yet, generate one securely:

```bash
# Using bip39 crate (install with: cargo install bip39-cli)
bip39 generate --words 24

# Or use any BIP39-compatible tool
```

**IMPORTANT**: Store this mnemonic securely! It controls your funds. Never commit it to version control.

### 2. Create Configuration File

Create a configuration file for your mint (e.g., `spark-mint.toml`):

```toml
[info]
name = "My Spark Mint"
description = "A nodeless Lightning Cashu mint"

[database]
engine = "sqlite"
connection_string = "./data/mint.db"

[[mint_keyset]]
unit = "sat"
max_order = 64

[lightning]
backend = "spark"

[lightning.spark]
network = "signet"  # Use "mainnet" for production
mnemonic = "your twenty four word mnemonic phrase goes here"
storage_dir = "./data/spark"
api_key = "your_optional_api_key"
fee_reserve_min_sat = 10
fee_reserve_percent = 0.01
```

### 3. Set Up Environment (Recommended for Production)

For production, use environment variables instead of storing the mnemonic in the config file:

```bash
# Create a .env file (add to .gitignore!)
echo "SPARK_MNEMONIC='your twenty four word mnemonic phrase'" > .env
echo "SPARK_API_KEY='your_api_key'" >> .env
```

Update your config to reference environment variables:

```toml
[lightning.spark]
network = "mainnet"
mnemonic = "${SPARK_MNEMONIC}"
api_key = "${SPARK_API_KEY}"
storage_dir = "./data/spark"
```

### 4. Start Your Mint

```bash
cdk-mintd --config spark-mint.toml
```

## Configuration Options

### Required Settings

| Setting | Type | Description |
|---------|------|-------------|
| `network` | string | Bitcoin network: `mainnet`, `testnet`, `signet`, or `regtest` |
| `mnemonic` | string | BIP39 mnemonic (12 or 24 words) |
| `storage_dir` | string | Directory for Spark wallet data |

### Optional Settings

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `passphrase` | string | None | BIP39 passphrase for extra security |
| `api_key` | string | None | Spark service provider API key |
| `fee_reserve_min_sat` | integer | 10 | Minimum fee in satoshis |
| `fee_reserve_percent` | float | 0.01 | Percentage fee (0.01 = 1%) |
| `reconnect_interval_seconds` | integer | 30 | Reconnect interval for background tasks |
| `split_secret_threshold` | integer | 2 | Threshold for secret sharing |

### Advanced: Custom Operators

You can configure custom Spark operator pools:

```toml
[lightning.spark.operator_pool]
operators = [
    { url = "https://operator1.example.com", pubkey = "02..." },
    { url = "https://operator2.example.com", pubkey = "03..." },
]
```

### Advanced: Custom Service Provider

Configure a custom Spark service provider:

```toml
[lightning.spark.service_provider]
url = "https://ssp.example.com"
api_key = "your_ssp_api_key"
```

## Testing Your Setup

### 1. Start on Signet

Always test on signet before going to mainnet:

```toml
[lightning.spark]
network = "signet"
```

### 2. Create a Mint Quote

Use the CDK CLI or API to create a mint quote:

```bash
# Using CDK CLI
cdk-cli mint --amount 100

# This will return a Lightning invoice
```

### 3. Pay the Invoice

Pay the invoice using any Lightning wallet that supports signet.

### 4. Verify Receipt

The mint should automatically detect the payment and issue ecash tokens.

## Monitoring

### Logs

Spark logs important events:

```
INFO Initializing Spark wallet for network: Signet
INFO Spark wallet initialized successfully
INFO Starting Spark payment processor
INFO Created invoice with payment hash: abc123...
INFO Received incoming payment event
```

### Check Wallet Balance

You can query the Spark wallet balance via the mint API or by examining the Spark storage directory.

## Troubleshooting

### Problem: Cannot Connect to Spark Network

**Symptoms**: Errors about operator connection failures

**Solutions**:
1. Check your internet connection
2. Verify firewall rules allow outbound HTTPS
3. Try with default operators (remove custom operator_pool config)
4. Check operator status on Spark network

### Problem: Payments Not Detected

**Symptoms**: Lightning invoices paid but mint doesn't issue ecash

**Solutions**:
1. Check logs for payment events
2. Verify storage directory is writable
3. Ensure sufficient disk space
4. Restart the mint to reconnect event listeners

### Problem: Payment Failures

**Symptoms**: Melting ecash fails with payment errors

**Solutions**:
1. Check Spark wallet has sufficient balance
2. Verify fee settings are reasonable
3. Ensure invoices are not expired
4. Try increasing fee reserves

### Problem: Storage Issues

**Symptoms**: Errors related to reading/writing Spark data

**Solutions**:
1. Check storage directory permissions
2. Verify disk space available
3. Ensure directory is not on a network mount (use local storage)
4. Back up and recreate storage if corrupted

## Security Best Practices

### Production Deployment

1. **Mnemonic Security**
   - Never commit mnemonic to version control
   - Use environment variables or secure key management
   - Consider using encrypted storage for config files
   - Backup mnemonic securely offline

2. **API Keys**
   - Store API keys as environment variables
   - Rotate keys periodically
   - Use different keys for staging and production

3. **Storage**
   - Set appropriate file permissions (600 for config, 700 for storage dir)
   - Use encrypted disk volumes in cloud environments
   - Regular backups of storage directory
   - Monitor for unauthorized access

4. **Network**
   - Use HTTPS for all mint communications
   - Consider running behind a reverse proxy
   - Implement rate limiting
   - Monitor for unusual payment patterns

5. **Monitoring**
   - Set up log aggregation
   - Alert on payment failures
   - Monitor wallet balance
   - Track payment volumes

### Backup and Recovery

**What to Backup**:
- BIP39 mnemonic (most important!)
- Storage directory (`./data/spark/`)
- Mint configuration file
- Mint database

**Recovery Process**:
1. Restore mnemonic and configuration
2. Create new storage directory
3. Spark will resync with network operators
4. Historical payments may need manual reconciliation

## Performance Tuning

### For High-Volume Mints

```toml
[lightning.spark]
# Faster reconnection for busy mints
reconnect_interval_seconds = 10

# Higher fee reserves for reliability
fee_reserve_min_sat = 50
fee_reserve_percent = 0.02

# Increase split secret threshold for security
split_secret_threshold = 3
```

### For Low-Volume Mints

```toml
[lightning.spark]
# Slower reconnection to save resources
reconnect_interval_seconds = 60

# Lower fees for competitiveness
fee_reserve_min_sat = 5
fee_reserve_percent = 0.005
```

## Migrating from Other Backends

### From LND

Spark uses different on-chain addresses and Lightning node identity. Plan for:
- New Lightning invoices (old invoices won't work)
- Different node pubkey
- No direct channel state migration

### From CLN

Similar to LND migration:
- Generate new invoices for pending operations
- Update any automation that uses node pubkey
- No plugin migration needed (Spark doesn't use plugins)

### From LDK-Node

LDK-Node and Spark are both nodeless solutions:
- Spark doesn't require channel management
- Both use similar key derivation
- Migration is primarily configuration change

## Advanced Features

### Spark Native Transfers

Beyond standard Lightning, Spark supports native protocol transfers:

```rust
// In custom mint logic
use cdk_spark::CdkSpark;

// The Spark backend automatically supports Spark addresses
// No additional configuration needed
```

### On-Chain Operations

Spark supports on-chain deposits and withdrawals:

```rust
// Access the underlying Spark wallet for advanced features
let wallet = spark_backend.wallet();

// On-chain operations available through wallet API
```

### BOLT12 Support (Coming Soon)

BOLT12 offer support is planned for future releases.

## Getting Help

- **Documentation**: [CDK Docs](https://github.com/cashubtc/cdk)
- **Spark SDK Docs**: [Spark Documentation](https://sdk-doc-spark.breez.technology/)
- **Community**: [Matrix Chat](https://matrix.to/#/#dev:matrix.cashu.space)
- **Spark Support**: [Telegram](https://t.me/breezsdk)
- **Issues**: [GitHub Issues](https://github.com/cashubtc/cdk/issues)

## Comparison with Other Backends

| Feature | Spark | LDK-Node | LND | CLN |
|---------|-------|----------|-----|-----|
| Setup Complexity | ⭐ Easy | ⭐⭐ Medium | ⭐⭐⭐ Hard | ⭐⭐⭐ Hard |
| Channel Management | ❌ Not Needed | ✅ Required | ✅ Required | ✅ Required |
| Liquidity Management | ❌ Not Needed | ✅ Required | ✅ Required | ✅ Required |
| Node Operations | ❌ Not Needed | ⚠️ Minimal | ✅ Full | ✅ Full |
| On-Chain Support | ✅ Yes | ✅ Yes | ✅ Yes | ✅ Yes |
| Self-Custodial | ✅ Yes | ✅ Yes | ✅ Yes | ✅ Yes |
| Resource Usage | 🟢 Low | 🟡 Medium | 🔴 High | 🔴 High |
| Operational Cost | 💰 Low | 💰💰 Medium | 💰💰💰 High | 💰💰💰 High |

## FAQ

**Q: Do I need to manage Lightning channels?**  
A: No! Spark handles this automatically through its network of operators.

**Q: Is Spark custodial?**  
A: No, Spark is fully self-custodial. You control your private keys.

**Q: Can I use Spark on mainnet?**  
A: Yes, but thoroughly test on signet first.

**Q: What happens if Spark operators go offline?**  
A: The network has multiple operators for redundancy. Spark will automatically route around offline operators.

**Q: Can I migrate my existing Lightning node to Spark?**  
A: Spark uses a different architecture, so direct migration isn't possible. You'll need to generate new invoices and transition gradually.

**Q: Does Spark support submarine swaps?**  
A: Spark handles Lightning-to-Spark conversions internally, which serve a similar purpose.

**Q: How much does it cost to use Spark?**  
A: Spark SDK is free. You only pay Bitcoin network fees and optional service provider fees.

**Q: Can I run multiple mints with one Spark wallet?**  
A: Each mint should have its own Spark wallet (separate mnemonic) for better isolation and security.

## Conclusion

Spark provides a simple, secure, and cost-effective way to add Lightning support to your Cashu mint without the complexity of traditional Lightning nodes. With just a mnemonic and configuration file, you can start accepting Lightning payments in minutes.

For production deployments, remember to:
- Secure your mnemonic
- Start on testnet/signet
- Monitor logs and performance
- Keep backups
- Join the community for support

Happy minting! ⚡🥜


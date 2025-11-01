# cdk-spark

Lightning backend for CDK (Cashu Development Kit) using the Spark SDK.

## Overview

`cdk-spark` provides a nodeless, self-custodial Lightning implementation for Cashu mints. Unlike traditional Lightning backends that require running and managing a full Lightning node, Spark SDK enables Lightning functionality without the operational complexity.

## Features

- **Nodeless**: No need to manage Lightning channels, liquidity, or node infrastructure
- **Self-custodial**: Private keys never leave your application
- **Multi-protocol support**:
  - ⚡ BOLT11 Lightning invoices (send & receive)
  - 🔗 Spark protocol native transfers (lower fees, faster settlement)
  - ⛓️ On-chain Bitcoin deposits and withdrawals
  - 🎫 Spark tokens (optional)
- **Network support**: Mainnet, testnet, signet, and regtest
- **Simple configuration**: Just provide a mnemonic and network

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
cdk-spark = "0.13.0"
```

## Usage

### Basic Setup

```rust
use cdk_spark::{CdkSpark, SparkConfig};
use cdk_common::common::FeeReserve;
use spark_wallet::Network;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create configuration
    let config = SparkConfig::default_for_network(
        Network::Signet,
        "your twelve or twenty four word mnemonic here".to_string(),
        "./data/spark".to_string(),
    );

    // Initialize Spark backend
    let spark = CdkSpark::new(config).await?;

    // Start the payment processor
    spark.start().await?;

    // Now the mint can accept Lightning payments!
    
    Ok(())
}
```

### With CDK Mint

In your `cdk-mintd` configuration file:

```toml
[lightning]
backend = "spark"

[lightning.spark]
network = "signet"
mnemonic = "your twelve or twenty four word mnemonic here"
storage_dir = "./data/spark"
api_key = "optional_spark_api_key"  # Optional: for Spark service provider
fee_reserve_min_sat = 10
fee_reserve_percent = 0.01
reconnect_interval_seconds = 30
split_secret_threshold = 2
```

## Configuration

### Required Settings

- **network**: Bitcoin network (`mainnet`, `testnet`, `signet`, `regtest`)
- **mnemonic**: BIP39 mnemonic phrase for wallet seed
- **storage_dir**: Directory path for Spark wallet persistent storage

### Optional Settings

- **passphrase**: Optional BIP39 passphrase
- **api_key**: API key for Spark service provider (recommended for production)
- **operator_pool**: Custom operator pool configuration
- **service_provider**: Custom service provider configuration
- **fee_reserve**: Fee settings
  - `min_fee_reserve`: Minimum fee in satoshis (default: 10)
  - `percent_fee_reserve`: Percentage fee (default: 0.01 = 1%)
- **reconnect_interval_seconds**: Reconnect interval for background tasks (default: 30)
- **split_secret_threshold**: Threshold for secret sharing in multi-sig (default: 2)

## How It Works

Spark SDK uses a novel approach to Lightning payments:

1. **Wallet Initialization**: On startup, Spark connects to the Spark network operators
2. **Invoice Creation**: When the mint needs to receive a payment, Spark creates a Lightning invoice
3. **Payment Reception**: Incoming Lightning payments are detected via event subscriptions
4. **Payment Sending**: When melting ecash, Spark routes Lightning payments through the network
5. **No Channel Management**: Unlike traditional Lightning, no channels or liquidity management required

## Comparison with Other Backends

| Feature | cdk-spark | cdk-ldk-node | cdk-lnd | cdk-cln |
|---------|-----------|--------------|---------|---------|
| Node Required | ❌ No | ✅ Yes | ✅ Yes | ✅ Yes |
| Channel Management | ❌ No | ✅ Yes | ✅ Yes | ✅ Yes |
| On-chain Support | ✅ Yes | ✅ Yes | ✅ Yes | ✅ Yes |
| Setup Complexity | 🟢 Low | 🟡 Medium | 🔴 High | 🔴 High |
| Operational Cost | 🟢 Low | 🟡 Medium | 🟡 Medium | 🟡 Medium |
| Self-Custodial | ✅ Yes | ✅ Yes | ✅ Yes | ✅ Yes |

## Security Considerations

- **Mnemonic Security**: Store your mnemonic securely. Anyone with access to it controls your funds.
- **Storage Directory**: Ensure the storage directory has appropriate permissions.
- **API Keys**: If using the Spark service provider, keep API keys confidential.
- **Network Selection**: Use testnet/signet for development and testing.

## Troubleshooting

### Connection Issues

If Spark cannot connect to the network:

1. Check your network configuration
2. Verify operator pool URLs are accessible
3. Ensure firewall rules allow outbound connections
4. Check logs for specific error messages

### Payment Failures

If payments fail:

1. Verify sufficient balance in Spark wallet
2. Check fee settings are appropriate
3. Ensure invoice is not expired
4. Review Spark service provider status

### Storage Issues

If wallet state appears corrupted:

1. Check storage directory permissions
2. Verify disk space is available
3. Review logs for write errors
4. Consider backing up and recreating storage

## Development

### Building

```bash
cargo build --package cdk-spark
```

### Testing

```bash
cargo test --package cdk-spark
```

### Integration Tests

```bash
# Run with other CDK integration tests
just itest REDB
```

## Further Reading

- [Spark SDK Documentation](https://sdk-doc-spark.breez.technology/)
- [CDK Documentation](https://github.com/cashubtc/cdk)
- [Cashu Protocol](https://github.com/cashubtc/nuts)

## License

This project is licensed under the MIT License - see the [LICENSE](../../LICENSE) file for details.

## Support

- GitHub Issues: [cashubtc/cdk](https://github.com/cashubtc/cdk/issues)
- Matrix Chat: [#dev:matrix.cashu.space](https://matrix.to/#/#dev:matrix.cashu.space)
- Spark SDK Support: [Telegram](https://t.me/breezsdk)


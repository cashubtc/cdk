# CDK LNBits

[![crates.io](https://img.shields.io/crates/v/cdk-lnbits.svg)](https://crates.io/crates/cdk-lnbits)
[![Documentation](https://docs.rs/cdk-lnbits/badge.svg)](https://docs.rs/cdk-lnbits)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

LNBits backend implementation for the Cashu Development Kit (CDK). This provides integration with [LNBits](https://lnbits.com/) for Lightning Network functionality.

**Note: Only LNBits v1 API is supported.** This backend uses the websocket-based v1 API for real-time payment notifications.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk-lnbits = "*"
```

## Configuration for cdk-mintd

### Config File

```toml
[ln]
ln_backend = "lnbits"

[lnbits]
admin_api_key = "your-admin-api-key"
invoice_api_key = "your-invoice-api-key"
lnbits_api = "https://your-lnbits-instance.com/api/v1"
fee_percent = 0.02       # Optional, defaults to 2%
reserve_fee_min = 2      # Optional, defaults to 2 sats
```

### Environment Variables

All configuration can be set via environment variables:

| Variable | Description | Required |
|----------|-------------|----------|
| `CDK_MINTD_LN_BACKEND` | Set to `lnbits` | Yes |
| `CDK_MINTD_LNBITS_ADMIN_API_KEY` | LNBits admin API key | Yes |
| `CDK_MINTD_LNBITS_INVOICE_API_KEY` | LNBits invoice API key | Yes |
| `CDK_MINTD_LNBITS_LNBITS_API` | LNBits API URL | Yes |
| `CDK_MINTD_LNBITS_FEE_PERCENT` | Fee percentage (default: `0.02`) | No |
| `CDK_MINTD_LNBITS_RESERVE_FEE_MIN` | Minimum fee in sats (default: `2`) | No |

### Example

```bash
export CDK_MINTD_LN_BACKEND=lnbits
export CDK_MINTD_LNBITS_ADMIN_API_KEY=your-admin-api-key
export CDK_MINTD_LNBITS_INVOICE_API_KEY=your-invoice-api-key
export CDK_MINTD_LNBITS_LNBITS_API=https://your-lnbits-instance.com/api/v1
cdk-mintd
```

### Getting API Keys

1. Log in to your LNBits instance
2. Go to your wallet
3. Click on "API Info" to find your admin and invoice API keys

## License

This project is licensed under the [MIT License](../../LICENSE).
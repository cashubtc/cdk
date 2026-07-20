# CDK LNBits

[![crates.io](https://img.shields.io/crates/v/cdk-lnbits.svg)](https://crates.io/crates/cdk-lnbits)
[![Documentation](https://docs.rs/cdk-lnbits/badge.svg)](https://docs.rs/cdk-lnbits)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

> **⚠️ Deprecation Notice:** 0.17.0 will be the last release where LNbits is supported as a first-class backend. Mints using LNbits should consider switching to another Lightning backend.

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
admin_api_key = "env:CDK_MINTD_LNBITS_ADMIN_API_KEY"
invoice_api_key = "env:CDK_MINTD_LNBITS_INVOICE_API_KEY"
lnbits_api = "https://your-lnbits-instance.com/api/v1"
fee_percent = 0.02       # Optional, defaults to 2%
reserve_fee_min = 2      # Optional, defaults to 2 sats
```

### Import and Start

Add the section above to a complete `mint.toml`. Set the referenced secrets,
then explicitly import the document into the mint database before the first
start:

```bash
export CDK_MINTD_LNBITS_ADMIN_API_KEY=your-admin-api-key
export CDK_MINTD_LNBITS_INVOICE_API_KEY=your-invoice-api-key
cdk-mintd config validate --file mint.toml
cdk-mintd config init --file mint.toml
cdk-mintd
```

The two environment variables above are secret inputs referenced by the
persisted document; they are not operational configuration overrides. They
must be available whenever mintd validates, applies, or starts from that
configuration.

Environment variables no longer override the LNbits backend, API URL, or fee
settings at daemon startup. To change them later, edit the complete file, run
`cdk-mintd config apply --file mint.toml` while the daemon is stopped, and
restart. Add `--rpc <endpoint>` to stage through a running daemon. See the
[`cdk-mintd` configuration guide](../cdk-mintd/README.md#configuration).

### Getting API Keys

1. Log in to your LNBits instance
2. Go to your wallet
3. Click on "API Info" to find your admin and invoice API keys

## License

This project is licensed under the [MIT License](../../LICENSE).

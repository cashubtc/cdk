# MultiMintWallet Unified Interface Guide

The MultiMintWallet has been enhanced with a unified interface that makes it easier to use by providing direct mint, melt, send, and receive functions similar to the single Wallet interface.

## Overview

Previously, MultiMintWallet was primarily a container for multiple Wallet instances, requiring manual wallet selection and management. The new unified interface provides:

- **Automatic wallet selection** based on balance, fees, and network conditions
- **Direct operations** without manual WalletKey management
- **Builder patterns** for complex operations
- **Multi-Path Payment (MPP)** support for large payments
- **Backward compatibility** with existing methods

## Key Methods

### Balance Operations

#### `total_balance(unit: &CurrencyUnit) -> Result<Amount, Error>`
Get the total balance across all wallets for a specific currency unit.

```rust
// Get total SAT balance across all wallets
let total_sats = multi_wallet.total_balance(&CurrencyUnit::Sat).await?;
println!("Total balance: {} sats", total_sats);
```

### Send Operations

#### `send(amount: Amount, unit: &CurrencyUnit, opts: SendOptions) -> Result<Token, Error>`
Send tokens with automatic wallet selection. The wallet with sufficient balance and best conditions is automatically selected.

```rust
use cdk::wallet::SendOptions;

let token = multi_wallet.send(
    Amount::from(1000),
    &CurrencyUnit::Sat,
    SendOptions::default(),
).await?;

println!("Token: {}", token);
```

#### `send_from_wallet(wallet_key: &WalletKey, amount: Amount, opts: SendOptions) -> Result<Token, Error>`
Send tokens from a specific wallet for explicit control.

```rust
let wallet_key = WalletKey::new(mint_url, CurrencyUnit::Sat);
let token = multi_wallet.send_from_wallet(
    &wallet_key,
    Amount::from(1000),
    SendOptions::default(),
).await?;
```

### Payment Operations (Melt)

#### `melt(bolt11: &str, unit: &CurrencyUnit, options: Option<MeltOptions>, max_fee: Option<Amount>) -> Result<Melted, Error>`
Pay a Lightning invoice with automatic wallet selection and Multi-Path Payment support.

```rust
let invoice = "lnbc10u1p3pj257pp5yztkwjcz5ftl5laxkav23zmzpsjd6gs7r3q33s6grge...";
let result = multi_wallet.melt(
    invoice,
    &CurrencyUnit::Sat,
    None,  // MeltOptions
    Some(Amount::from(10)), // max_fee
).await?;

println!("Payment successful: {:?}", result);
```

#### `melt_from_wallet(wallet_key: &WalletKey, bolt11: &str, options: Option<MeltOptions>, max_fee: Option<Amount>) -> Result<Melted, Error>`
Pay from a specific wallet.

### Optimization Operations

#### `swap(unit: &CurrencyUnit, amount: Option<Amount>, conditions: Option<SpendingConditions>) -> Result<Option<Proofs>, Error>`
Swap proofs with automatic wallet selection.

```rust
let swapped_proofs = multi_wallet.swap(
    &CurrencyUnit::Sat,
    Some(Amount::from(500)),
    None, // SpendingConditions
).await?;
```

#### `consolidate(unit: &CurrencyUnit) -> Result<Amount, Error>`
Consolidate proofs across wallets to optimize performance by combining smaller proofs into larger ones.

```rust
let consolidated_amount = multi_wallet.consolidate(&CurrencyUnit::Sat).await?;
println!("Consolidated {} sats worth of proofs", consolidated_amount);
```

## Builder Patterns

For complex operations, use the builder patterns which provide a fluent interface:

### Send Builder

```rust
use cdk::wallet::MultiMintWalletBuilderExt;

let token = multi_wallet
    .send_builder(Amount::from(1000), CurrencyUnit::Sat)
    .include_fee(true)
    .prefer_mint(specific_mint_url)
    .fallback_to_any(true)
    .max_fee(Amount::from(10))
    .send()
    .await?;
```

### Melt Builder

```rust
let result = multi_wallet
    .melt_builder(invoice_string, CurrencyUnit::Sat)
    .enable_mpp(true)
    .max_fee(Amount::from(20))
    .max_mpp_parts(3)
    .pay()
    .await?;
```

### Swap Builder

```rust
let proofs = multi_wallet
    .swap_builder(CurrencyUnit::Sat)
    .amount(Amount::from(500))
    .consolidate(true)
    .swap()
    .await?;
```

## Multi-Path Payment (MPP)

The unified interface includes support for Multi-Path Payment, which allows large payments to be split across multiple wallets:

```rust
// This will automatically use MPP if a single wallet doesn't have enough balance
let large_payment = multi_wallet.melt(
    large_invoice,
    &CurrencyUnit::Sat,
    None,
    Some(Amount::from(100)), // max total fee
).await?;
```

## Smart Wallet Selection

The automatic wallet selection algorithm considers:

1. **Available Balance** - Wallets with sufficient funds
2. **Fees** - Lower fee wallets are preferred
3. **Network Conditions** - Route availability for payments
4. **Proof Optimization** - Better proof distribution

## Error Handling

The unified interface provides clear error messages:

- `InsufficientFunds` - Not enough balance across all wallets
- `PaymentFailed` - Payment could not be completed
- `Custom(message)` - Specific error descriptions

## Migration from Old Interface

### Before (Container Approach)
```rust
// Manual wallet selection required
let wallets = multi_wallet.get_wallets().await;
let wallet = wallets.first().unwrap();
let prepared = wallet.prepare_send(amount, options).await?;
let token = prepared.confirm(None).await?;
```

### After (Unified Interface)
```rust
// Automatic wallet selection
let token = multi_wallet.send(amount, &unit, options).await?;
```

## CLI Usage

The CLI commands have been updated to use the new unified interface:

### Send Command
```bash
# Automatic wallet selection
cdk-cli send --amount 1000

# With specific mint (still supported)
cdk-cli send --amount 1000 --mint-url https://mint.example.com
```

### Melt Command
```bash
# Automatic wallet selection with MPP support
cdk-cli melt --bolt11 lnbc...

# With MPP explicitly enabled
cdk-cli melt --mpp --bolt11 lnbc...
```

### Balance Command
```bash
# Shows individual mint balances plus total
cdk-cli balance
```

## Backward Compatibility

All existing methods continue to work:
- `get_wallets()`
- `get_wallet(wallet_key)`
- `prepare_send(wallet_key, amount, opts)`
- `pay_invoice_for_wallet()`
- etc.

The new unified interface is additive and doesn't break existing code.

## Performance Considerations

- **Consolidation**: Use `consolidate()` periodically to optimize proof counts
- **Smart Selection**: The automatic selection reduces unnecessary swaps
- **MPP**: Large payments are automatically optimized across wallets
- **Caching**: Wallet states are cached for better performance

## Best Practices

1. **Use automatic selection** for most operations
2. **Specify wallets explicitly** only when needed for specific business logic
3. **Enable MPP** for large payments
4. **Consolidate regularly** to maintain optimal proof distribution
5. **Monitor total balances** rather than individual wallet balances
6. **Use builders** for complex operations with multiple parameters

## Examples

See the `examples/multi_mint_wallet_unified.rs` file for comprehensive usage examples.

## Testing

The new methods include comprehensive tests. Run them with:

```bash
cargo test --package cdk multi_mint_wallet
```
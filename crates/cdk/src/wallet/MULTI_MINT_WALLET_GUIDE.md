# MultiMintWallet Unified Interface Guide

The MultiMintWallet has been enhanced with a unified interface that supports only one currency unit per wallet instance, making it simpler to use by eliminating the need to specify both mint URL and currency unit for operations.

## Overview

The MultiMintWallet now operates on a single currency unit and manages multiple wallets for different mints that all use the same currency. Key changes include:

- **Single Currency Unit**: Each MultiMintWallet instance supports only one currency unit
- **MintUrl-based Operations**: Functions now take MintUrl instead of WalletKey parameters
- **Automatic wallet selection** based on balance, fees, and network conditions
- **Advanced mint control** with MultiMintSendOptions for fine-grained send operations
- **Mint prioritization** with preferred/excluded mint lists and selection strategies
- **Cross-mint sends** for splitting large payments across multiple mints
- **Direct operations** without manual mint/unit pair management
- **Multi-Path Payment (MPP)** support for large payments
- **SQLite in-memory database** for testing
- **MultiMintPreparedSend** struct for cross-wallet send operations

## Key Methods

## Constructor

The MultiMintWallet constructor now requires a currency unit to be specified:

```rust
use cdk::wallet::MultiMintWallet;
use cdk::nuts::CurrencyUnit;

// Create a new MultiMintWallet that only supports SAT
let multi_wallet = MultiMintWallet::new(
    localstore, 
    seed, 
    CurrencyUnit::Sat,  // All wallets must use this unit
    initial_wallets
)?;
```

### Balance Operations

#### `total_balance() -> Result<Amount, Error>`
Get the total balance across all wallets (since all use the same currency unit).

```rust
// Get total balance across all wallets
let total_balance = multi_wallet.total_balance().await?;
println!("Total balance: {} units", total_balance);
```

#### `get_balances() -> Result<BTreeMap<MintUrl, Amount>, Error>`
Get balances for individual mints.

```rust
// Get balance for each mint
let balances = multi_wallet.get_balances().await?;
for (mint_url, amount) in balances {
    println!("Mint {}: {} units", mint_url, amount);
}
```

### Send Operations

#### `send(amount: Amount, opts: SendOptions) -> Result<Token, Error>`
Send tokens with automatic wallet selection using default options (single mint, highest balance first).

```rust
use cdk::wallet::SendOptions;

let token = multi_wallet.send(
    Amount::from(1000),
    SendOptions::default(),
).await?;

println!("Token: {}", token);
```

#### `send_with_options(amount: Amount, options: MultiMintSendOptions) -> Result<Token, Error>`
Send tokens with advanced mint selection and priority control.

```rust
use cdk::wallet::{MultiMintSendOptions, MintSelectionStrategy};

let options = MultiMintSendOptions::new()
    .max_mints(2)
    .selection_strategy(MintSelectionStrategy::LowestBalanceFirst)
    .prefer_mint(trusted_mint_url);

let token = multi_wallet.send_with_options(
    Amount::from(1000),
    options
).await?;
```

#### `send_from_wallet(mint_url: &MintUrl, amount: Amount, opts: SendOptions) -> Result<Token, Error>`
Send tokens from a specific wallet for explicit control.

```rust
use cdk::mint_url::MintUrl;

let mint_url = MintUrl::from_str("https://mint.example.com")?;
let token = multi_wallet.send_from_wallet(
    &mint_url,
    Amount::from(1000),
    SendOptions::default(),
).await?;
```

### MultiMintPreparedSend

The `MultiMintPreparedSend` struct allows for preparing cross-wallet sends:

```rust
use cdk::wallet::MultiMintPreparedSend;

// This would be created internally when implementing cross-wallet sends
let prepared_sends = vec![prepared_send_1, prepared_send_2];
let multi_prepared = MultiMintPreparedSend::new(prepared_sends, CurrencyUnit::Sat)?;

println!("Total amount: {}", multi_prepared.amount());
println!("Total fee: {}", multi_prepared.fee());
println!("Wallets involved: {}", multi_prepared.wallet_count());

// Confirm all sends
let token = multi_prepared.confirm(None).await?;
```

### Advanced Send Options

The `MultiMintSendOptions` struct provides fine-grained control over which mints to use for sending:

#### Basic Usage

```rust
use cdk::wallet::{MultiMintSendOptions, MintSelectionStrategy};

// Simple send with specific mint preference
let options = MultiMintSendOptions::new()
    .prefer_mint(mint_url_1)
    .prefer_mint(mint_url_2);  // mint_url_1 has higher priority

let token = multi_wallet.send_with_options(
    Amount::from(1000),
    options
).await?;
```

#### Maximum Mint Control

```rust
// Limit to using at most 2 mints
let options = MultiMintSendOptions::new()
    .max_mints(2);

// Allow unlimited mints (for large cross-mint sends)
let options = MultiMintSendOptions::new()
    .unlimited_mints()
    .allow_cross_mint(true);
```

#### Mint Exclusion

```rust
// Exclude specific mints (e.g., due to high fees or unreliability)
let options = MultiMintSendOptions::new()
    .exclude_mint(unreliable_mint_url)
    .exclude_mint(high_fee_mint_url);

let token = multi_wallet.send_with_options(amount, options).await?;
```

#### Selection Strategies

```rust
// Use mint with highest balance first (default)
let options = MultiMintSendOptions::new()
    .selection_strategy(MintSelectionStrategy::HighestBalanceFirst);

// Use mint with lowest balance first (good for consolidation)
let options = MultiMintSendOptions::new()
    .selection_strategy(MintSelectionStrategy::LowestBalanceFirst);

// Use mints with lowest fees first
let options = MultiMintSendOptions::new()
    .selection_strategy(MintSelectionStrategy::LowestFeesFirst);

// Random mint selection
let options = MultiMintSendOptions::new()
    .selection_strategy(MintSelectionStrategy::Random);

// Round-robin across mints
let options = MultiMintSendOptions::new()
    .selection_strategy(MintSelectionStrategy::RoundRobin);
```

#### Cross-Mint Sends

```rust
// Enable cross-mint sends (split a large payment across multiple mints)
let options = MultiMintSendOptions::new()
    .max_mints(3)                    // Use up to 3 mints
    .allow_cross_mint(true)          // Allow splitting across mints
    .preferred_mints(vec![mint1, mint2]); // Prefer these mints first

// This will automatically split the amount across selected mints
let token = multi_wallet.send_with_options(
    Amount::from(10000),  // Large amount
    options
).await?;
```

#### Complete Example

```rust
use cdk::wallet::{MultiMintSendOptions, MintSelectionStrategy, SendOptions};

let options = MultiMintSendOptions::new()
    .max_mints(2)                           // Use at most 2 mints
    .prefer_mint(trusted_mint_url)          // Prefer this mint first
    .exclude_mint(slow_mint_url)            // Never use this mint
    .selection_strategy(MintSelectionStrategy::LowestFeesFirst)
    .allow_cross_mint(true)                 // Allow splitting if needed
    .send_options(SendOptions::default());  // Base send options

let token = multi_wallet.send_with_options(
    Amount::from(5000),
    options
).await?;

println!("Sent token: {}", token);
```

### Selection Strategy Use Cases

Different selection strategies are optimal for different scenarios:

#### HighestBalanceFirst (Default)
- **Best for**: General use, reliability
- **When to use**: When you want to use mints with the most available funds first
- **Benefits**: Reduces fragmentation, typically more reliable

#### LowestBalanceFirst  
- **Best for**: Consolidating small balances, cleanup
- **When to use**: When you want to empty out smaller mint balances first
- **Benefits**: Helps consolidate funds, reduces number of active mint connections

#### LowestFeesFirst
- **Best for**: Cost optimization
- **When to use**: When minimizing fees is the primary concern
- **Benefits**: Reduces transaction costs

#### Random
- **Best for**: Privacy, load distribution
- **When to use**: When you want to avoid predictable patterns
- **Benefits**: Better privacy, helps distribute load across mints

#### RoundRobin
- **Best for**: Even distribution, fairness
- **When to use**: When you want to use all mints equally over time
- **Benefits**: Balanced usage across all available mints

### Payment Operations (Melt)

#### `melt(bolt11: &str, options: Option<MeltOptions>, max_fee: Option<Amount>) -> Result<Melted, Error>`
Pay a Lightning invoice with automatic wallet selection and Multi-Path Payment support.

```rust
let invoice = "lnbc10u1p3pj257pp5yztkwjcz5ftl5laxkav23zmzpsjd6gs7r3q33s6grge...";
let result = multi_wallet.melt(
    invoice,
    None,  // MeltOptions
    Some(Amount::from(10)), // max_fee
).await?;

println!("Payment successful: {:?}", result);
```

#### `melt_from_wallet(mint_url: &MintUrl, bolt11: &str, options: Option<MeltOptions>, max_fee: Option<Amount>) -> Result<Melted, Error>`
Pay from a specific wallet.

```rust
let mint_url = MintUrl::from_str("https://mint.example.com")?;
let result = multi_wallet.melt_from_wallet(
    &mint_url,
    invoice,
    None,
    Some(Amount::from(10))
).await?;
```

### Optimization Operations

#### `swap(amount: Option<Amount>, conditions: Option<SpendingConditions>) -> Result<Option<Proofs>, Error>`
Swap proofs with automatic wallet selection.

```rust
let swapped_proofs = multi_wallet.swap(
    Some(Amount::from(500)),
    None, // SpendingConditions
).await?;
```

#### `consolidate() -> Result<Amount, Error>`
Consolidate proofs across wallets to optimize performance by combining smaller proofs into larger ones.

```rust
let consolidated_amount = multi_wallet.consolidate().await?;
println!("Consolidated {} units worth of proofs", consolidated_amount);
```

## Builder Patterns

For complex operations, use the builder patterns which provide a fluent interface:

### Send Builder

```rust
// Builder patterns are planned for future implementation
// For now, use direct methods:
let token = multi_wallet.send(
    Amount::from(1000),
    SendOptions::default()
).await?;
```

### Melt Builder

```rust
// Builder patterns are planned for future implementation
// For now, use direct methods:
let result = multi_wallet.melt(
    invoice_string,
    None, // MeltOptions
    Some(Amount::from(20)) // max_fee
).await?;
```

### Swap Builder

```rust
// Builder patterns are planned for future implementation
// For now, use direct methods:
let proofs = multi_wallet.swap(
    Some(Amount::from(500)),
    None // SpendingConditions
).await?;
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

### Before (WalletKey-based)
```rust
// Required specifying both mint URL and currency unit
let wallet_key = WalletKey::new(mint_url, CurrencyUnit::Sat);
let prepared = multi_wallet.prepare_send(&wallet_key, amount, options).await?;
let token = prepared.confirm(None).await?;

// Getting balance required unit parameter
let balance = multi_wallet.total_balance(&CurrencyUnit::Sat).await?;
```

### After (Single Currency Unit)
```rust
// Create wallet with fixed currency unit
let multi_wallet = MultiMintWallet::new(
    localstore, 
    seed, 
    CurrencyUnit::Sat,  // Fixed for this wallet instance
    initial_wallets
)?;

// Automatic wallet selection - no unit needed
let token = multi_wallet.send(amount, options).await?;

// Balance operations don't need unit specification
let balance = multi_wallet.total_balance().await?;

// Mint-specific operations use MintUrl directly
let mint_url = MintUrl::from_str("https://mint.example.com")?;
let prepared = multi_wallet.prepare_send(&mint_url, amount, options).await?;
```

## CLI Usage

The CLI commands work with the new single-currency-unit interface:

### Send Command
```bash
# Automatic wallet selection (uses configured currency unit)
cdk-cli send --amount 1000

# With specific mint
cdk-cli send --amount 1000 --mint-url https://mint.example.com
```

### Melt Command
```bash
# Automatic wallet selection with MPP support
cdk-cli melt --bolt11 lnbc...

# From specific mint
cdk-cli melt --mint-url https://mint.example.com --bolt11 lnbc...
```

### Balance Command
```bash
# Shows individual mint balances plus total (all in same currency unit)
cdk-cli balance
```

## Breaking Changes

**Note: This is a breaking change from the previous interface.**

Methods that previously took `WalletKey` now take `MintUrl`:
- `get_wallet(mint_url)` - previously `get_wallet(wallet_key)`
- `prepare_send(mint_url, amount, opts)` - previously `prepare_send(wallet_key, amount, opts)`
- `pay_invoice_for_wallet(bolt11, options, mint_url, max_fee)` - previously used `wallet_key`
- etc.

Methods that previously required currency unit parameters no longer need them:
- `total_balance()` - previously `total_balance(unit)`
- `send(amount, opts)` - previously `send(amount, unit, opts)`
- `melt(bolt11, options, max_fee)` - previously `melt(bolt11, unit, options, max_fee)`

The constructor now requires a currency unit parameter:
- `MultiMintWallet::new(localstore, seed, unit, wallets)` - previously `new(localstore, seed, wallets)`

## New Features

**New methods added:**
- `send_with_options(amount, MultiMintSendOptions)` - Advanced send control with mint selection
- `MultiMintSendOptions` - Fine-grained control over mint selection and priority
- `MintSelectionStrategy` - Various strategies for automatic mint selection
- Cross-mint send support (when enabled via `allow_cross_mint(true)`)
- `MultiMintPreparedSend` - For handling multi-wallet prepared sends

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

The MultiMintWallet now uses SQLite in-memory database for testing, providing better test isolation and performance:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    async fn create_test_multi_wallet() -> MultiMintWallet {
        let localstore = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("Failed to create in-memory database")
        );
        let seed = [0u8; 64];
        let wallets = vec![];

        MultiMintWallet::new(localstore, seed, CurrencyUnit::Sat, wallets)
            .expect("Failed to create MultiMintWallet")
    }
    
    #[tokio::test]
    async fn test_total_balance_empty() {
        let multi_wallet = create_test_multi_wallet().await;
        let balance = multi_wallet.total_balance().await.unwrap();
        assert_eq!(balance, Amount::ZERO);
    }
}
```

Run the tests with:

```bash
cargo test --package cdk multi_mint_wallet
```

The in-memory database ensures tests run quickly and don't interfere with each other.
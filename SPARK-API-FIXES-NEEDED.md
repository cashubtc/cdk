# Spark API Fixes Needed

**Status**: Code complete but needs API adjustments to match actual Spark SDK  
**Priority**: HIGH - Blocks compilation  
**Estimated Fix Time**: 1-2 hours  

---

## 🔴 Compilation Errors Found

### Error 1: Signer Creation

**Current (Wrong)**:
```rust
let keyset = KeySet::from_mnemonic(
    KeySetType::Mainnet,
    mnemonic.to_string(),
    config.passphrase.clone(),
)?;

let signer = Arc::new(DefaultSigner::new(keyset)?);
```

**Correct API**:
```rust
// Convert mnemonic to seed bytes
let seed = mnemonic.to_seed(config.passphrase.as_deref().unwrap_or(""));

// Create signer with seed and network
let signer = Arc::new(
    DefaultSigner::new(&seed, config.network)?
);
```

**File**: `cdk/crates/cdk-spark/src/lib.rs` lines 85-100

---

### Error 2: WalletBuilder API

**Current (Wrong)**:
```rust
let mut wallet_builder = WalletBuilder::new(config.network);
wallet_builder = wallet_builder.storage_dir(config.storage_dir.clone());
wallet_builder = wallet_builder.operator_pool(operator_config.clone());
// etc.
```

**Correct API**:
```rust
// Build SparkWalletConfig first
let wallet_config = SparkWalletConfig {
    network: config.network,
    operator_pool: config.operator_pool
        .unwrap_or_else(|| SparkWalletConfig::default_operator_pool_config(config.network)),
    reconnect_interval_seconds: config.reconnect_interval_seconds,
    service_provider_config: config.service_provider
        .unwrap_or_else(|| {
            SparkWalletConfig::default_config(config.network).service_provider_config
        }),
    split_secret_threshold: config.split_secret_threshold as u32,
    tokens_config: SparkWalletConfig::default_tokens_config(),
};

// Then create wallet
let wallet_builder = WalletBuilder::new(wallet_config, signer);
let wallet = wallet_builder.build().await?;
```

**File**: `cdk/crates/cdk-spark/src/lib.rs` lines 102-125

---

### Error 3: Event Subscription

**Current (Wrong)**:
```rust
let mut event_stream = wallet.subscribe_events().await;
```

**Correct API**:
```rust
let mut event_stream = wallet.subscribe_events();  // Not async!
```

**File**: `cdk/crates/cdk-spark/src/lib.rs` line 182

---

### Error 4: WalletEvent Variants

**Current (Wrong)**:
```rust
match event {
    WalletEvent::IncomingPayment { payment } => {
        // ...
    }
}
```

**Correct API**:
```rust
// WalletEvent only has these variants:
// - DepositConfirmed(TreeNodeId)
// - StreamConnected
// - StreamDisconnected  
// - Synced
// - TransferClaimed(WalletTransfer)

// For Lightning payments, need to check TransferClaimed events
// and filter for Lightning-related transfers
match event {
    WalletEvent::TransferClaimed(transfer) => {
        // Check if this transfer is from Lightning payment
        // Extract payment details from transfer
    }
    _ => {}
}
```

**File**: `cdk/crates/cdk-spark/src/lib.rs` lines 192-220

---

### Error 5: WalletTransfer Field Names

**Current (Wrong)**:
```rust
let total_spent_sat = result.transfer.amount_sat;
```

**Correct API**:
```rust
let total_spent_sat = result.transfer.total_value_sat;
```

**File**: `cdk/crates/cdk-spark/src/lib.rs` line 406

---

### Error 6: Amount Type Conversions

**Current (Wrong)**:
```rust
let amount_sat = amountless.amount_msat / 1000;
```

**Correct**:
```rust
let amount_sat = u64::from(amountless.amount_msat) / 1000;
```

**File**: `cdk/crates/cdk-spark/src/lib.rs` line 383

---

### Error 7: Unused Imports

Remove these unused imports:
```rust
use std::time::Duration;  // Remove
use bitcoin::hashes::Hash;  // Remove
use futures::stream::StreamExt;  // Remove
```

**File**: `cdk/crates/cdk-spark/src/lib.rs` lines 13, 16, 23

---

## 📝 Complete Fix Checklist

In `cdk/crates/cdk-spark/src/lib.rs`:

- [ ] Line 85-100: Fix signer creation (use `mnemonic.to_seed()` and `DefaultSigner::new(&seed, network)`)
- [ ] Line 102-125: Fix WalletBuilder (create `SparkWalletConfig` first)
- [ ] Line 182: Remove `.await` from `subscribe_events()`
- [ ] Line 192-220: Fix event handling (use `TransferClaimed` not `IncomingPayment`)
- [ ] Line 383: Fix amount division (convert to u64 first)
- [ ] Line 406: Change `amount_sat` to `total_value_sat`
- [ ] Lines 13, 16, 23: Remove unused imports

In `cdk/crates/cdk-spark/src/config.rs`:

- [ ] Update SparkConfig to match SparkWalletConfig requirements
- [ ] Add operator_pool as OperatorPoolConfig type (not Option)
- [ ] Add service_provider as ServiceProviderConfig type (not Option)
- [ ] Change split_secret_threshold to u32

---

## 🔧 RECOMMENDED APPROACH

Since these are API mismatches with the underlying Spark SDK, the next agent should:

### Step 1: Study Spark SDK APIs (30 min)

Read these files to understand actual API:
- `spark-sdk/crates/spark-wallet/src/wallet.rs` - Main wallet methods
- `spark-sdk/crates/spark-wallet/src/config.rs` - Config structure
- `spark-sdk/crates/spark-wallet/src/model.rs` - Event types
- `spark-sdk/crates/spark-wallet/src/wallet_builder.rs` - Builder pattern

### Step 2: Rewrite CdkSpark Constructor (1 hour)

The `CdkSpark::new()` method needs major changes to:
1. Convert mnemonic to seed properly
2. Create signer correctly
3. Build SparkWalletConfig with all required fields
4. Use WalletBuilder correctly

### Step 3: Fix Event Handling (30 min)

Since WalletEvent doesn't have IncomingPayment:
- Subscribe to TransferClaimed events
- Check if transfer is Lightning-related
- Extract payment info from transfer
- Map to CDK payment events

### Step 4: Fix Method Signatures (15 min)

Update method calls to match Spark SDK:
- Fix `pay_lightning_invoice()` arguments
- Fix amount conversions
- Fix field names

### Step 5: Test Compilation (5 min)

```bash
cargo check --package cdk-spark
```

Should compile without errors.

---

## 📋 ALTERNATIVE: Start From Reference Implementation

If fixes are too complex, consider:

1. **Find working Breez SDK integration** - Check if Breez has CDK integration examples
2. **Use spark-sdk examples** - Study how they use the wallet API
3. **Simplify initial version** - Get basic invoice creation working first

---

## 🎯 QUICK WIN APPROACH

For fastest results:

### Option A: Fix Incrementally

1. Fix signer creation first → compile → fix next error → repeat
2. This ensures each fix is correct before moving on
3. Easier to debug

### Option B: Rewrite CdkSpark::new() Completely

Based on spark-sdk examples:

```rust
pub async fn new(config: SparkConfig) -> Result<Self, Error> {
    config.validate()?;
    
    // Convert mnemonic to seed
    let mnemonic = bip39::Mnemonic::from_str(&config.mnemonic)
        .map_err(|e| Error::InvalidMnemonic(e.to_string()))?;
    let seed = mnemonic.to_seed(config.passphrase.as_deref().unwrap_or(""));
    
    // Create signer
    let signer = Arc::new(
        DefaultSigner::new(&seed, config.network)
            .map_err(|e| Error::Configuration(format!("Signer creation failed: {}", e)))?
    );
    
    // Build wallet config
    let wallet_config = SparkWalletConfig {
        network: config.network,
        operator_pool: config.operator_pool.unwrap_or_else(|| 
            SparkWalletConfig::default_operator_pool_config(config.network)
        ),
        reconnect_interval_seconds: config.reconnect_interval_seconds,
        service_provider_config: config.service_provider.unwrap_or_else(||
            SparkWalletConfig::default_config(config.network).service_provider_config
        ),
        split_secret_threshold: config.split_secret_threshold as u32,
        tokens_config: SparkWalletConfig::default_tokens_config(),
    };
    
    // Create wallet
    let wallet = WalletBuilder::new(wallet_config, signer).build().await?;
    
    // ... rest of initialization
}
```

---

## ⚠️ BREAKING CHANGES NEEDED

### In SparkConfig (config.rs)

Change types to match SparkWalletConfig:
```rust
pub struct SparkConfig {
    pub network: Network,
    pub mnemonic: String,
    pub passphrase: Option<String>,
    // Remove storage_dir - not used by SparkWalletConfig
    // pub storage_dir: String,  
    pub api_key: Option<String>,
    pub operator_pool: Option<OperatorPoolConfig>,  // Type change!
    pub service_provider: Option<ServiceProviderConfig>,  // Type change!
    pub fee_reserve: FeeReserve,
    pub reconnect_interval_seconds: u64,
    pub split_secret_threshold: u32,  // Type change: usize → u32
}
```

---

## 📚 REFERENCE: Correct Spark SDK Types

```rust
use spark_wallet::{
    DefaultSigner,
    Network,
    SparkWallet,
    SparkWalletConfig,
    WalletBuilder,
    WalletEvent,
    LightningReceivePayment,
    PayLightningInvoiceResult,
};
use spark::{
    operator::{OperatorConfig, OperatorPoolConfig},
    services::TokensConfig,
    ssp::ServiceProviderConfig,
};
```

---

## ✅ WHAT'S STILL GOOD

These parts don't need changes:
- ✅ Error handling structure
- ✅ Configuration validation logic
- ✅ Fee calculation
- ✅ MintPayment trait structure
- ✅ Documentation
- ✅ Test structure

Only the Spark SDK API calls need updating!

---

## 🔋 FOR LOW BATTERY SCENARIO

**Minimum to commit before closing**:

This document (SPARK-API-FIXES-NEEDED.md) contains everything needed to:
1. Understand what's wrong
2. Know how to fix it
3. Have code examples ready
4. Resume quickly

---

**Next agent: Start with "Quick Win Approach Option A" above!** 🚀

Total estimated fix time: **2-3 hours to working build**


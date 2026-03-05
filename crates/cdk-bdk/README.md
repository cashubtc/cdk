# CDK BDK

CDK onchain payment backend using [BDK (Bitcoin Development Kit)](https://bitcoindevkit.org/), providing on-chain Bitcoin payment functionality for CDK mint operations.

## Features

- On-chain Bitcoin payments (receive and send)
- BIP84 (Native SegWit) key derivation from a BIP39 mnemonic
- SQLite-backed wallet persistence via BDK
- Blockchain sync via Bitcoin Core RPC
- Confirmation tracking for incoming and outgoing transactions
- Pending transaction persistence via KV store

## Chain Sources

| Source | Status |
|---|---|
| Bitcoin Core RPC | Supported |
| Esplora | Supported |

## Usage

```rust,ignore
use cdk_bdk::{CdkBdk, ChainSource, BitcoinRpcConfig};

let chain_source = ChainSource::BitcoinRpc(BitcoinRpcConfig {
    host: "127.0.0.1".to_string(),
    port: 18443,
    user: "user".to_string(),
    password: "password".to_string(),
});

let backend = CdkBdk::new(
    mnemonic,
    Network::Regtest,
    chain_source,
    "/path/to/storage".to_string(),
    fee_reserve,
    kv_store,
    num_confs,
)?;
```

## Current Limitations

- The `stop()` method does not yet cancel background sync tasks

## Finality and Confirmation Policy

- Finalization is policy-based: once a send or receive reaches `num_confs`, it is
  treated as final and is not reopened after deep reorgs
- Incoming payment tracking is confirmed-only; unconfirmed tracked outputs are
  intentionally ignored until they satisfy the configured confirmation threshold

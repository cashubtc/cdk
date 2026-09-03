# CDK wallet engine

`cdk::Wallet` is the protocol-level engine for one mint and one currency unit.
It exposes proof selection, keysets, NUT-specific options, sagas, recovery, and
other controls needed by advanced integrations.

Most product applications should use the portable application facade in
`cdk-ffi`. That facade is directly callable from Rust and is also the single
source for all generated language bindings. See
[`docs/wallet-api.md`](../../../../docs/wallet-api.md).

## Engine example

```rust
use std::sync::Arc;

use cdk::nuts::CurrencyUnit;
use cdk::wallet::Wallet;
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let seed = random::<[u8; 64]>();
    let localstore = memory::empty().await?;
    let wallet = Wallet::new(
        "https://testnut.cashudevkit.org",
        CurrencyUnit::Sat,
        Arc::new(localstore),
        seed,
        None,
    )?;

    let report = wallet.recover_incomplete_sagas().await?;
    println!(
        "Recovered: {}, compensated: {}, pending: {}, failed: {}",
        report.recovered, report.compensated, report.skipped, report.failed
    );

    Ok(())
}
```

Prepared engine operations are persisted by operation ID. Keep the saga store
authoritative and use the wallet's loader methods when reconstructing a
prepared or pending operation after restart.

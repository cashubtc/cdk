# CDK Wallet

The CDK [`Wallet`] is a high level Cashu wallet. The [`Wallet`] is for a single mint and single unit. Multiple [`Wallet`]s can be created to support multi mints and multi units.


## Example

### Create and Initialize [`Wallet`]

```rust
use std::sync::Arc;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::Wallet;
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let seed = random::<[u8; 64]>();
    let mint_url = "https://testnut.cashudevkit.org";
    let unit = CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;

    // Required: Recover crashed operations (swap, send, receive, melt)
    // This prevents proofs from being stuck in reserved states.
    let report = wallet.recover_incomplete_sagas().await?;
    println!("Recovered: {}, Compensated: {}, Skipped: {}, Failed: {}",
        report.recovered, report.compensated, report.skipped, report.failed);

    // Optional: Check and mint pending mint quotes (makes network calls)
    let minted = wallet.mint_unissued_quotes().await?;
    println!("Minted {} from pending quotes", minted);

    Ok(())
}
```

## Choose a seed restore starting position

`restore()` scans each keyset from index zero. `restore_with_opts()` also starts
from zero by default, and accepts a `RestoreStart` mode through `NUT13Options`:

- `FromZero` performs a full scan.
- `KeysetIndices(HashMap<Id, u32>)` sets an inclusive starting index for each
  keyset. Missing entries start at zero. Use this with independently tracked
  recovery positions.
- `Lookback(u32)` starts at each keyset's stored next derivation index minus
  the lookback, clamped to zero. For example, a lookback of 100 starts keysets
  with counters of 1,000 and 250 at 900 and 150. Keysets without a stored counter
  start at zero. Empty batches starting before the stored counter do not count
  toward the gap limit, preserving forward scan coverage.

```rust,no_run
use cdk::wallet::{NUT13Options, RestoreStart};

# async fn example(wallet: cdk::Wallet) -> Result<(), cdk::Error> {
let opts = NUT13Options::new(100, 3)?.with_start(RestoreStart::Lookback(100));
let restored = wallet.restore_with_opts(opts).await?;
# Ok(())
# }
```

Recovered batches update the stored next derivation index past recovered
signatures without decreasing an existing counter. Empty batches do not
advance it. These counters track secret allocation, not completed recovery
work, so lookback can skip earlier proofs and does not reliably resume an
interrupted scan. Use a full scan or independently saved per-keyset recovery
positions when earlier history must be recovered. Returned amounts include all
proofs found in the scanned range, including proofs already stored locally.

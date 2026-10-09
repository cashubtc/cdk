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

## Resume a seed restore

`restore()` and `restore_with_opts()` scan from index zero. After an interrupted
restore, reuse the same database and seed and call `resume_restore(None)` to
continue from each keyset's stored next derivation index. To rescan recent
history, pass a lookback: `resume_restore(Some(100))` starts 100 indices before
each stored counter, clamped to zero. These methods apply to the wallet's mint
and unit; keysets without a stored counter start at zero.

Progress is saved after each successfully recovered batch. Empty batches do not
advance the stored counter, so resuming may repeat the trailing empty portion
of a scan. Returned amounts include all proofs found in the scanned range,
including proofs already stored locally.

Use `resume_restore_with_opts(how_far_back, opts)` to override the NUT-13 batch
size and gap limit, just as with `restore_with_opts(opts)`.

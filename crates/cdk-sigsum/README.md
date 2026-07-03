# CDK Sigsum

[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**EXPERIMENTAL** This crate accompanies `docs/adr/0001-append-only-transparency-log.md` and is not wired into `cdk-mintd` yet.

A client for the [Sigsum](https://www.sigsum.org/) transparency log protocol. Sigsum logs are content-agnostic append-only Merkle-tree logs of signed 32-byte checksums, backed by real, already-operating public infrastructure (see [sigsum.org/services](https://www.sigsum.org/services/)).

This crate lets a CDK mint anchor its own local transparency-log checkpoints to a public Sigsum log instead of the mint (or the cashu ecosystem) having to run and operate new transparency-log infrastructure of its own.

## What this crate does

- Implements the five HTTP endpoints of the [Sigsum log server protocol](https://git.glasklar.is/sigsum/project/documentation/-/blob/main/log.md): `get-tree-head`, `get-inclusion-proof`, `get-consistency-proof`, `get-leaves`, `add-leaf`.
- Implements the submitter side of [domain-based rate limiting](https://git.glasklar.is/sigsum/project/documentation/-/blob/main/log.md#4--rate-limiting) (DNS `_sigsum_v1.<domain>` TXT record + per-log submit token).
- Assembles a self-contained, [offline-verifiable proof of logging](https://git.glasklar.is/sigsum/core/sigsum-go/-/blob/main/doc/sigsum-proof.md) for a submitted entry.

## What this crate does not do

- It does not verify proofs. Use the [`sigsum`](https://docs.rs/sigsum) crate (maintained by Mullvad) for offline verification of the proof format produced here.
- It does not decide what gets anchored. The mint's own event log and Merkle tree (see the ADR) remain the source of truth; only the periodic checkpoint's root hash is ever submitted here — never raw mint data.

## Usage

See the doc-tested example on [`anchor`](https://docs.rs/cdk-sigsum/latest/cdk_sigsum/fn.anchor.html), or:

```rust,no_run
use cdk_sigsum::{anchor, SigsumClient};
use ed25519_dalek::{SigningKey, VerifyingKey};
use url::Url;

# async fn example(
#     log_public_key: VerifyingKey,
#     submit_key: SigningKey,
#     checkpoint_bytes: &[u8],
# ) -> Result<(), cdk_sigsum::Error> {
let client = SigsumClient::new(Url::parse("https://seasalp.glasklar.is/")?);
let proof = anchor(&client, &log_public_key, &submit_key, None, checkpoint_bytes).await?;
println!("{}", proof.to_ascii());
# Ok(())
# }
```

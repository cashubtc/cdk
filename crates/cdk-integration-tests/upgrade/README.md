# Mint and wallet upgrade integration tests

Run in the development shell with local release tags available:

```sh
nix develop .#stable
just test-upgrade
# Quick smoke test:
just test-upgrade --rounds 2 --cases normal --orders mint-first
# Choose a different released baseline:
just test-upgrade --from v0.17.6
```

The Rust runner builds **v0.17.7** and the current checkout, then upgrades
the mint and wallet using their existing SQLite databases. It tests both
**mint-first** and **wallet-first** orders across six configurations: mnemonic, short raw seed, custom metadata
public key, their combination, omitted optional settings, and non-default settings.
It checks config migration/application, wallet settings, signing identity,
balances, double-spend rejection, and restoration from the original wallet seed.
Migration checks every fixture field and extracted secret; no-op applies and
rejected signing-key changes must preserve the stored configuration.

Each scenario runs four workload phases. A round completes six money operations:
one mint, two sends, two receives, and one melt. Handoffs and pending fixtures
add nine operations per scenario; polling and internal swaps are excluded.

| Run | Rounds per phase | Operations per completed scenario | All twelve scenarios |
| --- | ---: | ---: | ---: |
| Local / manual CI | 25 | 609 | 7,308 |
| Push / PR CI | 5 | 129 | 1,548 |

The suite carries these states through upgrades:

- Unredeemed sent token: receive it and reconcile the sender afterward.
- Paid, unissued mint quote: claim it once and reject replay.
- Unpaid mint quote: retain its claim key and mint after payment arrives.
- Prepared send and prepared melt: recover reserved proofs and allow reuse.
- Pending melt, successful payment: retain locked funds until settlement.
- Pending melt, failed payment: refund after failure without redispatching.

Recovery checks include proof identities, expected balances, and idempotence
after reopening the wallet. A file-backed fake payment controller is injected
into disposable builds to hold payments across restarts. This covers SQLite,
the embedded signer, and graceful shutdown; real Lightning and crash recovery
require separate scenarios.

Failures preserve sources, binaries, databases, config, and logs in the printed
artifact directory. `report.json` and `operations.json` record operation counts.
Successful runs remove artifacts. Scratch defaults to `/data/rust/tmp` when
available; override it with `--scratch-root PATH`.

The metadata cases currently expose **BK-003** and fail during config import;
they are compatibility assertions, not expected failures. Keep a v0.17 baseline
for legacy seed and metadata coverage; later baselines can use `--cases normal`.

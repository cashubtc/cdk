# Federated CDK TUI

`fed-cdk-tui` is a persistent Cashu wallet and live federation dashboard for
CDK's federated mint mode. It talks to federation members directly through the
CDK wallet library; it does not invoke `cdk-cli` in the background.

The interface provides:

- federation import from any member URL, `federation-public.json`, or a
  `cashufed` invite;
- BOLT11 and on-chain mint and melt flows, gated by the methods advertised by
  the federation;
- exact-value Cashu token send and receive flows;
- persistent wallet state with startup saga recovery;
- a transaction history view fed by the wallet database;
- independent `/v1/info` and `/v1/keysets` probes for every member;
- live HTTP status, latency, success rate, response volume, active keysets,
  endpoint identity, and federation metadata verification;
- a scrollable request activity feed and per-member latency sparkline;
- in-TUI leave and rejoin without restarting the process;
- terminal paste and system clipboard support.

The health dashboard observes the public wallet-facing REST API. A public
wallet cannot read the federation's authenticated private AlephBFT status, so
`READY` means that the member is reachable and advertises the expected member
ID, federation ID, config digest, and keysets. It is not a claim that the member
has exposed its private consensus internals.

## Build

The TUI is a standalone Rust workspace. Building it does not add anything to
CDK's root workspace or modify CDK's root lockfile.

From the CDK repository root:

```bash
cd scripts/fed-cdk-tui
cargo build --release
```

Run it with the default wallet directory, `~/.fed-cdk-tui`:

```bash
target/release/fed-cdk-tui
```

On first launch, paste a federation member URL, the path to a public federation
config, or a `cashufed` invite into the join dialog. Member-URL discovery fetches
`GET /federation/v1/config` and verifies it against that member's `GET /v1/info`
response before creating the wallet. You can also import at startup:

```bash
target/release/fed-cdk-tui \
  --mint-url https://mint.example.com

target/release/fed-cdk-tui \
  --federation-config ./federation-public.json

target/release/fed-cdk-tui \
  --invite 'cashufed...'
```

The imported federation is persisted, so later launches need no join argument.
Use `--data-dir PATH` to maintain multiple independent wallets. A wallet
directory belongs to one federation: joining is refused when the stored wallet
already belongs to a different federation ID, so eCash from two federations can
never be mixed in one database. Use another `--data-dir`, or `reset` the wallet,
to join a different federation.
Member operations use the deployment-tested 90-second request, fallback, and
quote-status timeouts so a slow consensus round does not create rapid quote
fallback churn.

## Connect To A Federation

The shortest path is discovery through any public member URL:

```bash
target/release/fed-cdk-tui \
  --mint-url https://mint.example.com
```

The dashboard should settle at `LIVE n/n` once every member endpoint has
answered and its advertised federation metadata matches the roster.

The mint and melt dialogs offer only the methods advertised through the
federation's NUT-04/NUT-05 metadata. Press `m`, select an advertised method in
the dialog, and enter an amount in satoshis.
For on-chain minting, send funds to the displayed address; the wallet waits for
the federation's confirmation policy and then aggregates the Cashu signature
shares. Press `p` and select on-chain to melt eCash to an address; enter the
address and amount in their separate fields. The TUI selects the lowest-fee
quote offered by the federation. The last mint and melt methods selected during
the session are reused whenever the federation supports them.

Quote and wallet operations run independently. A pending on-chain confirmation
does not disable the controls, so additional mint, melt, send, or receive
operations can be started while earlier ones remain active.
From a displayed invoice or address, pressing `m`, `p`, `s`, or `r` opens the
next operation directly; the latest payload remains available through `c`.

## Controls

| Key | Action |
|---|---|
| `1`, `2`, `3`, `4` | Overview, members, activity, and history views |
| `j` / `k`, arrows | Select a member; scroll the feed on activity/history |
| `PgUp` / `PgDn` | Page through the activity or history feed |
| `Home` | Return a scrolled feed to the live head |
| `m` | Mint eCash; select an advertised payment method in the dialog |
| `p` | Melt eCash; select an advertised payment method in the dialog |
| `s` | Prepare an exact-value Cashu token |
| `r` | Receive and swap a pasted Cashu token |
| `c` | Copy the latest address or token |
| `f` | Open the join dialog when disconnected |
| `x` | Leave the federation; seed, proofs, and config are preserved |
| `?` | Show help |
| `Esc` | Close a dialog |
| `q`, `Ctrl-C` | Quit |

Pasting is handled through bracketed-paste events, so long invoices and Cashu
tokens are entered as one value rather than interpreted as shortcuts.

The history view lists the wallet's recorded mint, melt, send, and receive
transactions from newest to oldest, refreshed after every operation and every
few seconds. Pressing `x` disconnects the wallet and stops the member probes
without deleting anything; press `f` afterwards to rejoin the same federation
or join another one from a fresh data directory.

## Send And Receive Between Two Wallets

Use a separate data directory for the receiver:

```bash
target/release/fed-cdk-tui \
  --data-dir ~/.fed-cdk-tui-receiver \
  --federation-config target/federation-public.json
```

1. In the funded wallet, press `s`, enter the amount, then press `c` to copy the
   token.
2. In the receiver wallet, press `r`, paste the token, and press Enter.
3. Wait for the receive swap to complete and verify the new available balance.

Do not try to receive a token back into the wallet that created and still owns
it. Use two independent wallet data directories for a realistic transfer.

## Wallet Storage

The default directory contains:

| File | Purpose |
|---|---|
| `federation.json` | Validated public roster and selected local mint URL |
| `seed` | BIP39 wallet seed, created with private file permissions |
| `wallet.sqlite` | Proofs, quotes, counters, and saga state |
| `wallet.sqlite-wal`, `wallet.sqlite-shm` | SQLite runtime files when present |

On Unix, the wallet directory is restricted to mode `0700`; the seed and
federation configuration are written with mode `0600`.

Back up the seed before funding a wallet that matters. Deleting the seed and
database can permanently destroy access to its eCash.

## Delete A Wallet And Start Fresh

Close every TUI process using the wallet, then run:

```bash
target/release/fed-cdk-tui reset
```

Type `DELETE` at the confirmation prompt. This removes the entire default data
directory, including its seed, proofs, quotes, and imported federation config.
Start the TUI again with `--mint-url`, `--federation-config`, or `--invite` to
create a new seed and wallet.

For a named wallet, pass the same data directory before `reset`:

```bash
target/release/fed-cdk-tui \
  --data-dir ~/.fed-cdk-tui-receiver \
  reset
```

For disposable test automation only, confirmation can be skipped:

```bash
target/release/fed-cdk-tui reset --yes
```

## Development Checks

```bash
rustfmt --edition 2021 --check src/*.rs
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

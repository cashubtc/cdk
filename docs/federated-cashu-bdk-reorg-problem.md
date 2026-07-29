# Federated BDK: pre-liability reorg leaves spendable deposits stuck

**Status:** Implemented; coordinated deployment pending (observed 2026-07-17,
positive disconnection proof completed 2026-07-29)

**Related:** `docs/federated-bdk-operations.md`,  
`docs/federated-cashu-bdk-frost-implementation-plan.md` (Milestone 9),  
`docs/federation-cleanup-todo.md` item 48

> **Update 2026-07-18:** the follow-up "require a reorg observation quorum"
> is now implemented. Reorg transitions, the post-liability alarm, and the
> quote-credit reorg veto all require the UTXO's `required_observers` in
> current-epoch reorg evidence, and pre-liability quarantines self-repair
> through the matching honest quorum (milestone `P5` in
> `docs/federated-cashu-consensus-deadlocks-2026-07-18.md`, with
> red-before-green regressions in
> `crates/cdk-common/src/federation/chain_observation.rs`). A single false
> emission like the one journal'd below can no longer reorg, alarm, block
> credit, or permanently quarantine.
>
> **Update 2026-07-29:** the observer now emits `Reorg` only when the
> member-local persisted BDK checkpoint graph contains a different block at
> the stored anchor height. A missing output, an unconfirmed position, a
> checkpoint gap, or a tip below the anchor remains `Unknown` and produces no
> reorg evidence. Focused BDK restart and Axum observer-policy tests cover the
> distinction. The liar-first field-establishment case (`DB-003`) remains a
> separate open follow-up.

## Summary

A deposit can be **confirmed and unspent on Bitcoin** while the federated
wallet permanently classifies the same outpoint as **`reorged`**. Quote payment
never becomes spendable, so the wallet cannot mint Cashu liability for that
deposit. Restarting members does not help: reorg evidence is accepted
consensus state.

This is fail-closed by design for post-liability safety, but it is **too
sticky for pre-liability reorgs** when deposit evidence later reconfirms the
same outpoint. On the live nocean case, members **did** re-submit confirmed
deposits after Reorg; apply logic still refused to leave `Reorged`.

## Intended model

On-chain minting is split across:

1. **Member-local BDK watch-only sync** (Esplora / Bitcoin RPC) that sees
   revealed allocation scripts.
2. **Canonical chain observations** accepted into federation consensus
   (`FederationChainObservation::Deposit` / `::Reorg`).
3. **UTXO lifecycle** in
   `crates/cdk-common/src/federation/chain_observation.rs`:  
   `observed` → `confirmed` → `spendable` (or `reorged` / `quarantined`).
4. **Quote credit** only from UTXOs that are spendable under confirmation
   depth and observation quorum (`quote_payment_at_or_before`).

Docs state:

- confirmed deposits create liability only after depth + member quorum;
- a **pre-acceptance reorg blocks credit**;
- a **post-liability reorg** raises a sticky liability alarm.

The live bug is that pre-acceptance reorg is implemented as a **one-way**
state transition with no re-confirmation path, and Reorg becomes sticky after
**a single** accepting member.

## Failure mechanism (code)

### 1. Reorg emission is “local BDK ≠ stored block”, not proven disconnect

`crates/cdk-axum/src/federation/payment_observation.rs` (`observe_onchain_wallets`):

1. Submits `Deposit` for allocated outputs seen in `wallet.observed_outputs()`.
2. For each consensus UTXO this observer already deposited that is not already
   reorged/quarantined: if the outpoint is **not** confirmed in the same
   stored `(block_hash, block_height)`, submits `Reorg`.

That treats any of the following as reorg:

- outpoint missing from the local BDK list;
- present but `ChainPosition::Unconfirmed` (`confirmation = None`);
- confirmed with a different hash/height.

There is **no** check that the stored confirmation block is actually absent
from the local tip ancestry.

`FederatedBdkWallet::observed_outputs` maps BDK `list_output()`:

- `Confirmed { anchor }` → confirmation;
- `Unconfirmed { .. }` → `None` → can trigger Reorg once `utxo.block` is set.

### 2. One Reorg observation bricks the federation UTXO

`FederationChainObservationState::apply_reorg`:

```text
utxo.reorg_observations.insert(observer, evidence);
utxo.state = Reorged;   // immediate, global
utxo.liability_alarm |= utxo.liability_issued;
```

| Path | Quorum |
|---|---|
| Become **Spendable** | `required_observers` (on nocean: **consensus_threshold = 4**) |
| Become **Reorged** | **1** accepted Reorg |

### 3. Reconfirm deposits do not leave `Reorged`

`apply_deposit` still stores newer deposit evidence, but **does not recompute
state** when already `Reorged` / `Quarantined` / `Reserved` / `Spent`:

```rust
// crates/cdk-common/src/federation/chain_observation.rs (apply_deposit)
if !matches!(
    utxo.state,
    FederationWalletUtxoState::Reorged
        | FederationWalletUtxoState::Quarantined
        | FederationWalletUtxoState::Reserved
        | FederationWalletUtxoState::Spent
) {
    utxo.state = /* Observed | Confirmed | Spendable */;
}
```

`utxo_is_spendable_at_or_before` also fails closed if any reorg evidence remains
in `reorg_observations`.

Consequence:

| Chain truth | Federation truth |
|---|---|
| Tx confirmed, UTXO unspent | `reorged`, not spendable |
| All members later deposit confirmed evidence again | still `reorged` |
| Wallet mint quote | stays `UNPAID` / `amount_paid = 0` |

## Live example (nocean, 2026-07-17)

Environment: 5 members, signature threshold 3, consensus threshold 4,
`PAYMENT_MODE=federated-bdk`, Mutinynet Esplora
`https://mutinynet.com/api` (`chain_source_scope = shared`).  
Federation id:

`f412e564dc3ce97c71be69da5c62d0e31b76e45babf0856aa9713646413153a4`

Evidence read from  
`member-1/work/cdk-mintd.sqlite` →  
`kv_store (federation, chain_observation, state_v1)` and  
`federation_journal_entries`.

### Client

`fed-cdk-tui` at `~/.fed-cdk-tui`:

| Field | Value |
|---|---|
| mint | `https://cdk-1.nocean.xyz` |
| quote | `019f7193-bc27-7e01-a50d-219e5a9c8e5f` |
| method | onchain |
| address | `tb1peac8ufzjcm3ys89dta8wz85zk5dcfksw3vw0vvxp5gl88ewsg3sqshsccz` |
| wallet state | `UNPAID`, `amount_paid = 0`, `amount_issued = 0` |
| proofs / txs | none |

### Bitcoin (Mutinynet Esplora)

| Field | Value |
|---|---|
| txid | `66b5e65cd440026d52c525e9b63316261cbdd5aedf37e3817086a1ec8f1ddbce` |
| vout 0 | 11111 sat → quote address (unspent) |
| status | `confirmed: true`, `in_best_chain: true` |
| confirmation block | height **3269886**, hash `000000c5e30af12979c5dd4964802b54539ee7a48f0022659e635aca746e53ac` |

### Materialised wallet UTXO (consensus)

Outpoint `66b5e65c…dbce:0`:

| Field | Value |
|---|---|
| `state` | **`reorged`** |
| `block` | still `(000000c5e30af1…746e53ac, 3269886)` |
| `required_observers` | **4** |
| `minimum_confirmations` | **1** |
| deposit observers | 1–5 (all reconfirmed at depth 1 on that block) |
| reorg observers | **1, 3, 5** |

### Accepted journal timeline for this outpoint

```text
idx 2  deposit  observer 3  confirmation=None
idx 3  deposit  observer 1  confirmation=None
idx 4  deposit  observer 5  confirmation=None
idx 5  deposit  observer 4  confirmation@3269886 (sets utxo.block)
idx 6  deposit  observer 2  confirmation@3269886
idx 7  reorg    observer 3  disconnect 3269886 / 000000c5e30af1…746e53ac  ← first Reorg
idx 8  reorg    observer 1  disconnect same
idx 9  reorg    observer 5  disconnect same
idx 10 deposit  observer 1  confirmation@3269886 again
idx 11 deposit  observer 3  confirmation@3269886 again
idx 12 deposit  observer 5  confirmation@3269886 again
```

Notes:

- **Member 3’s Reorg at index 7 alone** was enough for global `state = Reorged`.
- Members **2 and 4 never** submitted Reorg for this outpoint.
- Members **1, 3, 5 reconfirmed** the same block after Reorg; apply kept
  `Reorged`.
- Disconnected block recorded by every Reorg is the **same** confirmation block
  that Mutinynet still reports as best-chain for the deposit.

### Tip lag is not a fork proof

A later diagnosis snapshot once showed members on tip heights `3269888` vs
`3269889`. Block `3269889` parents to `3269888`—linear poll lag, not competing
tips. That snapshot is **not** the cause and is **not** required to explain the
reorg; the journal alone is sufficient.

### Operator status

```text
utxos=... reorged:1/11111sat ... spendable:… (other deposits may succeed)
```

Later successful deposits (e.g. another outpoint becoming spendable) do not
heal this reorged outpoint.

## What does *not* fix an already-stuck outpoint

- Restarting tmux/binaries (`DEPLOY_MODE=restart`).
- Waiting for more Bitcoin confirmations (deposits reconfirm; state stays
  reorged).
- Client mint retries (quote never credits from consensus).
- Members agreeing on tip height/hash.

## Workarounds for operators (until a protocol fix)

1. **New mint quote + new deposit** to a fresh allocation. The reorged 11111 sat
   remains non-mintable in the current projection.
2. **Preserve** old `dkg-final` FROST material if a later recovery product must
   spend historical UTXOs; `fresh` DKG creates a new descriptor.
3. Diagnose with journal / chain_observation state plus explorer—not tip lag
   alone.

## Minimal fix

Keep the conservative reorg trigger for the first patch and make same-block,
pre-liability recovery require a fresh observation epoch.

### Policy

| Case | Behavior |
|---|---|
| Post-liability (`liability_issued`) | Keep sticky `Reorged` + liability alarm |
| Pre-liability (`!liability_issued`) | Reconfirm the same block in the next observation epoch; remain `Reorged` until the normal member quorum confirms in that epoch |

Epoch zero is omitted from serialization for compatibility. Reorg evidence is
retained, while only confirmations from the current epoch count toward
spendability. This prevents one reconfirmation from reviving stale votes and
allows the same observer to report a later real reorg in a newer epoch.

### Implementation sketch

1. Add `observation_epoch` to deposit/reorg evidence, UTXOs, and consensus
   conflict keys. A reorg invalidates the current epoch; recovery deposits use
   the next epoch.
2. Accept recovery only when the output identity and original block anchor
   match and the confirmation policy is satisfied. Advance the epoch on the
   first recovery observation, then wait for `required_observers` in that
   epoch before returning to `Spendable`.
3. While `Reorged`, observers submit same-block confirmations even when the
   capped confirmation depth is unchanged. Each member may report one reorg
   per epoch, including after another member has already failed the UTXO
   closed.
4. Deploy all members together. Existing epoch-zero state needs no database
   edit or new Bitcoin deposit: upgraded members submit epoch-one evidence and
   the UTXO recovers after the normal quorum is accepted.

### Follow-ups (not minimal; optional later)

- Require a reorg observation quorum.
- Support pre-liability re-inclusion of the same transaction in a different
  block through a separate anchor-generation design.
- Operator metrics distinguishing pre-liability reorg vs liability alarm.

### Explicit non-goals

- No operator “force mark spendable” that bypasses observation quorum.
- No changing Mutinynet confirmation defaults only to paper over this.
- No silent credit after post-liability reorg.
- No different-block recovery in the first patch.

## Acceptance criteria

- Class of incidents matching the journal timeline can mint after an upgraded
  member quorum reconfirms, without a new Bitcoin deposit.
- Post-liability reorg remains fail-closed with alarm.
- Focused unit tests above pass.
- Operator runbook updated in `docs/federated-bdk-operations.md` when landed.

## References

- `crates/cdk-common/src/federation/chain_observation.rs`  
  (`apply_deposit`, `apply_reorg`, `quote_payment_at_or_before`,
  `utxo_is_spendable_at_or_before`)
- `crates/cdk-axum/src/federation/payment_observation.rs`  
  (deposit and reorg submission loops)
- `crates/cdk-bdk/src/federation.rs` (`observed_outputs`)
- `crates/cdk/src/mint/federation/tests/chain_observation.rs`
- Live DB path: `~/cdk-federation/member-*/work/cdk-mintd.sqlite`
- Nocean deploy: `scripts/deploy_nocean_federation.sh`,
  `scripts/nocean_federation_control.sh` (`PAYMENT_MODE=federated-bdk`)

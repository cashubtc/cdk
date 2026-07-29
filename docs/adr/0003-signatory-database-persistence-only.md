# Signatory database as persistence only

* Status: accepted
* Authors: Cesar Rodas
* Date: 2026-07-27
* Targeted modules: cdk-signatory (DbSignatory)
* Associated tickets/PRs: groundwork for cashubtc/cdk#2253 (auto-rotate
  keysets on an age interval)

## Context and Problem Statement

`DbSignatory` owns its keys database as a single-process store and keeps the
full keyset set in memory (`keysets` and `active_keysets`, both behind an
`RwLock`). Signing, verification, and queries already read from memory, but
`rotate_keyset` did not: it re-read the active keyset from the database to
compute the next derivation index, and after committing the new keyset it called
`reload_keys_from_db`, which cleared and re-hydrated the entire in-memory set
from the database. What is the database's role relative to the in-memory state,
and where is it allowed to be touched?

This also unblocks cashubtc/cdk#2253 (auto-rotate keysets on an age interval).
Making memory the single source of truth and having rotation update it in place,
rather than reloading from the database, means a periodic auto-rotation only has
to call `rotate_keyset`: no extra reads, no reload, and the resilient background
boot keeps an age-based rotator running through transient database outages.

## Decision Drivers

* Memory is already the source of truth for every signing, verification, and
  query path; only rotation still round-tripped the database.
* The database is owned by this single process. The code assumes no other
  writer, so there is no external invalidation to reconcile against.
* Rotation should not re-read the whole keyset set to learn one new keyset it
  just wrote itself.
* The watch-channel publish contract from ADR-0002 (snapshot on subscribe, one
  per rotation) must be preserved.

## Considered Options

#### Database as the live store, read on every operation

Every signing/verification/query reads keysets from the database.

**Pros:**

* Good, because memory can never drift from the persisted state.

**Cons:**

* Bad, because it puts a database read on the hot signing path for state that
  never changes between rotations.
* Bad, because it discards the in-memory design the signatory already relies on.

#### Database as a write-through cache, reloaded after each write (prior state)

Rotation writes the database, then calls `reload_keys_from_db` to rebuild the
in-memory set from it.

**Pros:**

* Good, because one funnel (`reload_keys_from_db`) turns database state into
  memory, so the refresh is simple.

**Cons:**

* Bad, because rotation reads the active keyset from the database to compute the
  next index even though memory already holds it.
* Bad, because it re-reads and rebuilds every keyset to reflect a single
  addition.

#### Database as persistence only (chosen)

Read the database once on boot, write it only when rotating, and serve
everything else (including the post-rotation refresh) from memory.

**Pros:**

* Good, because the hot paths never touch the database and rotation reads
  nothing back.
* Good, because the database's role becomes a single clear rule.

**Cons:**

* Bad, because rotation must update the in-memory maps by hand instead of
  leaning on a reload, so the deactivation of the prior active keyset is now
  explicit code.

## Decision Outcome

Chosen option: "database as persistence only", because memory is already the
source of truth and the database only needs to survive restarts.

The invariant, enforceable by `grep -n "self.localstore"
crates/cdk-signatory/src/db_signatory.rs`:

* **Reads** hit the database only during `new`: `load_keys_from_db` (renamed
  from `reload_keys_from_db`, now boot-only) plus the `init_keysets`
  reactivation. Nothing else reads it.
* **Writes** hit the database only inside the `rotate_keyset` transaction
  (`add_keyset_info` + `set_active_keyset` + `commit`).
* Every other operation (`blind_sign`, `verify_proofs`, `keysets`,
  `subscribe_keysets`) and the post-rotation state refresh are served from and
  mutate memory only.

Rotation, after committing, updates memory to match what it just persisted
without reading the database back. It mirrors what a fresh boot would compute:
the active pointer for the unit moves to the new keyset, so the previously
active keyset for that unit is marked inactive in memory.

```rust
// after tx.commit()
let mut info = info;
info.active = true;
let signatory_keyset: SignatoryKeySet = (&(info.clone(), keyset.clone())).into();

let mut keysets = self.keysets.write().await;
let mut active_keysets = self.active_keysets.write().await;

if let Some(prev_id) = active_keysets.insert(args.unit, id) {
    if let Some((prev_info, _)) = keysets.get_mut(&prev_id) {
        prev_info.active = false;
    }
}
keysets.insert(id, (info, keyset));
self.publish_snapshot(&keysets);
```

The watch publish from ADR-0002 is preserved. The `watch::Sender::send_replace`
call moved into a `publish_snapshot` helper, now invoked from `load_keys_from_db`
(boot) and `rotate_keyset` (each rotation) instead of from a single
post-write reload. The observable contract, current snapshot on subscribe and
one per rotation, is unchanged.

### Positive Consequences

* Signing, verification, and query paths never touch the database, and rotation
  reads nothing back from it.
* The database's role is one rule that a single grep verifies.
* Memory remains the single source of truth; the watch contract is untouched.

### Negative Consequences

* Correctness depends on this process being the only writer. An out-of-band
  database mutation is not observed until the next boot.
* Rotation keeps the in-memory maps and the database in lock-step by hand
  (explicitly deactivating the prior active keyset) rather than relying on a
  reload.
* Boot is still allowed to write: `init_keysets` reactivates the highest-index
  matching keyset during `new`. This is a one-time boot concern, outside the
  steady-state "write only on rotation" rule.

## Boot load: try-once then background retry

Because the database is read only on boot, a database that is briefly
unavailable at startup would otherwise take the whole process down: the original
`new` awaited the load and propagated the error. To make startup resilient, the
boot load is now try-once with background retry.

`new` builds the in-memory state with a `loaded` flag set to `false` and
attempts `boot_load` (the `init_keysets` reactivation plus `load_keys_from_db`)
once:

* On success it sets `loaded` and returns. A healthy database behaves exactly as
  before: keysets are present the moment `new` returns and the mint boots ready.
* On failure it logs a warning, returns anyway, and spawns a task that retries
  `boot_load` with exponential backoff (1s, doubling, capped at 30s) until it
  succeeds, then sets `loaded`. The task holds a `Weak` reference so it stops
  when the signatory is dropped.

Until the first successful load, every key-using operation returns the new
transient error `Error::KeysetsNotLoaded`: `blind_sign`, `verify_proofs`,
`keysets`, and `rotate_keyset` are gated on `loaded`. `subscribe_keysets` and
`name` are not gated: `subscribe_keysets` is the watch channel through which
consumers receive the keysets once the background load publishes them, so it
must keep working while loading. `Error::KeysetsNotLoaded` is classified as a
non-definitive (retryable) failure.

To keep the shared state reachable from the background task without changing
`new`'s signature, the fields live in an inner struct behind an `Arc`
(`DbSignatory { inner: Arc<Inner> }`); the retry task holds `Weak<Inner>`.

**Embedded mint.** `Mint::new_internal` previously refused to start unless the
signatory already had a non-Auth active keyset (`Error::NoActiveKeyset`). That
check is relaxed to a warning: the mint starts with an empty keyset snapshot and
the existing keyset drain task (ADR-0002) installs the keysets once the
background load publishes them. The mint's `pubkey` bootstrap still works because
the watch snapshot always carries `pubkey` even when the keyset list is empty.

Consequence of the relaxation: a mint whose keyset set is genuinely empty now
starts (its endpoints return keyset / not-loaded errors) instead of failing at
construction. This is the intended resilience tradeoff, and it makes "empty
because still loading" and "empty because unconfigured" behave the same at
startup, distinguishable only by whether the background load eventually
publishes keysets.

## Links

* Refines [ADR-0001](0001-signatory-mint-key-segregation.md)
* Builds on [ADR-0002](0002-signatory-keyset-subscription.md): the watch publish
  it defines is preserved, moved into `publish_snapshot`.
* Groundwork for [cashubtc/cdk#2253](https://github.com/cashubtc/cdk/pull/2253):
  auto-rotate keysets on an age interval.

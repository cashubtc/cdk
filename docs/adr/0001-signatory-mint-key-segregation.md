# Signatory and mint key segregation

* Status: accepted
* Authors: Cesar Rodas
* Date: 2026-07-14
* Targeted modules: cdk-signatory, cdk (mint)
* Associated tickets/PRs: n/a (documents existing design)

## Context and Problem Statement

In Cashu the mint's private keys are the whole of its value: whoever holds
them can forge signatures and mint arbitrary ecash. The rest of the mint is
bookkeeping (quotes, proofs, spent-state, HTTP surface, wallet subscriptions),
it is large, changes often, and is exposed to the network. How do we keep the
private keys out of reach of a bug or compromise in that surface, while still
letting the mint sign and serve public keysets?

## Decision Drivers

* Private-key operations must be isolatable from the mint's network surface.
* The mint's key-read path is on the hot path (`/keys`, `/keysets`, signing)
  and must stay cheap.
* The same mint code should run against an in-process signatory or a remote
  one (separate process, separate host, HSM) without changes.

## Considered Options

#### Keep keys inside the mint process

Private keys live in the same process as the HTTP and database layers.

**Pros:**

* Simplest: no extra process, no wire protocol.

**Cons:**

* Bad, because every bug in the HTTP layer, database layer, or a dependency is
  a potential key-exfiltration bug.
* Bad, because it forecloses running keys on a different host or an HSM.

#### Segregate keys behind a `Signatory` trait

All private-key operations sit behind one trait; the mint holds only public
keysets.

**Pros:**

* Good, because private keys can move to a separate process or host without
  touching mint code.
* Good, because the trait is a clean seam for alternative backends.

**Cons:**

* Bad, because the mint can only pull keyset state; it cannot be pushed
  changes (addressed in ADR 0002).

## Decision Outcome

Chosen option: "Segregate keys behind a `Signatory` trait", because it is the
only option that isolates private-key material from the mint's network surface
while keeping mint code backend-agnostic.

The boundary is `Signatory` in `crates/cdk-signatory/src/signatory.rs`, the
only way the mint touches private-key material:

* `blind_sign(blinded_messages) -> Vec<BlindSignature>` produces signatures.
* `verify_proofs(proofs) -> ()` checks proof signatures.
* `keysets() -> SignatoryKeysets` returns the public half of every keyset.
* `rotate_keyset(args) -> SignatoryKeySet` creates a new keyset.
* `name()` identifies the signatory.

Every method that could touch a secret key runs on the signatory side. The
mint only receives public data: `SignatoryKeysets` carries a `pubkey` and a
`Vec<SignatoryKeySet>`, where each `SignatoryKeySet` holds `{ id, unit,
active, keys, amounts, input_fee_ppk, final_expiry, issuer_version, version }`
and `keys` are public keys. No secret key type crosses the trait.

Three implementations sit behind it:

* `DbSignatory` (`crates/cdk-signatory/src/db_signatory.rs`) is the in-process,
  database-backed source of truth. It holds
  `keysets: RwLock<HashMap<Id, (MintKeySetInfo, MintKeySet)>>` and
  `active_keysets: RwLock<HashMap<CurrencyUnit, Id>>`.
* `SignatoryRpcClient` (`crates/cdk-signatory/src/proto/client.rs`) is a gRPC
  client for running the signatory in another process, optionally over mutual
  TLS. Each call clones the tonic channel and issues a one-shot unary request.
* The embedded wrapper (`crates/cdk-signatory/src/embedded.rs`) adapts an
  in-process signatory to the same interface used by the remote path.

The wire contract (`crates/cdk-signatory/src/proto/signatory.proto`) has four
unary RPCs: `BlindSign`, `VerifyProofs`, `Keysets`, `RotateKeyset`. A
`VersionInterceptor` stamps a schema-version header on every request and the
server rejects a mismatch.

The `Mint` (`crates/cdk/src/mint/mod.rs`) holds
`signatory: Arc<dyn Signatory + Send + Sync>` and
`keysets: Arc<ArcSwap<Vec<SignatoryKeySet>>>`. It never stores a private key.
It stores an in-memory copy of the public keysets in an `ArcSwap` so hot-path
reads are lock-free. The keysets are populated by calling `signatory.keysets()`
once at construction; the only write path after that is `rotate_keyset` in
`crates/cdk/src/mint/keysets/mod.rs`, which rotates, re-fetches the full list,
and atomically swaps the `ArcSwap`.

### Positive Consequences

* Private keys can be isolated in a separate process or host; a compromise of
  the mint's network surface does not directly yield the keys.
* The mint's key-read path is lock-free and cheap.
* The trait is a clean seam for alternative backends (HSM, remote service).

### Negative Consequences

* The mint pulls keysets: it loads them once at startup and re-reads them only
  when it is the one that called `rotate_keyset`. The signatory cannot push a
  change.
* A keyset rotated on the signatory side, out of band from a mint instance, is
  never seen. `get_keyset_info` is a pure in-memory lookup with no lazy
  reload, so an unknown keyset returns `Error::UnknownKeySet`.
* `SignatoryRpcClient` has no reconnect logic; a dropped connection is noticed
  only on the next call and no mechanism refreshes state after reconnect.

## Links

* Refined by [ADR-0002](0002-signatory-keyset-subscription.md)

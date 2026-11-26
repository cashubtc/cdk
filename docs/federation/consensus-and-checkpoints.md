# Federation consensus and checkpoints

A federation is several CDK mint members operating one logical mint. Members
order every safety-critical state change through AlephBFT, apply the ordered
operations deterministically, and each produce a BLS share of every blind
signature. A federation-aware wallet validates the members' responses and
aggregates a threshold of shares into ordinary Cashu proofs.

This document covers the protocol model, ordering and application, signature
shares, the wallet side, and the checkpoint, startup and catch-up design.
Storage retention and state transfer are in
[storage and retention](storage-and-retention.md).

## Threat model

The federation must stop a wallet from obtaining more valid ecash than an
accepted operation permits. The motivating attack is threshold-signing
mix-and-match: a wallet presents different requests to different members and
combines overlapping subsets of shares. Members therefore sign only the exact
operation accepted by consensus.

Assumptions:

- Faulty members may send invalid shares, refuse requests, replay or
  equivocate, or propose conflicting operations.
- Networks may delay, duplicate, reorder, partition or drop traffic.
- Payment backends are nondeterministic and may return delayed, conflicting,
  ambiguous or stale results.
- Wallets are not trusted, one member's local state is not authoritative, and
  a local HTTP request never authorizes signing.

Safety holds while Byzantine faults stay within the consensus and signing
thresholds; availability needs the relevant thresholds of live members.

## Thresholds

For `n` members, `ThresholdParams::with_bft_consensus` uses
`f = floor((n - 1) / 3)` and `consensus_threshold = n - f`:

| Members | Faults tolerated | Consensus threshold |
|---:|---:|---:|
| 4 | 1 | 3 |
| 5 | 1 | 4 |
| 7 | 2 | 5 |
| 10 | 3 | 7 |

The BLS `signature_threshold` is configured separately and must not exceed the
consensus threshold. It may be lower (for example 3-of-5 signing with 4-of-5
consensus) only because shares are produced after the exact operation is
accepted. The FROST custody threshold equals the BLS signature threshold (see
[custody](custody.md)). Configuration rejects zero thresholds, thresholds above
the member count, and, unless development policies are explicitly allowed,
profiles below the production BFT bounds.

## Configuration and identity

The public `FederationConfig` holds the federation ID (derived from the
roster), wallet protocol version, thresholds, member IDs, public mint URLs,
peer-facing federation API URLs, identity public keys, payment-observation
policy, v3 BLS aggregate keysets and member public shares, FROST root and
application configs, checkpoint intervals, and the setup authorization hash.
It never holds secrets. Each member holds the complete public roster and only
its own identity key and key shares.

Federation keysets are v3 only (`KeySetVersion::Version02`, ID prefix `02`).
Config, startup, signing, restore and wallet validation reject non-v3 keysets
and mismatched aggregate keys, public shares or local secret shares.

Member identity keys authenticate mint-to-mint traffic. Private REST requests
bind the protocol version, federation ID, sender, receiver, message kind,
sender-local sequence, canonical body hash and member signature; unknown
members, wrong receivers, bad signatures and body-hash mismatches are rejected
before any mutation. Effectful DKG traffic uses durable sender sequences;
status, catch-up and AlephBFT traffic use bounded, kind-isolated replay
windows; operation submission relies on the operation ID and the journal for
idempotency. Iroh is an optional transport for the same requests: it decorates
the existing `Transport`, so peer clients dispatch by URL scheme and the routes
are the same.

## Operations

Every value-relevant state change is a `FederationOperation`
(`crates/cdk-common/src/federation/operation.rs`):

| Family | Operations |
|---|---|
| Cashu | `MintQuote`, `MintQuotePayment`, `Mint`, `MeltQuote`, `MeltQuotePayment`, `Melt`, `Swap`, `KeysetRotation` |
| Joint on-chain wallet | `AddressAllocation`, `ChainObservation`, `TransactionProposal`, `TransactionSigned`, `TransactionBroadcastIntent`, `TransactionBroadcastObservation` |
| Applications | `Application` (Bark operations, in the Bark application's namespace) |

An operation is carried in a versioned `FederationOperationEnvelope` (version
7) holding the federation ID, the complete typed payload, the authoring member,
the AlephBFT session it was stamped in, and the author's signature. The
domain-separated operation ID commits to the version, federation ID and
complete payload, so changing any quote, proof, output or observation changes
the ID. Authorization separately signs the author, the session stamp and the
unsigned envelope bytes; changing the author needs a new authorization but not
a new ID. Payment observations require the signed author to be the claimed
observer. Operation IDs use canonical JSON from `canonical.rs`, pinned by golden
vectors.

Each operation declares conflict keys (for example one per quote for
`MintQuote`, `Mint` and `Melt`, one per quote, state and observer for payment
observations, one per keyset for `KeysetRotation`). Swaps have no conflict key:
overlapping inputs are decided by transactional proof-state checks at apply
time. Proof-Y reservations in the journal are deliberately not used, because
reserving before verifying the referenced proof would let a malicious member
reserve an honest user's proofs, and the finalizer checks only envelope
authorization and payload validity, not proofs.

Backend calls, non-final payment states, local saga locks, private key shares,
per-member signature shares, DKG traffic, telemetry and ordinary reads are not
operations.

## Ordering and application

Public request handling prepares and validates an operation without
irreversible effects, submits its envelope through the member's submission
funnel (which stamps the session), and waits for the result.

AlephBFT orders units of `FederationConsensusItem`s: `Operation` envelopes and
`RecoveryBarrier` requests (a test-only `ModuleFact` item exists only in test
builds). Finalization assigns deterministic order, decides each item as
accepted, duplicate, rejected (invalid, outside its validity window, or
conflicting) and appends accepted operations to the journal. Every member
applies the same accepted sequence through one deterministic apply path
(`apply_accepted_federation_operation` in
`crates/cdk/src/mint/federation/application.rs`), which commits the domain
effect, the signature shares or the definitive rejection, and the entry status
in one database transaction.

Application is free of backend calls, local time, randomness and network I/O;
anything nondeterministic enters as an accepted observation operation first.
Evidence must be accepted at or before the operation it authorizes. A failed
apply that is not a definitive rejection is retried; a definitive rejection is
recorded and replay continues past it. Raw standalone signing and state paths
refuse to run on a configured federation member.

The federated swap, mint and melt application rules are guarded against the
standalone ones by a parity test (see
[design decisions](design-decisions.md#2-standalone-mint-code-is-not-split-into-validateplancommit)).

## Signature shares

A member returns or restores a share only for an output of an accepted
operation. `FederationSignatureShareRecord` binds the operation ID, output
index, member ID, keyset ID, amount, blinded message and share bytes; SQL stores
it in `federation_signature_shares` keyed by `(operation_id, output_index,
member_id)`. Mint, swap and melt-change application write share rows in the
apply transaction. Exact replay is idempotent; a different share for the same
key is a conflict.

Restore never signs new outputs: it looks up share rows by blinded message and
checks keyset and amount. A blind signature without a matching federation share
row is ignored.

To trace a share: find its row by blinded message or `(operation_id,
output_index)`, load the accepted envelope from `federation_journal_entries`,
confirm the output at that index matches, confirm the member and the signer ID
embedded in the share, and verify the share against the member's public share
for that keyset and amount.

## Wallet side

`FederatedMintConnector` (`crates/cdk/src/wallet/federation.rs`) implements the
ordinary `MintConnector`. It owns member fan-out, timeouts and retries, member
metadata validation, quote coordination, response grouping, error agreement,
share verification, BLS aggregation, restore and check-state. Mint and swap
succeed only with enough valid shares for the exact requested outputs. Quote
status, melt, restore and check-state need a matching threshold view. A
definitive Cashu error is returned only when a threshold of members agree on
it; mixed errors, conflicting successes, invalid shares and timeouts stay
ambiguous and fall back to the ordinary wallet saga and recovery paths. The
HTTP client's `preserve_error_responses` mode exists for this comparison.

`FederationRegistry` (`crates/cdk/src/wallet/federation/registry.rs`)
discovers a federation from a member URL, invite or public config. URL
discovery fetches and fully validates `GET /federation/v1/config` (public,
`Cache-Control: no-store`), requires the URL to identify exactly one roster
member, and checks that member's `GET /v1/info` advertisement (member ID,
federation ID, wallet protocol version, config digest) before storing. Entries
are keyed by federation ID; every member URL and the wallet's local mint URL
are aliases, and an alias belongs to at most one federation.
`RegisteredFederation::wallet_config` builds the wallet for the local mint URL
with the federated connector and the other aliases as mint URL aliases
(`WalletConfig::with_mint_url_aliases`). `Wallet::accepts_mint_url` admits a
token or NUT-18 request that names any member, a NUT-18 payment names the
alias the request lists, and `WalletRepository` lookups resolve an alias to
the wallet that owns it. Removing a federation (FFI `remove_federation`)
deletes the registry entry and its in-memory wallets but keeps its proofs.

## Checkpoints

A checkpoint authenticates an already ordered and terminally applied prefix.
It never orders operations, never authorizes an AlephBFT backup, and is not a
database snapshot (state transfer uses separately stored snapshot rows bound to
its digest).

| Object | Role |
|---|---|
| `FederationCommittedFrontier` | Canonical journal, finalized-item, terminal-application and AlephBFT recovery cursors, with exact-match and monotonic coverage rules |
| `FederationCheckpointFrontier` | Durable projection of those cursors plus trusted-checkpoint and audit metadata |
| `FederationCheckpoint` | Versioned, config-bound signed commitment to one complete-unit boundary: journal order digest, finalized-item digest, application digest, materialized-state digest, AlephBFT position, and lineage (scheduled sequence or the scheduled anchor of a recovery checkpoint) |
| `VerifiedFederationCheckpointQuorum` | Returned only after config, signer, threshold, signature and checkpoint checks; promotion and trusted recovery require it |
| `FederationSuffixVerifier` | The single incremental verifier for startup, genesis reconstruction and catch-up; it buffers at most one incomplete unit |
| `VerifiedFederationSuffix` | Opaque verifier result; `FederationConsensusRuntime::recover_verified_suffix` is the production entry point |
| `FederationDatabaseBackupManifest` | Operator-only binding of a quiesced database backup to its frontier, trusted checkpoint and secrets; not a consensus proof |

A checkpoint quorum is `consensus_threshold` distinct configured members.

**Scheduled checkpoints.** After a complete unit is terminally applied, the
checkpoint manager checks the consensus-configured intervals (journal
operations, finalized items, canonical bytes; defaults 10,000, 10,000 and
32 MiB), persists one deterministic candidate that extends the latest captured
candidate, collects member signatures over it, and promotes quorums in
sequence. Candidate boundaries do not depend on when signatures arrive.
Capturing a candidate never establishes trust; only a verified quorum does.

**Recovery checkpoints.** When sustained traffic keeps a lagging member from
reaching the scheduled tip, it asks for recovery. One member proposes a
`RecoveryBarrier` through ordinary AlephBFT ordering under a bounded lease, and
the barrier's complete-unit boundary deterministically defines the recovery
candidate. Recovery checkpoints use the same signature, quorum and promotion
machinery, and are pruned once a scheduled quorum supersedes them. A recovery
checkpoint anchored to an older scheduled checkpoint is ignored once a newer
scheduled checkpoint is trusted.

## Startup

1. `cdk-mintd` opens the database and loads the federation config once
   (`FederationStartup::load`): the public and private config files, or the
   certified setup handoff while `federation.dkg` is enabled. With
   `restore_manifest_path` set, it validates the restored config, quorum,
   frontier, backup metadata, secrets and schema before anything else.
2. `FederationNode::prepare` (`crates/cdk-federation/src/construction.rs`)
   builds the node runtime, reverifies the stored trusted quorum against the
   configured federation, and loads the active AlephBFT session and frontier.
3. At an exact checkpoint frontier, startup reads no journal or finalized-item
   prefix. With a local tail after the checkpoint it loads only those rows; the
   verifier proves them, and a partially applied tail is completed by the
   idempotent apply path. Without a trusted checkpoint the same verifier
   reconstructs from genesis.
4. Startup restores pending operations not yet accepted, reconciles custody
   projections, loads only the active-session AlephBFT backup, revalidates
   checkpoint candidates and computes a replay target from the trusted
   checkpoint and compatible peer status. Authoritative routes stay behind
   readiness until every target is met.

Normal startup performs no full materialized-state scan; the full audit is an
explicit operator command.

`FederationServices` starts the workers when listeners are ready. A failed
essential worker cancels its siblings and stops serving. Shutdown stops
scheduling, waits up to 30 seconds for tracked work, then aborts the rest and
joins the AlephBFT session. Workers never restart an external action on
failure; recovery inspects its durable identity.

## Catch-up

1. Peer sync compares signed status and selects the next stored quorum
   checkpoint on the local lineage.
2. Network code requests one bounded history page from one authenticated peer.
   Body limits apply before decoding; item and byte limits apply after.
3. The page must name the target checkpoint, start at the exact durable local
   cursors, and contain only complete finalized units, except an explicitly
   incomplete trailing page.
4. Only after network work finishes, catch-up takes the application lock and
   then the consensus lock; the verifier extends the verified local suffix.
5. Finalized items and journal entries are persisted, business effects applied
   idempotently, and the in-memory runtime replaced with the verifier result.
6. A complete page promotes the target only when the durable journal,
   finalized-item and application frontiers exactly equal it. Promotion is
   transactional, and the runtime is rebased at the promoted checkpoint.
7. A missing active AlephBFT backup is fetched on a separate bounded path that
   cannot change application or checkpoint trust.

A request below the peer's retention floor is answered with
`BelowRetentionFloor` (HTTP 410); see
[state transfer](storage-and-retention.md#state-transfer).

## Readiness and failure

- A crash after persisting finalized items and journal entries but before
  application leaves the trusted checkpoint unchanged; startup completes the
  missing suffix.
- A failed page or promotion leaves trust behind application state; retry
  resumes from durable cursors.
- Wrong signatures, signers, config bindings, lineage, cursors, digests, units,
  envelopes or outcomes fail before promotion.
- Invalid AlephBFT backup data can block AlephBFT recovery but never authorizes
  mint state.
- A materialized-state audit mismatch durably latches the member. No restart,
  promotion, peer answer, background task or operator command clears it;
  recovery replaces the database (see
  [operations](operations.md#audit-mismatch)). Digest health is a typed flag;
  presentation text cannot change it.
- Already accepted Bark payments use a caught-up recovery gate, so local
  recovery degradation cannot stop a member from observing and settling an
  external payment that consensus already authorized. Creating a new payment
  plan still requires full readiness.

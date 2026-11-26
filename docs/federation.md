# Federation

A CDK federation is a set of independently operated mint members that act as
one Cashu mint. Members order every value-relevant state change through
AlephBFT, apply it deterministically, and each produce a BLS share of every
blind signature; wallets aggregate a threshold of shares into ordinary v3
Cashu proofs. Optional custody backends hold Bitcoin jointly under a FROST
threshold key: a watch-only BDK on-chain wallet and Ark-backed Lightning
through Bark.

Federation is undeployed and follows a cut-through policy: federation formats,
schemas and APIs change in place, and a federation starts from fresh state for
each revision. Standalone CDK behaviour and migration history are unchanged.

## Documents

| Document | Covers |
|---|---|
| [Design decisions](federation/design-decisions.md) | The decisions that shape the implementation, with their rationale |
| [Consensus and checkpoints](federation/consensus-and-checkpoints.md) | Threat model, thresholds, operations and envelopes, ordering and application, signature shares, wallet aggregation, checkpoints, startup and catch-up |
| [Storage and retention](federation/storage-and-retention.md) | Storage contract, validity window, retention floor, terminal entities, digest rule, state transfer, stale-member reset, bounded and unbounded readers |
| [Payment trust model](federation/payment-trust-model.md) | Payment observation policy, mint and melt flows, Bark and on-chain evidence, open limitations |
| [Custody](federation/custody.md) | FROST root and derivation, nonces and signing, joint on-chain wallet, Bark custody |
| [Setup ceremony](federation/setup-ceremony.md) | Multi-operator setup, BLS and FROST DKG, wire format and limits |
| [Operations](federation/operations.md) | Build, configuration, setup commands, monitoring, backup, FROST nonce rule, restore, reset and audit recovery |
| [Maintainer guide](federation/maintainer-guide.md) | Concept map, operation walkthroughs, rules and review checklist, tests and supported feature combinations |

Maintainers new to the federation should start with the
[maintainer guide](federation/maintainer-guide.md); operators with
[operations](federation/operations.md).

## Crates and ownership

| Responsibility | Owner | Boundary |
| --- | --- | --- |
| Cashu proof and signature encodings, BLS v3 keysets | `cashu` | Cryptographic vectors and canonical encodings |
| Configuration, BLS DKG planner, authorization, ordering, journal, checkpoints, setup transcript | `cdk-common::federation` | Pure domain, no I/O; explicit versions; deterministic replay |
| Federation storage traits | `cdk-common::database::mint::federation`; SQL in `cdk-sql-common` | One transaction per applied operation |
| Admission and ordered mint, swap and melt application | `cdk::mint::federation` | Transactional state checks before value effects |
| FROST DKG, nonce storage and signing | `cdk-frost`, `cdk-frost-bip32`, `cdk-frost-bark` | Accepted operation authority, signer binding, single nonce use |
| On-chain wallet engine and sync | `cdk-bitcoin-wallet`, `cdk-bdk` | Watch-only; shared RPC timeout and sync policy |
| Bark operations, projection rules and store trait | `cdk-federation-bark` | Quorum-certified evidence |
| Bark drivers, custody and recovery | `cdk-bark` | Exact accepted plans and durable external identities |
| The federation node: consensus session, sync, checkpoints, retention, state transfer, peer client, DKG execution, payment and custody workers, setup ceremony | `cdk-federation` | Authenticated sequencing, deterministic replay, readiness gating |
| HTTP routes, request extraction and authentication | `cdk-axum::federation` | Bounded requests over the node |
| Iroh transport | `cdk-iroh` | Decorates the existing `Transport` |
| Operator commands and startup | `cdk-mintd` | Parses arguments, loads config once, prints results |
| Wallet aggregation and discovery | `cdk::wallet` (`FederatedMintConnector`, `FederationRegistry`) | Validated member metadata, threshold aggregation |
| CLI, mobile and dashboard wallets | `cdk-cli`, `cdk-ffi`, `cdk-fed-tui` | Use the shared wallet registry |

Worker lifetime is owned by `FederationServices` and
`RunningFederationServices`: explicit start, observed failure, cancellation and
joined shutdown.

## Feature flags

`federation` supplies the core protocol; `federation-bdk` adds joint on-chain
custody; `federation-bark` adds Ark-backed Lightning; `iroh` enables the Iroh
transport. `federation-test-utils` (in `cdk-mintd`) and the crates' `test` and
`test-utils` features expose test fixtures only and are not production builds.

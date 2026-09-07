# Federation architecture and operator entry point

This is the current guide for the undeployed BLS federation branch. Federation formats and databases follow a cut-through policy: initialize fresh federation state for this revision. Historical federation migrations and wire formats are unsupported. Existing standalone mint migration history remains unchanged.

## Ownership and safety boundaries

| Responsibility | Owner | Required boundary |
| --- | --- | --- |
| Cashu proof and signature encodings | `cashu` | Cryptographic vectors and canonical encodings |
| Configuration, BLS DKG, authorization, ordering, checkpoints | `cdk-common::federation` | Signed roster/config binding, explicit versions, deterministic replay |
| Admission and ordered mint/swap/melt application | `cdk::mint::federation` | Transactional state checks before value effects |
| FROST nonce storage, derivation, signing | `cdk-frost`, `cdk-frost-bip32`, `cdk-frost-bark` | Accepted operation authority, signer binding, single nonce use |
| On-chain wallet synchronization | `cdk-bitcoin-wallet` | Shared validated RPC timeout and sync policy |
| Bark accepted actions and projections | `cdk-federation-bark` and SQL projections | Quorum-certified evidence and current operation schema |
| Bark execution and recovery | `cdk-bark::runtime` | Exact accepted plans and durable external identities |
| HTTP/Iroh extraction, authentication, orchestration | `cdk-axum::federation` | Authenticated sequencing, bounded requests, readiness gating |
| Worker lifetime | `FederationServices` / `RunningFederationServices` | Explicit start, observed failure, cancellation and joined shutdown |
| Wallet aggregation and discovery | `FederatedMintConnector` | Validated member metadata and threshold signature aggregation |
| Mobile interfaces | `cdk-ffi::federation`, `WalletRepository` | The maintained Rust API; no generated source patch overlay |

The mint's admission, application, and helpers are ordinary modules. Bark execution is divided into incoming payments, outgoing payments, maintenance, and on-chain recovery. Axum scenario tests use shared parent fixtures and ordinary child modules.

Transaction construction and independent signer validation remain separate trust boundaries. Fee/weight limits, derivation domains, and quorum thresholds are agreed policy, not arbitrary local tuning. Operational retry intervals and queue limits do not authorize a payment or change accepted state.

Swaps still use transactional proof-state checks for overlapping inputs. Adding proof-Y journal reservations before verifying the referenced proof could let a malicious member reserve an honest user's proofs. The finalizer currently checks envelope authorization and pure payload validity; it does not perform that proof verification. See the implementation ledger for this retained decision.

## Maintainer reading path

Start with the [ordinary-CDK concept map and operation walkthroughs](federation-maintainer-walkthroughs.md): [mint](federation-maintainer-walkthroughs.md#incoming-payment-and-mint-issuance), [swap and ecash](federation-maintainer-walkthroughs.md#swap-and-ecash-transfer), [outgoing payment](federation-maintainer-walkthroughs.md#outgoing-payment-and-change), and [restart/catch-up](federation-maintainer-walkthroughs.md#startup-failure-restart-and-catch-up). Each identifies authoritative state, transaction boundaries, external effects, and recovery ownership.

## Current records

All operation kinds use envelope version 6. Bark payloads use their current version 2. Incoming Bark claims require certified receive evidence in the claim plan; replay applies that evidence from the accepted plan. The earlier standalone HTLC-ready transition cannot authorize a claim.

Private status has a required `application_operators` map and an explicit `onchain_wallets` array, including when empty. Digest health is the typed checkpoint audit flag; presentation text cannot change health decisions. Pending AlephBFT storage requires the current wrapper. Bark allocations store one VTXO collection and its materialized amount.

Federation creation SQL contains the final chunked backup schema and indexes. The migration checksum regression freezes upstream standalone SQL. Payment scheduler records are a recoverable local cache: unsupported/corrupt cache data is rebuilt, never migrated into the current format.

Scheduled checkpoint candidates extend the latest captured candidate at deterministic ordered boundaries, independently of when peer signatures arrive. Pending ancestors remain available for signature exchange; quorums are collected and promoted in sequence. Candidate capture alone never establishes trust. Recovery checkpoints remain separately replaceable and are pruned once a scheduled quorum supersedes them.

Already accepted Bark payments use the caught-up recovery gate so local recovery degradation cannot prevent observing and settling an external payment. Selected outgoing observers check the exact payment status before initiation, including the original actor; only a typed missing result permits initiation. Creating a new payment plan still requires full readiness. Wallet-unavailable, invalid-projection, and unavailable-participant states cannot enter this recovery path.

## Operator workflow

Build the needed features explicitly. `federation` supplies the core protocol; `federation-bdk` adds joint on-chain custody; `federation-bark` adds Ark-backed Lightning. `iroh` enables Iroh transport. Production builds do not need `federation-test-utils`; that feature exposes the trusted-dealer fixture command and fake backend support.

Use `cdk-mintd --work-dir <member-dir> config init --new-mint --file <runtime.toml>` for a new member and `config apply --file <runtime.toml>` for a configuration update. Start and restart with `cdk-mintd --work-dir <member-dir>`. Keep the work directory explicit for every operator command. Runtime configuration is database-backed; top-level startup `--config` and runtime environment overrides are obsolete. Ceremony subcommands retain their own valid `--config` argument.

Production setup proceeds through `federation setup create`, protected invitations, `join`, `propose`, independent `review`/`approve`, `serve`, and `activate`. Each member approves the exact manifest hash. Dealer-free BLS/FROST completion and final-config confirmations precede activation. Keep private configuration and secret references protected; copying a public config does not authorize membership.

Use `federation status`, `federation digest-compare`, `federation wallet-status`, and Bark operator commands with explicit public/private config files or the member work directory. Recovery/rebroadcast actions require the command's explicit acknowledgement. A digest halt requires investigation; changing display text or restarting a worker does not clear divergence.

The daemon starts prepared federation services when listeners are ready. A failed essential worker stops serving. Shutdown first stops scheduling and waits up to 30 seconds for tracked work, then aborts remaining tasks and joins the AlephBFT session. Recovery inspects durable payment/signing/broadcast identities; workers do not automatically restart external actions on failure.

## Development and verification

Generated fake-backend fixtures use `scripts/start_federation_test_member.py` to import their settings through the real configuration commands before starting a member. Its environment variables are explicit test-fixture inputs. Named signet deployment profiles remain separate from local regtest verification.

The Nix check groups cover federation protocol/storage, custody, runtime feature combinations, binary/test feature separation, and FFI. Normal CI also exercises the federation/FROST decoder seeds; the longer security fuzz workflow remains manually dispatched. Large debug federation tests use a 16 MiB thread stack.

Bark is pinned to the reviewed federation revision `8d1eda643dd11e9eb600ec44e36622c8f77b6322` in [callebtc/bark](https://github.com/callebtc/bark/tree/codex/frost-wallet-cdk-20260907). Cargo fetches that immutable Git revision; a sibling Bark checkout is no longer required. See the [cleanup plan and verification ledger](bls-federation-cleanup-plan-2026-09-05.md) for build verification and local payment evidence.

## Decision and history index

- [FROST protocol and security](federated-frost-protocol-and-security.md), [BIP32 derivation](federated-cashu-frost-bip32-derivation.md), and [FROST DKG](federated-cashu-frost-dkg.md) retain cryptographic rationale.
- [On-chain architecture](federated-cashu-bdk-frost-architecture-decision.md) and [checkpoint architecture](federated-cashu-checkpoint-architecture.md) explain custody and recovery boundaries.
- [Bark implementation record](federated-cashu-bark-frost-implementation-plan.md) and the July Bark incident/replay reports retain failure evidence; use current source and this guide for interfaces.
- [July cleanup ledger](cleanup-2026-07-29.md), [older TODO ledger](federation-cleanup-todo.md), security audits, baseline reports, and presentation directories/archives are historical artifacts. They are retained without bulk deletion. They do not prescribe compatibility with undeployed federation data or supersede the current implementation ledger.

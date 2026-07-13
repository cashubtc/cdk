# Federated Cashu RFC

This RFC records the current protocol and safety model for federated Cashu support in CDK. The implementation is authoritative when this document and code disagree. Update this RFC whenever federation wire formats, trust assumptions, consensus operations, DKG, payment policy, recovery, or wallet aggregation change.

## Objective and compatibility

Federated Cashu lets multiple CDK mint members operate one logical mint. Members order safety-critical state transitions through Byzantine fault tolerant consensus and jointly produce BLS blind signature shares. A federation-aware wallet validates member metadata and responses, rejects invalid shares or inconsistent views, and aggregates a valid threshold into ordinary Cashu proofs.

The public wallet-to-mint API remains close to the existing NUT endpoints. Federation behavior is enabled only by explicit feature gates and configuration; standalone mint and wallet behavior remains unchanged when federation is disabled.

No federation mint or federation database format has been deployed. This is the first implementation, so federation development uses cut-through compatibility: experimental federation protocols, APIs, wire formats, invitations, configurations, databases, migrations, and fixtures may be replaced together without legacy adapters or upgrade paths. This does not relax standalone CDK or public Cashu compatibility.

Federation keysets are v3/BLS-only: `KeySetVersion::Version02`, serialized with keyset ID prefix `02`. Federation configuration, startup, signing, restore, and wallet validation must reject non-v3 keysets and mismatched aggregate keys, public shares, or local secret shares.

## Threat model

The federation must prevent wallets from obtaining more valid eCash than an accepted operation permits. The motivating attack is threshold-signing mix-and-match: a wallet presents different outputs or requests to different members and combines overlapping share subsets. Members must therefore sign only the exact canonical operation accepted by consensus.

The federation assumes:

- Honest members verify canonical operation IDs, consensus outcomes, keysets, and request validity before exposing signature shares.
- Faulty or unavailable members may send invalid shares or responses, refuse requests, replay messages, equivocate, or propose conflicting operations.
- Networks may delay, duplicate, reorder, partition, or temporarily drop traffic.
- External payment backends are nondeterministic and may return delayed, conflicting, ambiguous, or stale observations.
- Safety holds when Byzantine faults remain within the configured consensus and signing assumptions; availability requires the relevant thresholds.

The federation does not assume wallets are honest, one member's local state is authoritative, a local HTTP request authorizes signing, or development fakewallet behavior proves production payment safety.

## Fault and threshold model

The live runtime uses AlephBFT. For `n` members its consensus threshold is:

```text
consensus_threshold = n - floor((n - 1) / 3)
```

The BLS `signature_threshold` is configured separately and may be lower than or equal to the consensus threshold. Configuration must reject zero thresholds, thresholds above the member count, or a signature threshold above the consensus threshold. Consensus ordering and BLS aggregation remain separate security parameters even when configured to the same value.

Test-only deterministic helpers must never be presented or admitted as production consensus.

## Core invariants

- Consensus acceptance precedes signature-share creation and externally visible safety-critical state changes. No member may independently authorize issuance, swaps, or melts.
- Every operation has a versioned canonical envelope containing the federation ID and complete typed payload. Its domain-separated `FederationOperationId` commits to that entire envelope.
- Accepted operations apply in the same order, deterministically and idempotently. Journal replay must reproduce the same materialized state.
- A quote can issue only its consensus-authorized mintable amount. Proofs transition to spent at most once, and blinded outputs are signed only for their accepted operation, amount, and keyset.
- Signature shares are bound to member, operation ID, output index, blinded secret, amount, and keyset, and are durably recorded before being served.
- Member-local payment or database state is not quorum evidence. Payment results mutate federated state only through accepted observation operations and the configured policy.
- Insufficient agreement is ambiguous. Minority disagreement does not veto a valid payment threshold, but two incompatible threshold-supported payment results leave the quote unresolved and must not produce spendable proofs or definitive success.
- Startup, DKG, catch-up, rotation, configuration, roster, transcript, keyset, and materialized-state mismatches fail closed.
- Local time, randomness, unordered data, network I/O, and backend calls must not affect deterministic application unless the relevant value was committed through consensus.

## Public federation configuration and discovery

An active member exposes `GET /federation/v1/config` as a public, unauthenticated, read-only discovery endpoint. Standalone mints return no federation config. Members must not serve an incomplete setup without a finalized transcript and active aggregate keyset. Responses use `Cache-Control: no-store` so rotation or restarted configuration is not hidden by intermediary caches.

`FederationConfig` contains the federation ID, wallet protocol version, thresholds, member IDs, public mint URLs, peer-facing federation API URLs, identity public keys, payment-observation policy, v3/BLS aggregate keysets and member public shares, and the signed setup transcript. Peer-facing URLs are public metadata; access to their routes is authenticated. The config must never contain identity secret keys, BLS secret shares, credentials, or private runtime state.

URL-based discovery must:

1. fetch and fully validate the public config;
2. require the supplied URL to identify exactly one roster member;
3. fetch that member's `GET /v1/info` and verify its member ID, federation ID, wallet protocol version, and complete config digest; and
4. persist or construct the connector only after those checks succeed.

Invite or file import validates the complete embedded config. Reconstruction from a previously validated local registry may use the validated-config path, while runtime member use still validates advertised federation metadata and active keysets.

## Member identity and private transport

Member identity keys authenticate mint-to-mint traffic; BLS shares sign Cashu blinded messages. Every member holds the complete public roster and only its own identity secret key and BLS secret shares.

Private REST requests bind the REST protocol version, federation ID, sender, receiver, message kind, sender-local sequence, canonical body hash, payload, and member signature. Unknown members, wrong federation or receiver IDs, wrong key kinds, invalid signatures, body-hash mismatches, and unsupported versions are rejected before mutation.

Replay handling is message-class specific:

- Effectful DKG commitment, reveal, secret-share, transcript-signature, and activation traffic uses durable sender sequence state.
- Status, readiness, catch-up, and AlephBFT traffic uses bounded, kind-isolated volatile replay windows.
- `SubmitOperation` authenticates its sequence for provenance but uses the canonical operation ID and durable consensus record for restart-safe idempotency.

Private signed REST is transport, not an alternate consensus or authorization path.

## Dealer-free DKG

Production federation key material is established through restartable dealer-free Pedersen DKG. The ceremony binds its identifier, federation ID, threshold parameters, deterministic participant order, member identity keys, keyset policy, public commitments, aggregate keys, and member public shares.

Members exchange readiness, commitment, reveal, private secret-share contribution, transcript-signature, and activation messages over authenticated REST. Effectful state and local participant material are persisted so exact retries and restart resume are safe. A member activates only the exact finalized configuration derived from the signed transcript and verifies that its local secret shares match its advertised public shares. Members must converge on the same final public configuration before starting the consensus runtime.

## Consensus operations and application

The canonical operation set includes:

- mint quote creation and mint payment observation;
- mint issuance;
- melt quote creation and melt payment observation;
- melt execution and optional change signing;
- swap execution; and
- federated keyset rotation.

Public request handling prepares and validates an operation without performing irreversible state changes, submits its canonical envelope, and waits for finality or a recoverable pending result. AlephBFT orders admitted operations. A single deterministic apply path durably records the accepted order, applies domain state, records shares or deterministic rejection metadata, and notifies waiting requests. Exact duplicate operation IDs join or return the existing result; conflicting operation reuse is rejected.

Raw standalone signing and state-transition paths reject use by a configured federation member. Wallet-facing shares are reconstructed from persisted operation-bound share records, not generated from an unaccepted request.

## Payment handling

Payment observation policy is explicit and independently configured for mint and melt quotes:

- `member_quorum` requires the configured number of matching member observations and must be at least the consensus threshold.
- `development_single_observation` is restricted to local development or regtest and requires an explicit mintd startup opt-in.

Backend status is probed outside deterministic apply. Observations include their member, quote, method, state, amounts, and available payment identity or proof, then enter consensus as canonical operations. The payment policy applies one minimal conflict rule: minority disagreement is reported but cannot block a matching threshold, while two incompatible observation groups that both reach threshold leave the quote unresolved. There is no quarantine operation or operator-controlled quote freeze. Evidence must be accepted at or before the value operation it authorizes; later observations cannot retroactively authorize an earlier operation.

For melts, the accepted operation deterministically elects one member to attempt the outgoing payment. A durable saga compare-and-set claims `SetupComplete -> PaymentAttempted` before the backend call, a quote-scoped guard prevents concurrent local attempts, and restart recovery checks ambiguous payment state rather than paying again. Other members observe the result, and accepted policy-satisfying observations determine final quote, proof, change, and saga state.

Fakewallet is a development backend for exercising these same consensus and recovery paths. Its success is not a production payment guarantee.

## Catch-up, readiness, and lifecycle

The accepted journal and finalized AlephBFT items are durable and replayable. Order digests commit to the ordered operation chain; materialized-state digests cover deterministic database state.

Catch-up responses carry finalized AlephBFT items as the authoritative replay history plus compact operation/application metadata. Members sign contiguous-range catch-up certificates binding the federation, range, ordered operation IDs, order digest, and materialized-state digest. A lagging member verifies a consensus-threshold certificate quorum, replays the finalized items, and confirms both order and materialized state before becoming ready.

Startup restores DKG state, active configuration, session markers, pending operations, finalized history, application state, and catch-up evidence before public federation operations are admitted. Missing evidence, divergence, unsupported versions, task failure, or stale readiness leaves the member fail closed. Session rotation and restart must preserve or atomically rebind pending operations; terminal runtime failures tear down inbound admission and supervised tasks.

## Keyset rotation

Rotation is a consensus operation containing a complete active federated keyset and every member's public shares. Admission validates v3 metadata, aggregate interpolation, roster coverage, signer IDs, and the local member's proof of possession for its secret shares. Applying a rotation deactivates the previous active keyset for the same unit, retains old keysets for proof verification, and updates the effective configuration and wallet-facing keyset responses. Members and wallets must reject incomplete or inconsistent rotated views.

## Federation-aware wallet behavior

Federation behavior is centralized in the federation connector rather than duplicated across wallet workflows. The connector owns bounded member fan-out, retry and timeout policy, member metadata validation, quote coordination and threshold visibility, response grouping, error agreement, share verification, BLS aggregation, restore, and check-state aggregation.

Mint and swap succeed only with enough valid shares for the exact requested outputs. Melt, quote-status, restore, and check-state reads require the operation-specific matching threshold view. A matching definitive Cashu rejection may be returned only when it reaches threshold; mixed errors, conflicting successful responses, invalid shares, timeouts, or insufficient agreement remain ambiguous and use existing wallet saga and recovery paths. Wallet applications should otherwise retain ordinary CDK issue, swap, melt, send/receive, restore, and balance workflows.

## Architecture boundaries

- AlephBFT orders operations; the mint domain owns Cashu validation and deterministic application.
- The federation journal is authoritative history; one member's materialized database is not authorization.
- DKG output is authoritative for aggregate keys and member shares.
- Public HTTP handlers remain thin adapters, and the wallet connector owns multi-member aggregation.
- Consensus, DKG, transport, journal/catch-up, readiness, rotation, and signing coordination remain in dedicated federation modules behind feature gates.
- Federation and standalone flows reuse existing CDK domain validation, error types, sagas, and persistence where semantics match; federation orchestration must not duplicate the mint implementation or scatter conditionals through unrelated standalone code.

## Review checklist

Every federation change should answer:

- Does it preserve standalone behavior and non-federation builds?
- Can any path create or expose a signature share without accepted operation provenance?
- Does every operation retain canonical encoding, stable ID, complete value commitment, bounded size, and conflict handling?
- Is deterministic apply free of backend calls, local time, randomness, and uncommitted nondeterminism?
- Are payment observations temporally authorized, quorum-checked, and replay safe?
- Can retries, cancellation, restart, catch-up, or rotation duplicate value, lose pending work, or serve stale state?
- Do wallet responses require valid threshold agreement, including matching definitive errors?
- Do stateful changes cover replay/restart and distributed changes cover adverse ordering or partial failure?
- Are queues, caches, retries, waiters, payloads, and diagnostics bounded?
- Are identity secrets, BLS secret shares, DKG private messages, credentials, payment secrets, sensitive payloads, local paths, and generated private configs excluded from logs, fixtures, and commits?

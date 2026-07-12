# Federated Cashu Consensus in CDK (Historical Snapshot)

This archived paper records the pre-AlephBFT federation scaffold. It is retained
as development history and does **not** describe the current implementation.
For current architecture and verified status, use
`docs/federated-cashu-implementation-plan.md` and
`docs/federated-cashu-progress.html`.

- **Status:** Archived pre-AlephBFT snapshot
- **Historical checkpoint:** Signed REST, replay, core flows
- **Historical decision source:** Local deterministic journal (removed)
- **Current implementation:** AlephBFT-only federation runtime

## Abstract

Federated Cashu support in CDK introduces a consensus boundary around mint state transitions before threshold BLS blind signature shares can be returned. The current code establishes that boundary with versioned federation operation envelopes, canonical operation IDs, quote-scoped conflict keys, an accepted-operation journal, signed private REST payloads, durable journal replay, lag-aware public-route guards, and a local deterministic consensus driver. Accepted entries now apply into member mint state for mint quotes, mint payment observations, mint issuance, swaps, melt quotes, melts, melt payment observations, restore lookup support, and check-state aggregation.

The present implementation is intentionally not a distributed BFT consensus algorithm. It is a testable in-memory scaffold whose API and data objects are designed to be reused by a future AlephBFT engine, quorum-signed catch-up certificates, state digests, real payment-observation policy, and dealer-free DKG.

## 1. What Is Implemented Today

The current implementation is best understood as the first working MVP skeleton of a federated Cashu mint. It does not yet try to be the final multi-node Byzantine system. Instead, it creates and exercises the vocabulary that such a system needs: a way to name a federation, a way to identify its members, a way to represent the exact operation that must be agreed on, a way to reject incompatible proposals, a way to record accepted operations in an ordered journal, and a way to replay those operations into member-local mint state. In other words, the code now has a real consensus boundary, even though the decision source behind that boundary is still a local in-memory driver rather than a BFT protocol.

The first layer is configuration. `FederationConfig` describes the public federation: its roster, threshold parameters, identity keys, and federated keysets. `MemberConfigPublic` is the public part of one participant, while `MemberConfigPrivate` is the local secret material that one mint process is allowed to hold. The code validates that the private identity key derives to the public identity key in the roster, that member IDs are valid, and that the federation ID is derived from the sorted public federation data. That matters because a member must never accidentally accept operations for a different federation with a similar looking configuration.

The second layer is key material. For now the distributed key generation ceremony is mocked by a trusted-dealer helper, `generate_trusted_dealer_configs`. This function creates a public federation config plus one private member config per participant. Each private config contains BLS secret shares, while the shared public config contains the aggregate Cashu public keys and each member's public BLS shares. This gives the project a practical development environment for threshold blind signatures without forcing the DKG protocol to be solved before the rest of the federation machinery can be tested.

The third layer is the operation model. CDK now has a `FederationOperation` enum that can describe mint quote creation, mint quote payment observation, mint issuance, melt quote creation, melt payment observation, melt execution, swap, and restore. Those variants are wrapped in a `FederationOperationEnvelope` that carries a version and a `federation_id`. The envelope is important: the operation is not just "please mint these blinded outputs"; it is "this exact federation, at this operation-version, is being asked to agree on this exact Cashu state transition."

From that envelope the code computes a `FederationOperationId`. Today this is a domain-separated SHA-256 hash over a sorted JSON representation of the envelope. That gives the implementation deterministic duplicate detection: if the same request is submitted twice, it produces the same operation ID and the journal can return the previous entry instead of appending a second one. If a wallet tries to use the same quote for a different output set, the operation ID changes, but the conflict key remains the same. That is the core shape needed to stop the threshold-signature mix-and-match attack.

The fourth layer is the journal. `InMemoryFederationJournal` stores accepted entries in local order, maintains an index from operation ID to journal position, maintains an index from conflict key to operation ID, and extends a digest chain every time a new operation is appended. It can distinguish an exact duplicate from a conflicting operation, which is already enough to make the quote-scoped mint hooks meaningful: a mint quote can only map to one quote operation, and a mint request for that quote can only map to one exact set of blinded outputs. Accepted entries and private REST sequence watermarks can be persisted through the mint database and replayed on startup.

The fifth layer is transport. `cdk-axum` now mounts a private federation REST API under `/federation/v1` when a mint is configured as a federation member. The routes expose signed peer status, signed local status, signed operation submission, and signed journal catch-up. Requests carry a member identity signature, sender, receiver, message kind, monotonically increasing sequence, and body hash; unknown senders, wrong receivers, stale sequences, body-hash mismatches, and wrong message kinds are rejected before mutation. A peer client can query status, fetch catch-up ranges, submit operations, and run automatic sync rounds against configured federation partners.

Finally, the public eCash paths now cross this boundary in federation mode. Mint quote creation, quote payment observation, mint issuance, swap, melt quote creation, melt execution, melt payment observations, restore, and check-state handling either submit accepted operations or consume consensus materialized state. Member responses return BLS signature shares for accepted mint, swap, melt-change, and restore outputs; the wallet federation connector verifies member advertisements and shares, then aggregates threshold BLS shares into ordinary Cashu signatures.

> [!WARNING]
> ### Important status distinction
>
> At this historical checkpoint, the code recorded operations into
> `InMemoryFederationConsensus`. It persisted and replayed the accepted
> operation journal, but did not yet run AlephBFT, attach quorum-signed session
> outcomes, or compute materialized-state digests. That scaffold has since been
> removed.

## 2. Cryptographic Foundation

Federated Cashu uses two separate key families. Member identity keys authenticate federation participants and sign private REST member-to-member envelopes. Threshold BLS shares sign Cashu blinded outputs. Keeping those roles separate is a central design invariant.

### 2.1 Member Identity Keys

`MemberConfigPublic` contains a secp256k1 `identity_public_key`. `MemberConfigPrivate` contains the matching `identity_secret_key`. Startup validation rejects mismatches and rejects non-secp256k1 identity keys. These keys are not Cashu mint keys; they are federation membership keys.

### 2.2 Threshold BLS Shares

A trusted-dealer setup creates BLS secret shares per member and amount. Each `FederatedKeyset` stores aggregate BLS public keys per amount and each member's public BLS share per amount. CDK validates that public shares interpolate back to the configured aggregate key. Federation mode is v3/BLS-only: startup rejects aggregate keysets that are v1, v2, or otherwise non-BLS, and mint, swap, restore, and melt-change signing paths reject outputs that target non-v3 keysets before consensus signing.

```text
aggregate public key = interpolate(public shares, threshold)
```

The signing helper `MemberConfigPrivate::sign_blinded_messages` signs a BLS G1 blinded message with the local secret share, verifies the resulting `BlsBlindSignatureShare` against the public share, and returns a `FederationBlindSignatureShareSet` tied to one `FederationOperationId`.

### 2.3 Operation Binding

Signature-share objects carry `operation_id`, `member_id`, `keyset_id`, `amount`, `blinded_secret`, and the BLS share. This is the intended audit link: every signature share should be traceable to an accepted operation hash.

## 3. Current Consensus Model

The current consensus mechanism is a deterministic local append log. It accepts one operation envelope at a time, computes its operation ID, rejects wrong-federation envelopes, detects quote-scoped conflicts, deduplicates exact retries, and records accepted operations with a chained order digest.

> [!NOTE]
> ### Library use today
>
> The code uses CDK-native Rust modules in `cdk-common`, `cdk`, and `cdk-axum`. It uses `serde` and sorted `serde_json` values for the first canonical operation encoding, `bitcoin_hashes` SHA-256 for operation and journal digests, `BTreeMap` for deterministic ordering, Axum for private REST routes, and a Tokio mutex around the in-memory consensus handle. It does not yet use Fedimint's code or the `aleph-bft` crate.

### 3.1 Operation ID Construction

`FederationOperationEnvelope::operation_id()` serializes the envelope into JSON, recursively sorts all JSON object keys, and hashes the result with a domain separator.

```text
operation_id = sha256(
  len("cdk-federation-operation-v1") ||
  "cdk-federation-operation-v1" ||
  len(canonical_envelope_json) ||
  canonical_envelope_json
)
```

This is enough to make exact duplicate requests deterministic inside the current Rust implementation. The implementation plan still calls for a future deterministic binary encoding, so this should be read as the current scaffold rather than the final wire encoding.

### 3.2 Conflict Keys

Conflict keys prevent two different operation IDs from claiming the same quote-scoped resource. For example, two mint operations with the same quote ID but different blinded outputs produce different operation IDs while sharing the same conflict key; the second is rejected.

The implemented conflict keys are deliberately conservative around quotes. A `MintQuote` operation claims its quote ID, so the same quote cannot be redefined with a different request or response. A `MintQuotePayment` operation claims the quote together with the observed state, which lets the current scaffold record payment observations while still rejecting incompatible observations for the same quote-state pair. A `Mint` operation claims every quote ID touched by its single or batch mint request. That is the essential protection for issuance: the federation must not sign different blinded output sets against the same paid quote.

Melt quote, melt payment, and melt operations have quote-scoped conflict keys. Swap and restore currently do not use journal conflict keys: swap double-spend protection is enforced by deterministic apply against the materialized proof state, and restore is a lookup path for already accepted outputs rather than a spending operation.

## 4. Object Model

The object model is intentionally small, but each object has a narrow job. The implementation does not try to make every mint function federation-aware at once. Instead, it introduces a translation layer: public Cashu requests are converted into federation operations, federation operations are wrapped into envelopes, envelopes are accepted into a journal, and only then should state-machine code be allowed to produce signature shares or mutate mint state.

### 4.1 Configuration Objects

`FederationId` is the root identity of the federation. It is not a user-facing name; it is a 32-byte roster hash derived from the federation's threshold parameters and sorted public member data. This makes the ID stable for the same federation and sensitive to membership or key changes. `MemberId` is a nonzero participant identifier and also maps naturally to the BLS signer ID used in threshold signing.

`ThresholdParams` stores the member count, BLS signature threshold, and consensus threshold. It rejects empty federations, thresholds larger than the member count, and consensus thresholds below the signing threshold. The helper `with_bft_consensus` derives a practical consensus threshold from the member count, while still letting the signing threshold be explicit. This is already enough to model a three-of-five style mint federation in tests.

`FederatedKeyset` is where Cashu mint keys become federation keys. For each amount, it stores the aggregate BLS public key and the member public shares. Validation checks that member shares line up with the roster and interpolate back to the aggregate key. `FederationConfig` then ties this keyset data to the public member roster, and `MemberConfigPrivate` ties the local process to one member's identity secret key and BLS secret shares. The split is important: the public config can be shared by every member and eventually by wallets, while the private config must remain local to one mint instance.

### 4.2 Operation Objects

The operation enum is the bridge from Cashu protocol requests to consensus. `FederationMintQuoteOperation` records a quote ID, the original `MintQuoteRequest`, and the string-ID `MintQuoteResponse`. This makes quote creation visible to the federation instead of leaving it as a private fact inside one mint's database. `FederationMintQuotePaymentObservation` records what the member believes about that quote's payment state: method, state, amount paid, amount issued, and an optional payment proof. Local development runs often use the fakewallet backend as the configured CDK payment backend, but the observation object is backend-neutral and already shaped so delayed or failed observations can be modeled.

`FederationMintOperation` wraps a `FederationMintRequest`, which can be either a single `MintRequest<String>` or a `BatchMintRequest<String>`. The request exposes the quote IDs it touches, and those quote IDs become conflict keys. This is exactly where the protocol-level inflation issue is addressed: the federation is not supposed to sign "some outputs for some quote"; it is supposed to agree on the one exact output transcript that consumes that quote.

The melt, swap, and restore variants are now active protocol objects. `FederationMeltQuoteOperation`, `FederationMeltQuotePaymentObservation`, and `FederationMeltOperation` describe the outgoing-payment side. `FederationSwapOperation` wraps a `SwapRequest`, and `FederationRestoreOperation` wraps a `RestoreRequest`. Public routes and wallet connector paths now use these backend-neutral objects for federation flows that can be exercised with fakewallet in local development, while production BFT ordering and real payment policy remain future work.

### 4.3 Journal, Transport, Signing, and Runtime Objects

Once an operation exists, the journal gives it a place in order. `FederationJournalEntry` stores the accepted index, the operation ID, the original envelope, conflict keys, the previous and current order digest, and the local application status. `FederationJournalAppendOutcome` tells callers whether the submission appended a fresh entry or matched an existing one. That distinction is what makes retry behavior safe: a repeated request is idempotent, while a similar but different request can still be rejected as a conflict.

The transport objects carry those same concepts over the private REST boundary. `FederationSubmitOperationRequest` contains the envelope, `FederationSubmitOperationResponse` returns the receiver's computed operation ID and append outcome, `FederationPeerStatus` describes a member's local journal height and digest, and the catch-up request and response types move journal entries by index. The signing objects then bind the cryptographic output to the consensus object: `FederationBlindSignatureShare` and `FederationBlindSignatureShareSet` both carry the `operation_id` that authorized the share.

At the mint boundary, `FederationRuntimeConfig` is the object that attaches a validated public and private federation config to a running CDK mint. Its helper methods build operation envelopes from ordinary mint inputs and quote responses. That is the small but important bridge between existing CDK business logic and the new federation code.

## 5. Private Federation REST API

The private API is mounted only when the mint has a federation config. It is separate from public Cashu routes and currently exposes signed peer status, signed local sync status, signed operation submission, and signed journal catch-up.

### 5.1 Request and Response Objects

There are four private endpoints today, all using signed POST bodies. `POST /federation/v1/status` returns a `FederationPeerStatus`, which is a compact summary of the member's local accepted-order view: REST API version, federation ID, member ID, operation count, latest accepted index, and latest order digest. `POST /federation/v1/local-status` wraps the same peer status with local sync guard state (`healthy`, `lagging`, `catching_up`, or `halted`) so operator tooling can require members to be ready and converged before a smoke test proceeds.

`POST /federation/v1/operations` accepts a `FederationSubmitOperationRequest`. The request contains a `FederationOperationEnvelope`, not a trusted operation hash. The receiver recomputes the operation ID locally, checks that the envelope belongs to its configured federation, and then asks `InMemoryFederationConsensus` to append or deduplicate the operation. The response, `FederationSubmitOperationResponse`, returns the computed operation ID and the append outcome. The signed outer envelope authenticates the sender and receiver, while the receiver still derives the operation ID locally from the submitted operation rather than trusting a caller-supplied hash.

`POST /federation/v1/journal` accepts a `FederationJournalCatchUpRequest`. A lagging member says, in effect, "send me entries starting at this index, with this optional page limit, and here is the digest I believe I know." The response returns the entries, the peer's latest index and digest, and a `complete` flag. The handler checks the caller's known digest before serving the page, and the receiver verifies entry indices, operation IDs, conflict-key metadata, and digest-chain continuity before appending catch-up entries. The current handler does not yet prove those entries with quorum signatures, because real consensus sessions do not exist yet. Still, this is enough for offline members to discover a missing range, request it, replay it into local storage, and compare accepted-order digests.

### 5.2 Error Mapping

The error mapping is small and deliberately direct. If a mint is not configured as a federation member, private federation routes behave as unavailable and return `404`. If an operation conflicts with an existing conflict key, the route returns `409`, because the caller made a request that is semantically incompatible with the accepted journal. If the envelope carries the wrong federation ID, the route returns `400`. Other federation errors, such as invalid IDs, invalid keysets, encoding problems, or journal-state errors, also currently map to `400`.

The REST API now wraps these payloads in signed member envelopes. A private request carries federation ID, sender, receiver, sequence, message kind, body hash, and a secp256k1 signature from the sender's configured identity key. The receiver rejects wrong federation IDs, unknown senders, wrong receivers, stale sequences, body-hash mismatches, invalid signatures, and wrong message kinds before it mutates consensus or storage state.

### 5.3 Example Submit Payload

```text
{
  "envelope": {
    "version": 1,
    "federation_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "operation": {
      "type": "mint",
      "data": {
        "request": {
          "type": "single",
          "data": {
            "quote": "quote-1",
            "outputs": []
          }
        }
      }
    }
  }
}
```

Real mint requests contain blinded outputs. This abbreviated shape illustrates the envelope and tagged operation structure.

## 6. Journal State

The journal is the current materialization of "accepted consensus order." It stores entries in local submit order and maintains two indexes: operation ID to entry index, and conflict key to operation ID. In running mint state, accepted entries are also written through the mint database so startup replay and private REST catch-up can rebuild the in-memory consensus view.

### 6.1 Journal Entry

A `FederationJournalEntry` is the record of one accepted operation. It carries a zero-based `index`, the computed `operation_id`, the original `FederationOperationEnvelope`, and the conflict keys claimed by that operation. It also stores `previous_order_digest` and `order_digest`, which connect the entry to the accepted order before it. Finally, it stores a local `FederationOperationStatus`. The status says where this member is in applying the operation, but it is deliberately not part of the order digest. Accepted order and local application progress are related, but they are not the same fact.

The in-memory journal maintains two supporting indexes. The first maps operation ID to journal index and gives the implementation idempotent retry behavior. The second maps conflict key to operation ID and gives the implementation resource ownership. When an exact operation arrives twice, the journal returns a duplicate outcome. When a different operation tries to claim the same conflict key, the journal rejects it as a conflict. This is the mechanism that begins to turn "all members should sign the same thing" into an enforceable rule.

### 6.2 Digest Chain

```text
genesis_digest = sha256("cdk-federation-journal-genesis-v1")

order_digest_i = sha256(
  len("cdk-federation-journal-entry-v1") ||
  "cdk-federation-journal-entry-v1" ||
  len(previous_order_digest) ||
  previous_order_digest ||
  index_i ||
  len(operation_id_i) ||
  operation_id_i
)
```

The digest is over accepted order, not full mint materialized state. Later milestones add state digests over consensus-visible tables after deterministic apply functions exist.

That distinction is subtle but useful. The journal digest can tell two members whether they accepted the same sequence of operations. It cannot yet tell them whether their mint databases contain the same quote rows, proof rows, or signature-share outcomes. For that, the implementation still needs deterministic apply functions and state digests over the materialized consensus-visible tables.

### 6.3 Operation Status

`FederationOperationStatus` tracks local application status: `Accepted`, `Applying`, `Applied`, `Rejected`, and `Failed`. Updating status does not change the accepted order digest.

This gives the implementation a place to represent the future apply pipeline, and that pipeline is now partially in use. An operation can be accepted by consensus before a member has finished applying it to its database. If the member crashes during apply, durable storage can reload the accepted journal, replay entries through `Mint::apply_federation_journal_entries`, and mark successfully applied operation IDs as `Applied` without inventing a new accepted order.

## 7. State-Machine Boundary

The final architecture wants consensus to order operations and a deterministic mint state machine to apply them transactionally. The current code has moved substantially past the first boundary: accepted journal entries now replay into local mint storage for mint quotes, mint payment observations, mint issuance, melt quote creation, melt payment observations, swaps, and melts.

Today the public Cashu handler still receives a request in the ordinary way. In federation mode, the Axum handler asks the federation runtime to build an operation envelope from the Cashu request or response. That envelope is submitted to the local consensus driver, which appends it, deduplicates it, or rejects it as a conflict. The accepted entry is persisted and then applied through federation-aware mint functions that update local quote, proof, operation, and blind-signature state.

Mint issuance validates paid quote state, output balance, active v3/BLS keysets, and idempotent persisted shares before recording blinded messages, member share signatures, completed operations, and quote issuance. Swap apply verifies aggregate BLS inputs, checks balance and spent state, marks inputs spent, stores output blinded messages, and persists member output shares. Melt apply verifies aggregate BLS inputs, records configured-backend pending or paid state, spends proofs when appropriate, stores change shares, and lets later melt-payment observations finalize or roll back pending saga state.

As this extraction matures, the local dummy consensus and the future distributed consensus can keep sharing the same application code. That is the point of the boundary: ordering can evolve from a single-process append log to a real BFT protocol without rewriting the Cashu state transition semantics each time.

> [!CAUTION]
> ### Current limitation
>
> The present public federation flow records and applies consensus-facing operations, but it still uses a local deterministic append log rather than a distributed quorum. The remaining consensus limitation is not the absence of mint/swap/melt apply paths; it is the absence of a real replicated BFT ordering engine, quorum-signed catch-up certificates, and materialized state digests.

## 8. Operation Flows

### 8.1 Mint Quote Creation

Mint quote creation is the first public flow that has been made consensus-visible. A wallet asks for a mint quote through the existing public API. The member builds a `FederationMintQuoteOperation` containing the quote ID, the original request, and the response. That operation is wrapped in an envelope, submitted to `InMemoryFederationConsensus`, persisted, and applied into local quote storage. Catch-up replay applies the same quote operation on a member that was offline.

### 8.2 Mint Quote Payment Observation

Payment observation is represented separately from quote creation because in a real mint federation the payment state is an external fact that can be learned later. Local development members commonly use fakewallet to observe success, delay, or failure through the same CDK payment-backend boundary, and the object records enough information to model those states: the method, state, amount paid, amount issued, and optional payment proof. The current handler turns the observed quote status into a `FederationMintQuotePaymentObservation` and records it under a quote-and-state conflict key. Later, when real payment backends are used, this same operation is where members will agree that a quote is paid before issuance can proceed. Paid observations replay into the quote state and make issuance possible without making fakewallet part of the federation semantics.

### 8.3 Mint Issuance

Mint issuance is the core operation that motivated this work. The wallet submits blinded outputs for one or more paid quotes. Federation mode converts the handler's `MintInput` into a `FederationMintRequest`, preserving whether the request was single or batched. The operation claims every quote ID touched by that request. If the same quote is later submitted with a different set of blinded outputs, the operation ID will differ while the conflict key remains the same, and the journal rejects the second request. This gives the implementation the first concrete defense against a wallet collecting threshold shares for different output sets from different members.

Accepted mint operations now produce local BLS member shares and store them as blind signatures under the accepted operation. The public route returns member shares rather than standalone final signatures, and the wallet federation connector validates and aggregates threshold shares into ordinary Cashu proofs under the aggregate key.

### 8.4 Swap, Melt, and Restore

Swap is now wired through the federation path. The public swap route converts the request into a `FederationSwapOperation`, submits it to the journal, applies the accepted operation by verifying aggregate BLS input proofs, checking balance and spent state, spending inputs, storing output blinded messages, and returning local BLS output shares. The wallet connector aggregates member swap shares and rejects mismatched member responses.

Melt is implemented through the backend-neutral federation payment-observation path. Melt quote operations can be recorded, melt requests are accepted through consensus, aggregate BLS inputs are verified, the configured CDK payment backend reports pending, paid, or unpaid state, and accepted payment observations can finalize paid pending melts or roll back unpaid pending melts. Local tests use fakewallet to exercise those states, but change outputs are signed with member BLS shares only after accepted operation apply, and non-v3 change outputs are rejected before consensus signing.

Restore and check-state are also federation-aware. Restore returns shares for outputs that were already signed by accepted operations, and the wallet connector aggregates threshold restore shares. Check-state remains a read query over materialized state, but the connector requires threshold agreement and rejects divergent spent, pending, or unspent views.

## 9. Security Properties Achieved by the Current Scaffold

The current scaffold already achieves several useful safety properties, though each one has a clear production caveat. First, every operation is bound to a `federation_id`, and the local consensus driver rejects envelopes for the wrong federation. Private REST now also authenticates the sender with the member identity key from the configured roster, checks the receiver and message kind, binds the body hash to the signature, and rejects stale sequence numbers before mutation.

Second, operation identity is deterministic. The sorted JSON encoding and domain-separated SHA-256 hash give CDK a stable `FederationOperationId` for exact retries. This is not the final encoding decision, but it is enough for current tests to reason about duplicates and conflicts. Third, idempotent retry behavior exists inside the in-memory journal: the same operation does not create a new accepted entry.

Fourth, mint quote mix-and-match prevention has its first real mechanism. A wallet cannot present one set of blinded outputs for a quote and then present a different set for the same quote without colliding on the quote conflict key. In a single local journal this already rejects the conflict. In the final federation it must be enforced across quorum-agreed ordering before any member returns a signature share. A focused sliding-window regression now submits ABC, BCD, CDA, and DAB output sets for one paid mint quote and proves the journal accepts one canonical mint operation while rejecting the overlapping variants.

Fifth, the signing objects are shaped for auditability. A `FederationBlindSignatureShare` carries the `operation_id` that authorized it, plus the member ID, keyset ID, amount, blinded secret, and share. Public mint, swap, melt-change, and restore paths now use those member shares rather than returning standalone final signatures in federation mode, and the wallet verifies shares against configured public shares before aggregating them. Federation aggregate keysets are v3/BLS-only, and non-v3 federation outputs are rejected before signing.

Finally, status and catch-up expose the local journal height and order digest, durable storage can replay accepted entries and sequence watermarks on restart, and public federation submit routes fail closed when the local sync guard is lagging, catching up, or halted. The remaining security caveat is that catch-up entries are not yet backed by quorum-signed consensus catch-up certificates, and order digests are not yet paired with materialized state digests.

## 10. Future Outlook: Precise Next Steps

Everything above documents the implementation that exists today. This section is intentionally forward-looking. The major shape of the federation MVP is now present: signed private REST, durable journal replay, deterministic apply for backend-neutral eCash flows, wallet aggregation, lag guards, and operator status tooling. The remaining work is to replace the local decision source with real replicated consensus and harden the system around state digests, production networking, real payment backends, DKG, rotation, and review.

### 10.1 Freeze or Replace Canonical Encoding

The first future decision is encoding. The current sorted JSON hash is useful because it let the project move quickly, but the operation ID format must either be frozen as a supported canonical format or replaced by deterministic binary encoding before other implementations depend on it. The precise work is to define the canonical representation for every `FederationOperation` variant, add operation ID test vectors, and make every receiver recompute the hash locally. Private REST callers and federated wallets should never be able to smuggle in an operation ID; they should only send the operation, and the verifier should derive the identity from canonical bytes.

### 10.2 Replace the Local Log with Replicated Consensus

`InMemoryFederationConsensus` should become one implementation of a consensus-engine trait rather than the only decision source. The public route and private REST boundaries already submit envelopes, persist accepted entries, apply them, and return member shares from accepted outcomes. A real engine can reuse that shape if it provides operation submission, decided operation order, peer status, catch-up ranges, and shutdown.

AlephBFT remains a good candidate and Fedimint remains the best design reference for consensus items and deterministic module application. CDK should still keep the simpler CDK-native approach wherever it preserves the same invariant: all honest members decide the same ordered operations before any member produces a usable signature share.

### 10.3 Add State Digests, Audits, and Quorum-Proven Catch-Up

The current order digest proves that two members accepted the same operation IDs in the same order. It does not prove that their materialized mint tables are identical. The next storage layer should compute state digests over consensus-visible quote, proof, operation, and signature-share tables, excluding local private config, connection state, logs, and caches.

Catch-up currently verifies digest-chain continuity from a peer, but real production catch-up should include signed catch-up certificates or another quorum-verifiable proof. A lagging member should not have to trust whichever peer answered first. It should verify that the returned entries are decisions already accepted by the federation, replay them deterministically, then compare both order digest and state digest at session boundaries.

### 10.4 Run and Harden the Live Multi-Process Harness

The trusted-dealer generator now emits backend-agnostic public/private federation config by default, with local member configs that use the fakewallet backend and the guarded smoke script available through the explicit development/regtest opt-in. `cdk-mintd federation status` can require members to be ready and converged. The remaining validation is to run that harness where loopback listeners are permitted: start multiple members using the local fakewallet backend, issue mint/swap/melt/restore/check-state traffic, stop and restart one member, observe catch-up, and verify the roster returns to the same height and digest.

Private REST is the MVP transport because it is simple and fits existing CDK patterns, but it requires reachable private member endpoints. Later deployments may add TCP/TLS, Noise, Tor, VPN, reverse tunnels, or an Iroh adapter for members behind NATs and firewalls. Those transports should stay behind the same signed federation message and consensus interfaces.

### 10.5 Define Real Payment Backend Policy

Current local smoke tests exercise minting and melting with the fakewallet backend as a normal CDK payment backend. The implemented real-backend policy enforces only a configured quorum of authenticated matching observations; it deliberately does not serialize labels for shared-backend, origin-proof, threshold-gateway, or onchain-proof models that CDK does not verify. Before handling real funds, a deployment needs a concrete evidence verifier with explicit admission, conflict, retry, timeout, and operator-inspection behavior.

### 10.6 Replace Trusted Dealer with DKG and Add Rotation

Trusted-dealer key generation is useful for tests and local smoke, but long-running federations need dealer-free DKG. A DKG transcript should bind federation ID, roster, identity public keys, participant order, commitments, public shares, aggregate keys, and member signatures. Key rotation should later become a consensus operation so new aggregate keysets activate only after the federation accepts them.

### 10.7 Production Hardening and Security Review

Observability should become part of the consensus product: consensus height, apply lag, peer lag, signature-share production, conflict rejection, catch-up progress, state digest mismatches, and aggregation failures should all be visible. Before production use, the security review should trace every signature share to an accepted operation hash and review operation canonicalization, NUT-20 and SIG_ALL transcript binding, restore spam, huge batch behavior, repeated invalid operations, and all routes that can sign or regenerate member shares.

> [!NOTE]
> ### Near-term checkpoint
>
> The next practical consensus checkpoint is to run the generated multi-process development-backend federation smoke script in an environment where loopback listeners are permitted, then record signed status, restart, catch-up, and convergence evidence. The next purely offline checkpoint is materialized state digests and audit summaries over consensus-visible mint tables.

## 11. Code Map

| Area | File | Important objects |
| --- | --- | --- |
| Federation config and exports | `crates/cdk-common/src/federation.rs` | `FederationConfig`, `MemberConfigPrivate`, `FederatedKeyset`, `ThresholdParams`. |
| Canonical operation envelopes | `crates/cdk-common/src/federation/operation.rs` | `FederationOperationEnvelope`, `FederationOperation`, `FederationOperationId`, conflict keys. |
| Accepted operation log | `crates/cdk-common/src/federation/journal.rs` | `InMemoryFederationJournal`, `FederationJournalEntry`, `FederationJournalDigest`. |
| Local consensus driver | `crates/cdk-common/src/federation/consensus.rs` | `InMemoryFederationConsensus`, submit/status/catch-up methods. |
| Private REST DTOs | `crates/cdk-common/src/federation/transport.rs` | `FederationSignedRequest`, `FederationPeerStatus`, submit request/response, catch-up request/response, sequence tracking. |
| Threshold share signing | `crates/cdk-common/src/federation/signing.rs` | `FederationBlindSignatureShare`, `FederationBlindSignatureShareSet`. |
| Trusted-dealer setup | `crates/cdk-common/src/federation/trusted_dealer.rs` | `generate_trusted_dealer_configs`, trusted-dealer member and keyset specs. |
| Mint runtime helpers | `crates/cdk/src/mint/federation.rs` | `FederationRuntimeConfig`, operation builders, v3/BLS validators, member share signing, and deterministic apply for mint, swap, melt, and payment observations. |
| Private REST routes | `crates/cdk-axum/src/federation.rs` | Signed `/status`, `/local-status`, `/operations`, `/journal`, peer client sync rounds, and local sync status. |
| Public eCash flow hooks | `crates/cdk-axum/src/custom_handlers.rs` | Federation submission and member-share responses for mint, swap, melt, restore, and quote status routes. |
| Mint state and durable replay | `crates/cdk-axum/src/lib.rs` | Consensus handle, sequence handle, sync guard, journal persistence, catch-up application, and startup replay wiring. |
| Wallet aggregation | `crates/cdk/src/wallet/federation.rs` | Federated connector fan-out, aggregate keyset validation, BLS share verification, threshold aggregation, restore, melt-change, and check-state quorum handling. |
| Operator tooling | `crates/cdk-mintd/src/` | Trusted-dealer generation, non-secret federation inspect, signed status probing, readiness/convergence guards, and generated local smoke scripts. |

Sources read: `docs/federated-cashu-implementation-plan.md`, `docs/federated-cashu-progress.html`, `crates/cdk-common/src/federation.rs`, `crates/cdk-common/src/federation/operation.rs`, `crates/cdk-common/src/federation/journal.rs`, `crates/cdk-common/src/federation/consensus.rs`, `crates/cdk-common/src/federation/transport.rs`, `crates/cdk-common/src/federation/signing.rs`, `crates/cdk-common/src/federation/trusted_dealer.rs`, `crates/cdk/src/mint/federation.rs`, `crates/cdk/src/wallet/federation.rs`, `crates/cdk-axum/src/lib.rs`, `crates/cdk-axum/src/federation.rs`, `crates/cdk-axum/src/custom_handlers.rs`, and `crates/cdk-mintd/src/`.

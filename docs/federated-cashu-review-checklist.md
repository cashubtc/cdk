# Federated Cashu Review Checklist

This checklist is for PRs that touch federated Cashu mint, wallet, consensus,
networking, key generation, payment observation, or storage code. It complements
the general CDK review guide in `.github/agents/review.md` and the safety model
in `docs/federated-cashu-rfc.md`.

The default review stance is conservative: federation changes must preserve
standalone CDK behavior, and federation members must never produce usable
signature shares before the exact operation has been accepted by consensus.

## Reviewer Entry Questions

- Does this PR change any path that can create a blind signature, BLS share, or
  final aggregate signature?
- Does this PR change canonical operation bytes, operation IDs, conflict keys,
  accepted journal order, state digests, or catch-up replay?
- Does this PR change public Cashu routes in federation mode, private federation
  REST routes, member authentication, wallet member fanout, or share aggregation?
- Does this PR change payment observation policy, quote state transitions,
  pending melt recovery, or fake-wallet/regtest behavior?
- Does this PR add or modify database rows that are consensus-visible or used
  during replay?

If any answer is yes, review the relevant sections below before approving.

## Consensus Before Signing

- No public federation-mode route may call signing code directly from the local
  HTTP request path.
- Mint, swap, melt-change, and restore shares must be derived only from an
  accepted federation operation ID.
- Signature-share generation must happen after deterministic validation of
  quote state, proof state, output keysets, amounts, witnesses, and request
  signatures.
- Failed validation must leave no partially stored signature shares.
- Exact accepted-operation replay may return cached shares, but a conflicting
  operation must not generate fresh shares.
- Lagging members must fail closed instead of serving stale signing responses.

## Determinism And Operation Binding

- Operation IDs must commit to the federation ID, operation version, quote IDs,
  inputs, witnesses, outputs, amounts, keysets, payment method, and request
  signatures relevant to the operation.
- Any canonical encoding change must update golden vectors and transcript tests.
- JSON canonicalization must stay deterministic until a binary encoding replaces
  it.
- Conflict keys must reject incompatible operations that spend the same quote or
  state resource.
- SIG_ALL, P2PK, HTLC, and NUT-20 tests must prove that changing the signed
  transcript changes the operation ID or fails verification before signing.

## State Application And Digests

- Accepted operations must apply in journal order.
- Database writes for proof state, quote state, blinded outputs, and shares must
  be atomic for each accepted operation.
- Replaying accepted entries after restart must converge to the same materialized
  state.
- Digest comparisons must use consensus-visible state only and must not include
  local secrets, filesystem paths, random ordering, or volatile timestamps.
- Catch-up must verify entry order, operation IDs, conflict keys, and digest
  continuity before applying entries.

## Database And Storage

- Existing migrations must not be edited; add new migrations instead.
- New consensus-visible tables need tests for empty database migration,
  idempotent replay, conflicting replay, and standalone mode compatibility.
- Indexes must exist for hot lookups by quote ID, proof Y, blinded message,
  operation ID, journal index, and member/session identifiers when applicable.
- SQL-backed federation writes that touch multiple resources must follow the
  deterministic lock ordering rules in
  `docs/federated-cashu-sql-lock-ordering.md`.
- Redb remains wallet-only and out of scope for mint federation storage until a
  future Redb mint backend implements the federation mint storage APIs described
  in `docs/federated-cashu-storage-scope.md`.
- If signature shares move into a dedicated table, reviewers must be able to
  trace every stored share to operation hash, output index, member ID, amount,
  keyset ID, and blinded message. Use
  `docs/federated-cashu-signature-share-provenance.md` for the trace recipe.
- Storage code must not persist private BLS shares, identity secrets, payment
  secrets, or local paths in public status rows, fixtures, or logs.

## Networking And Member Authentication

- Private REST messages must validate federation ID, sender, receiver, member
  identity signature, body hash, and message kind before mutation. Effectful
  DKG messages must enforce durable sequence state; status, catch-up, readiness,
  and Aleph traffic must enforce kind-isolated volatile windows.
  `SubmitOperation` authenticates its sequence but uses the canonical operation
  ID and durable consensus outcome for replay safety.
- Unknown members and mismatched public keys must be rejected before processing
  payloads.
- Peer sync and catch-up requests need explicit timeouts, retry/backoff, page
  limits, and stale-peer handling.
- A member that restarts or falls behind must catch up before serving public
  signing routes.
- Operator status output must redact likely secrets, credentials, private
  endpoints, and local paths.

## Keysets, DKG, And BLS Shares

- Federation mode may create, advertise, accept, and sign only v3/BLS aggregate
  keysets.
- Startup must reject non-v3, non-BLS, mismatched, or malformed aggregate
  keysets in federation configs.
- Wallet federation connectors must reject members advertising mismatched or
  non-v3 aggregate keysets.
- Trusted-dealer setup remains a development bridge until production DKG is
  implemented.
- Secret shares should be zeroized where practical and must not be cloned,
  logged, serialized to public config, or committed in fixtures.

## Payments And Quote State

- Payment backend state must enter consensus as accepted observations or
  accepted payment events, not as nondeterministic reads during apply.
- Mint quotes may issue only the consensus-tracked mintable amount.
- Melt payment attempts need idempotency and recovery coverage that prevents
  double payment.
- Paid, failed, pending, timeout, delayed, duplicated, and conflicting
  observations need explicit tests when the touched path can see them.
- Fake wallet behavior must remain a normal replaceable CDK payment backend and
  must not become a production federation special case.

## Wallet Federation Connector

- Member fanout must require threshold-valid shares and reject bad, mismatched,
  stale, or non-v3 aggregate keyset responses.
- The wallet must aggregate shares only for the exact request it submitted.
- Offline, slow, or bad members must not block success when threshold can still
  be met, and must not be silently counted as valid responses.
- Mint, swap, melt change, restore, and check-state paths need coverage for
  threshold success, insufficient quorum, mismatched keysets, and retry safety.
- FFI bindings must be updated if wallet public APIs change.

## Attack Regression Requirements

Every behavior-changing federation PR should either run an existing attack suite
or add focused tests for the touched invariant. Relevant attacks include:

- Sliding-window threshold share mix-and-match for mint outputs.
- Sliding-window threshold share mix-and-match for swap outputs.
- Same quote with different blinded outputs.
- Same inputs with different output sets.
- Mutated NUT-20 signed output set.
- Mutated P2PK, HTLC, or SIG_ALL witness/transcript.
- Non-v3 or mismatched aggregate keyset advertisement.
- Lagging member serving public signing routes before catch-up.
- Conflicting payment observations and quarantine visibility.
- Restart replay across accepted but not-yet-responded operations.

## Minimum Evidence To Record

For each federation PR, record the narrow tests that cover the changed invariant
and at least one broader gate appropriate to the blast radius:

- Crypto or operation encoding changes: `cargo test -p cashu --features federation`
  or `cargo test -p cdk-common --features federation federation::operation`.
- Mint apply changes: `cargo test -p cdk --features mint,federation
  mint::federation::tests -- --test-threads 1`.
- Wallet connector changes: `cargo test -p cdk --features wallet,federation
  federated_mint_connector_ -- --test-threads 1`.
- HTTP/private REST changes: relevant `cdk-axum` federation tests plus a local
  smoke when process behavior changes.
- Mintd/config changes: `cargo check -p cdk-mintd --features
  federation,fakewallet,sqlite` and the relevant trusted-dealer or smoke tests.
- Broad behavior changes: a feature-disabled workspace gate and a
  feature-enabled workspace compile/check.

## Final Approval Questions

- Would a wallet obtain any usable extra eCash if one member signs this request
  and another member signs a slightly different request?
- Can an independent reviewer trace every returned share to an accepted
  operation hash using `docs/federated-cashu-signature-share-provenance.md`?
- Can a restarted member reconstruct the same state from durable data?
- Does a standalone non-federated mint still behave as before?
- Are all new logs, docs, examples, fixtures, status outputs, and commits free
  of private keys, payment secrets, credentials, local paths, and private
  endpoint details?

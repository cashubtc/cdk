# Federation FROST root DKG implementation review

This is the focused implementation review for Milestone 4 of
`federated-cashu-bdk-frost-implementation-plan.md`. It records the implemented
boundary and test evidence; it is not an independent cryptographic or security
audit.

## Result and composition

Federation setup now composes two independent dealer-free ceremonies under the
same federation roster, ceremony identifier, and `signature_threshold`:

- the existing BLS12-381 ceremony produces Cashu keysets without changing its
  cryptographic types or phase planner;
- the isolated `cdk-frost` ceremony uses `frost-secp256k1-tr` to produce one
  canonical, untweaked, application-neutral secp256k1 root package per member.

FROST round-one broadcasts, confidential round-two contributions, and public
root confirmations use distinct signed REST message kinds. Each message commits
to the federation, ceremony, root epoch, protocol, ciphersuite, threshold,
ordered roster, sender, and—where applicable—receiver. Existing authenticated
sender/receiver checks and durable replay sequences are reused.

The terminal setup transition independently reopens and verifies the local
sealed FROST share against the canonical public package, requires byte-identical
confirmation from every configured member, and pairs it with every local BLS
keyset result. Missing or conflicting output on either side prevents completion.

## Durable phase boundary

Round-one and round-two secret packages are sealed with AES-256-GCM before any
dependent package is sent. Confidential round-two wire contributions are also
stored as sealed participant-local retry records and are opened only for
authenticated delivery or local consumption. The canonical local root share is
stored only as authenticated ciphertext; public records never contain a
plaintext signing share. Associated data binds ciphertext to phase, member,
ceremony context, sender/receiver, and root public package. Exact duplicate
records are idempotent, while a second payload for the same
ceremony/kind/sender/receiver is rejected as equivocation.

The driver does not advance a FROST round until the complete outbound batch is
acknowledged. This preserves catch-up after partitions and response loss: a
member cannot consume all inbound messages and stop rebroadcasting a package
that another member still lacks.

## Evidence

- `cdk-frost`: 14 tests cover real 2-of-3, 3-of-5, 8-of-11, and 14-of-20 DKG,
  threshold signing, distinct verification shares, sealed restart at every
  phase, malformed and cryptographically corrupted packages, incomplete and
  duplicated sets, reordered messages, cross-context replay, wrong recipients,
  application separation, and immutable root bytes.
- `cdk-common` FROST integration tests cover exact BLS-threshold/roster binding,
  protocol/ciphersuite rejection, signed sender ownership, receiver ownership,
  and redacted confidential messages.
- The backend-generic persistence test covers every FROST record class,
  idempotent replay, append-only phase state, and sender/receiver equivocation.
  Driver assertions parse every durable round-two record as an authenticated
  ciphertext envelope and verify redacted diagnostics; the full SQLite suite
  passes with 159 tests.
- The authenticated three-member driver suite covers dual convergence,
  offline/partition recovery, response loss after durable receipt, restart at
  every BLS and sealed FROST phase, prior-ceremony isolation, mismatched root and
  BLS activation rejection, and fail-closed completion without FROST output.
- The existing BLS federation unit suite remains exercised alongside the new
  ceremony. Mechanical fixture updates were required for payment-quote fields
  and mock transport methods that had changed before this milestone.

## Residual risks and later milestones

- The current local sealing key is domain-separated from the member identity
  secret. Startup must keep the sealed-storage provider explicit and validated;
  production deployments should use
  an independently protected secret store where available.
- Public immutable root/application configuration snapshots, state-digest
  inclusion, and catch-up transfer belong to Milestone 5.
- Root refresh and in-place rotation are intentionally unsupported. Custody-key
  replacement requires a separate wallet and a threshold-authorized migration.
- `frost-secp256k1-tr`, the canonical conversion boundary, transcript design,
  sealed storage, and the dual activation gate still require the external review
  and audit gates later in the implementation plan before funds are at risk.

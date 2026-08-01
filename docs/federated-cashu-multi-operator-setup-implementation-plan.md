# Federated Cashu multi-operator setup implementation plan

> **Status — Milestone 1 complete, 2026-08-01:** The pure setup domain,
> canonical artifact validation, certified-manifest derivation, and mandatory
> public-config authorization binding are implemented and verified. Milestone
> 2 durable setup storage is next. This document
> defines the implementation sequence for replacing CDK's file-mediated
> federation enrollment and roster assembly with an authenticated, restartable
> multi-operator setup workflow.
> It is subordinate to
> [`federated-cashu-implementation-plan.md`](federated-cashu-implementation-plan.md).
> The parent document's safety invariants, architecture boundaries,
> cut-through compatibility policy, and release gates are mandatory here.

## Outcome

An operator should be able to create or join a federation by exchanging one
bounded invitation, reviewing one canonical proposal, and explicitly approving
that proposal. CDK then distributes a unanimously signed setup manifest and
starts the existing dealer-free BLS and FROST DKG on every member. No operator
copies another member's private material, no setup coordinator learns a DKG
share, and no member can be made to run a ceremony for a roster or policy it did
not approve.

The setup coordinator is an availability aid and signed-artifact rendezvous,
not a trusted dealer or configuration authority. It may collect, order, and
redistribute public records, but every member independently validates the same
canonical proposal and every configured member must sign it before DKG starts.
After every member has durably obtained the signed setup manifest, the
coordinator is not on the DKG or runtime path.

The target operator flow is:

```text
create invitations -> enroll members -> publish proposal -> unanimous approval
-> distribute certified manifest -> run existing all-to-all DKG
-> confirm one final config -> explicit local activation
```

Wallet users are unaffected. They continue to import or discover one finalized
federation through the existing wallet-facing federation invite and public
member APIs.

## Current boundary and gap

CDK already has the safety-critical DKG data plane:

- independent member identity generation;
- a fixed-roster setup-only `FederationConfig`;
- authenticated readiness, commitment, reveal, secret-share, transcript,
  FROST, and activation messages;
- durable restartable DKG state and equivocation rejection;
- all-member final-config confirmation; and
- identical public-config export with member-local private output.

The missing boundary is the control-plane bootstrap before those authenticated
routes can be trusted. Today:

1. Every operator must run `dkg-member` and transfer a public enrollment JSON.
2. One machine runs `dkg-roster`, chooses the complete policy and ceremony, and
   transfers the setup public JSON and DKG TOML to every operator.
3. Every operator runs `dkg-bind` against that transferred public config.
4. Operators separately coordinate process start, final export comparison, and
   restart into the finalized config.

This is workable with a carefully authenticated out-of-band ceremony, but CDK
does not provide a durable signed record that every operator approved the exact
roster and policy before DKG. The Nocean deployment script additionally creates
all identities on one host and is not a multi-operator deployment mechanism.

This plan replaces that bootstrap workflow. It does not replace or fork the
implemented DKG mathematics and private REST message flow.

## Non-goals

- Dynamic membership, in-place roster changes, or automatic member exclusion.
- Reusing initial bootstrap as runtime federation governance or key rotation.
- Coordinator election or failover before manifest distribution.
- NAT traversal or a transport other than the contract's operator-provided
  private signed REST connectivity.
- Remote process management, remote activation, or automatic finalization.
- Transferring TLS keys, payment credentials, database credentials, backups,
  or member-private configuration through the setup service.
- Replacing the wallet-facing `FederationInvite` or changing public Cashu NUT
  behavior.
- Building a graphical admin application in the first implementation. The CLI
  and bounded backend API must expose stable primitives a later UI can use.
- Resolving proof-bound client intent, payment-backend trust, or any other
  independent production blocker in the parent implementation contract.

## Contract decisions

The following decisions are fixed for the first implementation.

### One untrusted rendezvous coordinator

The federation creator runs a setup-only coordinator endpoint. Joining members
send signed public enrollments to it and poll it for the proposal and final
manifest. A malicious or failed coordinator can delay or abort setup, but
cannot produce an accepted manifest without every enrolled member's approval.
There is no coordinator failover in version 1; operators start a new setup ID
if the coordinator is permanently unavailable before every member has durably
obtained the certified manifest.

This deliberately avoids running a consensus system before the peers and its
authentication roots have been agreed.

### Invitation capability plus identity proof

The creator produces one independently random, one-use invitation capability
per unclaimed member slot. The creator enrolls its own member locally and does
not send its member identity through the coordinator API. An invitation commits
to the setup ID, assigned member ID, coordinator HTTPS URL, coordinator setup
identity, expiry, and protocol version. The high-entropy capability is a
credential: it must not appear in logs, status output, public manifests,
metrics, or error messages.

The coordinator stores only a hash of each invitation capability. Enrollment
atomically consumes the capability and proves possession of the new member
identity key by signing the full enrollment body. TLS authenticates and
encrypts the connection. The coordinator additionally signs enrollment
acknowledgements, proposal publication, manifest publication, and status
responses with the setup identity embedded in the invitation. Joining members
therefore bind the intended coordinator independently of redirects or ambient
DNS changes and retain attributable evidence of coordinator equivocation.

The invitation is the unavoidable first-contact trust root and must be sent to
the intended operator through an authenticated channel chosen by the operators.
CDK cannot distinguish the intended recipient from an attacker who steals an
unused bearer capability. A stolen invitation can claim one slot and force
operators to reject and restart setup; it must never grant access to another
slot or to any private member material.

### Unanimous immutable proposal approval

Every enrolled member must sign the same canonical proposal hash. Threshold
approval is not sufficient for initial setup: the fixed-roster DKG requires all
configured members, and no pre-existing federation governance exists that
could authorize a minority to define the initial roster.

A member may sign at most one proposal hash for one setup ID. Once any approval
exists, the proposal is immutable. A changed member, URL, threshold, policy,
keyset specification, protocol version, or application setting requires an
explicit abort and a new setup ID. CDK must not add approval revocation,
last-write-wins proposals, or a fallback that silently accepts a revised
proposal.

### Manifest-derived DKG context

The certified setup manifest is the only production input to a new DKG
ceremony. The setup-only `FederationConfig`, DKG ceremony ID, BLS keyset
specification, and FROST root epoch are derived from its proposal body. The
setup authorization hash is preserved in the setup config, every DKG context,
the signed DKG transcript, and the finalized public config.

The existing raw `ceremony_id` and keyset-policy TOML fields must not remain an
alternate production setup path after this work. Development fixtures use a
deterministic signed test manifest rather than a compatibility default.

### Explicit activation

Unanimous proposal approval authorizes DKG, not unattended service activation.
The existing transcript-signature and activation phases must still complete,
and each operator explicitly activates the locally validated final config.
Activation is local, atomic, idempotent, and must never be exposed as a remote
coordinator action. A deployment wrapper may restart the service after the
local command succeeds, but CDK does not add automatic finalization or a remote
restart endpoint.

### Cut through the experimental format

Federation setup has no supported deployed format. Add the new required setup
authorization fields directly, update SQLite and PostgreSQL's final federation
schema together, regenerate development configs and databases, and remove the
obsolete file-mediated production path. Do not add optional fields, legacy
invitation parsers, schema backfills, dual reads, or fallback-generated setup
authorization.

Standalone mint behavior and public Cashu NUT compatibility remain unchanged.

## Operator UX contract

The CLI should expose one nested workflow. Exact flags may follow existing
`cdk-mintd` conventions, but the concepts and security prompts are required.

### Creator

```bash
cdk-mintd federation setup create \
    --members 5 \
    --threshold 3 \
    --network bitcoin \
    --public-mint-url https://mint-a.example \
    --federation-api-url https://fed-a.example \
    --coordinator-url https://setup.example.com
```

The command creates distinct coordinator and member identities, durably
self-enrolls the creator, and prints or writes four separately scoped invitation
strings for the remaining slots. It does not print private identities or all
invitation credentials into a normal log. Machine-readable invitation output
requires an explicit protected output path.

The creator starts `cdk-mintd` in setup mode using the generated local setup
state. Setup mode exposes only the bounded bootstrap router and, after
certification, the existing DKG routes; it must not expose wallet minting,
swapping, or melting routes.

After every slot is enrolled, the creator reviews the enrollment set and
publishes the immutable proposal:

```bash
cdk-mintd federation setup propose --config member.toml
```

Proposal publication is not approval. The creator must subsequently review and
approve the proposal through the same member flow as every other operator.

### Joiner

```bash
cdk-mintd federation setup join '<invitation>' \
    --public-mint-url https://mint-b.example \
    --federation-api-url https://fed-b.example
```

The command:

- validates invitation version, expiry, coordinator identity, URL policy, and
  assigned member slot;
- creates the member identity locally with private-file permissions;
- generates and signs the public enrollment;
- submits it exactly once and persists the acknowledged enrollment; and
- writes the setup-mode configuration needed to start or resume that member.

Retries reuse the same identity and byte-identical enrollment. They must never
regenerate an identity after an ambiguous response.

### Review and approval

```bash
cdk-mintd federation setup review --config member.toml
cdk-mintd federation setup approve --config member.toml
```

`review` displays the security-relevant proposal without secrets:

- setup and proposed federation fingerprints;
- network and required protocol versions;
- ordered member IDs, public mint URLs, federation API URLs, and abbreviated
  identity-key fingerprints;
- member count and signature/consensus/payment-observation thresholds;
- checkpoint policy;
- BLS keyset unit, amounts, fees, activity, and expiry;
- FROST root epoch and enabled application commitments; and
- the exact proposal hash that `approve` will sign.

`approve` requires an explicit confirmation unless a non-interactive approval
flag names the exact proposal hash. It signs with the local federation identity
and durably records the one-proposal-per-setup fence before transmitting the
approval. A timeout can therefore retry the same approval but cannot induce a
second approval.

### Progress and recovery

```bash
cdk-mintd federation setup status --config member.toml
cdk-mintd federation setup abort --config member.toml
```

Status uses stable phases and bounded counters: invitations claimed,
enrollments accepted, approvals collected, manifest certified, DKG phase and
counts, final digest confirmations, and local activation state. It identifies
missing member IDs but does not expose invitation tokens, private messages,
keys, signatures in full, or machine paths.

Before certification, an explicit abort closes the setup ID. After any member
has signed a proposal, a correction always starts a new setup ID. During DKG,
the existing bounded retry and durable-resume behavior applies. Permanent
member loss requires an explicit abort, a newly agreed roster, and a new setup
and ceremony ID.

Before an invitation is consumed, the creator can replace it without changing
the setup ID:

```bash
cdk-mintd federation setup invite --member 3 --replace --config member.toml
```

Replacement atomically expires the old capability. A consumed invitation or
accepted enrollment cannot be replaced; correcting either requires a new setup
ID.

### Activation

```bash
cdk-mintd federation setup activate --config member.toml
```

Activation requires the certified manifest, complete local BLS and FROST
records, all transcript signatures and final-config confirmations, and an exact
local revalidation of the exported public/private pair. It atomically records
the active config reference and returns a stable result that a service manager
can use to restart the member. Running it twice returns the same success.

No operator manually copies enrollment, roster, ceremony, or finalized public
config files in the normal workflow. Export remains available for audit and
backup, not as the transport between setup phases.

## Protocol and data model

Add a dedicated feature-gated module under
`crates/cdk-common/src/federation/setup/`. Keep bootstrap types separate from
wallet-facing `FederationInvite`, DKG cryptographic records, and normal private
REST transport envelopes.

### Identifiers and hashes

- `FederationSetupProtocolVersion`: strict setup wire version.
- `FederationSetupId`: random 32-byte identifier created before invitations.
- `FederationSetupInvitationId`: public opaque identifier for one member slot.
- `FederationSetupCoordinatorPublicKey`: the ephemeral setup-service identity
  pinned by invitations and retired only after manifest distribution closes.
- `FederationSetupProposalHash`: canonical hash of the complete proposal body.
- `FederationSetupAuthorizationHash`: stable authorization value carried into
  the DKG and final config. Version 1 sets it to the proposal hash.

All hashes use explicit domain-separation constants and canonical field
appending. Do not define signed hashes as hashes of ordinary JSON serialization.
Collections are sorted by member ID; numeric widths, optional fields, URLs,
protocol versions, and policy variants are encoded explicitly.

### Invitation

`FederationSetupInvitation` contains only the data required to reach and
authenticate the coordinator plus the secret capability. Its display/from-string
encoding is versioned, checksummed, bounded, and distinct from the existing
wallet `FederationInvite` prefix.

The public durable invitation record stores the invitation ID, setup ID, member
ID, expiry, capability hash, and state (`available`, `consumed`, or `expired`).
The plaintext capability exists only in the one-time encoded invitation and
the joining operator's protected local state. It is sent as authorization
metadata, never in a URL path or query. After consumption it authorizes only
bounded reads and submission of payloads signed by the enrolled identity; the
joining member erases it after durably validating the certified manifest.

### Enrollment

`FederationSetupEnrollment` contains:

- setup ID, invitation ID, and assigned member ID;
- public mint URL and private federation API URL;
- member identity public key;
- a member-generated 32-byte setup nonce;
- the exact supported setup, DKG, federation-private-REST, wallet, BLS, and
  FROST protocol versions; and
- an identity signature over all preceding fields.

The invitation credential authorizes use of a slot; the enrollment signature
proves possession of the identity placed in that slot. Enrollment does not
contain operator names, email addresses, host paths, TLS private material, or
deployment credentials.

### Proposal

`FederationSetupProposalBody` contains:

- setup protocol version and setup ID;
- coordinator setup-identity public key;
- every complete enrollment in canonical member order;
- exact member, BLS signature, consensus, and payment-observation thresholds;
- wallet and private member protocol versions;
- payment-observation and checkpoint policies;
- BLS DKG keyset specifications;
- FROST root epoch and committed application configuration required at setup;
- Bitcoin network and any chain/payment configuration that must be identical
  across members; and
- bounded setup/DKG policy values that affect safety or interoperability.

Operational tuning that may legitimately differ by machine, such as local
thread count, bounded fanout width, log filter, database connection string, or
listen address behind a proxy, is not part of the shared proposal.

The proposal constructor validates production threshold safety, exact member
count, unique IDs/URLs/identity keys, HTTPS requirements, compatible protocol
versions, keyset-v3/BLS restrictions, policy completeness, and configured
maximum sizes before returning a proposal hash.

The setup-only `FederationConfig` and `FederationDkgCeremonyId` are derived
deterministically from the validated body. The ceremony ID is domain-separated
from the proposal hash; the member nonces already included in the proposal make
the ceremony unique without coordinator-only entropy.

### Approval and certified manifest

`FederationSetupProposal` contains the body, its canonical hash, and the
coordinator setup-identity signature over both. The signature makes conflicting
publication attributable but grants no authority to bypass member approval.

`FederationSetupApproval` contains setup ID, proposal hash, member ID, and the
member identity signature. Validation requires the signer to be the exact
identity in the proposal enrollment.

`FederationSetupManifest` contains the coordinator-signed proposal and exactly
one valid approval from every member. It validates by construction and exposes
the derived setup config, keyset specification, ceremony ID, and setup
authorization hash. Missing, duplicate, extra, mismatched, or conflicting
approvals fail closed.

The manifest is public and exportable. Its signatures are evidence of roster
authorization, not DKG output. The later DKG transcript and activation records
remain separately required.

### Public-config and DKG binding

Add a required setup authorization hash to federation setup and finalized
public configuration. Include it in full config validation and the config
digest. Preserve it unchanged when DKG keysets, transcript, and FROST wallet
configuration are added.

Bind the same value into:

- DKG readiness requests;
- BLS participant state, commitment, reveal, secret contribution, public
  ceremony, transcript, and activation validation;
- FROST DKG context and all three FROST message classes; and
- final member config assembly and startup validation.

Because the federation format is undeployed, bump the affected component and
transcript versions and update every fixture together. Do not accept a missing
setup authorization hash or infer one from an old config.

## State machines and invariants

### Coordinator aggregate state

```text
CollectingEnrollments -> ProposalPublished -> Certified -> Closed
          |                     |
          +---------------------+-> Aborted
```

- Invitation consumption and enrollment insertion are one transaction.
- Exact replay returns the existing enrollment; a different enrollment for a
  consumed invitation or member ID is a conflict.
- Proposal publication freezes enrollment and shared policy mutation.
- The first stored approval freezes the proposal permanently.
- Certification is derived only from the complete validated approval set.
- Closing or losing the coordinator after certification cannot invalidate the
  manifest. It cannot stop DKG after every member has durably obtained that
  manifest.

### Participant-local state

```text
IdentityCreated -> Enrolled -> ProposalReviewed -> Approved -> Certified
-> DkgRunning -> ReadyToActivate -> Activated
        |              |               |
        +--------------+---------------+-> Aborted
```

- Private identity generation is write-before-send and never repeated for an
  active setup ID.
- Review records the exact proposal hash; approval signs only that hash.
- The approval fence is persisted before network transmission.
- DKG participant generation cannot begin without a locally validated complete
  manifest.
- Setup authorization, roster, keyset policy, ceremony ID, and local member
  identity are validated again at every restart boundary.
- Activation cannot precede complete DKG and all-member final-config
  confirmation.

### Coordinator equivocation

A coordinator may sign and present different proposals to different members.
This affects availability only: each member's approval fence prevents it from
signing two hashes, and no proposal can obtain all signatures. Persist both
conflicting coordinator-signed artifacts for bounded diagnostics, then abort.
Do not add an automatic proposal-selection rule.

## HTTP and transport boundary

Add a setup-only Axum router, separate from wallet endpoints and existing
configured-roster private REST routes:

- `POST /federation/setup/v1/enrollments`
- `GET /federation/setup/v1/proposal`
- `POST /federation/setup/v1/approvals`
- `GET /federation/setup/v1/manifest`
- `GET /federation/setup/v1/status`

The enrollment route uses the one-time invitation capability and the signed
enrollment. Later setup reads and approval submission require that same
consumed invitation capability plus signed member payloads where applicable.
There is no network route for create, propose, abort, activate, config-file
write, process restart, or secret export; those remain local operator actions.

The router must have:

- HTTPS-only production URLs with normal WebPKI verification and redirects
  disabled;
- coordinator setup-identity verification from the invitation;
- strict body and response-size limits;
- per-invitation and per-source bounded admission;
- short request deadlines and bounded retry with idempotency;
- `Cache-Control: no-store` for invitation-scoped responses;
- no wallet, consensus, payment, catch-up, or runtime federation routes in
  setup-only mode; and
- structured errors that distinguish expiry, replay, conflict, not-ready,
  invalid signature, unsupported version, and storage failure without echoing
  credentials.

Reuse the existing HTTP client abstraction and signed-domain helpers where the
trust assumptions match. Do not pretend the pre-roster setup identity is an
ordinary configured member or insert bootstrap messages into the runtime
`FederationRestMessageKind` sequence space. Coordinator-produced responses use
a separate setup envelope whose signature covers setup ID, route kind, request
correlation nonce, body hash, and setup protocol version.

## Persistence design

Add bootstrap methods to the mint database boundary or a narrowly composed
setup-store trait backed by the same transaction abstraction. Prefer the former
unless implementation proves that setup must exist before a mint database can
be configured; do not build a second generic storage framework speculatively.

The final SQLite and PostgreSQL federation schemas need equivalent tables for:

- setup session lifecycle and local role;
- invitation metadata and capability hashes;
- signed enrollments;
- one immutable proposal body/hash;
- participant approval fences and received approvals;
- one certified manifest; and
- local activation status or finalized-config reference.

Use typed columns for lookup coordinates and bounded canonical JSON or binary
payloads for complete signed records, following existing DKG persistence. Every
insert is exact-replay idempotent and conflicting payloads at the same logical
coordinate return a structured federation setup conflict. Lifecycle changes
use compare-and-set transitions in the same transaction as their causal
records.

Invitation credentials, identity secret keys, BLS/FROST shares, database URLs,
TLS keys, and local paths must not enter public payload tables. High-entropy
invitation capabilities are compared by a fixed-size cryptographic hash; they
do not need password hashing designed for low-entropy input. Secret local files
reuse CDK's protected private-file writer and are never regenerated on resume.

Backend-generic tests must run against SQLite and PostgreSQL. Since federation
is undeployed, edit the direct final federation schema rather than creating a
compatibility migration chain, and regenerate development databases.

## Component integration

### `cdk-common`

- Add canonical setup types, signatures, hashes, validation, and pure state
  transition planning under `federation/setup/`.
- Add structured setup errors and redacted `Debug` implementations.
- Add required setup-authorization binding to federation config and DKG/FROST
  contexts.
- Keep the module independent of Axum, filesystem paths, databases, and async
  runtime behavior.

### `cdk-axum`

- Add the bounded setup-only coordinator router and handlers.
- Keep handlers as authentication, validation, persistence, and response
  adapters around pure `cdk-common` state transitions.
- Add a restartable setup client/driver that enrolls, polls, submits the one
  durable approval, validates the certified manifest, and then hands control to
  the existing DKG driver.
- Preserve current process-local CPU admission and bounded DKG artifact fanout;
  expose machine-local tuning separately from shared setup policy.

### `cdk-mintd`

- Add nested
  `federation setup create|invite|join|propose|review|approve|status|abort|activate`
  commands.
- Add an explicit setup-only runtime mode and generated member configuration.
- Replace raw roster/ceremony/keyset configuration with manifest-derived input.
- Reuse the existing DKG status/export implementation after manifest
  certification.
- Remove `dkg-member`, `dkg-roster`, and `dkg-bind` as independent production
  workflow commands when the new path reaches feature parity. Retain shared
  internal functions only where they eliminate duplication.

### SQL backends

- Extend `MintDatabase`/transaction behavior and `cdk-sql-common` once.
- Implement identical SQLite and PostgreSQL semantics.
- Add failure-injection coverage for invitation consumption, approval fencing,
  certification, and activation atomicity.

### Deployment and documentation

- Replace Nocean's centralized identity generation with the normal setup UX or
  keep it explicitly labeled as a single-host development adapter that invokes
  the same signed-manifest primitives.
- Add a Nocean deployment drill based on
  `scripts/deploy_nocean_federation.sh`. Reuse its local Linux cross-build,
  checksum-verified SSH/SCP installation, remote dependency checks, bounded
  readiness polling, and public/private health checks; do not preserve its
  centralized setup or current single tmux-session topology.
- For a fresh Nocean setup, run the creator and every joining operator in an
  independent named tmux session with a separate working directory, database,
  identity, private configuration, and log. The deployment driver must execute
  the same `federation setup create|join|propose|review|approve|status|activate`
  commands an operator would normally enter. It must not synthesize the final
  manifest, copy another member's private files, or call internal setup
  shortcuts. Invitation transfer may be automated only through the CLI's
  protected input/output mechanism and must not expose capabilities in command
  traces, process arguments, captured panes, or deployment logs.
- Start each member's DKG-capable setup runtime independently and allow all
  members to proceed concurrently after manifest certification. The deployment
  orchestration may wait for shared protocol gates, but it must not serialize
  members' DKG computation or drive DKG rounds member by member.
- Add a multi-host runbook covering TLS, endpoint reachability, system service
  setup, encrypted backups, approval review, activation, and failed-ceremony
  replacement.
- Do not add a second deployment-specific bootstrap protocol.

The wallet crates, wallet database, public Cashu routes, mint business logic,
AlephBFT ordering, and payment backends should require no setup-specific
behavior.

## Milestone sequence

Each milestone must be a coherent, independently testable change. Record exact
commands, meaningful results, and tests not run in the implementing commit or
its accompanying status update.

### Milestone 0 — Freeze the threat model and wire transcript

- [x] Write the exact canonical transcript for invitation-public fields,
  enrollment signatures, proposal hash, approval signatures, manifest
  validation, setup authorization, and ceremony-ID derivation.
- [x] Define coordinator, joining member, network attacker, replay attacker,
  lost member, and malicious-member capabilities.
- [x] Confirm maximum member count, body sizes, invitation lifetime, and
  supported protocol versions from existing federation bounds.
- [x] Document that coordinator equivocation prevents certification rather than
  selecting a winner automatically.
- [x] Obtain focused review of the transcript before implementing persistence
  or HTTP routes.

**Gate:** Fixed byte-vector fixtures demonstrate stable hashes and signatures
for at least 2-of-3, 3-of-5, and maximum-member proposal shapes.

**Completed 2026-08-01:**

- Frozen transcript and focused threat-model review:
  [`federated-cashu-setup-transcript-v1.md`](federated-cashu-setup-transcript-v1.md).
- Executable reference encoder/signature verifier:
  `crates/cdk-common/tests/federation_setup_transcript_vectors.rs`.
- Fixed expected values:
  `crates/cdk-common/tests/data/federation_setup_transcript_v1.json`.
- Verified with
  `cargo test -p cdk-common --features federation --test federation_setup_transcript_vectors`
  (1 passed), `cargo test -p cdk-common --features federation` (472 passed,
  4 ignored across unit, integration, and doc-test targets), and
  `cargo clippy -p cdk-common --features federation --test federation_setup_transcript_vectors -- -D warnings`.
  The targeted Rust source passed `rustfmt --check`.
- The workspace-wide `cargo fmt --all -- --check` is not usable as Milestone 0
  evidence because Cargo traverses the existing unformatted sibling Bark path
  dependency outside this CDK worktree; no sibling files were changed.

### Milestone 1 — Pure setup domain types

- [x] Implement setup IDs, enrollment, proposal, approval, manifest, canonical
  hashing, validation, and redacted diagnostics in `cdk-common`.
- [x] Derive setup-only federation config, keyset spec, ceremony ID, and FROST
  root policy from one validated manifest.
- [x] Add the required setup authorization hash to public config validation.
- [x] Add compile-time feature isolation so standalone builds do not acquire
  federation setup behavior.

**Tests:** canonical reordering, field tampering, wrong signer, missing/extra
approval, duplicate identity/URL/member ID, unsafe threshold, incompatible
version, oversized input, redacted debug, serialization round trip, and exact
derived-config equality.

**Completed 2026-08-01:**

- Added the feature-gated setup domain in
  `crates/cdk-common/src/federation/setup/mod.rs`, including bounded and
  versioned invitations, signed enrollments, canonical proposal publication,
  unanimous approvals, certified manifests, URL and identifier validation,
  exact protocol tuples, FROST application policy, and redacted invitation
  diagnostics.
- Setup proposals canonicalize keysets and member records, reject zero or
  duplicate setup bindings, use domain-separated binary transcripts, and
  derive the setup authorization and DKG ceremony ID independently. The frozen
  2-of-3, 3-of-5, and 64-member vectors now exercise the production encoder and
  signature validators rather than only the reference implementation.
- `FederationConfig` now requires `setup_authorization` during construction and
  deserialization, includes it in federation-ID derivation and therefore in
  full config validation and config digests, and preserves it when final
  configs are assembled. Digest fixtures affected by that deliberate identity
  change were regenerated.
- Added `crates/cdk-common/tests/federation_setup_domain.rs` with eight focused
  cases covering canonical reordering, proposal/manifest/approval tampering,
  wrong signers, missing and extra approvals, duplicate identity/URL/member and
  invitation bindings, unsafe thresholds, incompatible versions, size limits,
  expiry behavior, secret redaction, strict serialization, mandatory setup
  authorization, and exact manifest-derived configuration.
- Verified with `cargo test -p cdk-common --features federation` (480 passed,
  4 ignored), `cargo clippy -p cdk-common --features federation --all-targets -- -D warnings`,
  `cargo check -p cdk-common --no-default-features`, and
  `cargo check --workspace --all-targets`. All changed Rust sources passed
  targeted `rustfmt`; the Milestone 0 note about the unusable workspace-wide
  format check remains applicable.
- The existing file-mediated `federation dkg-roster` command receives an
  explicit temporary authorization so it remains buildable until its removal
  in Milestone 7. Test-only trusted-dealer setup uses a deterministic,
  domain-separated authorization until Milestone 7 replaces those callers with
  strict signed-manifest fixtures. DKG record/message binding to the manifest
  remains intentionally deferred to Milestone 5.

### Milestone 2 — Durable setup storage

- [x] Add setup persistence and compare-and-set lifecycle APIs to the shared
  mint database boundary.
- [x] Implement direct final-schema SQLite and PostgreSQL tables and row
  conversion.
- [x] Add exact-replay idempotence and structured conflicts for every record.
- [x] Persist the local approval fence before any transmission can occur.
- [x] Store only capability hashes and prove plaintext invitation credentials
  never enter public payloads or diagnostics.

**Tests:** run the complete backend-generic setup persistence suite against
SQLite and PostgreSQL, including transaction rollback at every multi-write
boundary and concurrent consumption/approval races.

**Completed 2026-08-01:**

- Added secret-free durable invitation and activation records plus explicit
  coordinator/participant lifecycle state in
  `crates/cdk-common/src/federation/setup/persistence.rs`. Every persisted
  timestamp fits the common signed-64-bit SQL range, lifecycle transitions are
  forward-only compare-and-set operations, and one transition cannot advance
  both local lifecycles.
- Added `FederationSetupTransaction` and `FederationSetupDatabase` to the shared
  mint database boundary. Causal helpers atomically consume an invitation with
  its enrollment, publish only from the enrollment-complete coordinator phase,
  verify an exact signed proposal before persisting the local approval fence,
  store a certified manifest with its lifecycle transition, and persist an
  activation reference with the final local transition. Multi-write helpers
  deliberately write the immutable artifact/fence before their compare-and-set
  so a failed second write rolls the complete transaction back and the public
  command has one durable boundary to call before transmitting an approval.
- Added eight direct final-schema setup tables to the still-undeployed initial
  federation schema for both SQLite and PostgreSQL. The SQL implementation
  stores typed lookup/conflict coordinates and bounded JSON records, accepts
  byte-identical replay, reports `SetupPersistenceConflict` for every divergent
  record coordinate, and exposes deterministic ordered reads. Migration digest
  fixtures and fresh-schema table/index assertions were updated to lock the
  intended final schema; no compatibility migration or legacy setup format was
  introduced.
- Added the backend-generic persistence, rollback, and concurrency suite in
  `crates/cdk-common/src/database/mint/test/federation_setup.rs`. It covers all
  record classes, exact replay, divergent conflicts, reads, the complete local
  lifecycle, rollback at invitation/proposal/approval/manifest/activation
  boundaries, and simultaneous conflicting invitation consumption and approval
  fencing. It also proves the stored invitation JSON and `Debug` output contain
  only the capability hash, never the plaintext capability.
- Verified the complete setup suite against SQLite and a configured local
  PostgreSQL test database (`3 passed` on each backend), fresh final-schema
  migrations on both backends (`1 passed` each),
  `cargo test -p cdk-common --features federation` (`481 passed`, `4 ignored`),
  `cargo test -p cdk-sql-common --features federation` (`12 passed`), and
  `cargo test -p cdk-sqlite --features federation` (`175 passed`). Strict
  Clippy passed for `cdk-common`, `cdk-sql-common`, `cdk-sqlite`, and
  `cdk-postgres` with federation tests enabled; the federation-disabled
  `cdk-common` check and `cargo check --workspace --all-targets` also passed.
  All changed Rust sources passed targeted `rustfmt`; the Milestone 0 note
  about the unusable workspace-wide format check remains applicable.
- Local identity generation with protected private-file persistence,
  invitation replacement, public CLI commands, and the guarantee that those
  commands call the durable fence before network transmission remain
  Milestone 3 work. Production HTTPS policy remains at the command/service
  boundary in Milestones 3 and 4; storage accepts insecure localhost records
  solely so the specified local integration tests can use the same persistence
  implementation.

### Milestone 3 — Local invitation, proposal, and approval commands

- [ ] Implement protected local identity and invitation generation.
- [ ] Implement atomic replacement of an unconsumed invitation while refusing
  replacement after enrollment.
- [ ] Implement strict invitation parsing and URL/expiry/setup-identity checks.
- [ ] Implement explicit proposal publication that freezes the complete
  enrollment and shared-policy set.
- [ ] Implement proposal rendering with one stable human-review format and
  bounded machine-readable output.
- [ ] Implement exact-hash non-interactive approval and the durable
  one-proposal-per-setup fence.
- [ ] Ensure ambiguous command retries reuse identical identity, enrollment,
  and approval bytes.

**Gate:** two independent work directories can generate, exchange through an
in-memory fake coordinator, review, and approve one proposal without copying
private material.

### Milestone 4 — Bounded coordinator service

- [ ] Implement the setup-only Axum router, invitation authentication,
  enrollment consumption, proposal publication, approval collection, manifest
  certification, and status.
- [ ] Add strict request/response limits, deadlines, rate limits, no-store
  headers, redirect rejection, and redacted structured errors.
- [ ] Keep local lifecycle actions off the network router.
- [ ] Add cancellation ownership so timed-out requests cannot leave overlapping
  uncancellable storage or signature work.

**Tests:** real localhost HTTP with network permission, invalid/expired/replayed
invitations, concurrent enrollment, wrong setup identity, signature failure,
proposal equivocation, approval conflict, slow client, oversized body, storage
failure, rate saturation, and clean shutdown.

### Milestone 5 — Certified-manifest handoff to DKG

- [ ] Require a validated complete manifest before participant-state
  generation or DKG readiness.
- [ ] Bind setup authorization into BLS and FROST records, signatures,
  transcripts, activation, export, and startup.
- [ ] Derive all shared DKG inputs from the manifest and remove arbitrary
  production overrides.
- [ ] Bump affected undeployed protocol versions and regenerate fixtures.
- [ ] Prove every member starts its own driver concurrently after certification;
  no member ID is a protocol coordinator.

**Tests:** existing DKG unit/route/restart/partition/equivocation suites plus
wrong-manifest, same-roster/different-policy, cross-setup replay, mismatched
authorization, and coordinator-disappears-after-all-members-persist-the-manifest
cases.

### Milestone 6 — Finalization and explicit activation UX

- [ ] Surface existing DKG progress through setup status without duplicating
  DKG state.
- [ ] Export public/private final configs concurrently with bounded local jobs,
  then validate every local output against the certified manifest and final
  transcript.
- [ ] Implement atomic, idempotent local activation and stable service-manager
  output.
- [ ] Require explicit local activation and remove any setup coordinator
  ability to finalize or restart a remote member.
- [ ] Add encrypted-backup guidance before real-funds activation.

**Tests:** restart before every setup and DKG phase, activation before ready,
double activation, final-config mismatch, partial export failure, interrupted
activation transaction, and restart into the exact activated config.

### Milestone 7 — Remove parallel experimental setup paths

- [ ] Move reusable identity/config construction into shared internal helpers.
- [ ] Remove the standalone production `dkg-member`, `dkg-roster`, and
  `dkg-bind` workflow and raw ceremony/keyset auto-start configuration.
- [ ] Update local generators, Nocean tooling, examples, fixtures, and docs to
  consume a signed setup manifest.
- [ ] Regenerate experimental federation databases/configs rather than adding
  compatibility code.
- [ ] Verify feature combinations without federation and without individual
  payment backends.

**Gate:** repository search finds no production DKG entry point that can start
without certified setup authorization; test-only trusted-dealer generation is
feature-gated and produces the current strict config shape.

### Milestone 8 — Adversarial local process matrix

- [ ] Add 3-, 5-, 11-, and maximum-member process runs using the same CLI and
  HTTP workflow intended for separate machines.
- [ ] Exercise reordered, duplicated, delayed, dropped, and ambiguously
  acknowledged bootstrap messages.
- [ ] Exercise a malicious coordinator presenting different proposals, a
  member attempting two approvals, member loss before/after certification,
  setup-service restart, DKG restart, and bounded resource saturation.
- [ ] Record setup wall time, per-process CPU/RSS, request counts/failures,
  retry counts, status latency, and DKG phase timing without secret material.
- [ ] Confirm standalone CDK and wallet behavior remain unchanged.

**Gate:** every successful run produces one proposal hash, setup authorization,
DKG transcript hash, final config digest, and public config across all members;
every adversarial run either recovers idempotently or aborts with a bounded
actionable error.

### Milestone 9 — Real multi-host deployment drill

- [ ] Add or update a Nocean deployment script, using
  `scripts/deploy_nocean_federation.sh` as the build, transfer, and health-check
  reference, that drives only the new invitation-based setup CLI.
- [ ] Deploy at least five members on the Nocean servers with real HTTPS
  endpoints and identities independently generated inside their respective
  operator environments. Where multiple members temporarily share a physical
  test host, keep them in independent tmux sessions with separate state and do
  not count that topology alone as proof of separate-machine behavior.
- [ ] In those tmux sessions, execute the ordinary operator sequence: creator
  setup and invitation export; independent joins; proposal review and
  publication; one approval per operator; concurrent DKG; final-config review;
  and explicit local activation.
- [ ] Exchange only invitations and protocol messages; prohibit shared private
  directories and centralized private-config generation.
- [ ] Complete approval, DKG, explicit activation, wallet smoke, coordinated
  restart, one-member restart/catch-up, and public-config audit.
- [ ] Exercise coordinator loss after every member has persisted the certified
  manifest and confirm DKG/runtime independence.
- [ ] Exercise failed pre-certification setup and operator-mediated replacement
  with a new setup ID.
- [ ] Perform encrypted backup and restore for one member without transferring
  another member's secrets.

**Gate:** the Nocean deployment completes from empty member state through mint
and wallet smoke tests using only the public CLI workflow, and publishes a
redacted drill report with exact code revision, topology, tmux session layout,
TLS model, commands, durations, resource measurements, failures, recovery, and
remaining limits. The report must distinguish a multi-machine run from any
single-machine isolated-session rehearsal. This closes only the multi-host
setup gate; it does not waive the parent plan's independent security,
payment-backend, or real-funds gates.

## Verification matrix

At minimum, implementation must cover:

| Boundary | Required evidence |
|---|---|
| Canonical domain | Fixed vectors, reordered input equality, every-field tamper rejection |
| Invitations | Entropy, expiry, one-use atomicity, replay/conflict, credential redaction |
| Enrollment | Identity proof, slot binding, URL policy, version compatibility |
| Proposal | Uniqueness, safe thresholds, complete policy, immutable-after-approval |
| Approval | Exact hash, all-member certificate, double-sign fence, coordinator equivocation |
| Persistence | SQLite/PostgreSQL parity, restart, concurrent races, rollback injection |
| HTTP | TLS identity, no redirects, limits, deadlines, rate saturation, cancellation |
| DKG handoff | Manifest required, cross-setup rejection, full existing DKG regression suite |
| Activation | All final confirmations, local-only authority, atomic idempotent restart |
| Feature isolation | Standalone builds/tests and federation-disabled HTTP/config rejection |
| Deployment | Same-host process matrix plus the CLI-driven Nocean independent-session and real multi-host drills |

Normal verification for touched milestones includes:

```bash
cargo fmt --all -- --check
cargo test -p cdk-common
cargo test -p cdk-sqlite
cargo test -p cdk-postgres
cargo test -p cdk-axum federation_setup --features federation
cargo test -p cdk-mintd federation_setup --features federation,sqlite
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

Use the repository's actual feature combinations when implementing each
milestone; the command shapes above are planning targets, not permission to
weaken or skip existing suites. PostgreSQL tests require an available instance.
Run localhost-listener tests with network permission from the first attempt as
required by the parent contract.

## Observability and operator diagnostics

Expose bounded metrics for:

- setup sessions by non-secret phase;
- available/consumed/expired invitation counts;
- enrollment and approval counts;
- setup request attempts by route and outcome;
- proposal and authorization fingerprints;
- setup-driver retries and last successful transition; and
- handoff time from certification to each existing DKG phase.

Logs may include setup ID, member ID, phase, bounded counts, shortened public
fingerprints, and structured failure kind. They must not include invitation
credentials, private identity material, full signatures, DKG shares, encrypted
private messages, local paths, database URLs, TLS secrets, or operator PII.

Status must distinguish `waiting for member N`, `proposal requires local
approval`, `manifest certified`, `DKG waiting in phase P`, and `ready for local
activation`. Do not hide a stuck member with indefinite retry or arbitrary
timeout increases.

## Failure and recovery policy

- Lost invitation before enrollment: revoke locally and create a fresh
  invitation for the same unclaimed slot.
- Ambiguous enrollment or approval response: retry byte-identically.
- Wrong enrollment before proposal: reject or explicitly abort it while no
  approval exists; never overwrite a consumed credential with different data.
- Any proposal change after approval: abort and create a new setup ID.
- Coordinator loss before every member stores the certified manifest: restart
  it from durable state or abort and create a new setup ID.
- Coordinator loss after every member stores the certified manifest: continue
  from local manifests without it.
- Member loss during DKG: use the existing bounded abort; agree a new roster and
  start a new setup and ceremony ID.
- Local database or identity loss: restore that member's own protected backup
  or abandon the setup. Never reconstruct it from another member.
- Conflicting signed setup artifacts: preserve bounded evidence, fail closed,
  and require operator investigation.

There is no automatic member exclusion, approval revocation, proposal winner,
coordinator election, DKG downgrade, trusted-dealer fallback, or partial
activation in version 1.

## Acceptance criteria

This plan is complete only when:

- five independently administered members can complete setup by exchanging
  invitations and protocol traffic, without manually transferring config JSON
  or any private material;
- every member persists and validates one unanimously signed proposal before
  generating DKG participant state;
- a malicious coordinator cannot certify a proposal that any member did not
  sign, make one member sign two proposals for a setup ID, or learn member
  private keys or DKG shares;
- coordinator loss after every member durably stores the certified manifest
  does not affect DKG or runtime;
- every setup and DKG phase is restartable or aborts boundedly with an
  actionable member/phase diagnostic;
- all members export the same setup authorization, transcript, final config
  digest, and public config while retaining distinct private shares;
- activation is explicit, local, atomic, and impossible before all-member final
  confirmation;
- SQLite and PostgreSQL behavior is identical, standalone CDK remains
  unchanged, and federation-disabled builds acquire no setup surface;
- obsolete undeployed setup formats and parallel production entry points are
  removed rather than supported indefinitely; and
- the CLI-driven Nocean drill, real multi-host evidence, and the parent plan's
  remaining release gates are reported independently and accurately.

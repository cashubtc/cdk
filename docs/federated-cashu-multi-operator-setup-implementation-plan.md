# Federated Cashu multi-operator setup implementation plan

> **Status — Milestone 6 complete, 2026-08-01:** The pure setup domain,
> durable SQLite/PostgreSQL state, protected local identities and invitations,
> immutable proposal review, exact-hash approval fence, and bounded setup-only
> coordinator HTTP service and authenticated client are implemented and
> verified. DKG now starts only from a locally persisted certified manifest,
> all BLS/FROST artifacts bind its setup authorization, and member drivers run
> independently and concurrently. Setup status now derives DKG progress from
> the existing durable records, final config publication is atomic and locally
> validated, and activation is an explicit idempotent operator action.
> Milestone 7's removal of the parallel experimental setup paths is next.
> This document
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

## Coding-agent goal prompt

Implement this plan milestone by milestone while following
`docs/federated-cashu-implementation-plan.md` as the governing contract. After
each milestone, run proportional checks, update this implementation plan with
the exact progress and verification evidence, and create a separate
conventional commit. Finish with a real Nocean deployment based on
`scripts/deploy_nocean_federation.sh`: use independent per-operator tmux
sessions and state, exercise the ordinary public CLI setup workflow, and prove
that every member enters DKG independently and concurrently without centralized
private configuration or member-by-member orchestration. A new deployment
script may be added for the invitation-based CLI flow.

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

- [x] Implement protected local identity and invitation generation.
- [x] Implement atomic replacement of an unconsumed invitation while refusing
  replacement after enrollment.
- [x] Implement strict invitation parsing and URL/expiry/setup-identity checks.
- [x] Implement explicit proposal publication that freezes the complete
  enrollment and shared-policy set.
- [x] Implement proposal rendering with one stable human-review format and
  bounded machine-readable output.
- [x] Implement exact-hash non-interactive approval and the durable
  one-proposal-per-setup fence.
- [x] Ensure ambiguous command retries reuse identical identity, enrollment,
  and approval bytes.

**Gate:** two independent work directories can generate, exchange through an
in-memory fake coordinator, review, and approve one proposal without copying
private material.

**Completed 2026-08-01:**

- Added the protected local setup application boundary in
  `crates/cdk-mintd/src/federation_setup_command.rs`. Creator and joining
  identities are generated inside their own state directories and written
  before any ambiguous network operation. Local config, secrets, and
  invitation files are bounded, non-symlink inputs and require private-file
  permissions on Unix. Debug formatting redacts identity secrets and
  invitation capabilities.
- Added the nested production CLI shape
  `federation setup create|invite|join|propose|review|approve`. Invitation
  credentials are accepted only through `--invitation-file`, never as a
  process argument. SQLite-backed creator create/invite/propose/review/approve
  commands are wired now. Joining-member submission, remote proposal fetch,
  and remote approval deliberately fail closed at the CLI boundary until the
  authenticated bounded coordinator client from Milestone 4 is available;
  their storage-first application operations are implemented behind the
  `FederationSetupCoordinator` boundary and exercised by the fake coordinator.
- Implemented the frozen `cashufedsetupA` URL-safe, unpadded Base64 invitation
  encoding with its six-byte domain-separated checksum. Parsing is bounded and
  rejects wrong prefixes, padding/alphabet errors, truncation, trailing bytes,
  bad public keys, corrupt checksums, unsupported versions, invalid URLs, and
  malformed setup bindings. Receipt-time validation separately enforces the
  caller's HTTPS/localhost policy and current expiry.
- Added atomic invitation replacement to the shared setup transaction API and
  common SQL implementation. Replacement requires a fresh ID, capability,
  later expiry, and the same available setup/member slot; the transaction
  refuses replacement if any enrollment already occupies that member slot.
  Exact replacement replay remains idempotent.
- Proposal publication now freezes the complete signed enrollment set and
  shared BLS, FROST, checkpoint, network, payment-observation, and protocol
  policy. Every operator validates its byte-identical local enrollment and
  pinned coordinator identity before review. The stable secret-free human
  review has a locked SHA-256 fixture, and the same model has bounded pretty
  JSON output for a future backend or web UI.
- Approval requires the exact displayed proposal hash. The signed approval and
  local participant transition commit atomically before transmission; a lost
  acknowledgement therefore retries the same durable bytes. Creator approval
  follows the same fence and is then inserted into the coordinator's received
  approval set.
- The gate test uses three independent operator directories and three separate
  SQLite databases (stronger than the required two-directory minimum). It
  replaces an unused invitation, refuses replacement after enrollment,
  simulates commit-then-lost-acknowledgement failures for enrollment and
  approval, proves secret/enrollment bytes are unchanged across retries,
  compares all three review documents, rejects a wrong hash before fencing,
  collects all three approvals, checks distinct identity keys, and verifies
  protected file permissions without copying another member's private state.
- Repaired the DKG-status test fixture to preserve its setup authorization when
  deriving the final config. This restores the federation-ID binding required
  by Milestone 1 instead of constructing an unrelated authorization in the
  test-only final config.
- Verification passed: the focused `cdk-mintd` setup-command suite (`2
  passed`), the setup CLI parser test, `cdk-common` with federation (`483
  passed`, `4 ignored`), `cdk-sql-common` (`12 passed`), `cdk-sqlite` (`176
  passed`), and the invitation-replacement test against a configured local
  PostgreSQL database (`1 passed`). The complete `cdk-mintd` federation suite
  with its required test-only setup feature passed serially (`229 passed`);
  the first parallel run exposed the existing process-global environment-test
  race (`3 failed` after one test left an invalid environment value and
  poisoned its lock), while `--test-threads 1` passed all tests.
- Strict Clippy passed for `cdk-common`, `cdk-sql-common`, and `cdk-sqlite` with
  federation targets, and for the `cdk-mintd` library after allowing only its
  three pre-existing unrelated lints (`field_reassign_with_default`,
  `too_many_arguments`, and `collapsible_match`). Federation-disabled
  `cdk-common`, the PostgreSQL-only `cdk-mintd` feature combination, and
  `cargo check --workspace --all-targets` passed. All changed Rust files passed
  targeted `rustfmt`; the Milestone 0 workspace-format limitation remains.

### Milestone 4 — Bounded coordinator service

- [x] Implement the setup-only Axum router, invitation authentication,
  enrollment consumption, proposal publication, approval collection, manifest
  certification, and status.
- [x] Add strict request/response limits, deadlines, rate limits, no-store
  headers, redirect rejection, and redacted structured errors.
- [x] Keep local lifecycle actions off the network router.
- [x] Add cancellation ownership so timed-out requests cannot leave overlapping
  uncancellable storage or signature work.

**Tests:** real localhost HTTP with network permission, invalid/expired/replayed
invitations, concurrent enrollment, wrong setup identity, signature failure,
proposal equivocation, approval conflict, slow client, oversized body, storage
failure, rate saturation, and clean shutdown.

**Milestone 4 implementation note (2026-08-01):**

- Added a setup-only Axum service with exactly five authenticated protocol
  routes: enrollment, proposal, approval, manifest, and bounded status. It
  validates the one-use invitation capability against durable metadata, pins
  the setup ID and coordinator identity, accepts exact enrollment and approval
  replays, rejects conflicting replays, and atomically stores the final
  approval, certified manifest, and coordinator phase. Invitation expiry blocks
  an unused slot but does not strand an operator that already consumed the
  invitation and is continuing the same ceremony.
- Added coordinator-signed response envelopes bound to the route, request
  correlation nonce, setup ID, protocol version, canonical response-body hash,
  and pinned setup identity. The client uses one nonce across bounded retries,
  disables redirects, enforces response limits and deadlines, validates every
  signed response, and never accepts ambient DNS or redirect identity as the
  trust root.
- Applied body and response limits, a whole-request timeout that includes body
  extraction, global/per-source/per-invitation concurrency admission, bounded
  weak-key admission maps, `Cache-Control: no-store` on success and failure,
  and stable redacted error codes. Timed-out operation tasks retain their owned
  permits until completion; shutdown rejects new work and waits for every owned
  task, closing the cancellation race without serializing DKG computation.
- Added `federation setup serve` as an isolated setup-only listener and made
  member `join`, `review`, `approve`, and `status` use the authenticated HTTP
  client. Member status atomically persists and validates a ready certified
  manifest. Proposal creation, creator review/approval, activation, abort, and
  service management remain local actions; no wallet, runtime, activation, or
  remote process-management route was added.
- The real-localhost setup suite passes (`6 passed`) and covers concurrent
  invitation ownership, exact replay, invalid/expired credentials, bad member
  signatures, proposal readiness and equivocation, coordinator-identity
  mismatch, approval conflict, complete certification, redirects, slow and
  oversized bodies, injected storage failure, admission saturation and stale
  key cleanup, no-store errors, timeout ownership, and clean shutdown. The
  ordinary three-operator HTTP workflow passes with independent directories and
  SQLite databases, concurrent member enrollment and approval, identical
  reviews/manifests, and local transition to `Certified`.
- `cdk-common` with federation passed (`484 passed`, `4 ignored`), the complete
  serial `cdk-mintd` federation suite passed (`230 passed`), and the final
  focused three-operator HTTP workflow passed after the timeout/admission
  hardening. The `cdk-axum` regression run with a 16 MiB test-thread stack
  passed `567` tests and exposed five failures in two older DKG test fixtures
  that rebuilt final configs with a hard-coded setup authorization; both
  fixtures now preserve the setup config's authorization, and all seven tests
  in the affected groups pass when rerun (`1 + 1 + 5`). The known deep AlephBFT
  test that overflows the platform's default test-thread stack passes with the
  larger stack.
- `cargo check --workspace --all-targets`, federation-disabled `cdk-common`,
  strict changed-crate Clippy, targeted `rustfmt`, and `git diff --check` pass.
  Clippy required explicit allowances only for pre-existing findings outside
  this milestone, and workspace-wide formatting remains blocked by the
  pre-existing unformatted sibling Bark dependency.

### Milestone 5 — Certified-manifest handoff to DKG

- [x] Require a validated complete manifest before participant-state
  generation or DKG readiness.
- [x] Bind setup authorization into BLS and FROST records, signatures,
  transcripts, activation, export, and startup.
- [x] Derive all shared DKG inputs from the manifest and remove arbitrary
  production overrides.
- [x] Bump affected undeployed protocol versions and regenerate fixtures.
- [x] Prove every member starts its own driver concurrently after certification;
  no member ID is a protocol coordinator.

**Tests:** existing DKG unit/route/restart/partition/equivocation suites plus
wrong-manifest, same-roster/different-policy, cross-setup replay, mismatched
authorization, and coordinator-disappears-after-all-members-persist-the-manifest
cases.

**Completed 2026-08-01:**

- Added a protected local manifest file and an independent 64-byte runtime seed
  to local setup state version 2. `setup status` persists the exact durable
  certificate locally; startup revalidates its unanimous signatures, proposal,
  URL policy, local enrollment/identity binding, and setup authorization before
  deriving any DKG input. Missing, conflicting, tampered, cross-setup, or
  locally mismatched manifests fail before participant-state generation.
- Made the manifest the only public construction path for
  `FederationDkgAutoStartConfig`. Its setup config, authorization, ceremony ID,
  BLS keyset policy, FROST root epoch, and secret-share transport policy are all
  derived from the certificate. `cdk-mintd` requires
  `federation.setup_config_path` while DKG is enabled, rejects simultaneous raw
  public/private configs, and rejects the old raw ceremony, replacement, and
  amount inputs. The direct DKG runner and auto-start fields are crate-private.
- Added setup authorization to durable BLS participant state, lifecycle,
  readiness, commitments, reveals, secret shares, local keyset shares, public
  ceremony, transcript requests/payloads/signatures, activation signatures,
  final member assembly, FROST DKG contexts/results, and exported FROST root
  configuration. The BLS participant/public-ceremony formats, commitment and
  transcript domains, private REST protocol, activation domain, FROST DKG
  protocol, and FROST public-root schema now use their strict undeployed v2
  forms; fixed vectors and fixtures were regenerated.
- Added an isolated post-certification router exposing only authenticated
  `/federation/v1/dkg/*` routes. Each member registers its own manifest-bound
  lifecycle record and spawns its own driver. The multi-member DKG harness now
  executes every member tick with `join_all`; there is no member-ID coordinator
  and the CPU admission semaphore remains machine-local, so it cannot serialize
  members on separate machines.
- The ordinary three-operator HTTP test now proves DKG is unavailable before
  certification, persists the certificate independently in all three stores,
  shuts the setup coordinator down, and only then derives identical public DKG
  context plus distinct local identities/lifecycle records. Domain tests prove
  same-roster/different-policy and cross-setup proposals derive different setup
  authorizations; readiness, FROST-context, activation, and signed-route tests
  reject mismatched authorization.
- Verification passed: setup domain and transcript vectors (`11 + 1` tests),
  `cdk-common` federation library (`465 passed`, `1 ignored`), the DKG-focused
  `cdk-axum` suite (`58 passed`) including restart/partition/equivocation and
  concurrent drivers, CDK mint federation regressions (`146 passed`), the
  complete serial `cdk-mintd` federation suite (`230 passed`), and the
  independent-operator setup suite (`3 passed`). The seven manifest-bound DKG
  persistence cases also pass against a live local PostgreSQL instance.
  Workspace all-target checking, the federation-plus-PostgreSQL/no-SQLite
  feature check, strict changed-library Clippy with explicit allowances only
  for recorded pre-existing findings, targeted `rustfmt`, and
  `git diff --check` pass.
- Version 1 intentionally starts exactly one initial BLS keyset ceremony, which
  matches the frozen singular ceremony-ID transcript and the public CLI. A
  multi-keyset initial ceremony needs a future versioned sub-ceremony
  derivation; accepting a multi-keyset manifest and silently reusing one
  ceremony ID is forbidden. `federation setup serve` selects coordinator-only
  or DKG-only mode at process start, so operators currently stop and relaunch it
  after local manifest persistence. Milestone 6 will make that handoff and DKG
  progress explicit in the finalization/status UX.

### Milestone 6 — Finalization and explicit activation UX

- [x] Surface existing DKG progress through setup status without duplicating
  DKG state.
- [x] Export public/private final configs concurrently with bounded local jobs,
  then validate every local output against the certified manifest and final
  transcript.
- [x] Implement atomic, idempotent local activation and stable service-manager
  output.
- [x] Require explicit local activation and remove any setup coordinator
  ability to finalize or restart a remote member.
- [x] Add encrypted-backup guidance before real-funds activation.

**Tests:** restart before every setup and DKG phase, activation before ready,
double activation, final-config mismatch, partial export failure, interrupted
activation transaction, and restart into the exact activated config.

**Completed:** `federation setup status` now returns one stable local read model
covering setup lifecycle, missing enrollment/approval slots, live DKG phase and
bounded record counts, confirmed members, the DKG-confirmed BLS digest, the
complete BLS-plus-FROST final digest, and local activation. After a manifest is
persisted this status is entirely local and continues to work after the setup
coordinator is stopped; it reads the existing DKG and FROST tables and stores
no duplicate progress state. Registering the manifest-derived ceremony now
atomically advances the participant lifecycle to `dkg_running`, and observing
complete signed records advances it to `ready_to_activate`.

`federation setup activate --config ...` assembles the final BLS config only
after every existing DKG activation confirmation is valid, attaches the exact
manifest-authorized FROST root and applications, revalidates the final public
and member-private pair against the certificate and transcript, and publishes
both files through two bounded local jobs into a protected staging directory.
The complete pair is re-read and validated before one atomic directory rename.
The secret-free final digest reference and `activated` lifecycle transition are
then committed in one database transaction. A crash between the filesystem
rename and database transaction leaves a validated but inactive pair; exact
replay revalidates it and completes activation. Partial staging output is never
published, conflicting existing output fails closed, and an exact second
activation returns the original durable result. Stable key-value stdout tells
a service manager to use the final public/private paths, unset the setup config,
disable DKG, and perform a local restart. There is still no HTTP create,
finalize, activate, or restart route.

The setup-only mint uses an isolated in-memory placeholder signatory because
the general `Mint` constructor requires an active signatory keyset. Its seed is
member-local, it never enters the durable mint key store, and the setup router
still exposes only authenticated DKG routes. Local config format version 3
adds the fixed activated-config directory without compatibility reads. The
activation command prints encrypted-backup guidance to stderr before any
real-funds handoff while keeping stdout stable for service managers.

**Evidence:** the ordinary three-operator HTTP workflow now retires the setup
coordinator, starts three manifest-derived DKG routers concurrently against
separate SQLite stores and identities, completes BLS plus FROST, reports local
readiness, rejects pre-DKG activation, recovers an interrupted activation
transaction and a partial concurrent export, returns identical double-
activation output, and restarts into one identical public config plus each
member's protected private config. The focused finalization/status suite passes
`19` tests, including atomic-pair mismatch and partial-export cases; the setup
CLI parser includes the local `activate` command. The all-feature `cdk-mintd`
check, workspace all-target check, federation-plus-PostgreSQL/no-SQLite feature
check, targeted formatting, and strict `cdk-mintd` Clippy with only recorded
pre-existing lint allowances pass. The serial normal-feature suite passes the
M6 workflow and `227` of `229` total tests; the two remaining startup tests use
trusted-dealer fixtures without enabling `federation-test-utils` and are now
correctly rejected by the pre-existing production trusted-dealer guard. Their
fixture regeneration/removal belongs to Milestone 7. The all-feature CLI parser
also remains subject to its pre-existing SQLCipher `--password` requirement,
so the setup parser is verified with the normal `federation` feature profile.

### Milestone 7 — Remove parallel experimental setup paths

- [x] Move reusable identity/config construction into shared internal helpers.
- [x] Remove the standalone production `dkg-member`, `dkg-roster`, and
  `dkg-bind` workflow and raw ceremony/keyset auto-start configuration.
- [x] Update local generators, Nocean tooling, examples, fixtures, and docs to
  consume a signed setup manifest.
- [x] Regenerate experimental federation databases/configs rather than adding
  compatibility code.
- [x] Verify feature combinations without federation and without individual
  payment backends.

**Gate:** repository search finds no production DKG entry point that can start
without certified setup authorization; test-only trusted-dealer generation is
feature-gated and produces the current strict config shape.

**Implemented:** the file-mediated production commands and their implementation
module are deleted. `FederationDkg` now accepts only bounded driver tuning and
rejects every retired ceremony, keyset, replacement, and URL-policy field via a
strict schema; a certified local setup manifest is the only production source
of shared DKG inputs. The reusable setup FROST-policy constructor commits both
CDK and optional Bark application policy before approval, so deployment tooling
does not mutate a finalized config after DKG.

The feature-gated trusted-dealer utility now emits only final strict public and
private configs plus an optional final-config runtime smoke. Its setup-only
configs, route probes, live-DKG config suffix, duplicate DKG smoke, and FROST
templates were removed rather than supported through compatibility reads. The
two general startup tests that intentionally consume trusted-dealer material
are now correctly scoped to `federation-test-utils`; the production guard test
continues to reject that setup method.

Local and Nocean control tooling now executes `setup create`, protected
invitation joins, proposal review, exact approval, per-member manifest
persistence, concurrent `setup serve` DKG processes, and explicit local
activation. The Nocean Bark renderer inserts its application policy into the
signed proposal. Runtime, benchmark, and health-check paths consume each
operator's activated directory. Historical route-probe/live-DKG Just recipes
were removed and replaced by one `fed-regtest-setup` recipe using the ordinary
CLI workflow.

**Evidence:** the new local script completed a three-operator run from empty,
separate state directories and SQLite databases through certification,
simultaneous DKG services, `3/3` finalized readiness, explicit activation, and
byte-identical public configs. The production `federation,sqlite` serial suite
passes `160` tests, including the ordinary HTTP workflow and the strict removed-
field test. The final-only trusted-dealer groups pass `3 + 4` tests and the
signed Bark-policy construction test passes. Checks pass for a federation-
disabled `sqlite,fakewallet` mint, setup-only `federation,sqlite`, and
`federation,postgres` without SQLite or a Lightning backend. The Nocean/local
scripts and rendered Bark control pass syntax checks, `just --list` parses, the
all-feature test targets compile, targeted formatting and `git diff --check`
pass, and strict all-feature `cdk-mintd` Clippy passes with explicit allowances
only for the recorded pre-existing findings in dependency crates.

### Milestone 8 — Adversarial local process matrix

- [x] Add 3-, 5-, 11-, and maximum-member process runs using the same CLI and
  HTTP workflow intended for separate machines.
- [x] Exercise reordered, duplicated, delayed, dropped, and ambiguously
  acknowledged bootstrap messages.
- [x] Exercise a malicious coordinator presenting different proposals, a
  member attempting two approvals, member loss before/after certification,
  setup-service restart, DKG restart, and bounded resource saturation.
- [x] Record setup wall time, per-process CPU/RSS, request counts/failures,
  retry counts, status latency, and DKG phase timing without secret material.
- [x] Confirm standalone CDK and wallet behavior remain unchanged.

**Gate:** every successful run produces one proposal hash, setup authorization,
DKG transcript hash, final config digest, and public config across all members;
every adversarial run either recovers idempotently or aborts with a bounded
actionable error.

**Progress:** complete. `scripts/run_federation_setup_matrix.sh` now drives
release-mode 3-, 5-, 11-, and 64-member profiles through the ordinary
invitation CLI and independent SQLite/state directories. The underlying
`scripts/run_test_federation_dkg.sh` supports reverse and delayed enrollment,
exact join/proposal/approval replay, a missing pre-certification member and
invitation replacement, coordinator restart, concurrent review pressure, a
held certified member, and one-member DKG restart. It records a redacted JSON
summary plus compact phase samples, including bootstrap/DKG/activation/total
wall time, request/failure/retry counts, maximum status latency, and per-process
CPU/RSS. Signal cleanup now includes transient status, saturation, and
activation children.

The process profiles completed at `3/2`, `5/3`, `11/8`, and the protocol
maximum `64/43`. Every completed profile produced one proposal hash and setup
authorization, one manifest and DKG transcript hash, one final config digest,
and byte-identical public config files across all operator stores. The
three-member adversarial release run completed in `5s` total with `10` status
requests and about `95 MB` peak aggregate DKG RSS. Earlier diagnostic runs
completed the five-member profile in `22s` and the eleven-member restart/stress
profile in `119s`. These timings are machine- and build-dependent, so they are
evidence for this run rather than service-level objectives.

The 64-member ceiling run exposed and then recovered from the intended hard
conditions rather than hiding them. A colocated debug run saturated `128`
processes at about `6.7 GiB` RSS and was stopped in favor of an optimized
binary. The optimized run reached full activation after durable coordinated
and one-member process restarts without changing its setup or ceremony ID or
moving private material. Its stress summary records `6,609s` from concurrent
DKG start through final validation, `1,218s` maximum observer latency, about
`4.9 GB` sampled DKG RSS, and about `107 MB` maximum sampled member RSS. Much of
that duration and `7,744` status requests came from the first runner version
starting 64 cryptographic status validators on every poll. Pausing those
observers while leaving all 64 DKG services concurrent allowed activation
confirmations to converge immediately. The committed runner therefore polls
one readiness sentinel during DKG and performs one all-member validation sweep
only after readiness, while the maximum profile supplies a generous but still
bounded `28,800`-tick/`7,200s` driver invocation. Setup `serve` exposes those
bounded tick, delay, and wall-time controls explicitly. Per-peer delivery
remains bounded for transport/DoS safety, but there is no member-order or
process serialization in the DKG protocol.

Deterministic HTTP/domain tests cover the cases that cannot safely be forged
through the ordinary operator CLI: coordinator equivocation, alternate approval
attempts, deliberately lost enrollment/approval acknowledgements, exact replay,
deadline ownership, saturation, storage failure, redirect refusal, invitation
expiry, and inactive-admission cleanup. All six setup HTTP tests pass with
loopback access, and the independent-operator command test passes with
same-clock-second invitation replacement. Federation-disabled
`cdk-mintd` (`sqlite,fakewallet`), wallet-only `cdk`, and `cdk-cli` checks pass.
The wallet-only *unit-test* target remains blocked by a pre-existing unrelated
`MockTransport` implementation missing `resolve_dns_txt`; the wallet-only
library and CLI compile checks are the isolation evidence for this milestone.
Likewise, `cdk-axum --features federation` test compilation has a pre-existing
unguarded reference to Bark-only test helpers, so the setup HTTP suite was run
and passed with all features. The complete production `federation,sqlite`
`cdk-mintd` suite passes `160/160`, all-feature test targets compile, targeted
formatting and shell syntax checks pass, and strict all-feature `cdk-mintd`
Clippy passes with only the already-recorded dependency allowances.

### Milestone 9 — Real multi-host deployment drill

- [x] Add or update a Nocean deployment script, using
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

**Progress:** in progress. The deployment tooling is implemented as
`scripts/deploy_nocean_multi_operator_federation.sh` with two deliberately
operator-local helpers. It accepts exactly five SSH operator environments,
checksum-verifies the Linux binaries, creates a protected directory and named
tmux session per operator, and runs the ordinary create, invitation-file join,
propose, review, approve, status, concurrent serve, and explicit activate
commands. Only protected invitations cross the deployment driver; each helper
creates its own setup identity, SQLite state, runtime seed, private config, and
encrypted backup locally. The driver also encodes the failed pre-certification
replacement, coordinator-loss, wallet, one-member catch-up, coordinated
restart, public-config comparison, resource sampling, and redacted-report
checks required by this milestone.

The topology preflight hashes (but neither logs nor records) the remote machine
identity and defaults to requiring five unique machines before any build,
upload, session stop, or remote write. On 2026-08-01 the configured Nocean
access exposed five public TLS names but only one reachable SSH machine, with
no SSH fan-out or container-cluster worker topology. The strict preflight
therefore aborted as designed with `1 < 5` before mutating the deployment.
Setting `REQUIRE_UNIQUE_MACHINES=1` permits the requested five-session isolated
rehearsal on that machine, but the generated report must call it a single-host
rehearsal and cannot close the multi-host checkbox or gate. Shell syntax,
executable-mode, help-surface, whitespace, and credential/PII scans pass for
all three new scripts. The live isolated-session drill and a real five-machine
run remain outstanding.

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

# Federated Cashu DKG implementation plan

This document is a standalone plan for replacing the federated Cashu trusted-dealer key setup with a production distributed key generation ceremony. It is intentionally scoped to DKG, DKG output validation, config integration, and later key rotation. The main federated mint implementation should continue using trusted-dealer shares until minting, swapping, melting, consensus, catch-up, and wallet share aggregation are working end to end.

## Goal

Federated Cashu should eventually let a fixed set of mint members create threshold BLS keysets without any member or setup machine ever learning the aggregate private key. Each mint member should generate and store only its own private BLS secret shares, learn the public shares of its federation partners, and agree on the same aggregate Cashu v3/BLS public keysets. The resulting federation config must be reproducible, auditable, transcript-bound, and safe to activate only when every honest member can verify that the DKG transcript matches the configured roster, thresholds, member identity public keys, Cashu keyset metadata, and BLS public shares.

## Design posture

- Keep trusted-dealer setup for the federation MVP.
- Add DKG only after the federation has working consensus, catch-up, private peer authentication, local multi-mint harnesses, and wallet threshold share aggregation.
- Use Fedimint as the primary reference for the operational model: fixed roster, setup ceremony, authenticated peers, per-member private config, shared public config, and threshold BLS keys.
- Prefer a simple CDK-native DKG implementation over copying unnecessary Fedimint complexity.
- Treat DKG as a setup and key-rotation protocol, not as part of the hot minting, swapping, or melting path.
- Keep the interface to existing CDK code thin: DKG should produce the same public and private federation config shapes that trusted-dealer setup already produces.
- Federation DKG is v3/BLS-only. It must only create Cashu keysets represented by `KeySetVersion::Version02`, serialized with keyset ID prefix `02`.

## Fedimint reference status

The expected Fedimint source area to review before implementation is its server config generation and G2 DKG code, especially the path commonly referenced as `fedimint-server/src/config/dkg_g2.rs`, plus the surrounding config generation and peer-authenticated setup code.

At the time this document was created, the local Fedimint checkout available to this workspace did not contain a populated working tree, so this plan records the intended Fedimint-inspired design but does not claim a fresh local source verification. Before implementing the CDK DKG milestone, populate the Fedimint checkout and do a focused source review of:

- DKG message flow and round structure.
- Peer identity and authentication model during config generation.
- How Fedimint binds DKG output into private and public configs.
- How it encodes final transcripts and handles restart or failure.
- How threshold BLS public shares and aggregate public keys are validated.
- What code can inspire CDK directly and what should stay out because CDK values simplicity.

## Starting point in CDK

Current federated Cashu work already has the right shape for a DKG replacement path:

- `crates/cashu/src/nuts/nut01/bls.rs` contains BLS scalar, G1, G2, threshold signer ID, secret share, public share, signature share, trusted-dealer key generation, share verification, public share aggregation, and blind signature share aggregation helpers behind the `federation` feature.
- `crates/cdk-common/src/federation.rs` defines static federation config types, member IDs, threshold params, aggregate keysets, member public keys, public BLS shares, and private BLS shares.
- `crates/cdk-common/src/federation/trusted_dealer.rs` currently generates the public federation config plus one private member config per mint from trusted-dealer polynomial coefficients.
- Member identity keys are already distinct from BLS signing shares and can be used to authenticate DKG messages.

The DKG implementation should reuse those types where possible and should ultimately output the same high-level data as `generate_trusted_dealer_configs`: one shared `FederationConfig` plus one `MemberConfigPrivate` per member.

## Non-goals for the first DKG version

- No dynamic federation membership.
- No asynchronous or research-grade adversarial DKG as the first production target.
- No DKG during normal wallet mint, swap, melt, restore, or check-state requests.
- No automatic member replacement if a member fails during setup.
- No attempt to support v1/v2 or secp256k1 federation keysets.
- No real-money activation until the DKG transcript, config validation, backup, and key rotation paths have been audited.

## Required security properties

- No participant learns the aggregate private key.
- No participant learns another member's final private share.
- Every DKG message is authenticated by a federation identity key from the configured roster.
- Unknown identities, duplicate member IDs, duplicate signer IDs, and malformed shares are rejected.
- Every secret share is bound to the member ID that receives it.
- Every public share is bound to the member ID that owns it.
- The final transcript binds federation ID, roster hash, threshold params, keyset version, units, amounts, aggregate keys, member public shares, the wallet protocol version, and DKG session ID. Member-only protocol versions remain in their governing payloads.
- All honest members derive identical aggregate public keysets and identical public configs.
- Each honest member derives exactly one private share per amount per keyset.
- Failed, equivocal, or incomplete DKG ceremonies abort safely and do not create an ambiguous config.
- Restart either resumes a stored session deterministically or starts a new session with a new session ID; it must not mix transcripts.
- DKG output must be verifiable before any mint member starts serving federation signing traffic.

## Protocol model

Use a fixed-roster VSS-based DKG ceremony.

For each federated keyset and amount:

1. Each participant generates a random polynomial of degree `signature_threshold - 1` over the BLS12-381 scalar field.
2. Each participant broadcasts public commitments to its polynomial coefficients in G2.
3. Each participant privately sends every other participant the polynomial evaluation for that recipient's signer ID.
4. Each recipient verifies every received secret evaluation against the sender's public commitments.
5. Each member sums all valid received evaluations, including its own local evaluation, to derive its final secret share.
6. Each member derives its public share from the sum of verified commitments evaluated at its signer ID.
7. The aggregate public key for the amount is the sum of every participant's constant-term commitment.
8. The final keyset ID is derived from the aggregate BLS G2 public keys using the normal Cashu v3 keyset rules.

This is the straightforward fixed-roster path that matches CDK's current threshold BLS shape. It can be implemented as Feldman-style VSS first. If the Fedimint source review shows a simple Pedersen-style hiding layer that fits CDK cleanly, add it as an explicit design decision before coding. Do not silently change the transcript or verification model halfway through implementation.

## Failure model

The first production DKG should use an abort-on-valid-complaint policy:

- If a member receives an invalid private share, it records a signed complaint and the ceremony aborts.
- If a member equivocates by sending different commitments or final transcript signatures to different peers, the equivocation proof aborts the ceremony.
- If a member goes offline or times out before the required rounds complete, the ceremony aborts.
- A new ceremony can be started with a new session ID after operators resolve the failure.

Disqualifying bad dealers and continuing with a reduced participant set is a later enhancement. It complicates config semantics, threshold assumptions, and auditability. The first CDK version should favor a clean abort over ambiguous recovery.

### All-member availability is an operator-mediated setup requirement

The ceremony is `n-of-n` by design: every configured roster member must be
online and mutually reachable for the entire ceremony. The all-member
readiness barrier fails boundedly when any configured member withholds
(`federation DKG readiness exhausted after N attempts with X/Y members
ready`), and every ceremony phase — commitments, reveals, secret shares,
transcript signatures, and activations — requires all configured members.
This is inherent to the protocol, not an implementation accident: a member's
key share cannot be produced without that member's contribution, so a
`t-of-n` fault-tolerant federation still needs `n-of-n` availability during
setup.

Operators must not treat a stuck ceremony as retryable forever. When a
member is permanently unavailable:

1. Drain the failure from the bounded readiness/tick errors and per-member
   progress (`commitment/reveal/share/signature/activation counts` below the
   roster size).
2. Reconfigure the roster out-of-band: all remaining operators agree on a
   replacement public config whose `members` list excludes or replaces the
   failed member, with thresholds sized for the new roster.
3. Distribute and sign the new roster per the normal config process, then
   run a **new** ceremony with a **new ceremony id** and fresh participant
   state. Never re-run the stuck ceremony id against a silently changed
   roster: config, transcript, and roster mismatches must keep failing
   closed.
4. Keep the failed member's prior DKG artifacts only for incident analysis;
   they are never reused across rosters.

A signed in-band exclusion/restart protocol (the remaining members agree
and sign a reduced roster before activation) remains future work tracked as
finding `S1` in `docs/federated-cashu-consensus-deadlocks-2026-07-18.md`.
Until it exists, roster reconfiguration followed by a fresh ceremony is the
only supported recovery from a permanently unavailable DKG member.

## DKG data model

Add a new feature-gated module, preferably under `crates/cdk-common/src/federation/dkg/`, with narrowly scoped types:

- `DkgProtocolVersion`: version byte or enum for DKG transcript compatibility.
- `DkgSessionId`: 32-byte session ID derived from setup entropy plus roster hash, or randomly generated and signed by all members.
- `DkgContext`: canonical context for federation ID, roster hash, member IDs, identity public keys, threshold params, keyset specs, the wallet protocol version, and curve suite.
- `DkgKeysetSpec`: unit, active flag, input fee, optional final expiry, amounts, and keyset purpose.
- `DkgMemberCommitments`: member ID plus per-keyset/per-amount polynomial commitments.
- `DkgPrivateShare`: sender ID, recipient ID, keyset spec ID, amount, scalar evaluation, and context hash.
- `DkgShareEnvelope`: signed and encrypted private share message.
- `DkgComplaint`: signed report of invalid share, missing share, malformed message, or equivocation.
- `DkgAck`: signed acknowledgement that a member verified all shares and commitments.
- `DkgFinalMemberOutput`: member ID, derived public shares, aggregate keys, and config hash.
- `DkgFinalTranscript`: full public transcript, final public federation config, transcript hash, and member signatures.
- `DkgPrivateOutput`: local member private shares and the final transcript hash.

Keep secret scalar material out of debug output, logs, public transcript JSON, and error messages.

## Transcript binding requirements

The final transcript hash must commit to:

- DKG protocol version.
- Curve suite and signing suite.
- Cashu keyset version `KeySetVersion::Version02`.
- Federation ID.
- Roster hash.
- Ordered member IDs.
- Ordered federation identity public keys.
- Public mint URLs and private federation API URLs if they are part of the startup trust model.
- Signature threshold and consensus threshold.
- DKG session ID.
- Keyset specs: unit, active flag, input fee, final expiry, amount set.
- Per-member polynomial commitments.
- Per-member public BLS shares.
- Aggregate public BLS keys per keyset amount.
- Derived Cashu keyset IDs.
- Complaint-free completion status.
- Member signatures over the final transcript hash.

The transcript should avoid machine-specific paths, local IP addresses, private keys, secret shares, and temporary operator details.

## Message authentication and transport

Use the existing federation member identity keys as the DKG authentication roots:

- Every DKG message must include sender member ID, recipient member ID when applicable, federation ID, roster hash, DKG session ID, round, monotonic message sequence, body hash, and signature.
- A mint only accepts DKG messages from identity public keys in the configured federation roster.
- A mint rejects messages whose sender member ID does not match the signing identity key.
- Replay protection must reject duplicate sequence numbers and messages from old session IDs.
- Private share messages must be encrypted to the recipient or transported over a mutually authenticated private channel.

Transport strategy:

- First implementation: use the private federation REST transport if all members are reachable in the local or operator-managed network.
- Required abstraction: define a DKG transport trait so Iroh, Noise, or another NAT-traversing transport can be added later without changing the DKG state machine.
- Later production option: use Iroh-like networking for operators behind firewalls, following Fedimint's broad deployment motivation, while keeping the DKG message and transcript format unchanged.

## Storage and recovery

DKG needs durable local state because setup can span multiple members and failures:

- Store active DKG session metadata.
- Store sent and received public commitments.
- Store sent and received encrypted private share envelopes.
- Store local decrypted secret evaluations only in private storage.
- Store complaints and acknowledgement messages.
- Store final transcript and transcript hash.
- Store local `DkgPrivateOutput` before writing member config.
- Never store private shares in public federation config or public transcript.

Recovery rules:

- On restart, reload the active session by `DkgSessionId`.
- If all prior messages verify, resume from the last completed round.
- If local state is incomplete or corrupted, abort the session and require a fresh DKG.
- Never resume a session with a changed roster, threshold, keyset spec, or federation ID.

## Config integration

The DKG output must feed the same config validation path as trusted-dealer setup:

- Build `FederationConfig` from final aggregate keysets and public member data.
- Build one `MemberConfigPrivate` per member from local private shares.
- Reuse `FederationConfig::new` and existing `validate_against_public` checks.
- Add explicit validation that DKG-produced keysets are v3/BLS-only.
- Add explicit validation that DKG public shares aggregate to the aggregate public key for each amount.
- Add explicit validation that each member's private share derives to its public share.
- Store `dkg_transcript_hash` and `roster_hash` in the member config or adjacent federation metadata.
- Keep trusted-dealer output compatible for tests by optionally emitting a mock transcript with a distinct `trusted-dealer` origin field.

## Milestone 0: source verification and design lock

Objective: verify the Fedimint DKG reference and lock CDK's exact DKG protocol before coding.

Checklist:

- [ ] Populate the local Fedimint checkout with a valid working tree.
- [ ] Review Fedimint's G2 DKG implementation and setup/config generation flow.
- [ ] Write a short comparison note: what CDK copies, adapts, or deliberately avoids.
- [ ] Decide Feldman VSS vs Pedersen VSS for CDK DKG v1.
- [ ] Decide canonical encoding for DKG messages and transcripts.
- [ ] Decide private share encryption mechanism for REST transport.
- [ ] Decide whether all members must be online for DKG v1. Recommended: yes.

Tests and success criteria:

- [ ] The design note identifies exact Fedimint files and concepts reviewed.
- [ ] The selected DKG protocol has deterministic transcript hashing.
- [ ] The design does not require changes to wallet-to-mint Cashu REST APIs.
- [ ] The design outputs the same public/private config shapes as trusted-dealer setup.

## Milestone 1: DKG type skeleton

Objective: add versioned DKG data types without implementing cryptography yet.

Checklist:

- [ ] Add `crates/cdk-common/src/federation/dkg/` with separate files for context, messages, transcript, storage, and errors.
- [ ] Keep all new code behind the `federation` feature.
- [ ] Add structured `FederationDkgError` variants.
- [ ] Implement redacted `Debug` for all types containing secret material.
- [ ] Add serde serialization for public messages and transcript types.
- [ ] Add deterministic canonical hash helpers for context and final transcript.
- [ ] Add conversion from `DkgKeysetSpec` to federated keyset metadata.

Tests and success criteria:

- [ ] Serialization round-trips for every public DKG type.
- [ ] Secret share types never print scalar values in `Debug`.
- [ ] Transcript hash changes when roster, threshold, keyset version, amount set, or identity key changes.
- [ ] Non-v3 keyset specs are rejected.

## Milestone 2: VSS math primitives

Objective: implement and test the DKG math independently of networking.

Checklist:

- [ ] Add polynomial generation over BLS12-381 scalar field.
- [ ] Add polynomial evaluation for `BlsSignerId`.
- [ ] Add G2 coefficient commitments.
- [ ] Add commitment evaluation at a signer ID.
- [ ] Add secret evaluation verification against commitments.
- [ ] Add deterministic summing of secret evaluations into a final `BlsSecretKeyShare`.
- [ ] Add deterministic derivation of public shares and aggregate public keys from commitments.
- [ ] Reuse existing `BlsSecretKeyShare`, `BlsPublicKeyShare`, and `BlsAggregatePublicKey` types.

Tests and success criteria:

- [ ] 1-of-1 DKG output behaves like standalone BLS keygen.
- [ ] 2-of-3 and 3-of-5 DKG outputs produce valid threshold blind signatures.
- [ ] Any threshold subset can aggregate blind signatures.
- [ ] Fewer than threshold shares fail aggregation.
- [ ] Duplicate signer IDs are rejected.
- [ ] A tampered private evaluation fails commitment verification.
- [ ] A tampered commitment changes the transcript hash and fails final verification.

## Milestone 3: in-memory local DKG simulator

Objective: prove the complete DKG state machine in one process before adding networking.

Checklist:

- [ ] Implement an in-memory `DkgSession` state machine.
- [ ] Simulate every member as an isolated participant object.
- [ ] Generate per-member polynomials and commitments.
- [ ] Route private shares through an in-memory transport.
- [ ] Verify shares and produce final member outputs.
- [ ] Produce one final public transcript and one private output per simulated member.
- [ ] Convert simulated outputs into `FederationConfig` and `MemberConfigPrivate`.

Tests and success criteria:

- [ ] 4-of-5 local DKG creates one public config and five private configs.
- [ ] Every private config validates against the public config.
- [ ] Every member derives the same final transcript hash.
- [ ] DKG-generated configs pass the same validation tests as trusted-dealer configs.
- [ ] Wallet-side aggregate public keys verify signatures created from threshold shares.

## Milestone 4: authenticated DKG messages

Objective: bind DKG messages to federation identity keys.

Checklist:

- [ ] Define `SignedDkgMessage`.
- [ ] Sign message body hashes with the member identity secret key.
- [ ] Verify signatures against the public roster.
- [ ] Reject sender/member ID mismatches.
- [ ] Add replay protection for session ID, round, and sequence number.
- [ ] Add equivocation detection for multiple conflicting commitment messages from one member.

Tests and success criteria:

- [ ] Unknown member signatures are rejected.
- [ ] Valid member signature with wrong sender ID is rejected.
- [ ] Replayed messages are rejected.
- [ ] Commitments from the same member with different hashes produce an equivocation error.
- [ ] The in-memory simulator can run using signed messages only.

## Milestone 5: private share protection

Objective: keep per-recipient secret evaluations confidential in transit.

Checklist:

- [ ] Select private share encryption mechanism for REST DKG v1.
- [ ] Bind ciphertext to sender ID, recipient ID, context hash, and session ID.
- [ ] Reject ciphertext intended for a different member.
- [ ] Reject plaintext whose embedded sender or recipient does not match the envelope.
- [ ] Ensure logs never include plaintext shares.

Tests and success criteria:

- [ ] Recipient decrypts and verifies its own share.
- [ ] Other members cannot decrypt a share not intended for them.
- [ ] Swapping recipient IDs causes verification failure.
- [ ] Tampered ciphertext fails before share verification.

## Milestone 6: private REST DKG transport

Objective: run the DKG ceremony across local mint instances.

Checklist:

- [ ] Add DKG endpoints under the private federation API, not the public Cashu wallet API.
- [ ] Gate endpoints behind federation member allowlist authentication.
- [ ] Add submit-message, poll-session, get-transcript, and abort-session endpoints.
- [ ] Add client-side retry with idempotent message IDs.
- [ ] Add clear timeout behavior for missing peers.
- [ ] Keep transport separate from DKG state machine.

Tests and success criteria:

- [ ] Five local mint processes complete DKG over localhost REST.
- [ ] A request from a non-roster identity is rejected.
- [ ] Duplicate message submission is idempotent.
- [ ] A temporarily unavailable peer can receive missing messages after restart if the session has not timed out.
- [ ] If a peer stays offline past timeout, all honest members abort the same session.

## Milestone 7: DKG config command and local ceremony

Objective: provide operator tooling for initial federation setup.

Checklist:

- [ ] Add a CLI or `cdk-mintd` subcommand to initialize a DKG session.
- [ ] Accept a roster file with member IDs, identity public keys, public mint URLs, and private federation URLs.
- [ ] Accept keyset specs: units, amounts, input fees, active flags, and final expiry.
- [ ] Write a public federation config and one private config per member.
- [ ] Write the final public transcript next to the config files.
- [ ] Clearly label private config files as secrets.
- [ ] Refuse to overwrite existing private shares unless an explicit force flag is used.

Tests and success criteria:

- [ ] Local 4-of-5 setup command creates usable config files.
- [ ] Generated configs do not contain machine-specific paths.
- [ ] Generated public config contains no private shares.
- [ ] Generated private config for member 1 cannot be used as member 2.
- [ ] Re-running the command with the same session ID and changed roster fails.

## Milestone 8: startup validation for DKG-produced configs

Objective: ensure a mint cannot start with mismatched or tampered DKG output.

Checklist:

- [ ] Load `dkg_transcript_hash` at startup.
- [ ] Verify final transcript signatures from all required members.
- [ ] Verify transcript roster hash matches public config roster hash.
- [ ] Verify aggregate public keys match transcript commitments.
- [ ] Verify local private shares derive to this member's public shares.
- [ ] Verify all aggregate keysets are v3/BLS.
- [ ] Emit clear startup errors that do not leak secrets.

Tests and success criteria:

- [ ] Startup succeeds with DKG-generated config.
- [ ] Startup fails if local private share is replaced with another member's share.
- [ ] Startup fails if public transcript hash is changed.
- [ ] Startup fails if one aggregate key is changed.
- [ ] Startup fails if a keyset ID is downgraded or rewritten as non-v3.

## Milestone 9: fault injection and adversarial tests

Objective: prove the ceremony fails safely under common setup attacks.

Checklist:

- [ ] Add tests for malformed commitments.
- [ ] Add tests for invalid private shares.
- [ ] Add tests for missing shares.
- [ ] Add tests for duplicate member IDs.
- [ ] Add tests for public share reassignment.
- [ ] Add tests for wrong roster hash.
- [ ] Add tests for wrong threshold params.
- [ ] Add tests for equivocation.
- [ ] Add tests for replayed messages from an old session.

Tests and success criteria:

- [ ] Every attack produces a deterministic abort or validation failure.
- [ ] No test leaves partially activated keysets.
- [ ] Honest members agree on the same abort reason class when possible.
- [ ] No secret scalar appears in test logs or error strings.

## Milestone 10: DKG persistence and resume

Objective: survive process restarts during setup.

Checklist:

- [ ] Define a `DkgDatabase` trait or small storage adapter.
- [ ] Add SQLite-backed persistence for local DKG sessions if needed by `cdk-mintd`.
- [ ] Persist inbound and outbound messages before acknowledging them.
- [ ] Persist local generated polynomial seeds or coefficients securely until finalization.
- [ ] Persist final transcript before writing final configs.
- [ ] Add cleanup for aborted and completed sessions.

Tests and success criteria:

- [ ] Kill and restart one member after commitments; session resumes.
- [ ] Kill and restart one member after private shares; session resumes.
- [ ] Corrupt one stored message; session aborts safely.
- [ ] Completed sessions cannot be resumed with new messages.
- [ ] Cleanup removes temporary DKG secrets after final private config is written.

## Milestone 11: key rotation DKG

Objective: use DKG to create future aggregate keysets without stopping the federation.

Checklist:

- [ ] Add a `PrepareKeysetRotation` federation operation.
- [ ] Run DKG for the next keyset while current keysets remain active.
- [ ] Commit the final DKG transcript hash into consensus before activation.
- [ ] Add `ActivateKeysetRotation` consensus operation.
- [ ] Make every member prove possession of its new secret share before activation.
- [ ] Ensure wallets observe keyset changes consistently across members.
- [ ] Keep old keysets verifiable and spendable according to normal expiry policy.

Tests and success criteria:

- [ ] A running local federation rotates to a new DKG-generated keyset.
- [ ] Minting after activation uses the new aggregate keyset.
- [ ] Swapping old proofs into new proofs works.
- [ ] Melting old proofs still works until configured expiry.
- [ ] A member missing the new private share cannot activate the new keyset.

## Milestone 12: operational hardening

Objective: make DKG usable by real operators.

Checklist:

- [ ] Add operator documentation for initial setup.
- [ ] Add operator documentation for failed DKG recovery.
- [ ] Add operator documentation for backups of private member config and transcript.
- [ ] Add transcript inspection command.
- [ ] Add config verification command that can run offline.
- [ ] Add metrics for active DKG sessions, failures, and duration.
- [ ] Add warnings for weak thresholds or non-BFT threshold choices.
- [ ] Add a production checklist before enabling real payment backends.

Tests and success criteria:

- [ ] Operators can verify a public transcript without private shares.
- [ ] Operators can verify a private member config against public config and transcript.
- [ ] Documentation includes a local 4-of-5 example.
- [ ] Documentation explains that DKG does not replace consensus.
- [ ] Documentation explains that federation remains v3/BLS-only.

## Recommended implementation order

1. Finish the trusted-dealer federation MVP.
2. Populate and review the Fedimint source checkout.
3. Lock the CDK DKG protocol design.
4. Implement DKG types and transcript hashing.
5. Implement and test VSS math in memory.
6. Add signed authenticated DKG messages.
7. Add private share encryption.
8. Add REST transport for local multi-mint setup.
9. Add CLI/setup tooling.
10. Add startup validation for DKG-produced configs.
11. Add persistence and resume.
12. Add adversarial/fault-injection tests.
13. Add key rotation through consensus.
14. Audit before real payment backend use.

## Review checklist before merging DKG

- [ ] No aggregate private key is ever constructed outside tests.
- [ ] No secret scalar appears in public config, public transcript, logs, or errors.
- [ ] Every DKG output keyset is Cashu v3/BLS-only.
- [ ] DKG output validates through the same runtime config checks as trusted-dealer output.
- [ ] Final transcript hash is stored and checked at startup.
- [ ] DKG implementation is feature-gated and does not affect standalone mints.
- [ ] Wallet-to-mint Cashu APIs remain unchanged.
- [ ] Local 4-of-5 and 3-of-5 ceremonies pass.
- [ ] Invalid share, equivocation, replay, wrong roster, and wrong identity tests pass.
- [ ] Key rotation has explicit consensus activation and cannot silently replace active keys.

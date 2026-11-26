# Federation setup ceremony

A federation is created by a multi-operator setup ceremony followed by two
dealer-free key generations (BLS for Cashu signing, FROST for custody). Every
member authorizes one exact roster and policy before any key is generated. The
operator commands are listed in [operations](operations.md#creating-a-federation);
this document describes the protocol and its wire format.

The ceremony library is `crates/cdk-federation/src/setup/` (coordinator
service, typed client, per-operator state machine, DKG finalization, protected
files); the pure domain and transcript encoding are in
`crates/cdk-common/src/federation/setup/`. `cdk-mintd` only parses arguments,
opens the setup database and prints results. The trusted-dealer command
(`federation test-trusted-dealer`) exists only with `federation-test-utils`
and is not a production path.

## Authorization chain

```text
one-use invitation capability
  -> member-signed enrollment
  -> coordinator-signed immutable proposal
  -> one member-signed approval from every enrolled identity
  -> coordinator-signed manifest publication
  -> setup authorization hash in every BLS/FROST DKG context and the final config
```

The coordinator (run by the creating operator) can relay, delay or withhold
public records, but cannot certify a different proposal, approve for a member,
or learn a member's identity secret or key share, and it is not needed once
every member has stored the certified manifest. Availability during setup and
DKG is all-member by design: there is no earlier governance that could replace
a member. A lost member before certification means restoring that member's
protected state or abandoning the setup under a new setup ID.

## Steps

1. **Create.** The creator fixes the member count, BLS threshold, Bitcoin
   network, payment-observation threshold, checkpoint intervals, the first BLS
   keyset (unit, amounts, input fee, expiry), the FROST root and CDK on-chain
   application epochs, confirmation depth and fee-policy version, and
   optionally the Bark application (Ark server key, epoch, policy file). It
   writes one protected invitation per joining member.
2. **Join.** Each member generates and persists its identity and a setup nonce,
   then submits a signed enrollment with its public mint URL and federation API
   URL, using the invitation capability once for its slot.
3. **Propose.** The creator freezes all enrollments and the policy into one
   immutable proposal, signed by the coordinator.
4. **Review and approve.** Each member reviews the exact proposal and approves
   its hash. Approval is fenced: a member approves at most one proposal hash per
   setup, so a coordinator that equivocates cannot collect a full set for two
   proposals.
5. **Certify.** With one approval from every enrolled member and no extras, the
   coordinator publishes the manifest. Each member validates the proposal,
   every approval, the certification hash and the coordinator signature, and
   persists the manifest (`setup status` reports `certified`).
6. **DKG.** Each member runs the certified DKG (`setup serve` on its federation
   API address, or mintd with `federation.setup_config_path` and
   `[federation.dkg] enabled = true`). The BLS and FROST ceremonies run
   concurrently; no phase is ordered by member ID, and only all-member phase
   barriers remain.
7. **Activate.** Once both DKG results are complete and every member's
   final-config confirmation is in, `setup activate` validates and atomically
   publishes the member's final public and private config
   (`<state-dir>/activated/`). Repeating it returns the recorded activation.

Every write precedes the send that depends on it, so after an ambiguous
response an operator reruns the same command and the identical bytes are
resent.

## DKG composition

The BLS ceremony produces Cashu keyset shares; the FROST ceremony produces one
canonical untweaked secp256k1 root (see [custody](custody.md)). Both use the
same roster, ceremony ID and threshold, with independent cryptographic state,
and the federation becomes ready only when both have completed, verified,
persisted and activated. The BLS, FROST and identity keys are never converted
into one another.

- Messages (readiness, commitments, reveals, confidential secret shares,
  transcript signatures, activations, FROST rounds one and two, root
  confirmations) travel over authenticated private REST with distinct message
  kinds. Each binds the federation, ceremony, epoch, protocol, ciphersuite,
  threshold, roster, sender and, where applicable, receiver.
- Phase state is sealed (AES-256-GCM, bound to phase, member, ceremony and
  peers) and persisted before any dependent message is sent. Exact duplicates
  are idempotent; a second payload for the same ceremony, kind, sender and
  receiver is rejected as equivocation.
- One planner (`plan_frost_dkg` and the BLS planner) decides the next step from
  stored records and the delivery tracker; a round advances only after every
  member acknowledged this member's messages and every other member's were
  received. Members do not resend to peers that already acknowledged.
- Activation requires each member's local shares to match its advertised
  public shares and every member's byte-identical public FROST result and
  final-config confirmation. Any mismatch aborts without changing the active
  configuration.

## Wire format

Setup hashes and signatures use a fixed binary encoding, never JSON (JSON is
only an operator representation):

- unsigned integers are big-endian; fixed-size byte strings have no length
  prefix; variable bytes and UTF-8 strings are a four-byte length plus bytes;
  collection counts are `u16`; booleans are one byte; options are a one-byte
  discriminant then the value;
- setup IDs, invitation IDs, nonces, hashes and commitments are 32 bytes;
  identity and Ark server keys are 33-byte compressed secp256k1 keys; member IDs
  and protocol versions are `u16`;
- URLs are normalized `MintUrl` strings without user information, query or
  fragment; units are lowercase canonical strings;
- members and approvals are strictly ascending by member ID; keysets and
  amounts are strictly ascending and unique;
- decoders reject non-minimal, unsorted, duplicate, unknown, trailing,
  invalid-UTF-8 or oversized data.

`H` is SHA-256 over a length-prefixed domain followed by the fields.
Signatures are 64-byte BIP340 Schnorr signatures by the member identity key.

| Value | Definition |
|---|---|
| Invitation | `cashufedsetupA` + URL-safe Base64 of the public body (version, setup ID, invitation ID, member ID, coordinator URL and key, expiry), the 32-byte capability and a 6-byte checksum |
| Capability hash | `H("cdk/federation/setup/invitation-capability/v1", setup_id, invitation_id, member_id, capability)` |
| Proposal hash | `H("cdk/federation/setup/proposal/v1", proposal body)`; the setup authorization hash equals it |
| Ceremony ID | `H("cdk/federation/setup/ceremony-id/v1", setup_id, proposal_hash)`, which carries every member's setup nonce through the proposal |
| Manifest certification | `H("cdk/federation/setup/manifest/v1", setup_id, proposal_hash, approvals in member order)` |

The proposal body contains the protocol version, setup ID, coordinator key,
every complete enrollment, member count, signature and consensus thresholds,
the required version tuple, network, mint and melt payment policies,
checkpoint intervals, keyset specifications, the FROST root epoch and the FROST
application commitments (CDK on-chain: epoch, confirmation and fee policy;
Bark: epoch, network, Ark server key, fee, CLTV, refresh, exit, receive-secret
retention and anti-DoS policy). Machine-local settings (endpoints, databases,
timeouts, fan-out, logging) are not part of it. Every enrollment must
advertise exactly the proposal's version tuple: setup protocol, BLS DKG,
private REST, wallet protocol, BLS keyset version (2, the v3 keyset), FROST DKG
and FROST ciphersuite. Production setup rejects the development
single-observation payment policy.

Coordinator responses are signed over a separate response transcript and
never enter the member private REST sequence space. The invitation checksum
detects transcription errors only; the capability is sent in a header or
protected field, never in a URL.

## Limits

| Limit | Value |
|---|---|
| Members | 2 to 64 (`FEDERATION_MAX_DKG_MEMBERS`) |
| Keysets per setup | 16 |
| Amounts per keyset | 64 |
| URL | 2,048 bytes |
| Invitation | 8 KiB |
| Enrollment | 16 KiB |
| Proposal or manifest | 1 MiB |
| Setup status | 256 KiB |
| Invitation lifetime | 5 minutes to 7 days, default 24 hours |

Byte limits apply before parsing. Constants are in
`crates/cdk-common/src/federation/setup/mod.rs`.

## Threats

- **Coordinator:** may omit, delay, reorder, replay or equivocate. It cannot
  forge member signatures; durable idempotence and the one-proposal approval
  fence turn equivocation into an abort.
- **Joining member:** may send malformed data, reuse an invitation, sign
  conflicting messages, withhold approval or disappear. Slot-bound capabilities,
  identity proofs, bounds and unanimous certification stop it from changing
  another slot; it can deny availability, as any member can during DKG. A
  member that approves and then misbehaves in DKG is rejected by DKG
  validation; it is not replaced automatically.
- **Network:** HTTPS (or Iroh), the coordinator key pinned in the invitation,
  setup and member IDs, correlation nonces, body hashes, per-message domains
  and expiry reject replay and redirection. A stolen unused invitation can
  claim only its one slot and force a restart; operators must deliver
  invitations over an authenticated channel.

## Test vectors

`crates/cdk-common/tests/federation_setup_transcript_vectors.rs` with
`crates/cdk-common/tests/data/federation_setup_transcript_v1.json` is the
byte-level reference. It covers 2-of-3, 3-of-5 and 22-of-64 proposals,
enrollment, proposal and approval signatures, manifest certification, ceremony
ID derivation, capability hashing and coordinator publication signatures. Any
incompatible change bumps the setup protocol version and regenerates the
vectors.

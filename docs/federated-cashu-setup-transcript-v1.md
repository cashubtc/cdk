# Federated Cashu multi-operator setup transcript v1

> **Frozen for implementation — 2026-08-01:** This document is the wire and
> threat-model input for Milestone 0 of
> [`federated-cashu-multi-operator-setup-implementation-plan.md`](federated-cashu-multi-operator-setup-implementation-plan.md).
> Any incompatible change requires a setup-protocol version bump and new fixed
> vectors. Federation setup is not yet deployed, so implementation must cut
> through to this shape rather than add compatibility fallbacks.

## Security outcome

Every member authorizes one exact federation roster and policy before DKG. The
setup coordinator can relay and withhold public records, but cannot certify a
different proposal, approve for a member, learn a member identity secret or DKG
share, or remain necessary after all members store the certified manifest.

The authorization chain is:

```text
one-use invitation capability
  -> member-signed enrollment
  -> coordinator-signed immutable proposal
  -> one member-signed approval from every enrolled identity
  -> coordinator-signed manifest publication
  -> setup authorization hash in every BLS/FROST DKG and final config
```

Availability is all-member during initial setup and DKG. This is intentional:
there is no prior governance capable of removing or replacing a member. A lost
member or coordinator failure before manifest distribution requires bounded
recovery or an explicit abort and new setup ID.

## Cryptographic and encoding rules

All integers are unsigned and big-endian. Fixed-size byte strings have no
length prefix. Variable bytes and UTF-8 strings are encoded as a four-byte
length followed by that many bytes. Collection counts are unsigned 16-bit
integers followed by elements in the stated order. Booleans are one byte (`00`
or `01`). Options are a one-byte discriminant (`00` absent, `01` present)
followed by the value when present. Enum discriminants are the unsigned values
assigned below. No other value is valid.

`LP(value)` below means the four-byte length plus `value`. `H(value)` is
SHA-256. A hash transcript begins with `LP(domain)` and then the fields shown.
Signatures are 64-byte BIP340 Schnorr signatures made by the existing CDK
secp256k1 identity key. CDK's identity helper hashes the supplied signature
transcript once with SHA-256 before signing and verification; that existing
behavior is part of this contract. Production signing uses auxiliary
randomness. Fixed vectors use the no-auxiliary-randomness helper only so the
signature bytes are reproducible.

Canonical values use raw bytes, not their display form:

- setup IDs, invitation IDs, nonces, hashes, and commitments are 32 bytes;
- identity and Ark server keys are compressed secp256k1 keys, 33 bytes;
- member IDs and protocol versions are `u16`;
- Unix expiry is `u64` seconds;
- amounts, fees, epochs, and checkpoint intervals use the widths below;
- URLs are their parsed, normalized `MintUrl::to_string()` UTF-8 value and must
  not contain user information, fragments, or query strings;
- currency units are lowercase canonical `CurrencyUnit::to_string()` values;
- members and approvals are strictly ascending by member ID;
- keysets are strictly ascending by `(unit bytes, input_fee_ppk, expiry tag and
  value, amounts)`; and
- amounts are strictly ascending and unique.

Decoders reject non-minimal, unsorted, duplicate, unknown, trailing, invalid
UTF-8, or oversized data. JSON is an operator/API representation only and is
never the input to a setup hash or signature.

## Constants and limits

| Name | Version 1 value | Basis |
|---|---:|---|
| Setup protocol version | 1 | First setup wire format |
| Invitation prefix | `cashufedsetupA` | Distinct from wallet invite `cashufedA` |
| Invitation capability | 32 random bytes | High-entropy one-use bearer secret |
| Invitation checksum | first 6 bytes of its checksum hash | Accidental-corruption detection, not authentication |
| Minimum members | 2 | Existing federation bound |
| Maximum members | 64 | `FEDERATION_MAX_DKG_MEMBERS` |
| Maximum keysets | 16 | Bounds aggregate setup and DKG work |
| Maximum amounts per keyset | 64 | `FEDERATION_MAX_DKG_KEYSET_AMOUNTS` |
| Maximum URL bytes | 2,048 | Bounded enrollment and diagnostics |
| Maximum invitation bytes | 8 KiB | Bounded CLI input |
| Maximum enrollment bytes | 16 KiB | Bounded single-slot request |
| Maximum proposal or manifest bytes | 1 MiB | Existing private DKG request boundary |
| Maximum setup status bytes | 256 KiB | Status contains counters/fingerprints only |
| Invitation lifetime | 5 minutes minimum, 24 hours default, 7 days maximum | Human coordination without permanent credentials |
| Setup HTTP request deadline | 15 seconds | Bounded individual request ownership |
| Setup HTTP retry budget | 10 attempts with capped exponential backoff and jitter | Bounded ambiguous-response recovery |

The setup implementation exposes constants for these values from
`cdk-common`. Axum applies the byte limits before parsing. The maximum manifest
shape must remain below one MiB at 64 members with maximum-length accepted
fields; construction rejects any otherwise-valid shape that exceeds it.

An enrollment advertises exactly these required version fields in order:

1. setup protocol (`u16`);
2. BLS DKG protocol (`u16`);
3. federation private REST API (`u16`);
4. wallet federation protocol (`u16`);
5. Cashu BLS keyset version (`u16`, value `2`, the CDK
   `KeySetVersion::Version02` representation of Cashu keyset v3);
6. FROST root DKG protocol (`u16`); and
7. FROST ciphersuite (`u16`, value `1` for secp256k1/SHA-256 Taproot FROST).

The proposal repeats one required version tuple. Version 1 requires exact
equality from every enrollment; it does not negotiate ranges or silently
downgrade. Milestone 5 bumps DKG/private-REST/FROST versions wherever adding
the setup-authorization field changes their wire transcript.

## Invitation

The canonical invitation-public body is:

```text
setup_protocol_version:u16
setup_id:[u8;32]
invitation_id:[u8;32]
member_id:u16
coordinator_url:LP(UTF-8)
coordinator_identity_public_key:[u8;33]
expires_at_unix:u64
```

The encoded invitation payload is the canonical public body, the 32-byte
capability, and the six-byte checksum, encoded with URL-safe unpadded Base64
after the invitation prefix. Its checksum transcript is:

```text
LP("cdk/federation/setup/invitation-checksum/v1")
LP(canonical invitation-public body)
capability:[u8;32]
```

The durable capability hash is:

```text
H(
  LP("cdk/federation/setup/invitation-capability/v1") ||
  setup_id || invitation_id || member_id:u16 || capability
)
```

The checksum detects transcription errors but grants no authority. Possession
of the capability authorizes one atomic enrollment attempt for its exact slot.
It is sent in an authorization header or protected request field, never in an
HTTP path or query.

## Enrollment

The unsigned enrollment body is:

```text
setup_protocol_version:u16
setup_id:[u8;32]
invitation_id:[u8;32]
member_id:u16
public_mint_url:LP(UTF-8)
federation_api_url:LP(UTF-8)
member_identity_public_key:[u8;33]
setup_nonce:[u8;32]
bls_dkg_protocol_version:u16
federation_rest_api_version:u16
wallet_protocol_version:u16
bls_keyset_version:u16
frost_dkg_protocol_version:u16
frost_ciphersuite_version:u16
```

The enrollment signature transcript is:

```text
LP("cdk/federation/setup/enrollment-signature/v1")
LP(canonical unsigned enrollment body)
```

`FederationSetupEnrollment` is the unsigned body followed by the 64-byte
signature. The member setup nonce is generated and persisted with the identity
before transmission. Exact retry bytes are immutable.

The coordinator's signed enrollment acknowledgement covers:

```text
LP("cdk/federation/setup/enrollment-ack/v1")
setup_id || invitation_id || member_id:u16 ||
H(LP("cdk/federation/setup/enrollment-record/v1") ||
  LP(canonical complete enrollment)) ||
request_correlation_nonce:[u8;32] || setup_protocol_version:u16
```

## Proposal body

The canonical proposal body is:

```text
setup_protocol_version:u16
setup_id:[u8;32]
coordinator_identity_public_key:[u8;33]
enrollment_count:u16
  repeated: LP(canonical complete enrollment)
member_count:u16
signature_threshold:u16
consensus_threshold:u16
required_version_tuple:7*u16
network:u8
mint_payment_policy
melt_payment_policy
checkpoint_interval_journal_operations:u64
checkpoint_interval_finalized_items:u64
checkpoint_interval_canonical_bytes:u64
keyset_count:u16
  repeated: canonical keyset specification
frost_root_key_epoch:u64
frost_application_count:u16
  repeated: canonical FROST application commitment
```

Network discriminants are `1=bitcoin`, `2=testnet`, `3=signet`, and
`4=regtest`. Payment policy is `1=member_quorum` followed by `required:u16`.
The development single-observation policy has discriminant `2` but is rejected
by production setup creation.

A keyset specification is:

```text
unit:LP(UTF-8)
active:bool
input_fee_ppk:u64
final_expiry:option<u64>
amount_count:u16
  repeated: amount:u64
```

Version 1 defines two FROST application commitment variants, ordered by their
tag and unique within a proposal:

```text
1 = CDK on-chain:
    tag:u8
    application_key_epoch:u64
    confirmation_policy_version:u16
    minimum_confirmations:u32
    fee_policy_version:u16

2 = Bark:
    tag:u8
    application_key_epoch:u64
    network:u8
    ark_server_public_key:[u8;33]
    bark_policy_version:u16
    receive_base_fee_sat:u64
    receive_fee_ppm:u32
    maximum_outgoing_base_fee_sat:u64
    maximum_outgoing_fee_ppm:u32
    minimum_incoming_cltv_delta:u32
    minimum_outgoing_cltv_delta:u32
    refresh_before_expiry_blocks:u32
    emergency_exit_before_expiry_blocks:u32
    receive_secret_retention_days:u16
    receive_anti_dos:u8
```

Bark anti-DoS discriminants are `1=no_proof`, `2=protected_token`, and
`3=accepted_vtxo`. Application endpoint URLs, database settings, thread counts,
timeouts, fanout, and log settings are machine-local and are not signed shared
policy. Network and application network values must agree.

The proposal hash is:

```text
H(
  LP("cdk/federation/setup/proposal/v1") ||
  LP(canonical proposal body)
)
```

Version 1 defines the setup authorization hash as exactly the proposal hash.
The BLS/FROST ceremony ID is separately derived as:

```text
H(
  LP("cdk/federation/setup/ceremony-id/v1") ||
  setup_id || proposal_hash
)
```

Because the proposal hash commits to every member setup nonce, the ceremony ID
contains entropy from every participant without depending on coordinator-only
randomness.

## Proposal publication, approval, and manifest

The coordinator proposal signature transcript is:

```text
LP("cdk/federation/setup/proposal-publication/v1")
setup_id || proposal_hash || setup_protocol_version:u16
```

The signed proposal contains the canonical body, proposal hash, and coordinator
signature. A member approval contains setup ID, proposal hash, member ID, and a
signature over:

```text
LP("cdk/federation/setup/approval/v1")
setup_id || proposal_hash || member_id:u16 || setup_protocol_version:u16
```

Approvals are stored and encoded in ascending member-ID order. Certification
requires exactly one valid approval for every enrollment and no extra member.
The manifest-certification hash is:

```text
H(
  LP("cdk/federation/setup/manifest/v1") ||
  setup_id || proposal_hash || approval_count:u16 ||
  repeated(member_id:u16 || approval_signature:[u8;64])
)
```

The coordinator manifest-publication signature transcript is:

```text
LP("cdk/federation/setup/manifest-publication/v1")
setup_id || proposal_hash || manifest_certification_hash ||
setup_protocol_version:u16
```

The signature is evidence of what the coordinator published, not an
authorization substitute. A member validates the proposal, all member
approvals, the certification hash, and the coordinator signature before
persisting the manifest.

Setup HTTP response envelopes use a separate transcript:

```text
LP("cdk/federation/setup/response/v1")
setup_id || route_kind:u8 || request_correlation_nonce:[u8;32] ||
body_hash:[u8;32] || setup_protocol_version:u16
```

Route kinds are `1=enrollment_ack`, `2=proposal`, `3=approval_ack`,
`4=manifest`, and `5=status`. The body hash uses
`H(LP("cdk/federation/setup/response-body/v1") || LP(body bytes))`. Response
signatures never enter the configured-member private REST sequence space.

## DKG and final-config binding

Milestone 5 adds the required 32-byte setup authorization hash to the setup and
final `FederationConfig`. It is included directly in the roster/config digest
and in every safety-relevant DKG context listed in the implementation plan.
Affected DKG records reject the old version rather than treating the new field
as optional. The final BLS transcript payload includes the setup authorization
hash adjacent to federation ID. The FROST context includes it adjacent to
ceremony ID. Activation and final-config confirmation signatures include it
adjacent to the final config digest.

No DKG phase is ordered by member ID. All members may compute and transmit the
current phase concurrently; only all-member phase-completion barriers remain.
Machine-local CPU admission and bounded network fanout are not part of the
shared proposal and may differ between hosts.

## Threat model

### Setup coordinator

The coordinator may omit, delay, reorder, replay, or equivocate about public
setup records; return ambiguous failures; restart; or disappear. It cannot
forge member signatures. Durable idempotence and one-proposal approval fences
turn equivocation into a bounded abort: conflicting proposals cannot both
collect every member signature. Coordinator loss after every member stores the
manifest has no DKG or runtime effect.

### Joining member

A joining member may submit malformed or oversized data, reuse an invitation,
claim mismatched URLs or identity keys, sign conflicting messages, withhold
approval, or disappear. Capability-slot binding, identity proof, strict bounds,
and unanimous certification prevent it from changing another slot. It can deny
availability, as any member can during the subsequent fixed-roster DKG.

### Network and replay attacker

The attacker may observe, delay, duplicate, reorder, truncate, or redirect
traffic; control DNS; present an ordinary WebPKI certificate for another host;
or replay records across routes, members, setup IDs, and ceremonies. HTTPS,
the coordinator key pinned in the invitation, setup/member identifiers,
correlation nonces, exact body hashes, per-message domains, expiry, and durable
idempotence reject these attacks. A stolen unused invitation capability can
claim only its one slot and force restart; CDK cannot identify the intended
human recipient without the operators' authenticated out-of-band delivery.

### Lost member or local state

Identity creation and approval fencing are write-before-send. After an
ambiguous response the operator resumes the identical bytes. Loss before
certification requires restoring that member's protected backup or abandoning
the setup. No other member can reconstruct its identity or DKG share.

### Malicious federation member

A member may approve and later send invalid or equivocating DKG messages. The
signed manifest proves what it approved; existing authenticated DKG validation
rejects invalid records and preserves bounded evidence. Version 1 does not
replace, exclude, or punish a member automatically.

## Review record and fixed vectors

The transcript was checked against the parent implementation contract and the
existing `MemberConfigPublic`, `ThresholdParams`, DKG, FROST, checkpoint,
payment-observation, private REST, and Schnorr identity boundaries. The focused
review checks were:

- no signed value is derived from ordinary JSON;
- every signature domain is distinct and commits to setup identity/context;
- all collections have one canonical order and bounded count;
- coordinator signatures are attributable but never replace unanimous member
  authorization;
- invitation theft is accurately treated as a one-slot availability failure;
- the proposal commits to every existing consensus-relevant setup policy while
  excluding legitimate machine-local tuning;
- setup authorization reaches both DKG systems and final activation; and
- DKG remains concurrent across members.

Executable fixed vectors live in
`crates/cdk-common/tests/federation_setup_transcript_vectors.rs`, with expected
outputs in `crates/cdk-common/tests/data/federation_setup_transcript_v1.json`.
They cover 2-of-3, 3-of-5, and 22-of-64 BLS proposal shapes (with production BFT
consensus thresholds 3, 4, and 43 respectively), deterministic enrollment,
proposal and approval signatures, manifest certification, ceremony derivation,
invitation capability hashing, and coordinator publication signatures.

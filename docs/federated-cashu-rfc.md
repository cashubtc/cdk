# Federated Cashu RFC

This RFC records the initial safety model for adding federated Cashu mint support to CDK. It is the Milestone 0 companion to `federated-cashu-implementation-plan.md` and should be updated whenever the federation architecture changes.

## Objective

Federated Cashu lets multiple CDK mint members operate one logical Cashu mint. A federation-aware wallet talks to multiple members, receives threshold BLS blind signature shares, verifies those shares against member public shares, and aggregates a threshold subset into ordinary Cashu proofs.

The public wallet-to-mint Cashu API should remain close to today's NUT endpoints. Federation-specific behavior should be enabled only by explicit feature gates and configuration.

## Initial Scope

The first implementation phase was intentionally conservative:

- Add feature gates and inert mint setup types.
- Preserve standalone mint and wallet behavior.
- Use trusted-dealer key shares for local tests before real DKG. This historical
  bridge is now restricted to the explicit `federation-test-utils` feature;
  production setup uses independent member enrollment and dealer-free Pedersen
  DKG.
- Use fake wallet payment backends for mint and melt happy paths and scripted failure paths.
- Add deterministic consensus and networking in later milestones before any member can safely return threshold signing shares.

## Threat Model

The federation must prevent wallets from obtaining more valid eCash than the accepted operation permits. The motivating attack is a threshold-signing mix-and-match or sliding-window attack: a wallet presents different output sets to different members and aggregates a threshold of shares for each output. The protocol must ensure members sign only the exact output set accepted by consensus.

The federation assumes:

- Honest members verify operation hashes and consensus outcomes before signing.
- Faulty or unavailable members may send invalid shares, refuse to respond, replay old messages, or propose conflicting operations.
- Network messages may be delayed, duplicated, reordered, or temporarily dropped.
- External payment backends are not deterministic consensus inputs. Payment status must enter consensus as explicit observations or accepted payment events.

The federation does not assume:

- Wallets are honest.
- A single member's local database state is authoritative.
- A member can safely sign based only on a local HTTP request.
- Fake wallet success means production Lightning payment safety.

## Fault Model

The default production target is Byzantine fault tolerant consensus. The initial configuration should use a Fedimint-style threshold rule:

```text
threshold = n - floor((n - 1) / 3)
```

For tests, the implementation may include crash-only or in-memory deterministic engines. Any such engine must be clearly marked as test-only and must not be presented as production-safe consensus.

If CDK later supports a signing threshold lower than the consensus threshold, that must receive a separate review. Consensus ordering and signing threshold are related but not interchangeable security parameters.

## Core Invariants

Every federation milestone must preserve these invariants:

- A member never returns a usable blind signature share before the exact operation payload is accepted by consensus.
- The operation hash commits to quote IDs, inputs, outputs, amounts, keysets, witnesses, payment method, request signatures, and operation version.
- All honest members apply accepted operations in the same order.
- All honest members converge to the same state digest at consensus boundaries.
- A mint quote can issue only the consensus-tracked mintable amount.
- A proof can transition to spent exactly once in consensus state.
- A blinded output can be signed only for the accepted operation and amount.
- Payment backend results enter consensus as accepted observations or payment events.
- Lagging members refuse to serve signing shares until they catch up to the relevant accepted operation.
- Standalone CDK behavior remains unchanged when federation is disabled.

## Member Identity

Federation member identity is separate from Cashu signing keys:

- Member identity keys authenticate mint-to-mint messages.
- Threshold BLS shares sign blinded Cashu messages.
- Every member stores the complete federation roster of member IDs, identity public keys, endpoints, BLS public shares, and aggregate Cashu keysets.
- Unknown member IDs, wrong federation IDs, mismatched public keys, invalid signatures, and body-hash mismatches must be rejected before state mutation. Effectful DKG messages must enforce durable sequence state; status, catch-up, readiness, and Aleph traffic must use kind-isolated volatile windows. `SubmitOperation` authenticates its sender-local sequence but deliberately derives replay safety and idempotency from the canonical operation ID and durable consensus record.

## Public Federation Discovery

An active federation member exposes its complete public federation config at
`GET /federation/v1/config`. This is a public, unauthenticated, read-only wallet
discovery endpoint, unlike the signed member coordination endpoints under the
same `/federation/v1/*` prefix. Standalone mints do not expose it, and a member
must not serve an incomplete setup config without a finalized transcript and
active aggregate keyset. Responses use `Cache-Control: no-store` so key rotation
or a restarted finalized config is not hidden by an intermediary cache.

The response contains only `FederationConfig`: member public URLs, member
identity public keys, thresholds, v3/BLS aggregate keysets and public shares,
payment-observation policy, one wallet-facing protocol version, and the signed
setup transcript. Member-only operation, consensus, REST, Aleph-unit, and
catch-up versions are carried by their private payloads and are not wallet
compatibility gates.
It must never contain a member identity secret key, BLS secret share, database
credential, payment credential, or private runtime state.

A wallet joining from one member URL treats the response as untrusted input and
must:

1. fully validate the federation ID, roster, thresholds, transcript, wallet
   protocol version, payment policy, and every v3/BLS aggregate keyset and
   member share;
2. require the supplied URL to identify exactly one member in the config;
3. fetch that member's `GET /v1/info` response and verify its member ID,
   federation ID, wallet protocol version, and full config digest against the
   fetched config; and
4. persist or construct a federation connector only after all checks succeed.

This bootstraps trust from the member URL chosen by the user while preventing a
stale, mismatched, or unrelated config response from silently selecting another
federation.

## Consensus-Before-Signing Rule

No direct signing path may be available in federation mode. All minting and swapping outputs, and all melt change outputs, must pass through:

1. canonical operation construction,
2. operation hash computation,
3. consensus acceptance,
4. deterministic state application,
5. durable share recording or deterministic share regeneration,
6. response to the wallet.

This applies even if the local member was the member contacted first by the wallet.

## Payment Handling

For the initial fake-wallet implementation:

- Incoming mint payments can be scripted as paid, pending, failed, delayed, duplicated, or conflicting.
- Outgoing melt payments can be scripted as succeeded, pending, failed, delayed, timed out, or recovered after restart.
- Fake wallet observations are useful for consensus and recovery tests but are not production payment policy.

For production backends:

- Payment status cannot be read inside deterministic consensus apply logic.
- Outgoing payments need idempotency keys.
- Crash recovery must never pay twice.
- Final accepted payment observations determine quote and proof state.

## Review Checklist

Every federation pull request should answer:

- Does this change preserve standalone behavior when federation is disabled?
- Can any route call `blind_sign` or future `blind_sign_share` before consensus acceptance?
- Does every consensus operation have a canonical encoding and stable operation hash?
- Does the operation hash commit to all value-bearing request material?
- Are external payment observations outside deterministic apply logic?
- Are retries idempotent?
- Can a lagging member serve stale or unsafe signing shares?
- Does the database transition have replay and crash-recovery coverage?
- Do all new public types have clear docs and safe debug output?
- Are private keys, key shares, payment secrets, local paths, and private endpoints excluded from logs, tests, fixtures, and commits?

## Milestone 0 Exit Criteria

Milestone 0 is complete when:

- The RFC exists.
- The `federation` Cargo feature exists and is disabled by default.
- CDK exposes an inert `FederationMode::{Standalone, Member}` type behind the feature gate.
- Mint setup defaults to standalone mode.
- Enabling the feature compiles the inert scaffold.
- Existing default behavior is unchanged.

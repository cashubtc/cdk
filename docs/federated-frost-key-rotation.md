# Federated FROST key rotation

This runbook covers reusable secp256k1 FROST roots and the CDK on-chain
application derived from them. Rotation is a consensus lifecycle, not a local
configuration edit. Existing mint business logic remains unchanged; the
wallet, DKG, signing, and lifecycle boundaries are isolated and communicate
through accepted federation operations.

## Invariants

- A federation ceremony succeeds only when both its BLS and FROST DKG results
  verify for the same roster and signature threshold. A rotating ceremony
  leaves the active BLS keysets and active FROST root untouched.
- Root and application epochs are independent. Rotating an application never
  mutates or reconstructs the reusable root secret.
- New addresses, deposits, payment intents, and change use only the active
  application epoch. Retiring descriptors remain watch-only recognition and
  signing targets for their existing UTXOs.
- A transaction contains inputs from exactly one root/application epoch.
  Old-to-new movement uses a dedicated epoch-sweep proposal with one
  consensus-allocated internal destination on the active epoch.
- Participant shares, nonce material, and private storage paths are never
  exposed by public status.

## Root and application rotation procedure

1. Configure a new ceremony ID and monotonically increasing root epoch on
   every member. Run the dual DKG and confirm that every member has the same
   complete BLS transcript and FROST public package. The new root is persisted
   as `proposed`; it is not selected for custody yet.
2. Order `ProposeRoot` in federation consensus. Reject any payload that does
   not exactly match the locally sealed, successfully completed DKG result.
   Order `ActivateRoot` only after every member has caught up and can open its
   local share. The former root becomes `retiring`.
3. Derive the next CDK application epoch from the active root and order
   `ProposeApplication`. Restart or catch up every member so its watch-only BDK
   descriptor and signing adapter are materialized. Order
   `ActivateApplication` only after the current epoch has no unresolved
   payment intent or transaction. New allocations then use the new epoch.
4. Keep all retiring descriptors loaded. The liveness worker builds one
   deterministic, replace-by-fee-enabled sweep from each retiring epoch to a
   consensus-allocated active internal address. The normal proposal, policy,
   reservation, FROST signing, signed-record, broadcast, confirmation, and
   reorg-recovery lifecycle applies. A restart resumes the accepted bytes and
   sticky reservations; it must not create a competing sweep.
5. Wait until the retiring application has no spendable or reserved UTXOs and
   no unresolved intent or transaction. Order `RetireApplication`. Complete
   the reviewed retirement contract for every other registered consumer, then
   order `RetireRoot`. Delete a local root share only after the public root is
   retired and the deletion guard succeeds.

Public federation status reports every root and application epoch as
`proposed`, `active`, `retiring`, or `retired`, plus descriptor/transcript
commitments and consumer counts. Operators should stop the procedure if
members disagree, a catch-up is incomplete, an old UTXO remains, or any
consumer is unaccounted for.

## Membership changes

Adding, removing, or renumbering members requires a fresh dual BLS/FROST DKG,
or a separately designed and security-reviewed resharing protocol that binds
the new roster and threshold. Basic FROST proactive refresh preserves the
existing participant set and must not be treated as federation
reconfiguration.

# Federated Signature Share Provenance

This note gives reviewers a concrete way to trace every federation-mode BLS
signature share back to the consensus operation that authorized it.

## Required Invariant

No member may return or restore a usable federation signature share unless the
exact wallet request was accepted into the federation journal first.

For every share, reviewers must be able to identify:

- The accepted `FederationOperationId`.
- The operation output index.
- The signing federation member ID.
- The blinded message.
- The output amount and keyset ID.
- The BLS share bytes and the signer ID embedded in the share.

## Durable Row

`FederationSignatureShareRecord` is the durable provenance envelope for one
member's threshold BLS share. It stores:

- `operation_id`
- `output_index`
- `member_id`
- `keyset_id`
- `amount`
- `blinded_secret`
- `share`

SQL backends persist those fields in `federation_signature_shares` with primary
key `(operation_id, output_index, member_id)`. Exact replay is idempotent, but a
different blinded message, amount, keyset, share, or JSON payload for the same
operation/output/member is a conflict.

## Apply Paths

Federation mint, swap, and melt-change paths persist signature-share records in
the same database transaction that applies the accepted operation. If a later
write in that transaction fails, the share rows roll back with the rest of the
operation state.

The current apply paths are:

- Mint issuance: `crates/cdk/src/mint/federation.rs`
- Swap: `crates/cdk/src/mint/federation.rs`
- Melt change: `crates/cdk/src/mint/melt/shared.rs`

The current restore path never signs new outputs. In federation mode it first
queries `federation_signature_shares` by blinded message, checks that the stored
row matches the requested keyset and amount metadata, and falls back to the
legacy compatibility blind-signature row only for already persisted older data.

## Reviewer Trace Recipe

1. Locate the returned `BlindSignature` share from the member response.
2. Locate the matching `federation_signature_shares` row by blinded message or
   by `(operation_id, output_index)`.
3. Load the accepted operation envelope with that `operation_id` from
   `federation_journal_entries`.
4. Confirm the output at `output_index` has the same blinded message, amount,
   and keyset ID as the stored share row.
5. Confirm the row's `member_id` matches the member that returned the share and
   the signer ID embedded in the BLS share.
6. Verify the BLS share against the member's public share for that keyset and
   amount.
7. Confirm the operation status is accepted or applied and that any conflicting
   operation for the same resource was rejected before signing.

## Evidence

The focused storage and apply-path tests cover:

- Idempotent share-row replay.
- Conflicting share-row rejection.
- Lookup by `(operation_id, output_index)`.
- Lookup by blinded message.
- Indexed SQLite query plans for both lookup forms.
- Mint, swap, melt-change, and restore paths writing or reading durable share
  rows.


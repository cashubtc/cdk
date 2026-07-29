# Federated Melt State Machine Decision

This note records the current design decision for federated melt reserve and
payment finalization. It closes the open question of whether CDK should rename
or split the current federated melt request into explicit `MeltReserve`,
`FinalizeMelt`, and `RollbackMelt` operation variants for federation operation
version 1.

## Decision

Keep the current operation shape for federation operation version 1:

- `FederationOperation::MeltQuote` creates and persists the melt quote.
- `FederationOperation::Melt` is the consensus-visible reserve/execution
  operation. It validates inputs and change outputs, reserves proofs, creates
  pending saga state, and elects the production payment actor from the accepted
  operation ID when the payment policy requires one actor.
- `FederationOperation::MeltQuotePayment` records accepted payment observations.
  Matching paid observations finalize the melt. Matching failed observations
  roll back the pending state. Pending or unresolved observations keep inputs
  pending until a later accepted observation or operator policy resolves the
  quote.
- Minority disagreement is retained for diagnostics without vetoing a matching
  threshold. If incompatible payment results both reach threshold, the quote
  remains unresolved.

Do not rename `FederationOperation::Melt` to `MeltReserve` for operation version
1. Do not add explicit `FinalizeMelt` or `RollbackMelt` variants for operation
version 1. If a future protocol version needs those names as wire-visible
variants, introduce them with a new operation version and migration rules.

## Rationale

The current split already gives the federation the important safety property:
members agree on the exact melt request before proofs are reserved or an
outgoing payment can be attempted, and payment outcome is represented by later
accepted observations. That is enough to avoid proof reuse and avoid a member
locally finalizing or rolling back based on a private backend read.

Keeping finalization and rollback as deterministic consequences of accepted
`MeltQuotePayment` observations also avoids duplicating payment evidence across
separate finalize/rollback payloads. Operators can inspect conflicting
observation rows, and replay the same journal after restart.

The name `Melt` is less descriptive than `MeltReserve`, but changing it now
would churn operation version 1 without changing the safety invariant. The
reviewed meaning of `Melt` in federation mode is therefore: accepted melt
reserve/execution request, not terminal payment finality.

## Required Invariants

- A melt quote must exist before an accepted `Melt` operation can apply.
- The accepted `Melt` operation validates proofs, witnesses, units, fees,
  balance, and change outputs against aggregate v3/BLS federation keysets.
- Inputs enter pending state before any production outgoing payment attempt.
- For production payment policies, exactly one member attempts the outgoing
  payment, selected deterministically from the accepted operation ID.
- Non-elected members reserve inputs and wait for accepted payment observations.
- A single real-backend observation cannot finalize or roll back a quote when
  the policy requires a quorum.
- Paid quorum finalizes the saga, marks inputs spent, records the quote paid,
  signs change outputs if present, and removes pending saga state.
- Failed quorum rolls back pending input state and clears pending saga state.
- Late pending observations after paid settlement or failed rollback are
  idempotent and do not resurrect stale state.
- Restart replay must preserve pending melt state and later converge from
  accepted paid or failed observations.

## Crash-boundary matrix

This matrix documents the intentional durable boundaries. It does not imply
that one database transaction spans consensus, external payment I/O, and
finalization.

| Boundary | Durable state before the next effect | Recovery guarantee | Test evidence |
|---|---|---|---|
| Accepted `Melt` before any payment probe | Quote `Pending`, proofs `Pending`, deterministic saga in `SetupComplete`, journal entry `Applied` | A non-author or restarted member can retain the reservation without paying | `accepted_federation_melt_persists_reserve_before_payment_attempt` |
| Payment claim before backend call | Saga advances to `PaymentAttempted`; caller-supplied recovery identity is stored where the method supports it | A crash cannot make startup treat an attempted payment as an unattempted reservation | `federation_lookup_id_is_durable_before_external_payment` |
| Backend success before observation finality | Payment is recoverable under the same method identity; inputs remain pending | Lease takeover checks/continues the same payment and emits matching evidence instead of paying a distinct request | `lease_takeover_after_author_backend_success_keeps_matching_paid_evidence` |
| Accepted observation before quorum | Immutable, member-bound observation row; quote and saga remain pending below threshold | Replay/catch-up retains evidence without premature spend or rollback | `real_policy_melt_payment_observations_finalize_from_stored_quorum`, `real_policy_failed_melt_observations_rollback_from_stored_quorum` |
| Paid quorum: settlement transaction before cleanup transaction | Quote `Paid`, proofs `Spent`, saga `Finalizing`; no change signature or completed-operation row yet | Restart skips the already-committed settlement, deterministically signs change, and performs cleanup without another payment | `paid_federation_melt_replays_finalizing_cleanup_without_second_payment` |
| Change-share/signature staging before cleanup commit | Change shares, aggregate signatures, completed operation, melt-request deletion, and saga deletion share the cleanup transaction | Injected failure rolls the whole cleanup transaction back; restart recreates one share/signature set and commits once | `paid_federation_melt_replays_finalizing_cleanup_without_second_payment` |
| Failed quorum before rollback commit | Observation evidence is durable; quote/proofs/saga remain in their pending state while rollback transaction is uncommitted | Replay atomically returns the quote to `Unpaid`, releases proofs, deletes request/saga state, and does not call payment again | `failed_federation_melt_rolls_back_atomically_and_replays` |
| Terminal state followed by stale observation or another restart | Paid or rolled-back quote, terminal proof states, no saga | Late pending evidence and repeated journal replay cannot resurrect or duplicate the melt | `stale_pending_melt_observation_after_paid_settlement_is_idempotent`, `stale_pending_melt_observation_after_failed_rollback_is_idempotent`, and the repeated-restart tail of the paid-finalization test |
| Offline member restart/catch-up | Ordered journal plus durable SQLite materialization | Memory and SQLite members replay pending state, then converge on later paid or failed evidence | `restart_replay_preserves_pending_melt_then_catches_up_paid_observation`, `restart_replay_preserves_pending_melt_then_catches_up_failed_observation`, `sqlite_restart_replays_pending_melt_then_catches_up_paid_observation`, `sqlite_restart_replays_pending_melt_then_catches_up_failed_observation` |

The recovery-identity contract is still payment-method-specific. This matrix
proves the staged saga and application behavior; it does not upgrade an
unsupported backend into a crash-recoverable payment backend.

## Test Evidence

Focused tests that cover this decision include:

- `real_policy_non_elected_melt_member_reserves_without_payment_attempt`
- `melt_payment_actor_selection_is_deterministic_for_real_policies`
- `real_policy_melt_payment_observations_finalize_from_stored_quorum`
- `real_policy_failed_melt_observations_rollback_from_stored_quorum`
- `stale_pending_melt_observation_after_paid_settlement_is_idempotent`
- `stale_pending_melt_observation_after_failed_rollback_is_idempotent`
- `catch_up_resolves_pending_melt_after_later_paid_observation`
- `catch_up_rolls_back_pending_melt_after_later_failed_observation`
- `restart_replay_preserves_pending_melt_then_catches_up_paid_observation`
- `restart_replay_preserves_pending_melt_then_catches_up_failed_observation`
- `sqlite_restart_replays_pending_melt_then_catches_up_paid_observation`
- `sqlite_restart_replays_pending_melt_then_catches_up_failed_observation`

## Revisit Criteria

Revisit this decision only if one of these becomes true:

- A production BFT backend needs separate module items for reserve, finalize, or
  rollback that cannot be represented as the current operation plus accepted
  payment observation.
- Cross-implementation protocol work decides that explicit operation names are
  more important than preserving operation version 1.
- Operator/audit tooling needs terminal finalize/rollback entries that cannot be
  derived from accepted payment observations and durable saga recovery state.

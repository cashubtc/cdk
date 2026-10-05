# Melt execution leases

Align the existing CDK melt execution lock with Nutshell's renewable leases.

- Reserve the proofs and record the executor token and its 60-second deadline
  in the existing saga setup transaction.
- Use database time for expiry checks on SQLite and PostgreSQL.
- Renew every 15 seconds while dispatch and finalization run, with a total
  execution budget of 120 seconds. Stop renewal if the owner loses its lease.
- Check the owner and deadline before updating a payment identifier, recording
  a saga outcome, settling internally, finalizing, or releasing proofs.
- Recovery atomically claims an expired or released lease with a fresh token.
  It reconciles the existing payment and never dispatches another payment.
- Preserve `PaymentAttempted` when the send is uncertain. A negative status
  lookup cannot compensate it; an acknowledged send records `PaymentPending`
  or `PaymentFailed`, and a confirmed paid result can finalize.
- Terminal transactions clear ownership with the quote transition. Every other
  executor exit releases its own token without releasing reserved proofs.
  Cancellation cleanup follows the existing asynchronous transaction-drop pattern.
- Startup and payment events respect active leases. Keep the existing recovery
  scheduling and process-local quote mutexes.
- Keep the operator unlock command as an explicit override. It does not itself
  change quote or proof state.

The new migration adds `melt_lock_expires_at`, defaulting to an expired deadline
for pre-existing locks. Stop older workers before upgrading; mixed versions
would not enforce lease ownership consistently.

Validation covers renewal, expired takeover, stale writes, timeout and
cancellation, startup recovery, uncertain payment outcomes, and the shared
SQLite/PostgreSQL database contract.

# Nutshell Migration Fuzzer Hardening

The Nutshell migration integration test validates migration from the immutable
`cashubtc/nutshell:0.20.2` image into CDK. It is intentionally layered so a defect cannot
hide behind matching row counts or wallet balances.

## Reproducible state-machine execution

The fuzzer uses a deterministic default seed and prints both the seed and every completed
operation. Replay a run with:

```bash
CDK_MIGRATION_FUZZ_SEED=12345 just nutshell-migration-itest
```

Setup and post-cutover operation errors are fatal. Nutshell's FakeWallet deliberately
injects melt payment failures; those attempts are logged, retained in the source database,
and do not count as successful coverage. Wallets used by failed attempts are isolated from
later pre-cutover random operations.
Before random operations begin, the test must successfully mint, swap, melt, rotate the
Nutshell keyset, and mint from the rotated keyset. Coverage counters make an empty or
degenerate fuzz run fail.

## Interrupted-operation coverage

After stopping Nutshell, the test creates a valid pending proof from a wallet proof and
marks the wallet copy pending as well. The migration must preserve the database row as
`PENDING`. The test does not delete pending proofs or rewrite them to unspent, which used
to mask failures in interrupted melt recovery state.

The fixture also creates a pending melt with two recovery outputs inserted in the opposite
order from their `order_index`. After migration, the database API must reconstruct the
melt request and return those outputs in their original protocol order. A failed melt is
also inserted and is explicitly normalized to `UNPAID`, the safe state supported by CDK's
persisted quote schema.

Nutshell can retain the same proof Y in both `proofs_used` and `proofs_pending` after an
interrupted operation. Migration and verification treat this as one proof, with `SPENT`
taking precedence, rather than reporting a false count mismatch or attempting duplicate
inserts.

## Independent semantic manifest

The test reads the source and target databases directly without calling migration mapping
helpers. It compares canonical, stably ordered representations of:

- mint quote `amount_paid`, `amount_issued`, and identifiers;
- melt quote method, request, amount, fee, state, and payment proof;
- promises including `B_`, `C_`, DLEQ values, quote association, and `order_index`;
- spent and pending proofs including `Y`, signature, secret, witness, quote, and state;
- keyset totals for issued and redeemed ecash.

Deterministic quote fixtures cover unpaid, partially issued, overpaid, and fully issued
accounting, so cumulative fields are tested even when the FakeWallet cannot naturally
produce those states.

For every keyset, the verifier checks that issued and redeemed totals match independently
between Nutshell and CDK and that redeemed value does not exceed issued value.

## Cryptographic continuity

CDK is started with the exact Nutshell seed and derivation path. After balances are
compared, every wallet submits all of its unspent proofs through a CDK swap. This exercises
key derivation and signature verification for the outstanding ecash rather than merely
checking that database rows exist.

The test rotates Nutshell to its v2 keyset format and issues proofs from that keyset.
Nutshell 0.20.2 hashes an explicit zero expiry into the v2 ID; CDK accepts that legacy hash
while treating zero as no expiry, preserving both identifier compatibility and spendability.

## Production verification

`migrate-nutshell` automatically runs a second, independent verification pass after the
target transaction commits. Operators can repeat it without writes using `--verify-only`.
The verifier checks:

- source and target row counts for keysets, quotes, promises, and proofs;
- exact cumulative mint quote accounting;
- exact promise order indexes;
- per-keyset issued and redeemed totals.

The command validates Nutshell's `dbversions` entry and accepts only mint schema version
36 from Nutshell 0.20.2. Unsupported schemas, malformed source rows, populated targets,
count mismatches, accounting mismatches, and liability mismatches fail the command.

SQLite writes into a hidden staging database and only renames it to the requested target
after migration and verification succeed. Unit tests prove that a failed migration leaves
no target and that an existing target is rejected without modification. PostgreSQL requires
an empty target and cleans imported domain tables if migration fails.

## Backend policy

SQLite sources are tested against SQLite targets. PostgreSQL migration and its independent
verifier use the equivalent mapping and are compiled and linted in CI. Cross-engine
migration is not supported and is rejected before either database is changed.

## CI expectations

The Docker-backed test remains the authoritative end-to-end check because it creates data
through the real Nutshell 0.20.2 server. CI runs three fixed replay seeds (`137`, `202002`,
and `2002002002`). Fast unit tests cover schema-version rejection, malformed full-page
pagination, atomic failure, and existing-target protection, while compilation and strict
Clippy cover both SQLite and PostgreSQL implementations.
Failures should always include the replay seed, operation log, source/target manifest
difference, and Nutshell container logs.

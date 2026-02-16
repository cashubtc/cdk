# CDK Mint Security Audit Plan

## Executive Summary

This plan outlines a comprehensive security audit for the CDK (Cashu Development Kit) mint implementation. The CDK is a Rust implementation of the Cashu protocol - a privacy-preserving Chaumian ecash system. The mint is the server-side component responsible for issuing blind signatures, verifying proofs, and managing ecash tokens.

**Audit Scope:**
- **Primary:** `crates/cdk/src/mint/`, `crates/cdk-mintd/`, `crates/cdk-common/`
- **Cryptographic core:** `crates/cashu/` (DHKE, hash-to-curve, DLEQ, spending conditions)
- **Signing infrastructure:** `crates/cdk-signatory/` (key management, gRPC signatory, process isolation)
- **API & networking:** `crates/cdk-axum/` (HTTP/WebSocket server, auth middleware, response caching)
- **Administrative:** `crates/cdk-mint-rpc/` (management gRPC, admin operations)
- **Payment layer:** `crates/cdk-payment-processor/` (gRPC payment abstraction)
- **Storage backends:** `crates/cdk-sqlite/`, `crates/cdk-postgres/`, `crates/cdk-redb/`, `crates/cdk-sql-common/`
- **Supporting:** `crates/cdk-ffi/` (FFI bindings), `crates/cdk-prometheus/` (metrics), `crates/cdk-npubcash/` (Nostr integration)
- **Lightning backends:** `crates/cdk-cln/`, `crates/cdk-lnd/`, `crates/cdk-lnbits/`, `crates/cdk-ldk-node/`

---

## 1. Critical Security Areas

### 1.1 Replay Protection & Double-Spend Prevention
**Risk Level: CRITICAL**

**Files:**
- `crates/cdk/src/mint/verification.rs` (lines ~139-161: `check_output_already_signed()`)
- `crates/cdk/src/mint/swap/mod.rs`
- `crates/cdk/src/mint/melt/mod.rs`

**Audit Tasks:**
- [x] Verify `check_output_already_signed()` is called on all swap/melt/issue paths
- [x] Review database transaction isolation levels for blind signature queries
- [x] Test race conditions with concurrent requests for same outputs
- [x] Verify Y-value (blinded secret) uniqueness checks
- [x] Check database indices on blind_signatures table for performance
- [x] Audit saga compensation logic for proper rollback on failure

**Audit Findings (Section 1.1):**

> **Overall Assessment: WELL-ARCHITECTED.** The replay protection and double-spend prevention mechanisms use defense-in-depth with three layers of protection. No critical vulnerabilities found. Several informational and low-risk findings documented below.

**1.1.F1 -- All critical paths correctly protected (CONFIRMED SAFE)**
- **Swap:** `check_output_already_signed()` called inside `setup_swap()` via `self.mint.verify_outputs(&mut tx, blinded_messages)` within TX1. (`swap_saga/mod.rs` ~line 195)
- **Melt:** Called inside `setup_melt()` via `self.mint.verify_outputs(&mut tx, outputs)` for change outputs within TX1. (`melt_saga/mod.rs` ~line 349)
- **Issue:** Called inside `process_mint_request()` via `self.verify_outputs(&mut tx, &mint_request.outputs)` within the DB transaction. (`issue/mod.rs` ~line 660)
- No bypass paths identified across all three flows.

**1.1.F2 -- Three-layer defense-in-depth (CONFIRMED SAFE)**
- **Layer 1 (In-memory):** `check_inputs_unique()` and `check_outputs_unique()` use `HashSet` deduplication to fast-fail duplicate Y/B values within a single request. (`verification.rs` lines 18-53)
- **Layer 2 (Transactional):** `check_output_already_signed()` queries within `FOR UPDATE` transactions. `add_proofs()` checks proof state with `FOR UPDATE`. (`verification.rs` lines 139-161; `cdk-sql-common/src/mint/proofs.rs` lines 130-188)
- **Layer 3 (Database constraint):** PRIMARY KEY on `proof.y` and `blind_signature.blinded_message` provides ultimate uniqueness backstop.

**1.1.F3 -- Race condition analysis (CONFIRMED SAFE)**
- **Concurrent swap with same proofs:** Second request blocks on `FOR UPDATE` (PG) or `BEGIN IMMEDIATE` (SQLite), then finds proofs in `Pending` state -> `Duplicate` error.
- **Concurrent swap with same blinded messages:** PRIMARY KEY constraint catches duplicates -> `Duplicate` error.
- **Concurrent melt with same quote:** `load_melt_quotes_exclusively()` acquires exclusive locks on target + all related quotes (preventing BOLT12 deadlocks via consistent ordering).
- **Concurrent mint issue on same quote:** `get_mint_quote()` with `FOR UPDATE` inside TX; second request sees `Issued` state -> error.

**1.1.F4 -- SQLite vs PostgreSQL isolation equivalence (LOW RISK)**
- PostgreSQL uses `READ COMMITTED` + `SELECT ... FOR UPDATE` for row-level locking.
- SQLite uses `BEGIN IMMEDIATE` (database-level write lock). `FOR UPDATE` clauses are silently stripped via `trim_end_matches("FOR UPDATE")` in `cdk-sqlite/src/async_sqlite.rs:32`.
- SQLite provides **equivalent or stronger** isolation (database-wide lock vs row-level) at the cost of lower write concurrency.
- **Fragility note:** The `trim_end_matches("FOR UPDATE")` approach only works when `FOR UPDATE` is the final clause. Future query patterns using `FOR UPDATE SKIP LOCKED` or `FOR UPDATE NOWAIT` would not be stripped correctly. Not currently exploitable but worth tracking.

**1.1.F5 -- SQLite missing partial unique index on melt_quote (MODERATE)**
- PostgreSQL enforces `CREATE UNIQUE INDEX unique_pending_paid_lookup_id ON melt_quote(request_lookup_id) WHERE state IN ('PENDING', 'PAID')` at the database level.
- SQLite migration (`20251127000000_allow_duplicate_melt_request_lookup_id.sql`) only creates a non-unique index, with a comment noting the constraint is "enforced in application logic".
- SQLite does support partial indexes (`CREATE UNIQUE INDEX ... WHERE`), so this gap could be closed.
- **Risk:** A bug in application logic could allow two melt quotes for the same payment to simultaneously reach `PENDING`/`PAID` state in SQLite, whereas PostgreSQL would reject the second at the DB level.
- **Recommendation:** Add the equivalent partial unique index for the SQLite backend.

**1.1.F6 -- Saga compensation: logging-only on failure (LOW RISK)**
- In both `SwapSaga::compensate_all()` and `MeltSaga::compensate_all()`, individual compensation actions that fail are logged with `tracing::error!` but execution continues.
- If compensation fails (e.g., DB connection lost), proofs may remain stuck in `Pending` state until the next startup recovery cycle.
- **Mitigated by:** Startup saga recovery retries all incomplete sagas before accepting requests.

**1.1.F7 -- No periodic background recovery task (LOW RISK)**
- Saga recovery runs only at startup (`Mint::start()` lines 268-282), before payment processors and API handlers start.
- Permanently stuck sagas (e.g., `PaymentAttempted` with LN backend returning `Pending` indefinitely) remain orphaned until the next process restart.
- On-demand resolution exists via `handle_pending_melt_quote` (triggered when a client checks melt quote status), but there is no proactive background sweep.
- **Recommendation:** Consider a periodic background task to resolve stuck sagas without requiring restart.

**1.1.F8 -- SQLite durability setting (INFORMATIONAL)**
- SQLite uses `synchronous = normal` (`cdk-sqlite/src/common.rs` lines 70-71), which in a power failure could lose the last few committed transactions.
- For a financial system, `synchronous = full` would provide stronger durability guarantees at the cost of write performance.
- With WAL mode, `synchronous = normal` is generally considered safe in practice for non-catastrophic failures.

**Key Questions (Answered):**
- **Is there a TOCTOU vulnerability?** No for swap/melt -- all checks and mutations happen within single transactions. Minor for issue flow (signing outside TX, but TX re-validates state and outputs via `verify_outputs()` and `FOR UPDATE` quote lock). Not exploitable.
- **Can an attacker exploit database replication lag in multi-node setups?** Not applicable for SQLite (single-node). For PostgreSQL, `READ COMMITTED` + `FOR UPDATE` prevents this within a single PG instance. Multi-master replication setups would require `SERIALIZABLE` isolation or application-level coordination -- this deployment pattern is not currently documented or supported.
- **Are failed transaction rollbacks guaranteed to remove reserved signatures?** Yes -- the `Drop` impl on `ConnectionWithTransaction` spawns a `ROLLBACK` task (`cdk-sql-common/src/database.rs` lines 99-111). Additionally, saga compensation logic specifically targets blinded message rows where `c IS NULL` (unsigned), preventing accidental removal of completed signatures.

---

### 1.2 Authentication & Authorization (NUT-21/NUT-22)
**Risk Level: HIGH**

**Files:**
- `crates/cdk/src/mint/auth/mod.rs` (lines ~145-207: `check_blind_auth_proof_spendable()`)
- `crates/cdk/src/oidc_client.rs` (line ~180: audience validation disabled)
- `crates/cdk-mintd/src/config.rs` (lines ~510-560: auth configuration)
- `crates/cdk-axum/src/auth.rs` (auth middleware and header extraction)

**Audit Tasks:**
- [x] Review blind auth token spending for race conditions in `check_blind_auth_proof_spendable()`
- [x] **FINDING**: OIDC audience validation is disabled (`validate_aud = false` at `oidc_client.rs:180`). The fallback `client_id`/`azp` claim check only runs if `client_id` is `Some`, which is not guaranteed by `OidcClient::new()`
- [x] Verify clear auth (OIDC) token replay protection
- [x] Test endpoint protection configuration enforcement
- [x] Audit auth token signature verification
- [x] Check auth keyset isolation from regular keysets
- [x] Verify auth token amount/keyset validation
- [x] Verify `AuthHeader` extraction in `cdk-axum/src/auth.rs` handles malformed headers safely

**Audit Findings (Section 1.2):**

> **Overall Assessment: MOSTLY SOUND with two MODERATE findings and one HIGH finding.** The blind auth token spending mechanism is well-designed with transactional protection. OIDC validation has a known audience bypass that is partially mitigated. A significant auth keyset isolation gap exists in the swap path.

**1.2.F1 -- Blind auth token race condition analysis (CONFIRMED SAFE)**
- `check_blind_auth_proof_spendable()` (`auth/mod.rs` lines 144-210) operates within a single database transaction.
- Flow: `begin_transaction()` -> `add_proof()` (INSERT, ignores duplicates) -> `update_proof_state(y, Spent)` (SELECT ... FOR UPDATE + UPDATE) -> check previous state -> `commit()`.
- **PostgreSQL:** `SELECT ... FOR UPDATE` at `cdk-sql-common/src/mint/auth/mod.rs:153` provides row-level locking. Two concurrent requests for the same proof: second blocks on `FOR UPDATE`, then sees `Spent` state -> `TokenAlreadySpent`.
- **SQLite:** `FOR UPDATE` is silently stripped (see 1.1.F4), but SQLite's `BEGIN IMMEDIATE` provides database-level write locking, which is strictly stronger. Second concurrent request blocks at transaction start.
- **Conclusion:** Double-spending of blind auth tokens is prevented on both backends.

**1.2.F2 -- OIDC audience validation disabled (MODERATE)**
- `oidc_client.rs:180`: `validation.validate_aud = false` disables the `jsonwebtoken` crate's built-in audience validation.
- The fallback check (lines 189-212) examines `client_id` and `azp` claims, but has three weaknesses:
  1. If the JWT contains neither `client_id` nor `azp` claim, no client/audience check runs at all -- the token is accepted. Many OIDC providers include `aud` but not `client_id`/`azp` in access tokens.
  2. If `client_id` claim is present but its value is not a string (e.g., an array), `as_str()` returns `None` and the check is silently skipped.
  3. If `azp` claim is present but its value is not a string, same silent skip.
- **Mitigating factor:** The mint constructor (`mod.rs:237-242`) always passes `Some(nut21.client_id.clone())` when creating the `OidcClient`, so `self.client_id` is always `Some` on the mint side. The `None` case only occurs in wallet/CLI code.
- **Validated properties:** Signature (via JWK), expiration (`validate_exp = true`), issuer (`set_issuer`). These are all correct.
- **Risk:** A JWT issued by the same OIDC provider for a different service/audience would be accepted by the mint, as long as it doesn't contain a conflicting `client_id` or `azp` claim.
- **Recommendation:** Enable `validate_aud = true` and add the `client_id` to `set_audience()`, or at minimum add `aud` claim checking to the fallback logic.

**1.2.F3 -- Clear auth (OIDC) replay protection (ACCEPTABLE)**
- OIDC JWTs are **stateless bearer tokens** -- there is no per-use spending or replay tracking.
- The same JWT can be reused for multiple requests until it expires.
- This is standard OIDC behavior and is acceptable because:
  - Clear auth is used primarily for minting blind auth tokens (`POST /v1/auth/blind/mint`), which are rate-limited by `bat_max_mint`.
  - The `validate_exp = true` setting ensures tokens expire.
  - There is no revocation mechanism for individual JWTs, but token lifetimes are typically short (minutes to hours).
- **No vulnerability here** -- this is by design.

**1.2.F4 -- Endpoint protection configuration enforcement (CONFIRMED SAFE)**
- All protected endpoints consistently call `state.mint.verify_auth(auth.into(), &ProtectedEndpoint::new(...))`:
  - `router_handlers.rs`: `post_swap` (line 252), `post_check` (line 187), `post_restore` (line 290), `ws_handler` (line 156).
  - `custom_handlers.rs`: All 6 payment-method endpoints (mint quote, check mint quote, mint, melt quote, check melt quote, melt).
- Unprotected endpoints (keys, keysets, mint_info) correctly skip auth -- these are public per spec.
- `POST /v1/auth/blind/mint` correctly requires `ClearAuth` specifically (`auth.rs:148-162`), rejecting blind auth or no auth.
- Endpoint auth types are persisted to the auth database at startup (`lib.rs:841-845`), and unprotected endpoints are explicitly removed from the DB (`lib.rs:843`).
- Configuration defaults are appropriate: `swap`, `mint`, `restore`, and `websocket_auth` default to `Blind`; quote and check endpoints default to `None`.

**1.2.F5 -- Auth token signature verification (CONFIRMED SAFE)**
- Blind auth: `verify_blind_auth()` (`auth/mod.rs:36-40`) converts `AuthProof` to `Proof` (with hardcoded `amount: 1` at `nut22.rs:177`) and delegates to `signatory.verify_proofs()`.
- The signatory (`db_signatory.rs:156-165`) looks up the keyset by ID, retrieves the key pair for the specified amount, and calls `verify_message()` which performs BDHKE signature verification.
- Clear auth: `verify_clear_auth()` delegates to `OidcClient::verify_cat()` which verifies JWT signature via JWK, expiration, and issuer.
- **Both paths are cryptographically sound.** No bypass identified.

**1.2.F6 -- Auth keyset isolation gap in swap path (HIGH)**
- **API-level listing is properly isolated:** `pubkeys()` and `keysets()` (`keysets/mod.rs:35,49`) filter out `CurrencyUnit::Auth` keysets. Auth keysets are only exposed via dedicated `/v1/auth/blind/keys` and `/v1/auth/blind/keysets` endpoints.
- **Proof spend tracking is properly isolated:** Blind auth proofs are tracked in a separate `auth_localstore` database, completely independent from the regular `localstore`.
- **However:** `keyset_pubkeys(keyset_id)` (`keysets/mod.rs:15-24`) does NOT filter by unit. If an attacker knows an auth keyset ID (discoverable via the auth endpoints), they can query its public keys via `GET /v1/keys/{keyset_id}`.
- **GAP in swap path:** The `verify_inputs_keyset()` and `verify_outputs_keyset()` functions (`verification.rs:59-136`) do NOT reject `CurrencyUnit::Auth`. The `verify_transaction_balanced()` function only checks that input and output units match. The swap saga uses `self.localstore` (regular DB) for proof tracking, while auth spending uses `auth_localstore`. These are separate databases with no cross-visibility.
- **Attack scenario -- unlimited auth from a single token:**
  1. Attacker has one legitimate auth token A.
  2. Attacker submits a swap: input = proof A (Auth keyset), output = blinded message for Auth keyset. Swap succeeds. Proof A is marked "spent" in the **regular** `localstore`. Attacker receives new valid auth token B.
  3. Attacker uses token B for authentication. `check_blind_auth_proof_spendable()` checks **`auth_localstore`** -- token B has never been seen there. Auth succeeds, B is marked spent in `auth_localstore`.
  4. **Repeatable:** Token A is only spent in `localstore`, not in `auth_localstore`. But more importantly, each swap produces a fresh token. The attacker can swap B in the regular DB (where B is unseen) to get C, use C for auth, swap C to get D, etc. Each cycle produces one free auth use.
  - A single swap does not produce multiple auth tokens -- it exchanges one for one. But the **two-database split** means the "spent" marks never cross over, enabling infinite cycling.
- **Practical impact:** An attacker with one auth token can perform unlimited authenticated requests without needing additional OIDC authentication. This bypasses the `bat_max_mint` rate limit and defeats the purpose of blind auth token scarcity. No monetary value is at risk (auth tokens have no financial denomination), but the auth rate-limiting/anti-spam purpose of NUT-22 is fully defeated.
- **Recommendation:** Add a check in `verify_inputs_keyset()` and/or `verify_outputs_keyset()` to reject `CurrencyUnit::Auth` keysets. For example: `if unit == CurrencyUnit::Auth { return Err(Error::UnitMismatch); }`.

**1.2.F7 -- Auth token amount and keyset validation (CONFIRMED SAFE)**
- `mint_blind_auth()` (`issue/auth.rs:42-44`) enforces `amount == 1` for every blinded message in a mint auth request.
- `MintAuthRequest::amount()` (`nut22.rs:267`) returns `outputs.len() as u64`, checked against `bat_max_mint` (`issue/auth.rs:31-37`).
- The `AuthProof -> Proof` conversion (`nut22.rs:177`) hardcodes `amount: 1.into()`, preventing amount manipulation at the type level.
- Auth keysets are created with `max_order = 1` (`builder.rs:406`), meaning only the `amount=1` key exists. Attempting to verify an auth proof with any other amount would fail with `UnknownKeySet` in the signatory.

**1.2.F8 -- AuthHeader extraction safety (CONFIRMED SAFE)**
- `FromRequestParts` impl (`auth.rs:41-87`) handles malformed headers gracefully:
  - Non-UTF8 header values: `to_str()` returns error -> `400 Bad Request` with generic message.
  - Invalid `BlindAuthToken` format: `BlindAuthToken::from_str()` returns error -> `400 Bad Request`.
  - Missing headers: returns `AuthHeader::None` (valid -- auth enforcement happens later in `verify_auth()`).
- **Blind-auth takes precedence** over Clear-auth if both headers are present (`auth.rs:49-68`). The Clear-auth header is silently ignored. This is not a vulnerability but is worth documenting.
- No panic paths, no information disclosure in error messages.

**1.2.F9 -- Response cache auth bypass (LOW RISK)**
- Cached mint/melt responses (`custom_handlers.rs:384-386, 527-533`) are returned without re-checking auth.
- Cache key is computed from the request body (not the auth header), so two different users with the same request payload would get the same cached response.
- **Mitigating factor:** For mint operations, the request body contains unique blinded messages (different for each user). For melt operations, the request contains a unique quote ID. In practice, cache key collisions between different users are extremely unlikely.
- **Theoretical risk:** If an authenticated user's response is cached and the endpoint's auth configuration is later changed to require auth, the cached response could be served to unauthenticated users until cache expiry. This is a very narrow timing window.

**1.2.F10 -- Misleading error type in auth localstore check (INFORMATIONAL)**
- `check_blind_auth_proof_spendable()` (`auth/mod.rs:156`) returns `Error::AmountKey` when `auth_localstore` is `None`. This should be a dedicated error like `Error::AuthNotConfigured` for clearer debugging.

**Key Questions (Answered):**
- **Can the same blind auth token be spent twice in concurrent requests?** No -- database transaction isolation prevents this on both PostgreSQL and SQLite. See 1.2.F1.
- **Are OIDC tokens validated (signature, expiration, issuer, audience)?** Signature, expiration, and issuer: yes. Audience: no (`validate_aud = false`). Partial `client_id`/`azp` fallback exists but has gaps. See 1.2.F2.
- **Can a JWT from a different service using the same OIDC provider be accepted?** Yes, if it doesn't contain a conflicting `client_id` or `azp` claim. See 1.2.F2.
- **Is there a revocation mechanism for compromised auth tokens?** No individual token revocation exists. Clear auth relies on JWT expiration. Blind auth is single-use (burned on spend).
- **What happens when `client_id` is `None`?** On the mint side, `client_id` is always `Some` (set from NUT-21 config). On wallet/CLI side, it's `None`, but this is not a security concern since the wallet is the token consumer, not the validator.

---

### 1.3 Saga Pattern & Transaction Atomicity
**Risk Level: HIGH**

**Files:**
- `crates/cdk/src/mint/swap/swap_saga/mod.rs` (typestate: Init -> Setup -> Signed -> Finalized)
- `crates/cdk/src/mint/swap/swap_saga/compensation.rs`
- `crates/cdk/src/mint/melt/melt_saga/mod.rs` (typestate for melt operations)
- `crates/cdk/src/mint/melt/melt_saga/compensation.rs`
- `crates/cdk/src/mint/saga_recovery.rs`

**Note:** The mint/issue flow does **not** use a saga pattern. It uses a direct DB transaction approach (see Section 1.6). Only swap and melt operations use sagas.

**Audit Tasks:**
- [x] Verify saga state machine correctness for both swap and melt
- [x] Test saga recovery after crashes at each state
- [x] Check saga compensation logic completeness
- [x] Audit saga state persistence durability
- [x] Verify saga operation idempotency
- [x] Test concurrent sagas with overlapping inputs
- [x] Verify the issue flow's direct transaction approach has equivalent safety guarantees

**Audit Findings (Section 1.3):**

> **Overall Assessment: WELL-ARCHITECTED with one HIGH finding (recovery errors non-fatal) and several MEDIUM observations.** The saga pattern uses compile-time typestate enforcement, write-ahead logging for crash safety, and idempotent recovery. The issue flow's single-transaction approach is actually safer than the saga pattern for its use case. No critical fund-loss vulnerabilities found.

**1.3.F1 -- Swap saga state machine correctness (CONFIRMED SAFE)**
- Three compile-time states: `Initial` -> `SetupComplete` -> `Signed` -> (consumed, returns `SwapResponse`). (`swap_saga/state.rs:10-35`)
- Typestate pattern enforced via Rust generics: `SwapSaga<Initial>` only has `setup_swap()`, `SwapSaga<SetupComplete>` only has `sign_outputs()`, `SwapSaga<Signed>` only has `finalize()`. Invalid transitions (e.g., calling `finalize()` on `SetupComplete`) do not compile.
- Each transition consumes `self`, preventing reuse of a previous state.
- **Persisted saga state:** Only `SetupComplete` is persisted to DB (`swap_saga/mod.rs:235`). The `Signed` state is intentionally NOT persisted because recovery always compensates from `SetupComplete` (signing has no persistent side effects).
- **TX1 (setup_swap, `mod.rs:150-242`):** Single atomic transaction covering: verify outputs, verify balance, add proofs, update proofs to Pending, add blinded messages, persist saga. All-or-nothing.
- **sign_outputs (`mod.rs:292-323`):** Non-transactional call to signatory. On failure, compensates all. On success, transitions to `Signed` in-memory only.
- **TX2 (finalize, `mod.rs:366-443`):** Single atomic transaction covering: add blind signatures, re-get proofs, update proofs to Spent, add completed operation, delete saga (best-effort). All-or-nothing.

**1.3.F2 -- Melt saga state machine correctness (CONFIRMED SAFE, MORE COMPLEX)**
- Three compile-time states: `Initial` -> `SetupComplete` -> `PaymentConfirmed` -> (consumed). (`melt_saga/state.rs:13-41`)
- Three persisted saga states: `SetupComplete`, `PaymentAttempted`, `Finalizing`. (`cdk-common/src/mint.rs:154-161`)
- Key design: `PaymentAttempted` state is persisted BEFORE the LN payment call (write-ahead log) at `melt_saga/mod.rs:715-722`. This ensures that if the process crashes during payment, recovery knows payment may have been sent and checks the LN backend rather than blindly compensating.
- The `Finalizing` state is set during finalize after proofs are marked Spent and quote is marked Paid, but before change signatures are generated (`melt_saga/mod.rs:976-981`). This allows recovery to re-sign change outputs without re-finalizing the core operation.
- **TX1 (setup_melt):** Atomic transaction covering: lock quote exclusively, add proofs, update to Pending, update quote to Pending, verify change outputs, add melt request, add blinded messages, persist saga.
- **Payment phase:** External LN call (`melt_saga/mod.rs:735-740`) or internal settlement (`mod.rs:486-556`), each in separate transactions from TX1.
- **TX2 (finalize):** Spans up to 3 sub-transactions with an external signatory call between them (see 1.3.F5).

**1.3.F3 -- Recovery errors do not prevent startup (HIGH)**
- `Mint::start()` (`mod.rs:268-282`) wraps both `recover_from_incomplete_sagas()` and `recover_from_incomplete_melt_sagas()` in `if let Err(e)` blocks that log but do NOT fail startup.
- **Consequence:** If recovery fails (e.g., DB connectivity issue, LN backend unreachable), the mint starts accepting new requests while incomplete sagas remain unrecovered.
  - For swap sagas: Proofs remain in `Pending` state, locking them from legitimate use but not causing fund loss.
  - For melt sagas in `PaymentAttempted` state: A paid-but-not-finalized melt remains incomplete. The user cannot retry (proofs locked), and the LN payment is already sent. This is a temporary fund-lock, not a fund-loss, since `handle_pending_melt_quote()` can resolve it on-demand when the client checks quote status.
  - For melt sagas in `Finalizing` state: Proofs are already Spent and quote is Paid, but change signatures are not yet stored. The user would need to check quote status to trigger recovery via `handle_pending_melt_quote()`.
- **Mitigating factor:** `handle_pending_melt_quote()` (`start_up_check.rs:631-666`) provides on-demand recovery when a client checks a pending melt quote, so sagas can be resolved without a full restart.
- **Recommendation:** Consider failing startup (or at minimum retrying) if recovery for `PaymentAttempted` or `Finalizing` sagas fails. `SetupComplete` sagas are less critical since no payment was sent.

**1.3.F4 -- Compensation failure silently swallowed (MEDIUM, confirms 1.1.F6)**
- Both `SwapSaga::compensate_all()` (`swap_saga/mod.rs:482-488`) and the melt saga's equivalent (`melt_saga/mod.rs:1088-1093`) log compensation errors but always return `Ok(())`.
- If compensation fails (e.g., DB connection lost during rollback), the system enters a state where:
  - Proofs are stuck in `Pending` (not spendable, not freed)
  - The saga record remains in the database
  - The user receives the original error (signing failed, finalize failed, etc.)
- **Recovery path:** Startup recovery re-reads sagas from DB and retries compensation. Additionally, `handle_pending_melt_quote()` can resolve stuck melt sagas on-demand.
- **Gap:** No equivalent on-demand recovery exists for stuck swap sagas -- they require a restart.
- **Recommendation:** Consider adding background periodic recovery or an on-demand swap saga recovery endpoint.

**1.3.F5 -- Melt finalize spans multiple transaction boundaries (MEDIUM)**
- `finalize()` in the melt saga spans up to 3 database transactions with an external signatory call between them:
  1. **TX2a** (`melt_saga/mod.rs:923-981`): Mark proofs Spent, update quote to Paid, set saga to `Finalizing`, commit.
  2. **External call**: `blind_sign()` for change outputs (via `shared.rs:263`).
  3. **TX2b** (`shared.rs:266-276`): Store change blind signatures.
  4. **TX2c** (`melt_saga/mod.rs:994-1020`): Delete tracking records, add completed operation, commit.
- **Crash between TX2a and TX2b:** Proofs are Spent, quote is Paid, but change signatures are NOT stored. Saga is in `Finalizing` state.
- **Recovery:** `start_up_check.rs:365-413` detects `Finalizing` state and calls `finalize_paid_melt_quote()`, which re-signs change and stores them idempotently (`shared.rs:551-556` handles already-Paid quotes, `shared.rs:577-601` handles existing change signatures).
- **Conclusion:** The multi-transaction design is necessary to avoid holding DB locks during external signatory calls. The `Finalizing` state provides the crash-safety net. Recovery is idempotent and correct.

**1.3.F6 -- Swap saga compensation removes proofs entirely (CONFIRMED SAFE)**
- `RemoveSwapSetup::execute()` (`compensation.rs:57`) calls `tx.remove_proofs(&self.input_ys, None)` which deletes proof records entirely, rather than restoring to `Unspent`.
- This is correct because `setup_swap()` adds the proofs fresh via `add_proofs()` at `mod.rs:189`. If the same proofs already existed, `add_proofs()` would have returned `Duplicate` error. So compensation only removes records that were freshly created.
- The wallet retains the original proofs and can retry.

**1.3.F7 -- Saga state persistence durability (CONFIRMED SAFE)**
- Saga state is persisted atomically within the same TX that modifies proof/quote state:
  - Swap: `add_saga()` at `swap_saga/mod.rs:237` is within TX1 (committed at line 242).
  - Melt: `add_saga()` at `melt_saga/mod.rs:396-399` is within TX1 (committed at line 401).
  - Melt state updates: `update_saga()` to `PaymentAttempted` at `melt_saga/mod.rs:717-720` is in its own TX; `update_saga()` to `Finalizing` at `melt_saga/mod.rs:976-980` is within TX2a.
- Saga deletion is best-effort in both flows (swap: `mod.rs:435-441`, melt: `mod.rs:997-1000`). Failed deletion leaves an orphaned saga that startup recovery handles.
- **Cross-ref with 1.1.F8:** SQLite `synchronous = normal` could theoretically lose the last committed saga state in a power failure, but this is an accepted tradeoff (see 1.1.F8).

**1.3.F8 -- Saga operation idempotency (CONFIRMED SAFE)**
- **Swap recovery:** Always compensates (rolls back). `RemoveSwapSetup` handles the case where proofs or blinded messages don't exist (empty lists are no-ops). Saga deletion is best-effort.
- **Melt recovery -- `Finalizing` state:** `finalize_melt_quote()` (`shared.rs:505-620`) handles idempotent re-execution:
  - At `shared.rs:551-556`: If quote is already `Paid`, skips core finalization (proofs already Spent).
  - At `shared.rs:577-601`: Checks if change signatures already exist before re-signing.
- **Melt recovery -- `PaymentAttempted` state:** Checks LN backend first. If `Paid`, finalizes. If `Unpaid`/`Failed`, compensates. If `Pending`/`Unknown`, skips (retries later).
- **Melt recovery -- `SetupComplete` state:** Always compensates (payment was never attempted). `rollback_melt_quote()` (`shared.rs:76-171`) handles already-removed proofs gracefully (`shared.rs:106-111`: `AttemptRemoveSpentProof` logged and continued).

**1.3.F9 -- Concurrent sagas with overlapping inputs (CONFIRMED SAFE)**
- **Same proofs used in concurrent swaps:** `add_proofs()` returns `Duplicate` (proofs already pending) or `AttemptUpdateSpentProof` (already spent). Second saga fails immediately. (`swap_saga/mod.rs:194-201`)
- **Same blinded messages in concurrent swaps:** `add_blinded_messages()` returns `Duplicate` -> `DuplicateOutputs` error. (`swap_saga/mod.rs:216-224`)
- **Same quote in concurrent melts:** `load_melt_quotes_exclusively()` (`shared.rs:309-352`) acquires `SELECT ... FOR UPDATE` locks on the quote and all related quotes (BOLT12 dedup). Second saga blocks on lock, then sees `Pending` state -> `PendingQuote` error.
- **Compensation vs. concurrent retry:** If compensation is in progress (deleting proofs) while a new saga tries to `add_proofs()` for the same proofs, database-level transaction isolation ensures the new saga either sees the proofs still exist (and fails with `Duplicate`) or sees them removed (and succeeds). No inconsistency.

**1.3.F10 -- Concurrent recovery and live operations (MEDIUM)**
- Startup recovery (`recover_from_incomplete_melt_sagas`) does NOT acquire exclusive locks before reading saga state. `handle_pending_melt_quote()` can run concurrently.
- **Scenario:** Recovery reads a saga as `PaymentAttempted`. Concurrently, `handle_pending_melt_quote()` calls `process_melt_saga_outcome()` which finalizes the saga (marks proofs Spent, quote Paid). Recovery then also tries to finalize.
- **Safety:** `finalize_melt_quote()` acquires exclusive locks via `load_melt_quotes_exclusively()` (`shared.rs:520`). The second finalizer blocks on the lock, then sees the quote is already `Paid` and skips (`shared.rs:551-556`). Saga deletion is also idempotent (error is logged but not fatal).
- **Conclusion:** Concurrent execution is safe due to database-level locking and idempotent recovery logic.

**1.3.F11 -- `melt_async()` spawns untracked background task (MEDIUM)**
- `melt/mod.rs:613-662` spawns a `tokio::spawn` task for the entire melt saga (settlement -> payment -> finalize). The `JoinHandle` is not stored, so there is no graceful shutdown handling.
- If the mint shuts down gracefully, this task could be killed mid-payment. The saga persistence ensures correctness on restart, but mid-operation cancellation means:
  - The LN payment might have been sent but not confirmed
  - Recovery on next startup will check LN backend and resolve
- If `finalize()` fails in the background task (`melt/mod.rs:629-638`), the error is logged but no compensation runs. This is correct for the melt saga post-payment (compensation would lose funds), but the saga remains stuck until the next restart or on-demand check.
- **Recommendation:** Store `JoinHandle` for graceful shutdown, or implement a periodic background sweep for stuck sagas.

**1.3.F12 -- Issue flow uses single-transaction approach (CONFIRMED SAFE, SUPERIOR FOR ITS USE CASE)**
- `process_mint_request()` (`issue/mod.rs:585-755`) uses a fundamentally different pattern from swap/melt:
  1. Read quote and check payment status (outside TX) -- lines 592-597
  2. Compute blind signatures (outside TX, external signatory call) -- line 604
  3. Single DB transaction: re-read quote (with lock), validate state, add blinded messages, add signatures, update quote to Issued, commit -- lines 606-735
  4. Publish notification -- lines 737-738
- **Blind signing before TX is safe:** Signatures are computed at line 604 but NOT returned to the client until after `tx.commit()` succeeds at line 735. If any step fails (DB errors, state validation), the error propagates via `?` and the client never receives the signatures. The computed signatures are lost (stateless, no persistent side effect).
- **No saga needed:** The entire state mutation is in a single atomic TX. Either the quote transitions to `Issued` with signatures stored, or nothing happens. No crash recovery needed beyond DB transaction atomicity.
- **This is safer than the saga approach** because there is no multi-step state machine that can leave partial state. The tradeoff is that the signatory call happens outside the TX (to avoid holding DB locks during external calls), but this is acceptable because the signatory operation is stateless.
- **Crash scenario:** If the mint crashes after `blind_sign()` (line 604) but before `tx.commit()` (line 735), the signatures are lost and the quote remains `Paid`. The client retries and gets new signatures. No inconsistency.
- **Double-read optimization:** The quote is read once outside the TX (line 592-596) to trigger LN payment checking, then again inside the TX (line 608-611) for the authoritative state check. This is correct -- the first read is an optimization, the second is the guard.

**1.3.F13 -- Fee calculation outside transaction context (LOW)**
- In both swap and melt sagas, `get_proofs_fee()` is called on `self.mint` rather than through the transaction:
  - Swap: `swap_saga/mod.rs:174`
  - Melt: `melt_saga/mod.rs:244`
- The fee calculation reads keyset information via `ArcSwap`. If keysets were rotated between this read and the transaction commit, the fee could be stale. However, fees are immutable once a keyset is created (the `fee_ppk` field does not change after creation), so this is not exploitable.

**1.3.F14 -- Backward compatibility fallback for old melt sagas (LOW)**
- `start_up_check.rs:243-301`: When `saga.quote_id` is `None` (old saga format without the field), recovery iterates ALL melt quotes and checks for proof Y overlap to find the matching quote.
- This opens a transaction per quote just to read proof Ys, then rolls it back.
- Uses `any()` overlap check which could theoretically match the wrong quote if proofs were somehow shared.
- **Risk:** Low. This is a backward-compatibility path for sagas created before the `quote_id` field was added. New sagas always have `quote_id`.

**Key Questions (Answered):**
- **Can a crash during `sign_outputs()` leave inputs spent but outputs unsigned?** No. During `sign_outputs()`, inputs are in `Pending` state (not `Spent`) and no DB changes occur. If signing fails or the process crashes, the saga remains in `SetupComplete` state in the DB. Recovery compensates by removing the Pending proofs and blinded messages. The proofs only transition to `Spent` in TX2 (`finalize()`), which is atomic with adding signatures.
- **Is saga recovery safe to run concurrently?** Yes, due to database-level locking (`load_melt_quotes_exclusively()` for melts) and idempotent recovery logic (see 1.3.F10). However, there is no explicit mutex -- safety relies on DB transaction isolation.
- **Are compensation actions guaranteed to succeed?** No. Compensation failures are logged but swallowed (see 1.3.F4). This can leave proofs stuck in `Pending` state until the next restart recovery cycle or on-demand check. This is an accepted tradeoff documented in 1.1.F6.
- **Is the non-saga issue flow at risk of inconsistency that sagas would prevent?** No. The issue flow is actually safer than the saga approach because it uses a single atomic DB transaction. See 1.3.F12 for detailed analysis.

**Cross-references from Section 1.1 audit:**
- See **1.1.F6**: Compensation failure is log-only; proofs can get stuck in `Pending` state until next startup recovery. Confirmed and expanded in 1.3.F4.
- See **1.1.F7**: No periodic background recovery task; stuck melt sagas remain orphaned until restart. Confirmed in 1.3.F11. Partially mitigated by `handle_pending_melt_quote()` for on-demand resolution.
- Swap saga `sign_outputs()` state is intentionally not persisted -- recovery always compensates from `SetupComplete`, which is safe because no external side effects occurred.
- Melt saga recovery correctly refuses to compensate after `PaymentAttempted` state; checks LN backend first.

---

### 1.4 Proof Verification & Spending Conditions
**Risk Level: CRITICAL**

**Files:**
- `crates/cdk/src/mint/verification.rs`
- `crates/cdk/src/mint/swap/mod.rs` (lines ~25-30: spending condition verification)
- `crates/cdk/src/mint/melt/mod.rs` (line ~545: `verify_spending_conditions()`)
- `crates/cashu/src/nuts/nut10.rs` (spending condition framework)
- `crates/cashu/src/nuts/nut11/mod.rs` (P2PK: signatures, locktime, multisig, SIG_ALL)
- `crates/cashu/src/nuts/nut14/mod.rs` (HTLC: hash preimage, locktime, refund paths)

**Audit Tasks:**
- [x] Verify spending condition validation (P2PK, HTLC, locktime)
- [x] Test input/output amount balancing with fees
- [x] Check keyset validity and activation status
- [x] Audit signature verification delegation to signatory
- [x] Verify uniqueness checks (Y-values, blinded secrets)
- [x] Test max input/output limits enforcement
- [x] Audit P2PK multisig threshold logic and duplicate signature detection
- [x] Verify HTLC hash preimage length and type validation
- [x] Test locktime enforcement edge cases (exactly at expiry, clock skew)
- [x] Verify SIG_ALL prevents output manipulation

**Audit Findings (Section 1.4):**

> **Overall Assessment: WELL-DESIGNED with one MEDIUM finding (fee calculation unchecked multiply), one LOW finding (dead code), and several INFORMATIONAL observations.** The spending condition verification is architecturally sound with clear separation between cryptographic proof validation (signatory) and spending condition enforcement (application layer). No critical vulnerabilities found that would allow bypassing spending conditions.

**1.4.F1 -- Spending condition validation flow (CONFIRMED SAFE)**
- **Swap path:** `verify_spending_conditions()` is called at `swap/mod.rs:30` BEFORE any DB interaction or saga construction. This is a CPU-only operation with no persistent side effects, so failure is clean.
- **Melt path:** `verify_spending_conditions()` is called at `melt/mod.rs:545` BEFORE `verify_inputs()` and the melt saga.
- **Non-NUT-10 proofs:** Proofs without NUT-10 spending conditions (plain secrets) are silently accepted in `verify_inputs_individually()` (`nut10.rs:491`) -- this is correct per the Cashu protocol since plain proofs have no spending conditions to enforce.
- **Architectural separation:** Spending conditions (P2PK, HTLC, SIG_ALL) are verified in the application layer (`nut10.rs`, `nut11/mod.rs`, `nut14/mod.rs`). Cryptographic proof validity (BDHKE blind signature) is verified by the signatory (`db_signatory.rs:155-165`). The signatory explicitly does NOT check spending conditions (comment at `mod.rs:923`). This separation is clean and correct.

**1.4.F2 -- P2PK verification correctness (CONFIRMED SAFE)**
- `Proof::verify_p2pk()` (`nut11/mod.rs:148-232`) correctly implements two-path verification per NUT-11:
  - **Primary path:** Always available. Checks signatures against the data pubkey + additional pubkeys from conditions. Requires `num_sigs` valid signatures (default 1).
  - **Refund path:** Available only after locktime. If refund keys are specified, requires `num_sigs_refund` valid signatures. If no refund keys, anyone can spend (0 sigs required).
- SIG_ALL proofs are correctly rejected with `Error::SigAllNotSupportedHere` (line 158), forcing them through the transaction-level SIG_ALL verification path instead.

**1.4.F3 -- HTLC verification correctness (CONFIRMED SAFE)**
- `Proof::verify_htlc()` (`nut14/mod.rs:104-202`) correctly implements two-path verification per NUT-14:
  - **Receiver path:** Requires valid preimage + signatures against pubkeys (if any). Always available.
  - **Sender/Refund path:** Available only after locktime. Requires signatures against refund keys (if any).
- Preimage validation: `verify_htlc_preimage()` (`nut10.rs:269-292`) decodes hex, hashes with SHA256, and compares against the hash in `secret.data`. Correct.
- Wrong witness type (e.g., `P2PKWitness` on an HTLC proof) is correctly rejected at line 128-139, with an escape hatch for the anyone-can-spend case.

**1.4.F4 -- HTLC preimage validation (CONFIRMED SAFE)**
- `HTLCWitness::preimage_data()` (`nut14/mod.rs:77-92`) enforces exactly 32 bytes (64 hex characters). Invalid hex -> `Error::InvalidHexPreimage`. Wrong size -> `Error::PreimageInvalidSize`.
- `SpendingConditions::new_htlc()` (`nut11/mod.rs:345-360`) also enforces 32-byte preimage at creation time.
- Hash comparison uses `ne()` on `Sha256Hash`, which is a constant-time-safe comparison via the Bitcoin crate's implementation.

**1.4.F5 -- Locktime enforcement (CONFIRMED SAFE, INFORMATIONAL NOTES)**
- Locktime comparison at `nut10.rs:174-177`: `locktime < current_time` -- strictly less than. A locktime exactly equal to the current time means the locktime has NOT passed (the refund path is NOT yet available).
- `unix_time()` (`util/mod.rs:19-24`) uses `SystemTime::now()` (wall clock) with `unwrap_or_default()` on failure (returns epoch 0, making all locktimes "passed" -- this is a fail-open on clock failure but `SystemTime` failure is practically impossible on modern systems).
- **Clock skew:** Locktime uses wall clock seconds. Different mint instances in a cluster could disagree on whether a locktime has passed if their clocks are not synchronized. This is inherent to the protocol design and mitigated by NTP.
- **No locktime on Conditions creation for mint-side:** `Conditions::new()` (`nut11/mod.rs:516-517`) checks `locktime >= unix_time()` to prevent creating conditions with past locktimes. This only applies to the wallet side (condition creation), not to the mint's verification of existing proofs.

**1.4.F6 -- Multisig threshold and duplicate signature detection (CONFIRMED SAFE)**
- `valid_signatures()` (`nut11/mod.rs:237-256`) iterates pubkeys × signatures. For each pubkey, it checks all signatures. If a pubkey verifies multiple signatures, it returns `Error::DuplicateSignature` (line 249).
- Uses `HashSet` for `verified_pubkeys` to track which public keys have produced valid signatures. Each pubkey is counted at most once.
- **Attack vector (duplicate signatures from SAME key):** Blocked. If an attacker provides two copies of the same signature, and it verifies against the same pubkey, the second `insert()` returns `false` -> `DuplicateSignature` error.
- **Attack vector (same signature verifying against DIFFERENT keys):** Not practically possible with Schnorr signatures due to the key being bound to the verification equation.
- **`num_sigs` default:** When `num_sigs` is not specified, it defaults to 1 (P2PK: `nut10.rs:198`; HTLC with pubkeys: `nut10.rs:232`; HTLC without pubkeys: `required_sigs = 0` at `nut10.rs:229-233`). The HTLC case where `pubkeys` is empty correctly sets `required_sigs = 0`, meaning no signatures are needed (only the preimage).

**1.4.F7 -- SIG_ALL output binding (CONFIRMED SAFE)**
- SIG_ALL prevents output manipulation by including ALL output amounts and blinded secrets in the signed message:
  - **SwapRequest** (`nut03.rs:99-117`): `msg = secret_0 || C_0 || ... || secret_n || C_n || amount_0 || B_0 || ... || amount_m || B_m`
  - **MeltRequest** (`nut05.rs:175-208`): Same as swap + appends the quote ID, binding the signature to a specific melt operation.
- **Concatenation without delimiters:** The message is built via plain string concatenation with no separators or length prefixes. For example, `secret.to_string() + c.to_hex()` has no delimiter. In theory, this creates ambiguity: `secret="ab" + C="cd"` produces the same string as `secret="abc" + C="d"`. However, this is not exploitable because:
  - `C` (public key) is always a 66-character hex string (02/03 prefix + 64 hex chars).
  - `secret` is a JSON-encoded NUT-10 secret (starts with `["`).
  - `amount` is a decimal number.
  - `B_` (blinded secret) is a 66-character hex string.
  - The structural constraints make collisions practically impossible.
- **Recommendation:** Consider adding explicit delimiters (e.g., `||`) or length prefixes for defense-in-depth. This is a low-priority hardening measure.
- **All inputs must match for SIG_ALL:** `verify_all_inputs_match_for_sig_all()` (`nut10.rs:360-404`) enforces that all inputs have the same kind, data, and tags. This prevents mixing SIG_ALL and non-SIG_ALL proofs or proofs with different conditions.
- **Only first input's witness is checked:** For SIG_ALL, signatures are only extracted from the first input's witness (`nut10.rs:537-540`). Other inputs' witnesses are ignored. This is by design per the NUT-11/NUT-14 spec.

**1.4.F8 -- Keyset validation asymmetry (CONFIRMED SAFE, BY DESIGN)**
- **Outputs require active keysets:** `verify_outputs_keyset()` (`verification.rs:67`) rejects inactive keysets with `Error::InactiveKeyset`. This is additionally enforced at the signatory level in `blind_sign()` (`db_signatory.rs:118-153`).
- **Inputs allow inactive keysets:** `verify_inputs_keyset()` (`verification.rs:104-136`) does NOT check `keyset.active`. This is correct -- tokens issued under old keysets must remain spendable.
- **Signatory verifies proofs from all keysets:** `verify_proofs()` (`db_signatory.rs:155-165`) loads ALL keysets (including inactive ones) and verifies BDHKE signatures. It does NOT check active status. This is correct for the same reason.
- **Confirms 1.2.F6:** Neither `verify_inputs_keyset()` nor `verify_outputs_keyset()` filters out `CurrencyUnit::Auth` keysets. The only protection is the unit-matching logic (all inputs/outputs must share one unit, and input unit must match output unit). Since `CurrencyUnit::Auth` is a distinct unit value, unit matching prevents mixing Auth and non-Auth tokens in a single swap. However, an Auth-to-Auth swap via the regular swap path remains possible (see 1.2.F6 for the full attack scenario).

**1.4.F9 -- Fee calculation: unchecked multiplication (MEDIUM)**
- `calculate_fee()` (`fees.rs:35`): `let proofs_fee = keyset_fee_ppk * proof_count;` uses the standard `*` operator on `u64`.
  - **Debug builds:** Panics on overflow (Rust's default debug behavior).
  - **Release builds:** Wraps silently (Rust's default release behavior).
- If `keyset_fee_ppk` is set to a very large value by the operator and `proof_count` is near `max_inputs` (100), the multiplication could theoretically wrap. Example: `keyset_fee_ppk = u64::MAX / 50` with `proof_count = 100` would wrap.
- **Practical risk:** Low. `keyset_fee_ppk` is set by the mint operator in configuration, not by the attacker. With the default `max_inputs = 100` and realistic fee values (typically < 1000), overflow is impossible.
- **Subsequent operations are safe:** `sum_fee.checked_add(proofs_fee)` on line 37-39 uses checked arithmetic. `sum_fee.checked_add(999)` on line 44 uses checked arithmetic.
- **Recommendation:** Replace line 35 with `let proofs_fee = keyset_fee_ppk.checked_mul(*proof_count).ok_or(Error::AmountOverflow)?;` for defense-in-depth.

**1.4.F10 -- Amount balancing verification (CONFIRMED SAFE)**
- `verify_transaction_balanced()` (`verification.rs:206-239`) enforces exact equality: `output_amount == input_amount - fee`. Not `>=`, not `<=`, but `==`.
- Uses `checked_sub()` on `Amount<CurrencyUnit>` (`amount.rs:513-525`) which returns `Err(Error::AmountOverflow)` if fee > input amount (preventing underflow).
- Unit matching is checked at line 215-222: input and output units must be identical.
- Fee calculation uses `get_proofs_fee()` which is deterministic (sorted `BTreeMap` iteration in `calculate_fee()`) and depends only on keyset configuration and proof count.
- **No floating point:** All amount operations use `u64` integers. No decimal or floating point anywhere in the financial calculations.

**1.4.F11 -- Uniqueness checks (CONFIRMED SAFE)**
- **Input Y-values:** `check_inputs_unique()` (`verification.rs:19-33`) computes `y()` (hash-to-curve of the secret) for all proofs and collects into a `HashSet`. If `HashSet.len() != proof_count`, duplicates exist -> `Error::DuplicateInputs`.
- **Output blinded secrets:** `check_outputs_unique()` (`verification.rs:38-53`) collects `blinded_secret` references into a `HashSet<&PublicKey>`. Same length check.
- **Both checks run before any DB interaction:** Called from `verify_inputs()` (line 195) and `verify_outputs()` (line 179) respectively.
- **Database-level backup:** Even if in-memory checks were bypassed, PRIMARY KEY constraints on `proof.y` and `blind_signature.blinded_message` would catch duplicates (see 1.1.F2).

**1.4.F12 -- Max input/output limits enforcement ordering (LOW)**
- **Swap:** Limits are checked at `swap/mod.rs:42-68`, AFTER `verify_spending_conditions()` (line 30) but BEFORE `verify_inputs()` (line 74). This means spending condition verification runs on potentially large input sets, but this is CPU-only (no DB, no signatory call).
- **Melt:** Limits are checked inside `setup_melt()` at `melt_saga/mod.rs:202-230`, which is AFTER both `verify_spending_conditions()` (line 545) and `verify_inputs()` (line 550). This means the signatory's `verify_proofs()` runs on potentially unbounded inputs before the limit is enforced.
- **Risk:** An attacker could submit a melt request with thousands of proofs. The signatory would verify all of them (CPU-intensive BDHKE checks) before the limit error is returned. With default `max_inputs = 100`, legitimate requests are capped, but the check occurs too late in the melt path.
- **Recommendation:** Move the max inputs/outputs check to the beginning of the `melt()` function (before `verify_spending_conditions()`) for consistency with the swap path.

**1.4.F13 -- Dead code: `enforce_sig_flag` and `BlindedMessage::verify_p2pk` (LOW)**
- `enforce_sig_flag()` (`nut11/mod.rs:728-765`): Defined but never called anywhere in the codebase. It was likely the old SIG_ALL handling approach, superseded by the `SpendingConditionVerification` trait.
- `BlindedMessage::verify_p2pk()` (`nut11/mod.rs:281-315`): Defined but never called from production code (only from tests in its own module). SIG_ALL output protection works through message concatenation, not per-output signature verification.
- **Risk:** Dead code increases maintenance burden and could confuse future auditors.
- **Recommendation:** Remove or mark as `#[deprecated]` if no longer needed. If they serve a purpose for wallet-side code, document that explicitly.

**1.4.F14 -- Signatory verification does not validate amount denomination (CONFIRMED SAFE)**
- `verify_proofs()` (`db_signatory.rs:160-162`): `key.keys.get(&proof.amount)` looks up the key pair for the proof's claimed amount. If no key exists for that denomination, `Error::UnknownKeySet` is returned. This means the cryptographic verification is tied to the claimed amount -- a proof for amount=2 is verified against the amount=2 key pair.
- **Attack vector:** Can an attacker claim a proof has a higher denomination than it was actually signed for? No. BDHKE verification requires the secret key for the specific denomination. If the attacker changes the amount, the verification against the wrong key pair will fail (`verify_message` returns error).

**Key Questions (Answered):**
- **Can an attacker bypass spending conditions with crafted proofs?** No. Spending conditions are enforced for all NUT-10 proofs via `verify_spending_conditions()`. Plain (non-NUT-10) proofs have no conditions to bypass. SIG_ALL proofs are correctly routed to transaction-level verification. Deactivated keysets are only bypassed for inputs (by design per Cashu protocol).
- **Is fee calculation deterministic and tamper-proof?** Deterministic: yes (BTreeMap ensures sorted iteration, `checked_add` for accumulation). Tamper-proof: fees are calculated server-side based on keyset configuration, not from client-supplied data. The client cannot influence the fee amount. One unchecked multiply exists (1.4.F9) but is not practically exploitable.
- **Can deactivated keysets still be used?** For inputs (spending): yes, by design. For outputs (new tokens): no, rejected by both `verify_outputs_keyset()` and the signatory's `blind_sign()`.
- **Can refund keys be used before locktime expiry?** No. `get_pubkeys_and_required_sigs()` (`nut10.rs:174-177`) checks `locktime < current_time`. If locktime has not passed, `refund_path` is `None` and refund keys cannot be used. The primary path (data + pubkeys) is always available regardless of locktime.

**Cross-references from Section 1.2 audit:**
- See **1.2.F6**: `verify_inputs_keyset()` and `verify_outputs_keyset()` in `verification.rs` do not reject `CurrencyUnit::Auth` keysets, allowing auth proofs to be used in the swap path. This is a keyset type isolation gap that should be addressed in this section's audit. **Confirmed in 1.4.F8**: Unit matching provides the primary protection, but an Auth-to-Auth swap remains possible via the regular swap path.

---

### 1.5 Key Management & Rotation
**Risk Level: HIGH**

**Files:**
- `crates/cdk/src/mint/keysets/mod.rs`
- `crates/cdk/src/mint/builder.rs` (lines ~409-459)
- `crates/cdk-signatory/src/db_signatory.rs` (BIP32 HD key derivation, blind signing)
- `crates/cdk-signatory/src/common.rs` (keyset creation, derivation path calculation)

**Audit Tasks:**
- [ ] Verify keyset rotation triggers correctly
- [ ] Check keyset activation/deactivation logic
- [ ] Audit signatory key generation and storage (BIP32/BIP39 derivation)
- [ ] Verify keyset version preferences respected
- [ ] Test fee change keyset rotation
- [ ] Check auth keyset isolation

**Key Questions:**
- Who can trigger keyset rotation?
- Are old keyset private keys properly destroyed?
- Can an attacker force premature keyset rotation?

---

### 1.6 Mint/Issue Operations
**Risk Level: CRITICAL**

**Files:**
- `crates/cdk/src/mint/issue/mod.rs` (lines ~600-735)
- `crates/cdk/src/mint/mod.rs` (lines ~895-937)

**Audit Tasks:**
- [ ] **CRITICAL**: Blind signing occurs BEFORE the DB transaction (`issue/mod.rs:604`). The code calls `self.blind_sign()` at line 604, then opens the DB transaction at line 606. This is intentional (to avoid holding a DB lock during an external signatory call) but means signatures can be lost if the subsequent DB transaction fails. Verify this tradeoff is acceptable and that lost signatures cannot be exploited.
- [ ] Check quote state transitions (Unpaid -> Paid -> Issued)
- [ ] Audit amount validation for BOLT11 vs BOLT12
- [ ] Test overpayment handling (multiple payments to same quote)
- [ ] Verify payment notification duplicate detection
- [ ] Check mint quote TTL enforcement
- [ ] Verify NUT-20 mint quote signature validation (see Section 1.17)

**Key Questions:**
- Can an attacker repeatedly cause DB transaction failures to extract blind signatures that never get committed?
- Can an attacker mint tokens without paying?
- Is quote state properly locked during processing?
- Since the issue flow does not use a saga (unlike swap/melt), what are the recovery guarantees after a crash between signing and commit?

---

### 1.7 Melt Operations & Lightning Integration
**Risk Level: HIGH**

**Files:**
- `crates/cdk/src/mint/melt/mod.rs` (lines ~57-64: MPP check; lines ~539-568: melt entry point)
- `crates/cdk/src/mint/verification.rs`

**Audit Tasks:**
- [ ] **CRITICAL**: Verify MPP internal payment check (lines ~62-64). This check only applies to the `MeltOptions::Mpp` variant. Verify that `Amountless` and `None` melt option variants have equivalent self-payment protections elsewhere (e.g., in internal settlement logic).
- [ ] Check async melt timeout handling
- [ ] Audit fee validation before melt execution
- [ ] Test amount limits enforcement (min/max, lines ~39-107)
- [ ] Verify payment processor trust boundaries
- [ ] Check internal settlement logic

**Key Questions:**
- Can a user pay themselves via MPP attack?
- Can a user bypass the MPP check by using `Amountless` or `None` melt options?
- Can async operations hang indefinitely?
- Is fee calculation vulnerable to manipulation?

---

### 1.8 Configuration & Secrets Management
**Risk Level: HIGH**

**Files:**
- `crates/cdk-mintd/src/config.rs`
- `crates/cdk-mintd/src/env_vars/*.rs` (16 files: mod, common, info, mint_info, ln, cln, lnd, lnbits, ldk_node, database, auth, limits, prometheus, fake_wallet, management_rpc, grpc_processor)
- `crates/cdk-mintd/src/main.rs`

**Audit Tasks:**
- [ ] Check mnemonic/seed generation and storage
- [ ] Audit backend credential handling (LND: cert_file/macaroon_file, CLN: rpc_path, LNbits: admin_api_key/invoice_api_key, LDK Node: bitcoind_rpc_user/password/mnemonic)
- [ ] Verify config file permissions requirements
- [ ] Check environment variable exposure in process listings (all 16 env_var modules)
- [ ] Test config validation completeness
- [ ] Verify sqlcipher password handling
- [ ] Verify mnemonic Debug impl hashes with SHA256 (not leaked in logs)

**Key Questions:**
- Are secrets ever logged or exposed in error messages?
- Is the mnemonic properly encrypted at rest?
- Can an attacker read secrets from `/proc/[pid]/environ`?
- Are gRPC processor and management RPC credentials handled securely?

---

### 1.9 Database Security
**Risk Level: MEDIUM-HIGH**

**Files:**
- `crates/cdk-common/src/database/mint/` (MintDatabase, MintTransaction traits, `Acquired<T>` wrapper)
- `crates/cdk-sql-common/src/` (shared SQL statements, value conversion)
- `crates/cdk-sqlite/src/` (optional SQLCipher encryption)
- `crates/cdk-redb/src/` (wallet only)
- `crates/cdk-postgres/src/` (native-tls for connections)

**Audit Tasks:**
- [ ] Review SQL injection risks in SQLite/Postgres implementations
- [ ] Check transaction isolation levels -- the auth token race condition safety depends on this
- [ ] Audit database migration safety
- [ ] Verify backup/restore procedures
- [ ] Test concurrent access patterns
- [ ] Check proof state transition atomicity (State: Unspent -> Pending -> Spent, with explicit invalid transition rules)
- [ ] Verify `Acquired<T>` wrapper properly implements row-level locking (`SELECT ... FOR UPDATE`)
- [ ] Check PostgreSQL TLS configuration (native-tls)
- [ ] Verify SQLCipher encryption key handling

**Key Questions:**
- Are parameterized queries used everywhere?
- Is there proper locking for concurrent operations?
- Can database corruption lead to security issues?
- Does the `Acquired<T>` pattern prevent TOCTOU in all critical paths?

**Cross-references from Section 1.1 audit:**
- See **1.1.F4**: SQLite `FOR UPDATE` stripping uses `trim_end_matches("FOR UPDATE")` in `async_sqlite.rs:32` -- fragile for future query patterns.
- See **1.1.F5**: SQLite is missing the partial unique index on `melt_quote(request_lookup_id)` that PostgreSQL has. Application-logic-only enforcement for this constraint.
- See **1.1.F8**: SQLite `synchronous = normal` is slightly relaxed for a financial system.
- Transaction isolation verified: PostgreSQL `READ COMMITTED` + `FOR UPDATE`; SQLite `BEGIN IMMEDIATE`. Both adequate for double-spend prevention.

**Cross-references from Section 1.2 audit:**
- See **1.2.F1**: Auth proof spend tracking uses the same `FOR UPDATE` pattern in `cdk-sql-common/src/mint/auth/mod.rs:153`. SQLite `FOR UPDATE` stripping applies here too, but `BEGIN IMMEDIATE` provides equivalent protection.
- See **1.2.F6**: Auth proofs used in the swap path are tracked in the regular `localstore` database, not the `auth_localstore`. This creates a split-brain scenario where proof state is inconsistent between the two databases.

---

### 1.10 Network & API Security
**Risk Level: MEDIUM-HIGH**

**Files:**
- `crates/cdk-axum/src/lib.rs` (lines ~174-206: CORS configuration)
- `crates/cdk-axum/src/router_handlers.rs` (REST endpoint handlers)
- `crates/cdk-axum/src/auth.rs` (auth middleware, header extraction)
- `crates/cdk-axum/src/cache/` (response caching: moka in-memory or Redis)
- `crates/cdk-axum/src/ws/` (WebSocket subscriptions, NUT-17)
- `crates/cdk-axum/src/custom_handlers.rs` (custom payment method handlers)
- `crates/cdk-axum/src/custom_router.rs` (dynamic route generation)

**Audit Tasks:**
- [ ] Check API rate limiting (currently none visible in HTTP layer)
- [ ] Verify input size limits (DoS prevention)
- [ ] Audit WebSocket authentication
- [ ] **FINDING**: CORS uses `Access-Control-Allow-Origin: *` -- verify this is acceptable for a financial service
- [ ] Verify management RPC authentication (see Section 1.13)
- [ ] Test error message information disclosure
- [ ] Audit response cache keying -- can cached responses leak across users?
- [ ] Verify Redis cache backend handles connection failures safely
- [ ] Audit custom payment method handler/router for injection or bypass
- [ ] Check dynamic route generation for path traversal or conflicts

**Key Questions:**
- Are there DoS vulnerabilities (large inputs, slowloris)?
- Do error messages leak sensitive information?
- Is the management interface properly protected?
- Can cache poisoning cause incorrect responses for other users?
- Could custom payment method routes shadow or override built-in endpoints?

**Cross-references from Section 1.2 audit:**
- See **1.2.F9**: Response cache returns cached mint/melt responses without re-checking auth. Cache key is body-derived (not auth-header-derived). Low risk due to unique blinded messages/quote IDs, but worth verifying during the cache audit.

---

### 1.11 Cryptographic Primitives
**Risk Level: CRITICAL**

**Files:**
- `crates/cashu/src/dhke.rs` (hash-to-curve, blind_message, sign_message, unblind_message, verify_message, construct_proofs)
- `crates/cashu/src/nuts/nut00/mod.rs` (core types: Proof, BlindSignature, BlindedMessage)
- `crates/cashu/src/nuts/nut01/secret_key.rs` (SecretKey with `non_secure_erase()` on Drop)
- `crates/cashu/src/nuts/nut12.rs` (DLEQ proofs for offline verification)
- `crates/cashu/src/secret.rs` (Secret type with `zeroize` on Drop)
- `crates/cashu/src/amount.rs` (Amount type with safe arithmetic)

**Audit Tasks:**
- [ ] Verify hash-to-curve implementation correctness (domain separator: `Secp256k1_HashToCurve_Cashu_`)
- [ ] Audit blind signature scheme (DHKE) for mathematical correctness
- [ ] Verify DLEQ proof generation and verification (NUT-12)
- [ ] Check that `SecretKey` uses `non_secure_erase()` on Drop -- note this is weaker than `zeroize`. Evaluate if upgrade to `zeroize` is needed.
- [ ] Verify `Secret` type uses full `zeroize` on Drop
- [ ] Audit `OsRng` usage for key generation (CSPRNG)
- [ ] Verify Amount type prevents overflow/underflow
- [ ] Check that Y-value (hash-to-curve output) computation is deterministic and collision-resistant
- [ ] Audit Schnorr signature usage in NUT-20 mint quote signatures

**Key Questions:**
- Is the hash-to-curve implementation compatible with the Cashu specification?
- Are there any timing side-channels in signature verification?
- Is `non_secure_erase()` sufficient or could compiler optimizations remove it?
- Are DLEQ proofs correctly binding to the specific transaction?

---

### 1.12 Payment Processor
**Risk Level: HIGH**

**Files:**
- `crates/cdk-payment-processor/src/` (gRPC service definition, server, client)
- `crates/cdk-payment-processor/src/proto/` (protobuf definitions)

**Audit Tasks:**
- [ ] Audit gRPC protocol definition (7 RPC methods: create payments, make payments, quote payments, etc.)
- [ ] Verify TLS configuration and certificate handling
- [ ] Check trust boundaries -- the payment processor can confirm or deny payments, which directly affects token issuance
- [ ] Audit error handling when payment processor is unreachable
- [ ] Verify payment confirmation cannot be spoofed by a compromised processor
- [ ] Check if payment processor responses are validated/authenticated
- [ ] Test behavior when payment processor returns contradictory results

**Key Questions:**
- Can a compromised payment processor fake payment confirmations to mint unbacked tokens?
- Is the gRPC channel authenticated (mTLS) or just encrypted?
- What happens if the payment processor is unavailable during a melt operation?
- Are payment amounts validated by the mint independently of the processor response?

---

### 1.13 Management RPC
**Risk Level: HIGH**

**Files:**
- `crates/cdk-mint-rpc/src/proto/` (gRPC management protocol)
- `crates/cdk-mint-rpc/src/` (server implementation, CLI binary `cdk-mint-cli`)
- `crates/cdk-mintd/src/env_vars/management_rpc.rs`

**Audit Tasks:**
- [ ] **CRITICAL**: The `update_nut04_quote` RPC can force a mint quote to "Paid" status, bypassing normal payment verification. Audit all admin RPCs for abuse potential.
- [ ] Verify keyset rotation RPC is properly authenticated
- [ ] Audit TLS configuration for management interface
- [ ] Check what configuration changes are possible at runtime
- [ ] Verify management RPC bind address is not exposed publicly by default
- [ ] Audit CLI tool (`cdk-mint-cli`) for credential handling

**Key Questions:**
- Who can access the management RPC? Is there authentication beyond TLS?
- Can `update_nut04_quote` be used to mint tokens without real payment?
- Can keyset rotation be triggered remotely to cause operational disruption?
- Is the management interface bound to localhost by default or is it network-accessible?

---

### 1.14 Signatory gRPC & Process Isolation
**Risk Level: HIGH**

**Files:**
- `crates/cdk-signatory/src/proto/signatory.proto` (4 RPCs: BlindSign, VerifyProofs, Keysets, RotateKeyset)
- `crates/cdk-signatory/src/proto/server.rs` (gRPC server with optional mTLS)
- `crates/cdk-signatory/src/proto/client.rs` (gRPC client with optional mTLS)
- `crates/cdk-signatory/src/embedded.rs` (actor-model embedded signatory)
- `crates/cdk-signatory/src/db_signatory.rs` (BIP32 key derivation, blind signing)

**Audit Tasks:**
- [ ] **FINDING**: When no TLS directory is provided, the gRPC signatory falls back to insecure plaintext with only a warning log. This means private key material could transit the network unencrypted in production.
- [ ] Verify mTLS implementation (server cert + key, client CA for mutual auth)
- [ ] Audit embedded signatory actor model -- verify private keys never leave the signatory task boundary
- [ ] Verify message channel between embedded signatory and mint cannot be intercepted
- [ ] Audit `BlindSign` RPC for input validation
- [ ] Verify `RotateKeyset` RPC cannot be called by unauthorized clients
- [ ] Check that the signatory validates requests before signing (not just a blind signing oracle)

**Key Questions:**
- Is there any deployment path where the signatory runs without TLS unknowingly?
- Can an attacker on the network intercept signing requests/responses if TLS is misconfigured?
- Does the embedded signatory actor model provide any memory isolation guarantees?
- Can the signatory be tricked into signing for a deactivated keyset?

---

### 1.15 FFI Bindings
**Risk Level: MEDIUM-HIGH**

**Files:**
- `crates/cdk-ffi/src/` (23 source files, UniFFI bindings)

**Audit Tasks:**
- [ ] Audit type conversions at FFI boundary for memory safety
- [ ] Check error handling across FFI boundary (panics must not cross FFI)
- [ ] Verify secret/key material is not leaked through FFI types
- [ ] Audit lifetime management of objects passed across FFI
- [ ] Check that UniFFI-generated code handles null/invalid inputs
- [ ] Verify thread safety of shared state accessed via FFI

**Key Questions:**
- Can malformed input from foreign code cause undefined behavior?
- Are there any `unsafe` blocks in the FFI layer (note: workspace forbids `unsafe_code`, but UniFFI may require exceptions)?
- Is secret material properly zeroed when FFI objects are dropped?

---

### 1.16 Prometheus Metrics Server
**Risk Level: MEDIUM**

**Files:**
- `crates/cdk-prometheus/src/server.rs` (hand-rolled HTTP server using `std::net::TcpListener`)
- `crates/cdk-prometheus/src/` (metrics collection)

**Audit Tasks:**
- [ ] **FINDING**: The Prometheus server uses a hand-rolled HTTP implementation with raw `std::io::Read/Write` -- no TLS, no authentication, basic HTTP parsing
- [ ] Verify metrics endpoint does not expose sensitive information (key material, secrets, user data)
- [ ] Test for DoS vulnerabilities in the raw HTTP parser (malformed requests, slowloris, resource exhaustion)
- [ ] Verify bind address configuration (should default to localhost only)
- [ ] Check if operational metrics could aid an attacker (e.g., timing information, error rates)

**Key Questions:**
- Is the metrics server exposed to the public internet by default?
- Can the hand-rolled HTTP parser be crashed with malformed input?
- Do metrics reveal information about active operations that could aid timing attacks?

---

### 1.17 NUT-20 Mint Quote Signatures
**Risk Level: HIGH**

**Files:**
- `crates/cashu/src/nuts/nut20.rs` (Schnorr signature verification on mint requests)
- `crates/cdk/src/mint/issue/mod.rs` (mint request processing)

**Audit Tasks:**
- [ ] Verify Schnorr signature verification is enforced on all mint request paths
- [ ] Audit signature binding -- does the signature cover all critical fields (outputs, quote ID, amounts)?
- [ ] Test that unsigned or incorrectly signed mint requests are rejected
- [ ] Verify the signature cannot be replayed from one quote to another

**Key Questions:**
- Can an attacker submit outputs for a paid quote without a valid NUT-20 signature?
- Is the signature scheme malleable?
- Are there code paths where NUT-20 verification can be bypassed?

---

### 1.18 Nostr Integration (npubcash)
**Risk Level: MEDIUM**

**Files:**
- `crates/cdk-npubcash/src/auth.rs` (NIP-98 authentication, JWT token caching)
- `crates/cdk-npubcash/src/` (Nostr key handling)

**Audit Tasks:**
- [ ] Audit NIP-98 event signing and verification
- [ ] Check JWT token caching for expiration handling and leakage
- [ ] Verify Nostr key material handling
- [ ] Test for token replay vulnerabilities

**Key Questions:**
- Can cached JWT tokens be stolen or replayed?
- Are Nostr private keys properly protected in memory?
- Is NIP-98 event validation complete (timestamp, URL, method)?

---

### 1.19 Lightning Backend Security
**Risk Level: MEDIUM**

**Files:**
- `crates/cdk-lnd/src/` (LND gRPC with macaroon + TLS cert auth)
- `crates/cdk-cln/src/` (CLN RPC socket)
- `crates/cdk-lnbits/src/` (LNbits v1 API with API keys)
- `crates/cdk-ldk-node/src/` (LDK Node with built-in web dashboard)

**Audit Tasks:**
- [ ] Audit LND macaroon and TLS cert file handling
- [ ] Verify CLN RPC socket permissions
- [ ] Check LNbits API key exposure in logs/errors
- [ ] **FINDING**: LDK Node includes a built-in web dashboard with HTTP handlers for channels, payments, invoices, on-chain operations. This is an additional attack surface.
- [ ] Verify credential storage and transmission for each backend

**Key Questions:**
- Are backend credentials exposed in error messages or logs?
- Is the LDK Node web dashboard authenticated?
- Can RPC socket paths be manipulated?

---

## 2. Testing & Fuzzing

### Existing Fuzz Targets
The repository has 19 fuzz targets in `fuzz/fuzz_targets/`:
- `fuzz_proof.rs`
- `fuzz_swap_request.rs`
- `fuzz_melt_request.rs`
- `fuzz_blind_signature.rs`
- `fuzz_spending_conditions.rs`
- `fuzz_p2pk_witness.rs`
- `fuzz_htlc_witness.rs`
- `fuzz_dleq.rs`
- `fuzz_token.rs`
- `fuzz_token_raw_bytes.rs`
- `fuzz_secret.rs`
- `fuzz_amount.rs`
- `fuzz_payment_request.rs`
- `fuzz_payment_request_bech32.rs`
- `fuzz_payment_request_bech32_bytes.rs`
- `fuzz_keyset_id.rs`
- `fuzz_currency_unit.rs`
- `fuzz_mint_url.rs`
- `fuzz_witness.rs`

**Audit Tasks:**
- [ ] Run existing fuzzers for at least 24 hours each
- [ ] Add fuzz targets for:
  - Saga state machine transitions
  - Quote payment notifications
  - Authentication flows (OIDC JWT parsing, blind auth token)
  - Keyset operations
  - NUT-20 signature verification
  - gRPC message parsing (signatory, payment processor, management RPC)
- [ ] Review fuzz corpus for coverage gaps

---

## 3. Static Analysis

### Recommended Tools
1. **cargo-audit** - Check for known vulnerable dependencies
2. **cargo-deny** - License and security policy enforcement
3. **cargo-geiger** - Detect unsafe code usage
4. **cargo-mutants** - Mutation testing (already configured)
5. **semgrep** - Custom security rules
6. **clippy** - Additional lints

**Audit Tasks:**
- [ ] Run cargo-audit and triage findings
- [ ] Run cargo-geiger on mint crates (note: workspace enforces `unsafe_code = "forbid"`)
- [ ] Execute mutation testing on security-critical code
- [ ] Review unsafe code blocks (if any -- check UniFFI-generated code in cdk-ffi)
- [ ] Check for panic conditions (note: workspace enforces `unwrap_used = "deny"`)

---

## 4. Manual Code Review Checklist

### General Security
- [ ] No hardcoded secrets or credentials
- [ ] No debug/development backdoors
- [ ] Proper error handling (no unwrap/expect in production -- enforced by lint)
- [ ] Logging doesn't leak sensitive data (verify mnemonic SHA256-hashed in Debug)
- [ ] Time-based operations use monotonic clocks
- [ ] Random number generation uses CSPRNG (OsRng)

### Cryptographic Security
- [ ] All signatures verified before state changes
- [ ] Proper key derivation (BIP32/BIP39)
- [ ] Y-value calculations are correct (hash-to-curve with domain separator)
- [ ] DLEQ proofs verified where applicable (NUT-12)
- [ ] Hash functions used correctly
- [ ] NUT-20 mint quote signatures verified on all mint paths
- [ ] `SecretKey` uses `non_secure_erase()` -- evaluate if upgrade to `zeroize` is needed
- [ ] `Secret` type correctly implements `zeroize` on Drop

### Financial Security
- [ ] All amount calculations use checked arithmetic
- [ ] Fee calculations cannot underflow/overflow
- [ ] Balance invariants maintained
- [ ] No negative amounts possible
- [ ] Decimal/floating point not used for money

### Network Security
- [ ] All gRPC services (signatory, payment processor, management) use TLS in production
- [ ] No insecure fallbacks without explicit operator opt-in
- [ ] CORS policy is appropriate for deployment context
- [ ] Response caching does not leak data across users
- [ ] All external service connections use TLS

---

## 5. Penetration Testing Scenarios

### Scenario 1: Double-Spend Attack
1. Create valid proof
2. Submit to swap endpoint
3. Immediately submit same proof to melt endpoint
4. Verify only one succeeds

### Scenario 2: Replay Attack
1. Complete valid swap transaction
2. Re-submit the same blinded outputs
3. Verify rejected as already signed

### Scenario 3: Auth Token Replay
1. Obtain valid blind auth token
2. Spend on protected endpoint
3. Attempt to spend same token again
4. Verify rejected

### Scenario 4: Self-Payment via MPP
1. Create internal mint quote
2. Attempt to melt using MPP to same invoice
3. Verify rejected
4. **Also test:** Attempt same with `Amountless` and `None` melt options

### Scenario 5: Saga Crash Recovery
1. Start swap operation
2. Kill mint process at various saga states
3. Restart and verify recovery is correct
4. Verify no funds lost or double-spent

### Scenario 6: Amount Manipulation
1. Attempt to swap with unbalanced inputs/outputs
2. Attempt to melt with incorrect fee
3. Attempt to issue without payment
4. Verify all rejected

### Scenario 7: Management RPC Abuse
1. Connect to management RPC
2. Attempt to force mint quote to "Paid" via `update_nut04_quote`
3. Attempt to rotate keysets
4. Verify all operations require proper authentication
5. Test without TLS to verify rejection

### Scenario 8: OIDC Token Cross-Service Attack
1. Obtain valid JWT from same OIDC provider but for a different service/audience
2. Attempt to use it for clear auth on the mint
3. Verify rejected (currently may succeed due to `validate_aud = false`)

### Scenario 9: Signatory Spoofing
1. Attempt to connect to mint's signatory gRPC port without valid client certificate
2. Attempt to issue BlindSign requests directly
3. Verify mTLS prevents unauthorized access
4. Test with TLS misconfigured (no certs provided)

### Scenario 10: Cache Poisoning
1. Submit request that gets cached
2. Determine if cache key includes all security-relevant parameters (auth state, user identity)
3. Attempt to retrieve another user's cached response
4. Test Redis cache backend specifically

### Scenario 11: Payment Processor Manipulation
1. If possible, intercept payment processor gRPC communication
2. Attempt to forge payment confirmation
3. Verify mint validates payment independently
4. Test mint behavior when processor returns inconsistent results

---

## 6. Dependencies Review

**Critical Dependencies to Review:**
- `secp256k1` / `k256` - Elliptic curve operations
- `sqlx` / `rusqlite` - Database access
- `axum` - HTTP server
- `tokio` - Async runtime
- `serde` - Serialization
- `bitcoin` / `lightning` - Bitcoin primitives
- `tonic` / `prost` - gRPC framework (signatory, payment processor, management RPC)
- `uniffi` - FFI bindings generation
- `moka` - In-memory cache (response caching)
- `jsonwebtoken` - OIDC JWT validation
- `reqwest` / `rustls` - HTTP client and TLS

**Audit Tasks:**
- [ ] Review each dependency's security track record
- [ ] Check for known CVEs
- [ ] Verify dependency pinning
- [ ] Review supply chain (crate owners, recent changes)
- [ ] Pay special attention to `jsonwebtoken` crate configuration (audience validation bypass)

---

## 7. Documentation Review

**Review:**
- [ ] SECURITY.md - Adequate?
- [ ] API documentation - Security considerations noted?
- [ ] Configuration examples - Secure defaults?
- [ ] Deployment guides - Security best practices?
- [ ] gRPC TLS setup documentation (signatory, payment processor, management RPC)
- [ ] Auth configuration documentation (OIDC setup, blind auth)

---

## 8. Prioritized Remediation Timeline

### Immediate (Before Production)
1. ~~Fix or document blind signing before DB transaction tradeoff (`issue/mod.rs:604`)~~ **RESOLVED** -- confirmed safe, see **1.3.F12**. Blind signatures are stateless and never returned to client until after DB commit. No fund-loss possible.
2. Enable OIDC audience validation (`oidc_client.rs:180`) or document risk acceptance -- see **1.2.F2** for detailed analysis of the gap
3. ~~Verify auth token spending race condition safety across all DB backends~~ **DONE** -- confirmed safe, see **1.2.F1**
4. Verify MPP internal payment check covers all melt option variants
5. Ensure management RPC requires authentication and is not publicly exposed
6. Ensure signatory gRPC rejects insecure connections in production (fail-closed, not warn-and-continue)
7. **NEW (1.2.F6):** Add `CurrencyUnit::Auth` rejection in `verify_inputs_keyset()` / `verify_outputs_keyset()` to prevent auth tokens from being used in the swap path
8. **NEW (1.3.F3):** Consider failing startup (or retrying) when recovery for `PaymentAttempted` or `Finalizing` melt sagas fails, rather than accepting requests while sagas are unrecovered

### Short-term (1-2 weeks)
1. ~~Comprehensive saga recovery testing~~ **DONE** -- see **1.3.F1-F14**. Recovery logic is sound with idempotent re-execution. Remaining concern is background sweep for stuck sagas (1.3.F11).
2. Rate limiting implementation on HTTP API
3. Secrets management hardening
4. Database transaction isolation audit across all backends
5. Audit payment processor trust boundaries
6. Review CORS policy for production appropriateness
7. Audit response cache keying for security
8. **NEW (1.4.F9):** Replace unchecked `keyset_fee_ppk * proof_count` multiply in `fees.rs:35` with `checked_mul()` for defense-in-depth
9. **NEW (1.4.F12):** Move max inputs/outputs limit check to the beginning of `melt()` before `verify_spending_conditions()` and `verify_inputs()`

### Medium-term (1 month)
1. Fuzz testing expansion (new targets for auth, gRPC, NUT-20)
2. Penetration testing (all 11 scenarios)
3. Dependency security review
4. Security documentation improvements
5. FFI boundary audit
6. Prometheus metrics server hardening (or replace with standard implementation)
7. LDK Node dashboard authentication
8. **NEW (1.3.F11):** Implement periodic background sweep for stuck sagas (or store `JoinHandle` for graceful shutdown of `melt_async()` tasks)
9. **NEW (1.3.F4):** Add on-demand swap saga recovery endpoint (currently only melts have `handle_pending_melt_quote()`)
10. **NEW (1.4.F7):** Consider adding explicit delimiters or length prefixes to `sig_all_msg_to_sign()` concatenation for defense-in-depth
11. **NEW (1.4.F13):** Remove dead code: `enforce_sig_flag()` and `BlindedMessage::verify_p2pk()` (defined but never called)

### Ongoing
1. Continuous fuzzing
2. Dependency monitoring
3. Security patch process
4. Regular audits

---

## 9. Tools & Commands

```bash
# Run cargo-audit
cargo install cargo-audit
cargo audit

# Run cargo-deny
cargo install cargo-deny
cargo deny check

# Run cargo-geiger
cargo install cargo-geiger
cargo geiger --output-format json

# Run mutation testing
cargo install cargo-mutants
cargo mutants --profile security-critical

# Run fuzzing
cd fuzz
cargo fuzz run fuzz_proof -- -max_total_time=86400

# Run clippy with all features
cargo clippy --all-features -- -D warnings

# Run tests
cargo test --all-features

# Build with security features
cargo build --release --features "auth sqlite"
```

---

## 10. Deliverables

1. **Security Audit Report** - Comprehensive findings document
2. **Risk Assessment Matrix** - Likelihood vs Impact for each finding
3. **Remediation Plan** - Prioritized fix recommendations
4. **Test Suite** - New security tests and fuzz targets
5. **Documentation** - Security best practices guide

---

## Contact

Security issues should be reported to: **tsk@thesimplekid.com**

**Note:** Do not disclose vulnerabilities publicly until fixed.

---

*This audit plan covers the full CDK project with emphasis on the mint component. The 23-crate workspace has extensive cross-crate security boundaries that require coordinated review.*

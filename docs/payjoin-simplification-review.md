# Payjoin Branch — Simplification Review

Review of the `payjoin` bookmark against `main` (2026-07-02), covering four
angles: reuse, simplification, efficiency, and altitude (right-depth design).
Scope is code quality only — reuse, wasted work, and maintainability — not
correctness bugs.

Overall: the core architecture is sound. The send-intent state machine, the
cut-through settlement state enum, and the typed NUT-30/31 protocol structs
were all judged well-designed. The recurring themes below are: state that
duplicates other state, copy-paste that should be a shared helper, and
per-tick work that redoes what hasn't changed.

> **Status (2026-07-03):** all Priority 2 and Priority 3 items plus 1.2, 1.4
> and 1.5 are fixed (marked ✅/◐ inline). The remaining open items are the
> contract-level changes 1.1 (typed payjoin field), 1.3 (nut17 dispatch), and
> the Priority 4 API-shape items — each of which the plan below recommends
> landing as its own change.

---

## Priority 1 — Structural

### 1.1 Payjoin negotiation rides on stringly-typed JSON blobs

- **Where:** `crates/cdk-common/src/payjoin.rs:18-21`,
  `crates/cdk/src/mint/melt/mod.rs:48-61,593-607`,
  `crates/cdk-common/src/payment.rs:419-424`,
  `crates/cdk-common/src/mint.rs:1171,1187`,
  `crates/cdk-bdk/src/payjoin.rs:207-219`, `crates/cdk-bdk/src/types.rs:293-322`
- **Problem:** The mint↔backend payjoin contract flows through
  `OnchainOutgoingPaymentOptions.metadata: Option<String>`,
  `PaymentQuoteResponse.extra_json`, and `MeltQuote.extra_json`, keyed by magic
  `"payjoin"` / `"payjoin_destination"` strings — even though the typed
  `PayjoinV2` (nut31) exists. The value is serialized/re-parsed at 5+ layer
  boundaries; every parse uses `.ok()`, so a malformed or renamed key silently
  degrades to a non-payjoin payment. `cdk-bdk`'s `requested_payjoin` /
  `accepted_payjoin_extra` hardcode the `"payjoin"` literal instead of the
  shared constant. In `types.rs`, the *same* metadata string is parsed under
  two incompatible schemas (`{"payjoin": PayjoinV2}` and
  `PaymentMetadata: HashMap<String,String>`), the second silently falling back
  to `Default`.
- **Fix:** Put `payjoin: Option<PayjoinV2>` directly on
  `OnchainOutgoingPaymentOptions`, an accepted/destination field on
  `PaymentQuoteResponse` (or an onchain-specific response variant), and a typed
  field on the onchain melt-quote variant. Keep `metadata`/`extra_json` as
  genuinely opaque passthrough only. Deletes the shuffle code (e.g.
  `onchain_melt_quote_extra_json`) and the key-routing tests that exist only to
  guard the strings.
- **Interim step (cheap):** the wire shape has a shared decoder
  (`payjoin_v2_from_extra_json` + `ONCHAIN_PAYJOIN_EXTRA_KEY`) but the encode
  side is hand-built at 4 sites (`payjoin.rs:215,273`,
  `mint/melt/mod.rs:601-604`, `payment.rs` `onchain_melt_payment_metadata`).
  Add `payjoin_v2_to_extra_json` next to the decoder and use the constant
  everywhere.

### 1.2 The broadcast/staging pipeline exists twice ✅ DONE
(`send/staging.rs` now owns the crash-safety ordering:
`stage_and_broadcast_signed_send_batch` persists Signed, assigns intents via
the `StageableSendIntent` enum, then `promote_signed_batch_and_broadcast`
persists Broadcast, marks intents, and broadcasts — returning
`StagedBroadcastOutcome` so each caller keeps its own error policy. Both
`build_sign_broadcast_batch` and `stage_and_broadcast_payjoin_send` call it,
recovery's Signed-batch branch reuses the promote tail, and the shared
`evict_unstaged_send_tx` replaces both eviction copies.)

- **Where:** `crates/cdk-bdk/src/payjoin.rs:2136-2249`
  (`stage_and_broadcast_payjoin_send`) vs
  `crates/cdk-bdk/src/send/service.rs:530-669`
- **Problem:** The payjoin path re-implements the batch staging pipeline
  (apply unconfirmed txs → persist → `SendBatchState::Signed` →
  `assign_to_batch` → persist `Broadcast` → per-intent `mark_broadcast` →
  broadcast) for a single-intent batch, including the crash-safety ordering
  comments. `evict_unstaged_payjoin_tx` (`payjoin.rs:2264`) likewise duplicates
  service.rs's inline eviction-on-staging-failure (`service.rs:571-583`). The
  crash-recovery ordering invariants now live in two implementations that
  `recovery.rs` must stay consistent with.
- **Fix:** Extract one shared "stage signed tx + assignments, then broadcast"
  helper on `CdkBdk` (error-policy differences as a parameter); reuse the
  eviction helper in service.rs.

### 1.3 nut17 field-sniffing deserialization ladder

- **Where:** `crates/cashu/src/nuts/nut17/mod.rs:215-270`
- **Problem:** `NotificationPayload` deserialization was rewritten from
  declarative `#[serde(untagged)]` into a hand-rolled ladder
  (`contains_key("fee_options")` → onchain melt, `"fee_reserve"` → bolt11 melt,
  `"state"` → bolt11 mint, `"amount"` → bolt12 mint, fallthrough → onchain
  mint). Every future quote-response variant must thread through this global
  ladder in exactly the right order; adding an optional field named `state` /
  `amount` / `fee_reserve` to any response silently reroutes payloads to the
  wrong variant. The invariant "each payload deserializes to its own variant"
  is unenforceable in general.
- **Fix:** Discriminate by context the subscriber already has — the
  subscription `Kind` / payment method is known per subscription ID, so
  deserialize the payload against the type implied by the subscription (or add
  a wire-level tag for new methods).

### 1.4 `check_outgoing_payment` now mutates state; `_status_only` twin on the trait ✅ DONE
(`check_outgoing_payment` is pure by trait contract again and the
`_status_only` twin is deleted from the trait, the metrics wrapper, and all
callers; `start_up_check.rs` uses the plain method. Payjoin negotiation and
fallback broadcasts are owned exclusively by the send/receive pollers, which
now run even without a payjoin config so leftover intents from a previously
configured run still settle; the `recover_payjoin_*_once` helpers are
test-only.)

- **Where:** `crates/cdk-common/src/payment.rs:499-511`,
  `crates/cdk-bdk/src/lib.rs:915-949`, `crates/cdk/src/mint/start_up_check.rs:69`
- **Problem:** In cdk-bdk, `check_outgoing_payment` drives payjoin
  negotiation/fallback broadcasts as a side effect, so
  `check_outgoing_payment_status_only` was added to `MintPayment` for callers
  that need purity. This institutionalizes "status checks may mutate backend
  state": every call site on every backend must choose the right method; every
  wrapper must forward both; and the gRPC bridge in `cdk-payment-processor`
  does not implement `_status_only`, so remote backends silently fall back to
  the *effectful* default exactly where recovery wanted the guarantee.
- **Fix:** Keep `check_outgoing_payment` pure by contract (it already is for
  every other backend) and drive payjoin intents exclusively from the existing
  `run_payjoin_send_poller` / recovery tasks. Removes the trait change
  entirely.

### 1.5 Reservation invariant implemented twice across crates ✅ DONE

- **Where:** `crates/cdk-bdk/src/payjoin.rs:2565`
  (`reserved_cut_through_candidate_matches`) vs inline in
  `crates/cdk-bdk/src/storage/mod.rs:300-313`
- **Problem:** The intent-vs-settlement reservation check (state is
  `CutThroughReserved` with matching `settlement_id`, plus
  `quote_id`/`amount_sat`/`max_fee_amount_sat` equality) exists in two places;
  a change to the invariant in one silently diverges the exposure-time check
  from the reuse-time check.
- **Fix:** One shared predicate, e.g.
  `CutThroughSettlementRecord::matches_reserved_intent(&SendIntentRecord)` in
  `storage/types.rs`, used by both.

---

## Priority 2 — Efficiency (mostly the 15s pollers)

### 2.1 New `reqwest::Client` per payjoin HTTP request ✅ DONE

- **Where:** `crates/cdk-bdk/src/payjoin.rs:~2789` (`payjoin_http_request`)
- **Problem:** Every directory/relay request builds a fresh
  `reqwest::Client::new()` — client construction plus a fresh TCP+TLS
  handshake per poll, for every open session, every 15s. Connection reuse is
  completely defeated.
- **Fix:** One shared `reqwest::Client` (field on `CdkBdk` or
  `static LazyLock<Client>`); clients are cheap to clone and pool internally.

### 2.2 Blocking Bitcoin Core RPC on the async runtime ✅ DONE

- **Where:** `crates/cdk-bdk/src/chain/bitcoin_rpc.rs:~312`
  (`accepts_broadcast_bitcoin_rpc`), called via the `can_broadcast` closure in
  `crates/cdk-bdk/src/payjoin.rs:762-781`
- **Problem:** `testmempoolaccept` uses the synchronous `bitcoincore_rpc`
  client (new client + blocking HTTP round trip) directly inside an async fn —
  blocks a tokio worker for up to a full RPC round trip each time an original
  PSBT is checked.
- **Fix:** Run the check before entering the payjoin closure via
  `spawn_blocking` (as `fetch_fee_rate_bitcoin_rpc` /
  `any_confirmed_spend_bitcoin_rpc` already do) and pass the boolean in; reuse
  one RPC client.

### 2.3 Poller ticks do full-table N+1 scans of the KV store ◐ PARTIAL
(`list_records` now issues its per-key reads concurrently (bounded at 16), so a
tick no longer pays one sequential DB round trip per record. A true
`kv_read_many` and state-scoped listing would require a `KVStore` trait change
across every backend — left for a dedicated change.)

- **Where:** `crates/cdk-bdk/src/storage/mod.rs:153-181` (`list_records` =
  `kv_list` + one `kv_read` per key); callers at `payjoin.rs:287,910,985,1679,1703,1904`
- **Problem:** Every 15s tick lists all sessions/intents/settlements with one
  DB round trip per record, then JSON-deserializes each (receive-session
  records embed the full event log including PSBTs; settlements in the 7-day
  retention window are re-fetched every tick; the send poller deserializes
  every non-payjoin intent just to filter it out).
- **Fix:** A batched `kv_read_many` / range read in `list_records`, plus
  state- or expiry-scoped listing (separate namespace or index for open
  sessions / `PayjoinNegotiating` intents / non-terminal settlements) so
  terminal records aren't touched every tick.

### 2.4 Unconditional full-record re-persist on no-progress polls ✅ DONE
(`RecordingSessionPersister` now tracks a dirty flag; both the receive-session
final persist and `persist_payjoin_send_progress` skip the rewrite when the
session made no progress.)

- **Where:** `crates/cdk-bdk/src/payjoin.rs:723-726` (receive) and
  `:2013-2015` (send `Stasis`)
- **Problem:** Even when the directory returns "nothing yet", the whole record
  — full event log JSON (with embedded PSBT-sized payloads) plus
  `original_tx_bytes` on sends — is re-serialized and rewritten; each write is
  itself a read+read+write via `update_record_state`. Also
  `persister.events()?` clones the entire event vec 3-5 times per pass.
- **Fix:** Dirty flag on `RecordingSessionPersister` (skip persist when clean);
  take the events once at the end.

### 2.5 Startup blocks on sequential network-driven session recovery ✅ DONE
(`start()` now runs only the DB-only `release_stale_cut_through_reservations`
inline; the pollers' immediate first tick handles network-driven recovery, and
a one-shot background pass covers the no-payjoin-config case.)

- **Where:** `crates/cdk-bdk/src/lib.rs:499` (`start()` awaits
  `recover_payjoin_sessions_once()`); `payjoin.rs:311-322,1891-1899`
- **Problem:** `start()` synchronously drives every open receive session and
  send intent through OHTTP directory requests (35s timeout each), one at a
  time, before the payment processor reports started. The pollers spawned
  immediately afterwards do identical work on their first tick anyway.
- **Fix:** Keep only the DB-only recovery inline (releasing `Reserved`
  cut-through settlements); let the pollers' first tick (or a spawned task)
  handle network-driven recovery.

### 2.6 Remaining efficiency items ✅ DONE (one sub-item partial)

- ✅ **Sessions processed serially per tick** (`payjoin.rs:297-301,1655-1659`):
  one slow relay head-of-line-blocks all sessions →
  `for_each_concurrent(limit, …)`. (Both pollers now drive up to 8 concurrently.)
- ✅ **Per-input chain queries every tick for unconfirmed cut-throughs**
  (`payjoin.rs:1746-1753`, `chain/esplora.rs:286-309`,
  `chain/bitcoin_rpc.rs:360-388`): gated on tip-height change via
  `payjoin_spend_check_tip`, and esplora per-call helpers now reuse a shared
  `AsyncClient` keyed by URL.
- ✅ **`check_outgoing_payment` reads the same intent record 3×**
  (`lib.rs:915-940`, `payjoin.rs:1926-1945`): the inline negotiation (and its
  record read + directory I/O) is skipped entirely when the background send
  poller is running; it remains only for the pre-`start()` / no-poller case.
- ✅ **Redundant event-log rewrite in `start_payjoin_send`**
  (`payjoin.rs:1513-1515`): deleted.
- ◐ **`exposed_cut_through_for_proposal`** (`payjoin.rs:975-999`): now uses
  `unsigned_tx.compute_txid()` (no PSBT clone/extract). The full-settlement
  scan remains — an index by `receive_quote_id` would be a storage-schema
  change.
- ◐ **Sequential per-outpoint DB lookups** (`payjoin.rs:1289-1298,2698-2725`):
  the seen-input checks now run concurrently via `try_join_all`. Caching the
  derived original-tx outpoints on the record is a schema change — left.
- ✅ **`contribute_payjoin_inputs`** (`payjoin.rs:1384-1390`): keeps only
  `candidate_inputs.first().cloned()` as the fallback and moves the vec in.

---

## Priority 3 — Simplification / dead state

### 3.1 Always-identical field pair in cut-through settlements ✅ DONE

- **Where:** `crates/cdk-bdk/src/storage/types.rs:77-80`;
  `payjoin.rs:186-187,1175-1176`
- **Problem:** `receive_payment_id` and `receive_outpoint` are always identical
  — the only creation site sets the second to a clone of the first (the field
  is even doc-commented "Legacy receive outpoint field" in a brand-new schema).
  The pair is threaded through ~6 functions.
- **Fix:** One field (`receive_outpoint`); derive the payment id where needed.
  Do it now, before a migration is required.

### 3.2 Dead field: `ProposalExposed::proposal_tx_bytes` ✅ DONE

- **Where:** `crates/cdk-bdk/src/storage/types.rs:70` (written at
  `payjoin.rs:2588`, never read)
- **Fix:** Drop it — `proposal_txid` is what's used; new record type, no
  migration concern.

### 3.3 Send-intent transitions rebuild the struct ~10 times ✅ DONE

- **Where:** `crates/cdk-bdk/src/send/payment_intent/mod.rs:100-445,460-545`
- **Problem:** `assign_to_batch` (×3), `revert_to_pending`, `fail` (×2), and 7
  `from_record` arms each copy the same 8 identity fields into a new
  `SendIntent` literal; adding one field means editing ~10 sites.
- **Fix:** A private `fn with_state<T>(self, state: T) -> SendIntent<T>` (and
  an analogous record→intent constructor) collapses each transition to two
  statements.

### 3.4 Duplicated upsert SQL per dialect ✅ DONE

- **Where:** `crates/cdk-sql-common/src/wallet/mod.rs:1195-1245,1306-1360`
- **Problem:** Four ~25-line upsert statements (mint_quote ×2, melt_quote ×2)
  duplicated wholesale in an `if RM::Connection::name() == "postgres"` fork
  whose only difference is `(:payjoin)::jsonb` vs `:payjoin` — first fork of
  its kind in the crate; the copies will drift.
- **Fix:** Substitute just the placeholder into one template, or store TEXT in
  both dialects (the value is `serde_json::to_string`'d anyway and read back
  via `CAST(payjoin AS TEXT)`).

### 3.5 Cut-through exposure handling: deep nesting + repeated evict-error block ✅ DONE
(`evict_cut_through_or_wrap` + `persist_fresh_cut_through_exposure` extracted; the
`if let` chain is now a `match`. The two send-side evict blocks have different
error wrapping/wording and were left as-is.)

- **Where:** `crates/cdk-bdk/src/payjoin.rs:571-707` (also
  `:1487-1496,2181-2186`)
- **Problem:** ~100 lines at 5 nesting levels; the "evict proposal tx, else
  concatenate `; additionally could not evict…` into the error" block is
  copy-pasted at 5 sites; `suppress_final_progress_persist` flag toggling
  obscures control flow.
- **Fix:** Extract `async fn evict_and_wrap(&self, txid, err) -> Error` used by
  all sites; extract the `Fresh` branch into
  `persist_fresh_cut_through_exposure(…)`; replace the
  `if let … else if let … else` chain with a `match` on the proposal enum.

### 3.6 `PhantomData` typestate over a single bool ✅ DONE

- **Where:** `crates/cdk-bdk/src/receive/payjoin_session.rs:1-102`
- **Problem:** ~100 lines of `Open`/`Closed` markers,
  `PayjoinReceiveSessionAny`, `from_record`/`into_record` — with no
  compile-time-prevented transition (decided at runtime, escapable), and the
  sole consumer immediately unwraps it.
- **Fix:** Put `is_expired`, `should_prune`, `close(&mut self, storage)`
  directly on `PayjoinReceiveSessionRecord` and branch on `record.closed`.

### 3.7 `cached_ohttp_keys`: `Option<Result<Keys, Keys>>` as a freshness flag ✅ DONE

- **Where:** `crates/cdk-bdk/src/payjoin.rs:2281-2376`
- **Problem:** Ok = fresh, Err = stale misleads readers; the `Some(Err)` and
  `None` arms duplicate the lock/re-check/fetch sequence; the freshness
  predicate exists twice (`fresh_cached_ohttp_keys` vs the closure) and can
  drift. (The caching design itself — TTL, single-flight, stale fallback — is
  good.)
- **Fix:** One `fn cached(&self, config, now) -> Option<(Keys, bool)>` helper;
  linear path: check fresh → lock → re-check → fetch → on error return stale.

### 3.8 nut31 copy-pasted impls ✅ DONE

- **Where:** `crates/cashu/src/nuts/nut31.rs:26-131,221-256`
- **Problem:** `PayjoinOhttpKeys`/`PayjoinReceiverKey` duplicate ~110 lines of
  Display/FromStr/Serialize/Deserialize; `decode_prefixless_key<const N>` takes
  a redundant `expected: usize` (always == N), leaving an unreachable error
  arm.
- **Fix:** Drop `expected` and use `N`; generate the four impls with a small
  local macro or generic newtype, keeping only the per-type pubkey-offset
  validation.

### 3.9 Small dead code / one-liners ✅ DONE

- ✅ `payjoin.rs:2841` — `find_output_outpoint` is a one-line alias of
  `find_payment_outpoint`; delete.
- ✅ `payjoin.rs:2599-2612` (+ test at `:3036`) — `zero_receiver_fee_range()`
  returns two constants for a single caller, with a test that re-asserts the
  constants. Inline and drop the test.
- ✅ `payjoin.rs:2135` — stale `#[allow(clippy::too_many_arguments)]` on a 3-arg
  fn; doc comment still references a removed param.
- ✅ `payjoin.rs:2656-2681` — `payjoin_original_input_outpoints_from_events`
  duplicates the sibling's "find latest `RetrievedOriginalPayload`" scan and
  abuses a `check_no_inputs_seen_before` callback to collect outpoints; it can
  be one line over `payjoin_original_tx_from_events(events)?` — or extract a
  shared `latest_original_payload(events)` helper.
- ✅ `cdk-common/src/payjoin.rs:136-150` — `padded_fractional.is_empty()` is dead
  (the loop pads to exactly 8 chars); `format!("{fractional:0<8}")` removes the
  branch. (Made moot by 3.11: codec replaced with `bitcoin::Amount`.)
- ✅ `lib.rs:522-549,915-940` — the receive/send poller spawn blocks are verbatim
  twins (a `spawn_supervised(name, fut)` closure halves them); in
  `check_outgoing_payment` both branches end in the identical
  `check_outgoing_payment_status_local` call — drop the inner return.
- ✅ `payjoin.rs:1824-1827` — parses `"txid:vout"` via `split_once(':')`;
  `bitcoin::OutPoint::from_str` already handles the format.
- ✅ Test fixture: the identical regtest `original_tx` + `fallback_script`
  construction is repeated 3× across `lib.rs` (~1564, ~1628, ~1914) — extracted
  a shared `fn test_original_tx(address, amount_sat) -> Transaction`. (The
  `payjoin.rs` test fixture turned out to be a different shape and was left.)

### 3.10 Bypassed existing helpers ✅ DONE

- `payjoin.rs:1565` — inlines
  `FeeRate::from_sat_per_vb_u32(sat_per_vb.ceil() as u32)`, skipping the
  NaN/≤0/overflow validation in `crate::fee::fee_rate_from_sat_per_vb`
  (`fee.rs:33`), which the quote path already uses.
- `payjoin.rs:2446` (`extract_bip21_payjoin_endpoint`) — hand-rolls BIP21
  query splitting + percent-decoding; `receiver.pj_uri().extras.endpoint()` is
  public in payjoin 0.25 and returns the endpoint directly.

### 3.11 BTC amount codec re-implements `bitcoin::Amount` ✅ DONE

- **Where:** `crates/cdk-common/src/payjoin.rs:16,106-157`
- **Problem:** `SATS_PER_BTC`, `format_bip21_amount_from_sats`,
  `parse_bip21_amount_to_sats` — ~60 lines + a 4-variant error enum — duplicate
  `bitcoin::Amount::from_str_in(s, Denomination::Bitcoin)` (incl.
  `TooPrecise`/overflow errors), `to_string_in`, and `Amount::ONE_BTC`.
- **Fix:** Use `bitcoin::Amount`. Parse side is a direct replacement; verify
  trailing-zero formatting parity of `to_string_in` before swapping the format
  side.

### 3.12 bech32 fragment codec duplicated between the two new files ✅ DONE

- **Where:** `crates/cashu/src/nuts/nut31.rs:221-288` vs
  `crates/cdk-common/src/payjoin.rs:203-242`
- **Problem:** Both implement the same bech32-`NoChecksum` codec
  (`CheckedHrpstring` decode + HRP check + length check + `encode_upper`) with
  near-identical `map_bech32_error` functions and parallel error variants;
  cdk-common already depends on cashu.
- **Fix:** Expose nut31's `decode_prefixless_key` / `write_prefixless_key` /
  `map_bech32_error` (generalized) from cashu and have
  `decode_bip77_expiry`/`encode_bip77_expiry` use them.

---

## Priority 4 — API-shape / future-proofing (optional)

### 4.1 BIP21+payjoin URI codec exists in ~5 places

- **Where:** build: `cdk-bdk/src/payjoin.rs:2493`,
  `cdk-cli/src/sub_commands/mint.rs:240`,
  `cdk-integration-tests/tests/onchain_regtest.rs:160-165`; parse:
  `cdk-cli/src/sub_commands/melt.rs:188`, `payjoin.rs:2446`. cdk-bdk and the
  regtest also both repeat the
  `payjoin::Uri::try_from(…).assume_checked().check_pj_supported()` ceremony.
- **Problem:** The `amount`/`pj`/`pjos` encoding rules live in 5+ files with
  inconsistent case normalization (CLI lowercases bech32, mint side uppercases,
  cdk-bdk neither); FFI exposes `PayjoinV2` but no URI support, so frontends
  must reimplement.
- **Fix:** One `build_bip21_payjoin_uri` / `parse_bip21_payjoin_uri` pair in
  `cdk-common::payjoin` (which already owns the BIP77 endpoint converters),
  consumed by CLI, FFI, cdk-bdk, and tests. Also consolidates the duplicated
  bech32-HRP prefix detection in `melt.rs:226` (`normalize_onchain_address`) vs
  `mint.rs:259` (`uppercase_qr_address`).

### 4.2 Wallet quote structs grow another method-specific `Option`

- **Where:** `crates/cdk-common/src/wallet/mod.rs:205-247`
- **Problem:** `payjoin` joins `estimated_blocks`, `fee_index` as a nullable
  method-specific field — fanning out to ~15 `payjoin: None` construction sites
  (bolt11/bolt12/custom melt, sagas, tests, FFI) and three parallel storage
  migrations; nothing enforces `payjoin` only when
  `payment_method == Onchain`. Every future method-specific attribute pays the
  same tax.
- **Fix:** A per-method payload enum (or single JSON `method_data` column keyed
  by `payment_method`) so new methods extend a variant instead of widening the
  shared row.

### 4.3 Method-permutation API

- **Where:** `crates/cdk/src/wallet/melt/onchain.rs:43-58`
- **Problem:** `quote_onchain_melt_options_with_payjoin(…, Option<PayjoinV2>)`
  beside `quote_onchain_melt_options` — the next optional parameter yields
  `_with_payjoin_and_x` combinatorics across wallet + FFI.
- **Fix:** An `OnchainMeltQuoteOptions` struct/builder mirroring
  `MeltQuoteOnchainRequest`.

### 4.4 Single-use CLI wrappers

- **Where:** `cdk-cli/src/sub_commands/melt.rs:226-244`, `mint.rs:259-269`
- `parse_bip21_amount_sat` and `onchain_payjoin_from_endpoint` are single-use
  one-line `map_err(Into::into)` wrappers — inline them. (Subsumed by 4.1 if
  done.)

---

## Deliberately not flagged

Reviewers judged these to carry real behavior and left them alone:

- The staged-persister replace dance in `check_payjoin_inputs_not_seen`
  (ordering vs. the seen-input index).
- The `ExistingCutThroughReservation::StaleAbandoned` vs `None` distinction
  (prevents immediate re-reservation).
- The large `ReceiveSession` state-machine match (mirrors the payjoin crate's
  typestates; arms already chain into shared helpers).
- OHTTP key caching design (TTL, single-flight fetch lock, stale fallback) —
  only its internal code shape is flagged (3.7).
- Wallet-mutex discipline (locks scoped and dropped before awaits/network
  I/O); `start_up_check.rs` relying on `check_outgoing_payment` being a pure
  status read (guaranteed by the trait contract since the 1.4 fix).
- Storage record CRUD properly reuses the generic `KvRecord` machinery;
  mint/melt correctly consume the shared `payjoin_v2_*` decode helpers; the
  lib.rs refactor removed pre-existing duplication in address parsing and
  status mapping.

## Suggested order of attack

1. **Mechanical, low-risk:** 3.1, 3.2, 3.3, 3.9, 3.10, 2.1, 2.2 (pure wins, no
   behavior change). — ✅ done
2. **Poller efficiency:** 2.3, 2.4, 2.6 (biggest runtime impact; touch storage
   listing once). — ✅ done (2.3 partial: no `kv_read_many` trait change)
3. **Shared helpers:** 1.2, 1.5, 3.4, 3.5, 3.11, 3.12, 4.1. — ✅ 1.5, 3.4, 3.5,
   3.11, 3.12 done; 1.2 and 4.1 remain
4. **Contract changes (own change each):** 1.1 (typed payjoin field — wire +
   persistence), 1.3 (nut17 dispatch), 1.4 (trait purity), 2.5 (startup), 4.2.
   — ✅ 1.4 and 2.5 done; 1.1, 1.3, 4.2 remain

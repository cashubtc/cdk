# Development Plan: BOLT12 Batch Minting Implementation

**Author:** Claude Code
**Date:** 2025-11-21
**Status:** Development Plan
**Scope:** Implement batch minting support for BOLT12 offers

---

## Executive Summary

This plan outlines the implementation of BOLT12 batch minting (NUT-XX extension), building on the recently completed BOLT11 batch minting infrastructure. The architecture will leverage existing batch minting patterns while adding BOLT12-specific features including dynamic amounts, cryptographic signatures (NUT-20), and spending conditions.

### Key Statistics
- **Lines of Code (Existing):** ~500 lines wallet + ~400 lines mint batch logic
- **Test Coverage:** 24 integration tests, 8 unit tests for batch minting
- **API Endpoints:** 3 new endpoints (batch check, batch mint, batch status)
- **Maximum Batch Size:** 100 quotes per request (NUT-XX spec)

---

## Current State Analysis

### What's Already Implemented (BOLT11)

#### 1. Wallet-Side Batch Minting (`cdk/src/wallet/issue/batch.rs`)
```
mint_batch() - Main entry point
├── Validates batch constraints (size, duplicates, payment method)
├── Creates pre-mint secrets for amount splitting
├── Generates blinded messages from total amount
├── Checks quote status via batch API
├── Sends batch mint request to mint
├── Constructs proofs from blind signatures
├── Stores proofs locally
└── Cleans up quote storage
```

#### 2. Mint-Side Batch Processing (`cdk/src/mint/issue/mod.rs`)
```
process_mint_request() / process_batch_mint_request()
├── Wrapper-only: metrics + HTTP-specific caching requirements
├── Both normalize into BatchMintRequest (single quote -> Vec len 1)
└── Delegate to process_mint_workload()

process_mint_workload()
├── Validates batch structure and constraints (size, duplicates, origin-aware)
├── Generates blind signatures immediately
├── Begins transaction
├── Loads and refreshes all quotes in transaction
├── Validates payment method & unit consistency
├── NUT-20 signature verification (if present)
├── Validates all quotes PAID / not already issued
├── Calculates per-quote & total mintable amounts
├── Verifies blinded message totals
├── Records outputs + signatures in the DB (single origin keeps quote association)
├── Increments per-quote amount_issued atomically
├── Commits transaction
└── Publishes mint_quote_issue events
```

#### 3. Protocol Types (`cdk-common/src/mint.rs`)
```
BatchMintRequest
├── quote: Vec<String> (up to 100 quote IDs)
├── outputs: Vec<BlindedMessage> (shared outputs)
└── signature: Option<Vec<Option<String>>> (NUT-20)

BatchQuoteStatusRequest
└── quote: Vec<String> (quote IDs)

BatchQuoteStatusResponse
└── Vec<MintQuoteBolt11Response<String>>
```

#### 4. HTTP Endpoints & Client
- **Mint:** `POST /v1/mint/{method}/batch` - Batch mint
- **Mint:** `POST /v1/mint/bolt11/check` - Batch status
- **Wallet:** `post_mint_batch_quote_status()` - Check status
- **Wallet:** `post_mint_batch()` - Execute batch mint

#### 5. Test Coverage
- Handler validation (empty, duplicates, size limits)
- NUT-20 signature validation
- Quote state validation
- Payment method/unit consistency
- Protocol serialization
- Dedicated unit test for the single-origin normalization path hitting `process_mint_workload`
- ~32 comprehensive test cases

### What's NOT Implemented (BOLT12)

1. **Wallet-side BOLT12 batch minting** - Still returns `BatchBolt12NotSupported`
2. **BOLT12 spending conditions in batch** - No support yet
3. **BOLT12 batch quote status** - Single quotes work, batch doesn't
4. **Transactional quote cleanup** - Quote storage not cleaned in transaction
5. **Documentation** - NUT-XX spec compliance details

---

## Architecture Design

### Phase 1: BOLT12 Batch Architecture (Non-Breaking)

#### 1.1 Wallet Layer (`cdk/src/wallet/issue/batch.rs`)

**Additions:**
```rust
// Modify mint_batch() to support BOLT12
pub async fn mint_batch(
    &self,
    quote_ids: Vec<String>,
    amount_split_target: SplitTarget,
    spending_conditions: Option<SpendingConditions>,
    payment_method: PaymentMethod,
) -> Result<Proofs, Error> {
    // Current: Returns BatchBolt12NotSupported for BOLT12
    // New: Full BOLT12 support with NUT-20 signatures

    // Key difference from BOLT11:
    // - BOLT12 quotes have optional amounts
    // - Must generate secret keys for each quote (NUT-20)
    // - Must create signatures covering all blinded messages
    // - Total amount = sum of all quote amounts (after payment)
}
```

**New internal function:**
```rust
fn prepare_bolt12_batch_signatures(
    quote_infos: &[MintQuote],
    blinded_messages: &[BlindedMessage],
) -> Result<Vec<String>, Error> {
    // For each BOLT12 quote with pubkey:
    // 1. Get the secret key from wallet storage
    // 2. Create signature covering quote_id + all blinded_messages
    // 3. Return signatures array
}
```

**Changes to validation:**
```rust
// Replace this check:
if payment_method != PaymentMethod::Bolt11 {
    return Err(Error::BatchBolt12NotSupported);
}

// With BOLT12-specific validation:
if payment_method == PaymentMethod::Bolt12 {
    validate_bolt12_batch_constraints(quote_infos)?;
}
```

#### 1.2 Mint Layer (`cdk/src/mint/issue/mod.rs`)

**Modifications to `process_batch_mint_request()`:**

```rust
// Existing NUT-20 verification already present
// For BOLT12, signature handling is identical to BOLT11

// Key differences for BOLT12:
// 1. Amount calculation: quote.amount (fixed or from payment)
//    vs BOLT11: quote.amount (exact match required)

// 2. Quotes may have identical amounts
//    vs BOLT11: quotes are typically different

// 3. Verify all amounts are definite before processing
//    (BOLT12 can have optional amounts that become definite on payment)
```

**New validation function:**
```rust
fn validate_bolt12_batch_amounts(
    quote_infos: &[MintQuote],
) -> Result<Amount, Error> {
    // Ensure all BOLT12 quotes have definite amounts
    // Sum total for output verification
    // Return total or error if any amount is None
}
```

#### 1.3 Protocol Types (`cdk-common/src/mint.rs`)

**No changes needed** - `BatchMintRequest` and `BatchQuoteStatusResponse` are payment-method agnostic.

### Phase 2: Spending Conditions Support

#### 2.1 Wallet Layer

**Enhancement to batch minting:**
```rust
pub async fn mint_batch(
    &self,
    quote_ids: Vec<String>,
    amount_split_target: SplitTarget,
    spending_conditions: Option<SpendingConditions>, // Currently ignored
    payment_method: PaymentMethod,
) -> Result<Proofs, Error> {
    // Current: Accepts but ignores spending_conditions
    // New: Apply spending_conditions to all generated proofs

    // This requires:
    // 1. Verify spending conditions are valid for batch size
    // 2. Apply to all proofs after signature construction
    // 3. Document NUT-12 interaction with batch minting
}
```

**Constraints to enforce:**
- HTLC spending conditions: max locktime applies to all quotes
- Spending signature: different pubkey per output vs shared in batch?
- P2PK spending conditions: compatible with batch minting

### Phase 3: Transactional Cleanup

#### 3.1 Wallet Database Operations

**Enhancement to quote cleanup:**
```rust
// Current pattern (synchronous cleanup):
for quote_id in &quote_ids {
    self.localstore.remove_mint_quote(quote_id).await?;
}

// Proposed (transactional cleanup - future):
// Add transaction support to MintQuotesDatabase trait
pub async fn transaction<F>(&self, f: F) -> Result<T, Error>
where
    F: FnOnce(&dyn MintQuotesDatabase) -> Result<T, Error>

// Then:
self.localstore.transaction(|db| {
    for quote_id in &quote_ids {
        db.remove_mint_quote(quote_id)?;
    }
    Ok(())
}).await?
```

This is **lower priority** and can be addressed in a follow-up PR.

---

## Implementation Steps

### Step 1: Modify Wallet Batch Minting (2-3 hours)

**File: `cdk/src/wallet/issue/batch.rs`**

**Key Insight: Wallet must calculate total amount to mint based on `amount_mintable()` per quote**

**IMPORTANT: Quote Cleanup Behavior**

BOLT12 batch minting must NOT remove quotes (unlike BOLT11):
- BOLT11 single quote: **removes** quote after minting (no partial minting)
- BOLT12 single quote: **updates** quote, keeps it (supports partial minting via overpayment)
- BOLT11 batch: **removes** all quotes after minting
- **BOLT12 batch: must UPDATE quotes with new amount_issued, DO NOT DELETE**

This allows users to mint multiple times from overpaid BOLT12 quotes.

1. **Replace the BOLT12 restriction check (line 79-80):**
   ```rust
   // Current code
   if payment_method != crate::nuts::PaymentMethod::Bolt11 {
       return Err(Error::BatchBolt12NotSupported);
   }

   // Replace with (allow both BOLT11 and BOLT12)
   match payment_method {
       PaymentMethod::Bolt11 | PaymentMethod::Bolt12 => {},
       _ => return Err(Error::UnsupportedPaymentMethod),
   }
   ```

2. **Modify amount calculation (line 120+):**
   ```rust
   // OLD (BOLT11 only):
   let total_amount: Amount = quote_infos.iter()
       .map(|q| q.amount.unwrap_or(0))
       .sum();

   // NEW (BOLT11 and BOLT12):
   let total_amount = calculate_batch_total_mintable_amount(
       &quote_infos,
       payment_method
   )?;

   // For BOLT11: sum of all quote.amount
   // For BOLT12: sum of all quote.amount_mintable()
   //   - amount_mintable() = amount_paid - amount_issued
   //   - This allows issuing partial amounts for overpaid quotes
   ```

3. **Add helper function:**
   ```rust
   fn calculate_batch_total_mintable_amount(
       quote_infos: &[MintQuote],
       payment_method: PaymentMethod,
   ) -> Result<Amount, Error> {
       let mut total = Amount::ZERO;
       for quote in quote_infos {
           let amount = match payment_method {
               PaymentMethod::Bolt11 => {
                   // BOLT11: exact amount from quote
                   quote.amount.ok_or(Error::AmountUndefined)?
               }
               PaymentMethod::Bolt12 => {
                   // BOLT12: amount_mintable allows partial minting
                   let mintable = quote.amount_paid - quote.amount_issued;
                   if mintable == Amount::ZERO {
                       return Err(Error::UnpaidQuote);
                   }
                   mintable
               }
               _ => return Err(Error::UnsupportedPaymentMethod),
           };
           total = total.checked_add(amount)?;
       }
       Ok(total)
   }
   ```

4. **Add BOLT12 signature generation (after amount calculation):**
   ```rust
   // For BOLT12 quotes with pubkey, generate signatures
   let signatures = if payment_method == PaymentMethod::Bolt12 {
       Some(self.prepare_bolt12_batch_signatures(&quote_infos, &blinded_messages).await?)
   } else {
       None  // BOLT11 doesn't use signatures
   };

   // Pass to request
   request.signature = signatures;
   ```

5. **Add signature construction helper:**
   ```rust
   async fn prepare_bolt12_batch_signatures(
       &self,
       quote_infos: &[MintQuote],
       blinded_messages: &[BlindedMessage],
   ) -> Result<Vec<String>, Error> {
       let signatures: Vec<String> = quote_infos
           .iter()
           .map(|quote| {
               let secret_key = quote.secret_key
                   .as_ref()
                   .ok_or(Error::MissingSecretKey)?;

               // Sign: quote_id || all_blinded_messages
               let message = format!(
                   "{}{}",
                   quote.id,
                   blinded_messages.iter()
                       .map(|bm| bm.blindedMessage.to_string())
                       .collect::<String>()
               );

               let sig = secret_key.sign(message.as_bytes());
               Ok(sig.to_string())
           })
           .collect::<Result<Vec<_>, Error>>()?;
       Ok(signatures)
   }
   ```

6. **Replace quote cleanup to UPDATE instead of REMOVE (for BOLT12):**
   ```rust
   // Current code (line 251-256) removes all quotes:
   for quote_id in quote_ids.iter() {
       if let Err(e) = self.localstore.remove_mint_quote(quote_id).await {
           tracing::warn!("Failed to remove quote {}", quote_id);
       }
   }

   // Replace with conditional cleanup:
   match payment_method {
       PaymentMethod::Bolt11 => {
           // BOLT11: remove all quotes (no partial minting)
           for quote_id in quote_ids.iter() {
               if let Err(e) = self.localstore.remove_mint_quote(quote_id).await {
                   tracing::warn!("Failed to remove quote {}", quote_id);
               }
           }
       }
       PaymentMethod::Bolt12 => {
           // BOLT12: update amount_issued, keep quotes (supports partial minting)
           for (i, quote_id) in quote_ids.iter().enumerate() {
               if let Ok(Some(mut quote)) = self.localstore.get_mint_quote(quote_id).await {
                   quote.amount_issued += proofs.total_amount()? / quote_infos.len() as u64;
                   let _ = self.localstore.add_mint_quote(quote).await;
               }
           }
       }
       _ => unreachable!()
   }
   ```

   **Note:** This is approximate - better to track per-quote amounts from mint response.

**Tests to add:**
- `test_mint_batch_bolt12_multiple` - 2+ BOLT12 quotes (min is 2 quotes for batch)
- `test_mint_batch_bolt12_overpayment` - BOLT12 quote overpaid, issues amount_mintable
- `test_mint_batch_bolt12_with_nut20_signatures` - Signature generation and passing
- `test_mint_batch_bolt12_preserves_quotes` - Quotes NOT deleted, kept for partial minting
- `test_mint_batch_bolt12_updates_amount_issued` - Quote amount_issued updated correctly

### Step 2: Modify Mint Batch Processing (1-2 hours)

**File: `cdk/src/mint/issue/mod.rs`**

**Key Insight: Mint-side already has correct amount handling AND signature verification!**

**Critical Discovery: BOLT12 batch minting REQUIRES all signatures (transitive property)**
- Batch minting requires all quotes same payment method
- BOLT12 always requires NUT-20 signatures (single quote code line 163-168)
- Therefore: BOLT12 batch requires ALL quotes to have signatures

The existing code already:
- Lines 798-828: Validates NUT-20 signatures per quote
- Lines 846-894: Uses `amount_mintable()` for BOLT12 amounts
- Line 921: Tracks per-quote `amount_issued` correctly

**What to do:**

1. **Add validation that BOLT12 batch REQUIRES all signatures (after line 828):**
   ```rust
   // For BOLT12 batch, ALL quotes must have signatures (transitive property)
   // Batch requires same payment method, and BOLT12 always requires signatures
   if payment_method == PaymentMethod::Bolt12 {
       if batch_request.signature.is_none() {
           return Err(Error::BatchBolt12RequiresSignatures);
       }
       // Verify all signatures are present (no null entries)
       for (i, sig) in batch_request.signature.iter().enumerate() {
           if sig.is_none() {
               return Err(Error::BatchBolt12RequiresSignatures);
           }
       }
   }
   ```

2. **Add clarifying comment at line 777-787:**
   ```rust
   // Per NUT-XX: BOLT12 batch minting requires all quotes to have NUT-20 signatures
   // This is transitive: batch requires same payment method (Bolt12), and BOLT12
   // always requires signatures (per NUT-20), therefore all quotes must be signed
   ```

3. **No changes needed for:**
   - Amount validation (already uses `amount_mintable()`)
   - Signature verification logic (already correct)
   - Per-quote tracking (already correct)

**Tests to add:**
- `test_batch_mint_handler_bolt12_rejects_no_signatures` - BOLT12 batch requires signatures
- `test_batch_mint_handler_bolt12_rejects_partial_signatures` - All quotes must have signatures
- `test_batch_mint_handler_bolt12_validates_all_signatures` - Each signature verified
- `test_batch_mint_handler_bolt12_overpayment_issues_mintable` - amount_mintable() used

### Step 3: Verify BOLT12 Batch Quote Status (already implemented!)

**File: `cdk/src/wallet/issue/batch.rs`**

**Good News: Batch quote status checking already works for BOLT12!**

The existing code at line 139+ already calls:
```rust
let quote_statuses = self
    .client
    .post_mint_batch_quote_status(BatchQuoteStatusRequest {
        quote: quote_ids.clone(),
    })
    .await?;
```

This endpoint works identically for BOLT11 and BOLT12 quotes. No changes needed!

**What to verify:**
1. Test that batch status check works with BOLT12 quotes
2. Ensure state transitions (Unpaid -> Paid) are tracked via batch endpoint
3. Verify amount_paid and amount_issued are returned correctly

**Tests to add:**
- `test_mint_batch_bolt12_status_check` - Batch status with BOLT12
- `test_mint_batch_bolt12_status_tracks_amount_paid` - Verify amount_paid updates
- `test_mint_batch_bolt12_status_tracks_partial_issued` - amount_issued for partial minting

### Step 4: Spending Conditions Integration (2-3 hours)

**File: `cdk/src/wallet/issue/batch.rs`**

**Note: Spending conditions for batch minting are LOWER PRIORITY and can be deferred to Phase 2.**

Spending conditions are already supported by the existing code structure. No changes needed for initial BOLT12 batch support. Future enhancement can add this capability.

**Future implementation pattern (Phase 2):**
```rust
// After proof construction
let proofs = construct_proofs(...)?;

if let Some(spending_conditions) = spending_conditions {
    // Apply to all proofs in batch
    // All proofs from all quotes share same conditions
    validate_spending_conditions_for_batch(
        &spending_conditions,
        batch_size,
    )?;
}
```

For now: **Remove spending_conditions parameter from Phase 1** or accept but ignore it with a note.

### Step 5: Comprehensive Testing (4-5 hours)

**File: `cdk-integration-tests/tests/batch_mint.rs`**

Add ~15-18 new test cases (revised to require min 2 quotes):

**Core BOLT12 Batch Tests (Minimum 2 quotes required):**
1. `test_batch_mint_bolt12_two_quotes` - Minimum batch size
2. `test_batch_mint_bolt12_exceeds_limit` - >100 quotes rejected

**BOLT12-Specific Validation:**
5. `test_batch_mint_bolt12_rejects_zero_mintable` - No payment yet
6. `test_batch_mint_bolt12_overpayment_issues_mintable` - Partial amount handling
7. `test_batch_mint_bolt12_mixed_amounts_all_issued` - Different amounts per quote

**NUT-20 Signature Tests (ALL required for BOLT12 batch):**
8. `test_batch_mint_bolt12_generates_nut20_signatures` - Signature creation for all quotes
9. `test_batch_mint_bolt12_signature_verification` - Server verifies all signatures
10. `test_batch_mint_bolt12_rejects_missing_signatures` - Error if no signatures provided
11. `test_batch_mint_bolt12_rejects_partial_signatures` - Error if any signature null
12. `test_batch_mint_bolt12_rejects_invalid_signatures` - Invalid sig rejected

**State & Consistency Tests:**
13. `test_batch_mint_bolt12_updates_amount_issued_per_quote` - Per-quote tracking
14. `test_batch_mint_bolt12_atomic_all_or_nothing` - Atomicity guarantee
15. `test_batch_mint_bolt12_event_publishing` - Events for all quotes

**Mixed Payment Method Tests (Error Cases):**
16. `test_batch_mint_rejects_mixed_bolt11_bolt12` - Must be uniform

**Integration Tests:**
17. `test_batch_mint_bolt12_end_to_end` - Full wallet -> mint -> wallet flow
18. `test_batch_mint_bolt12_with_fake_wallet` - Against FakeWallet backend

### Step 6: Documentation (1-2 hours)

**Files to create/update:**

1. **NUT-XX Specification Reference**
   ```
   Path: docs/NUT-XX-BATCH-MINTING.md
   Content:
   - BOLT11 batch minting spec
   - BOLT12 batch minting extensions
   - NUT-20 signature integration
   - API endpoint documentation
   - Examples
   ```

2. **Code Documentation**
   ```
   Update:
   - cdk/src/wallet/issue/batch.rs - rustdoc comments
   - cdk/src/mint/issue/mod.rs - batch processing docs
   - cdk-common/src/mint.rs - type documentation
   ```

3. **Examples**
   ```
   Create: examples/batch_mint_bolt11.rs
   Create: examples/batch_mint_bolt12.rs
   Create: examples/batch_mint_spending_conditions.rs
   ```

---

## Implementation Timeline

| Phase | Task | Hours | Owner | Status |
|-------|------|-------|-------|--------|
| 1 | Wallet: enable BOLT12, add signatures, amount calculation | 2-3 | TBD | Pending |
| 1 | Mint: validation and comments (amount handling already works) | 1 | TBD | Pending |
| 1 | Verify batch quote status works with BOLT12 | <1 | TBD | Pending |
| 2 | Comprehensive test suite (15-18 tests) | 4-5 | TBD | Pending |
| 2 | Documentation | 1-2 | TBD | Pending |
| 3 | Spending conditions (Phase 2 enhancement) | 2-3 | TBD | Deferred |
| **Phase 1 Total** | | **7-9 hours** | | |
| **Phase 2 Total** | | **5-7 hours** | | |

---

## Risk Analysis & Mitigation

### Risk 1: Overpayment Handling (LOW - Already Implemented)
**Risk:** BOLT12 allows overpayment; must issue only amount_mintable
**Status:** ✅ RESOLVED - Mint-side already uses `quote.amount_mintable()` correctly
**Mitigation:** Wallet must match this behavior by using `amount_paid - amount_issued` for amount calculation

### Risk 2: NUT-20 Signature Implementation (LOW - Already Implemented!)
**Risk:** Signature generation and verification for BOLT12 batch could be incorrect
**Status:** ✅ RESOLVED - Signature verification already exists in batch code (lines 798-828)
**Mitigation:**
- Signature verification logic already tested for BOLT11 batch
- Same verification code works for BOLT12
- Only need to add validation that ALL signatures must be present (not optional)

### Risk 3: BOLT12 Signature Requirement (LOW - Transitive Property)
**Risk:** BOLT12 batch must require all signatures; missing signatures could bypass NUT-20
**Status:** ✅ RESOLVED - Enforced by transitive property
**Mitigation:**
- Add validation at line 828: check `batch_request.signature.is_some()` for BOLT12
- Check all signatures are non-null (no `None` entries)
- Matches single BOLT12 minting behavior (line 163-168 in issue_bolt12.rs)

### Risk 4: Atomicity (LOW - Already Implemented)
**Risk:** Partial failures in batch could leave inconsistent state
**Mitigation:**
- ✅ Already using transaction pattern from BOLT11
- ✅ All-or-nothing semantics via `tx.begin/commit()`
- Test failure scenarios with insufficient amount_mintable

### Risk 5: Breaking Changes (NONE)
**Risk:** Changes could break existing BOLT11 clients
**Status:** ✅ MITIGATED - No API changes needed
- Mint-side amount validation already handles both methods
- NUT-20 signature verification already present
- Only wallet-side restriction check needs removal

---

## Testing Strategy

### Unit Tests (Per-Module)
- `cdk/src/wallet/issue/batch.rs` - Wallet batch logic
- `cdk/src/mint/issue/mod.rs` - Mint processing
- `cdk-common/src/mint.rs` - Type serialization

### Integration Tests
- `cdk-integration-tests/tests/batch_mint.rs` - End-to-end scenarios
- Wallet + Mint interaction with FakeWallet backend
- ~50 total test cases (32 existing + 20 new)

### Mutation Testing
```bash
just mutants-batch-mint  # Validate test quality
```

### Database Testing
```bash
# Test against all backends
CDK_TEST_DB_TYPE=memory cargo test
CDK_TEST_DB_TYPE=sqlite cargo test
CDK_TEST_DB_TYPE=redb cargo test
```

---

## Key References

### Existing Code
- **BOLT11 Batch Wallet:** `cdk/src/wallet/issue/batch.rs` (500 lines)
- **BOLT11 Batch Mint:** `cdk/src/mint/issue/mod.rs` (400 lines)
- **BOLT12 Single Mint:** `cdk/src/wallet/issue/issue_bolt12.rs` (80 lines)
- **BOLT12 Single Mint Handler:** `cdk/src/mint/issue/mod.rs` (parallel logic)

### Protocol Specs
- **NUT-00:** Cryptographic Primitives
- **NUT-04:** Mint Specification
- **NUT-12:** BOLT12 Spending Conditions
- **NUT-20:** Deterministic Secrets
- **NUT-25:** BOLT12 Payments (Offers)
- **NUT-XX:** Batch Minting (draft)

### Test Files
- `cdk-integration-tests/tests/batch_mint.rs` - Handler tests
- `cdk/tests/wallet_batch_mint.rs` - Wallet tests

---

## Success Criteria (Phase 1)

1. ✅ BOLT12 batch minting enabled in wallet (remove restriction)
2. ✅ Wallet correctly calculates total_mintable from `amount_paid - amount_issued`
3. ✅ NUT-20 signatures generated and passed to mint
4. ✅ Mint validates all BOLT12 quotes have pubkey
5. ✅ Overpayment handled correctly (issues only amount_mintable)
6. ✅ Amount-issued tracking per-quote uses amount_mintable()
7. ✅ Batch quote status verification works with BOLT12
8. ✅ 15-18 comprehensive test cases pass
9. ✅ >90% test coverage for batch BOLT12 paths
10. ✅ All tests pass on memory, sqlite, and redb backends
11. ✅ No breaking changes to existing BOLT11 batch behavior
12. ✅ Documentation updated with BOLT12 batch specifics

## Success Criteria (Phase 2 - Future)

1. ✅ Spending conditions support added to batch minting
2. ✅ Integration tests with spending condition scenarios
3. ✅ Documentation updated for spending conditions + batch

---

## Related Work

### Prior Art in BOLT11 Batch Minting
- **Commit a8f05f4c:** Docs cleanup
- **Commit e5076632:** Integration test batch mint twice
- **Commit a93e30c9:** Fix batch mint integration tests
- **Commit 1bbf6511:** Add payment method path param
- **Commit 484f640f:** Code review
- **Commit 81cc02d1:** Implement batched quote minting

### Future Enhancements
- Transaction support in quote cleanup (Phase 3)
- BOLT12 spending condition advanced features
- Performance optimization for very large batches (>100)
- Caching optimization for repeated batch requests

---

## Appendix: Detailed Code Examples

### Example 1: BOLT12 Batch Request (Wallet) - Minimum 2 Quotes
```rust
let quote_ids = vec![
    "bolt12_quote_1".to_string(),
    "bolt12_quote_2".to_string(),
    "bolt12_quote_3".to_string(),
];

// All quotes must be BOLT12, have pubkey, be PAID state
let proofs = wallet.mint_batch(
    quote_ids,
    SplitTarget::default(),
    None,  // Spending conditions deferred to Phase 2
    PaymentMethod::Bolt12,
).await?;

// Returns proofs for total of all amount_mintable() amounts
// If quotes are overpaid, issues only the remaining amount per quote
```

### Example 2: BOLT12 Batch Amount Calculation (Wallet)
```rust
// In wallet.mint_batch() for BOLT12:

// Scenario: 3 BOLT12 quotes with different states
// Quote 1: amount_paid=1000, amount_issued=0 -> amount_mintable=1000
// Quote 2: amount_paid=1000, amount_issued=600 -> amount_mintable=400
// Quote 3: amount_paid=1000, amount_issued=1000 -> amount_mintable=0 (REJECT!)

let total_mintable = calculate_batch_total_mintable_amount(&quote_infos)?;
// Returns 1400 (1000 + 400), or error if any quote has amount_mintable=0

// Then create blinded messages for 1400 total
let premint_secrets = PreMintSecrets::from_seed(
    active_keyset_id,
    count,
    &self.seed,
    total_mintable,  // 1400
    &amount_split_target,
    &fee_and_amounts,
)?;

let blinded_messages = premint_secrets.blinded_messages();
// blinded_messages.len() equals number of denominations in 1400
// NOT number of quotes!
```

### Example 3: BOLT12 Batch Signature Generation (Wallet)
```rust
// In wallet.mint_batch() for BOLT12:
let signatures: Vec<String> = quote_infos
    .iter()
    .map(|quote| {
        // Get the secret key stored with this BOLT12 quote
        let secret_key = quote.secret_key
            .as_ref()
            .ok_or(Error::MissingSecretKey)?;

        // Create signature covering quote_id + all blinded messages
        // All quotes sign the SAME blinded messages!
        let message = format!(
            "{}{}",
            quote.id,
            blinded_messages.iter()
                .map(|bm| bm.blindedMessage.to_string())
                .collect::<String>()
        );

        let sig = secret_key.sign(message.as_bytes());
        Ok(sig.to_string())
    })
    .collect::<Result<Vec<_>, Error>>()?;

request.signature = Some(signatures);
// signatures.len() == quote_ids.len() (one per quote)
// blinded_messages.len() != quote_ids.len() (shared blinded messages)
```

### Example 4: BOLT12 Batch Amount Validation (Mint) - Already Correct!
```rust
// In process_batch_mint_request() for BOLT12:
// No changes needed! Existing code already handles this correctly:

// Line 854-858: Uses amount_mintable() per quote
PaymentMethod::Bolt12 => {
    if quote.amount_mintable() == Amount::ZERO {
        return Err(Error::UnpaidQuote);
    }
    quote.amount_mintable()  // = amount_paid - amount_issued
}

// Line 884-893: Allows outputs <= total_amount for BOLT12
if outputs_amount > total_mint_amount {
    return Err(Error::TransactionUnbalanced(...));
}

// Line 921: Increments per-quote amount_issued
PaymentMethod::Bolt12 => quotes[i].amount_mintable(),
```

### Example 5: BOLT12 Overpayment Scenario
```rust
// Scenario: Wallet overpaid BOLT12 offer
//
// Quote requested: 1000 sats
// Wallet paid: 1500 sats (overpayment!)
// amount_paid: 1500
// amount_issued: 0
// amount_mintable: 1500

// Wallet behavior:
// 1. Calls mint_batch() with total_mintable = 1500
// 2. Creates blinded messages for 1500
// 3. Sends to mint with signatures

// Mint behavior (line 854-858):
// 1. Calculates total_mintable = 1500 (sum of all quotes)
// 2. Verifies outputs_amount <= 1500
// 3. Issues all minted proofs to wallet
// 4. Updates amount_issued += 1500 for that quote
// 5. Next call: amount_mintable = 0 (fully redeemed overpayment)

// This matches single BOLT12 minting behavior!
```

---

## Questions for Review

1. **Signature Format:** Confirmed - signature covers `quote_id || all_blinded_messages` (per existing batch code line 814).

2. **BOLT12 Signature Requirement:** Confirmed - BOLT12 batch requires ALL signatures (transitive: batch same payment method + BOLT12 requires signatures = all must sign).

3. **Wallet Amount Calculation:** Confirm that wallet should calculate `total_mintable = sum(quote.amount_paid - quote.amount_issued)` for BOLT12 batches? This allows partial minting when quotes are overpaid.

4. **Error Handling:** Need to add `BatchBolt12RequiresSignatures` error type to `cdk_common::Error` enum?

5. **Spending Conditions:** Defer to Phase 2 for initial BOLT12 batch support. Acceptable?

6. **Any additional error scenarios** beyond signature requirement, amount_mintable validation, and per-quote tracking?

---

**End of Development Plan**

This plan is ready for implementation. All architectural decisions are informed by the existing BOLT11 batch implementation and follow the established patterns in the CDK codebase.

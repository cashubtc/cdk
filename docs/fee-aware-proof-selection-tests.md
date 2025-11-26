# Fee-Aware Proof Selection Test Cases

This document outlines test cases for validating the proof selection and swap logic when input fees are enabled.

## Implementation Status

### ✅ Implemented (in `crates/cdk/src/wallet/proofs.rs`, `crates/cdk/src/wallet/send.rs`, and `crates/cdk/src/fees.rs`)

| Category | Tests Implemented |
|----------|-------------------|
| Fee Calculation (`fees.rs`) | 7 tests |
| Basic Selection (No Fees) | 4 tests (existing) |
| Selection With Input Fees (fee_ppk = 200) | 7 tests |
| Selection With High Fees (fee_ppk = 1000) | 4 tests |
| Edge Cases | 5 tests |
| Stress Tests | 3 tests |
| Regression Tests | 3 tests |
| `split_proofs_for_send` (proof split logic) | 35 tests |

### ❌ Not Yet Implemented

| Category | Reason |
|----------|--------|
| Integration Tests (`prepare_send` + `confirm`) | Requires full wallet + mint setup |
| Integration Tests (Spending Conditions) | Requires P2PK/HTLC infrastructure |
| Integration Tests (Melt With Fees) | Requires Lightning integration |
| Property-Based Tests | Requires proptest dependency |

---

## Background

When a mint charges input fees (fee per proof, expressed as parts per thousand - ppk), the wallet must account for these fees during:

1. **Proof Selection**: Selecting proofs to spend must account for the fee that will be deducted
2. **Send Preparation**: Splitting proofs between direct send and swap must ensure the swap has sufficient funds
3. **Swap Execution**: The swap must produce the correct output amounts after paying input fees

### Fee Calculation

```
fee = ceil(num_proofs * fee_ppk / 1000)
```

For example, with `fee_ppk = 200`:
- 1 proof: `ceil(200/1000) = 1 sat`
- 3 proofs: `ceil(600/1000) = 1 sat`
- 5 proofs: `ceil(1000/1000) = 1 sat`
- 6 proofs: `ceil(1200/1000) = 2 sats`

---

## Unit Tests: `select_proofs` ✅ IMPLEMENTED

### Basic Selection (No Fees) ✅

| Status | Test | Proofs Available | Amount | include_fee | Expected Selection | Notes |
|--------|------|------------------|--------|-------------|-------------------|-------|
| ✅ | Exact match | [8, 4, 2, 1] | 5 | false | [4, 1] | `test_select_proofs_exact` |
| ✅ | Over-select minimal | [8, 4, 2] | 5 | false | [8] | `test_select_proofs_over` |
| ✅ | Multiple proofs needed | [4, 2, 1, 1] | 7 | false | [4, 2, 1] | `test_select_proofs_smaller_over` |
| ✅ | Insufficient funds | [4, 2, 1] | 10 | false | Error | `test_select_proofs_insufficient` |

### Selection With Input Fees (fee_ppk = 200) ✅

| Status | Test | Proofs Available | Amount | include_fee | Expected Selection | Notes |
|--------|------|------------------|--------|-------------|-------------------|-------|
| ✅ | Single proof exact | [4096] | 4095 | true | [4096] | `test_select_proofs_with_fees_single_proof_exact` |
| ✅ | Single proof insufficient | [4096] | 4096 | true | Error | `test_select_proofs_with_fees_single_proof_insufficient` |
| ✅ | Two proofs fee threshold | [4096, 1024] | 5000 | true | [4096, 1024] | `test_select_proofs_with_fees_two_proofs_fee_threshold` |
| ✅ | Iterative fee adjustment | [4096, 1024, 512, 256, 128, 8] | 5000 | true | All needed | `test_select_proofs_with_fees_iterative_fee_adjustment` |
| ✅ | Fee increases with proofs | [1024, 1024, 1024, 1024, 1024] | 5000 | true | 5x1024 | `test_select_proofs_with_fees_fee_increases_with_proofs` |
| ✅ | Standard proofs | powers of 2 up to 4096 | 5000 | true | Optimal selection | `test_select_proofs_with_fees_standard_proofs` |
| ✅ | Mixed proofs | various denominations | 5000 | true | Optimal selection | `test_select_proofs_with_fees_mixed_proofs` |

### Selection With High Fees (fee_ppk = 1000) ✅

| Status | Test | Proofs Available | Amount | include_fee | Expected Selection | Notes |
|--------|------|------------------|--------|-------------|-------------------|-------|
| ✅ | 1 sat per proof | [4096, 1024, 512, ...] | 5000 | true | Selected covers amount | `test_select_proofs_high_fees_one_sat_per_proof` |
| ✅ | Prefers larger proofs | [64x100, 4096, 1024] | 5000 | true | Uses large proofs | `test_select_proofs_high_fees_prefers_larger_proofs` |
| ✅ | Exact with fee | [4096, 1024] | 5118 | true | [4096, 1024] | `test_select_proofs_high_fees_exact_with_fee` |
| ✅ | Large proofs only | [4096, 2048, 1024, 512, 256] | 5000 | true | Optimal selection | `test_select_proofs_high_fees_large_proofs` |

### Edge Cases ✅

| Status | Test | Proofs Available | Amount | include_fee | Expected Selection | Notes |
|--------|------|------------------|--------|-------------|-------------------|-------|
| ✅ | Zero amount | standard_proofs() | 0 | true | [] | `test_select_proofs_with_fees_zero_amount` |
| ✅ | Empty proofs | [] | 5000 | true | Error | `test_select_proofs_with_fees_empty_proofs` |
| ✅ | All proofs same size | [1024x6] | 5000 | true | 6x1024 | `test_select_proofs_with_fees_all_proofs_same_size` |
| ✅ | Fee exceeds small proof | [1] (fee_ppk=1000) | 1 | true | Error | `test_select_proofs_with_fees_fee_exceeds_small_proof` |
| ✅ | Barely sufficient | [4096, 1024, 512, 256, 128, 8, 1] | 5000 | true | All proofs | `test_select_proofs_with_fees_barely_sufficient` |

---

## Unit Tests: `split_proofs_for_send` ✅ IMPLEMENTED

> **Note:** The core logic of `internal_prepare_send` has been extracted into the `split_proofs_for_send` pure function
> in `crates/cdk/src/wallet/send.rs`, which can be unit tested without a wallet instance.

These tests verify the splitting of proofs between `proofs_to_send` and `proofs_to_swap`.

### No Swap Needed (Exact Proofs) ✅

| Status | Test | Selected Proofs | Amount | send_fee | Notes |
|--------|------|-----------------|--------|----------|-------|
| ✅ | Exact match simple | [8, 2] | 10 | 1 | `test_split_exact_match_simple` |
| ✅ | Exact match six proofs | [2048, 1024, 512, 256, 128, 32] | 4000 | 2 | `test_split_exact_match_six_proofs` |
| ✅ | Exact match ten proofs | [4096, 2048, 1024, 512, 256, 128, 64, 32, 16, 8] | 8000 | 2 | `test_split_exact_match_ten_proofs` |
| ✅ | Exact match powers of two | [4096, 512, 256, 128, 8] | 5000 | 1 | `test_split_exact_match_powers_of_two` |

### Swap Required - Partial Match ✅

| Status | Test | Selected Proofs | Amount | Notes |
|--------|------|-----------------|--------|-------|
| ✅ | Single mismatch | [8, 4, 2, 1] | 10 | `test_split_single_mismatch` |
| ✅ | Multiple mismatches | [4096, 1024, 512, 256, 64, 32, 16, 8] | 5000 | `test_split_multiple_mismatches` |
| ✅ | Half match | [2048, 2048, 1024, 512, 256, 128, 64, 32] | 5000 | `test_split_half_match` |
| ✅ | Large swap set | [1024×5, 512, 256, 128, 64, 32, 16, 8] | 5000 | `test_split_large_swap_set` |
| ✅ | Dense small proofs | [512, 256×2, 128×3, 64×4, 32×2, 16×2, 8×2, 4×2, 2×2] | 1500 | `test_split_dense_small_proofs` |

### Swap Required - No Match ✅

| Status | Test | Selected Proofs | Amount | Notes |
|--------|------|-----------------|--------|-------|
| ✅ | Fragmented no match | [64×10, 32×5, 16×10, 8×5] | 1000 | `test_split_fragmented_no_match` |
| ✅ | Large fragmented | [256×8, 128×4, 64×8, 32×4, 16×8, 8×4] | 500 | `test_split_large_fragmented` |

### Swap Fee Adjustment (Moving Proofs from Send to Swap) ✅

| Status | Test | fee_ppk | Notes |
|--------|------|---------|-------|
| ✅ | Swap sufficient | 200 | `test_split_swap_sufficient` |
| ✅ | Swap barely sufficient | 200 | `test_split_swap_barely_sufficient` |
| ✅ | Move one proof | 200 | `test_split_move_one_proof` |
| ✅ | Move multiple proofs | 200 | `test_split_move_multiple_proofs` |
| ✅ | High fee many proofs | 1000 | `test_split_high_fee_many_proofs` |
| ✅ | Fee eats small proofs | 1000 | `test_split_fee_eats_small_proofs` |
| ✅ | Cascading fee increase | 500 | `test_split_cascading_fee_increase` |

### Complex Scenarios with Many Proofs ✅

| Status | Test | Proof Count | fee_ppk | Notes |
|--------|------|-------------|---------|-------|
| ✅ | 20 proofs mixed | 20 | 200 | `test_split_20_proofs_mixed` |
| ✅ | 30 small proofs | 30 | 200 | `test_split_30_small_proofs` |
| ✅ | 15 proofs high fee | 15 | 500 | `test_split_15_proofs_high_fee` |
| ✅ | Uniform 25 proofs | 25 | 200 | `test_split_uniform_25_proofs` |
| ✅ | Tiered 18 proofs | 18 | 200 | `test_split_tiered_18_proofs` |
| ✅ | Dust consolidation | 250 | 100 | `test_split_dust_consolidation` |

### Force Swap Scenarios ✅

| Status | Test | Proof Count | Notes |
|--------|------|-------------|-------|
| ✅ | Force swap 8 proofs | 8 | `test_split_force_swap_8_proofs` |
| ✅ | Force swap 15 proofs | 15 | `test_split_force_swap_15_proofs` |
| ✅ | Force swap fragmented | 40 | `test_split_force_swap_fragmented` |

### Edge Cases ✅

| Status | Test | Notes |
|--------|------|-------|
| ✅ | Single large proof | `test_split_single_large_proof` |
| ✅ | Many 1-sat proofs | `test_split_many_1sat_proofs` |
| ✅ | All same denomination | `test_split_all_same_denomination` |
| ✅ | Alternating sizes | `test_split_alternating_sizes` |
| ✅ | Power of two boundary | `test_split_power_of_two_boundary` |
| ✅ | Just over boundary | `test_split_just_over_boundary` |

### Regression Tests ✅

| Status | Test | Notes |
|--------|------|-------|
| ✅ | Insufficient swap fee | `test_split_regression_insufficient_swap_fee` |
| ✅ | Many small in swap | `test_split_regression_many_small_in_swap` |

---

## Integration Tests: `prepare_send` + `confirm` ❌ NOT IMPLEMENTED

> **Note:** These tests require a full wallet + mint setup with the fake wallet or regtest environment.

End-to-end tests that verify the complete send flow.

### Basic Send With Fees (fee_ppk = 200)

```rust
// Setup: Wallet has proofs [16, 8, 4, 2, 1] = 31 sats
// Action: Send 10 sats with include_fee = true

#[tokio::test]
async fn test_send_with_fees_exact_match() {
    // Mint 31 sats worth of proofs
    // Attempt to send 10 sats
    // Verify: Token contains proofs totaling 10 + send_fee
    // Verify: Wallet balance reduced by 10 + total_fee
}
```

### Send Requiring Swap

```rust
// Setup: Wallet has proofs [16, 1, 1, 1, 1] = 20 sats (no 8, 4, 2)
// Action: Send 10 sats with include_fee = true
// Expected: Swap is required to create correct denominations

#[tokio::test]
async fn test_send_requiring_swap() {
    // The 16-sat proof must be swapped to get [8, 2] for sending
    // Verify swap succeeds
    // Verify correct proofs in token
}
```

### Send With High Fees

```rust
// Setup: fee_ppk = 1000 (1 sat per proof)
// Wallet has many small proofs [1, 1, 1, 1, 2, 2, 4, 8]
// Action: Send 10 sats

#[tokio::test]
async fn test_send_high_fees_avoids_small_proofs() {
    // Should prefer [8, 4] over [8, 2, 1, 1] to minimize fees
    // Verify selection minimizes proof count
}
```

### Edge Case: Small Swap Proof

```rust
// Setup: fee_ppk = 200
// Wallet has [8, 2, 1] = 11 sats
// Action: Send 10 sats (send_amounts = [8, 2], need 11 with fee)

#[tokio::test]
async fn test_send_small_swap_proof_moves_to_swap() {
    // Initial split: to_send = [8, 2], to_swap = [1]
    // Problem: swap needs to produce 1 sat but 1-sat proof - 1 fee = 0
    // Solution: Move 2-sat to swap
    // Final: to_send = [8], to_swap = [2, 1]
    // Swap produces: 3 - 1 = 2 sats for spending condition
    // Plus 8-sat direct = 10 sats + fee ✓
}
```

---

## Integration Tests: Swap With Spending Conditions ❌ NOT IMPLEMENTED

> **Note:** These tests require P2PK/HTLC infrastructure and a full wallet + mint setup.

### P2PK Send

```rust
#[tokio::test]
async fn test_p2pk_send_with_fees() {
    // Setup: Wallet has [16, 8, 4, 2, 1]
    // Action: Send 10 sats to a P2PK pubkey with include_fee = true
    // Verify: Output proofs have P2PK spending condition
    // Verify: Amount covers 10 + redemption fee
}
```

### HTLC Send

```rust
#[tokio::test]
async fn test_htlc_send_with_fees() {
    // Similar to P2PK but with HTLC conditions
}
```

---

## Integration Tests: Melt With Fees ❌ NOT IMPLEMENTED

> **Note:** These tests require Lightning integration (regtest or fake wallet).

### Basic Melt

```rust
#[tokio::test]
async fn test_melt_with_input_fees() {
    // Setup: Wallet has proofs, mint has fee_ppk = 200
    // Action: Melt to pay a Lightning invoice
    // Verify: Proof selection accounts for input fees
    // Verify: Mint accepts the proofs (no TransactionUnbalanced error)
}
```

### Melt Edge Case: Just Enough

```rust
#[tokio::test]
async fn test_melt_just_enough_with_fees() {
    // Setup: Invoice = 100 sats, fee_reserve = 10, input_fee = 1
    // Wallet has exactly 111 sats
    // Verify: Melt succeeds without InsufficientFunds
}
```

---

## Stress Tests ✅ IMPLEMENTED

| Status | Test | Location |
|--------|------|----------|
| ✅ | Many small proofs (500x16 + 200x8 + 100x4) for 5000 sats | `test_select_proofs_many_small_proofs_with_fees` |
| ✅ | Fee convergence (600x16) for 5000 sats | `test_select_proofs_fee_convergence_with_many_proofs` |
| ✅ | Fragmented proofs (various denominations) for 5000 sats | `test_select_proofs_fragmented_proofs_with_fees` |

---

## Property-Based Tests ❌ NOT IMPLEMENTED

> **Note:** Requires adding `proptest` as a dev dependency.

Using a property testing framework (e.g., proptest):

```rust
proptest! {
    #[test]
    fn prop_selected_proofs_cover_amount_plus_fee(
        proofs in vec(1u64..1000, 1..50),
        amount in 1u64..500,
        fee_ppk in 0u64..1000,
    ) {
        // Property: If selection succeeds, selected_total - fee >= amount
        let selected = select_proofs(amount, proofs, fee_ppk, true);
        if let Ok(selected) = selected {
            let total: u64 = selected.iter().sum();
            let fee = (selected.len() as u64 * fee_ppk + 999) / 1000;
            assert!(total - fee >= amount);
        }
    }

    #[test]
    fn prop_swap_produces_correct_output(
        input_proofs in vec(1u64..100, 1..20),
        amount in 1u64..50,
        fee_ppk in 0u64..500,
    ) {
        // Property: swap(amount) produces proofs totaling amount
        // (when include_fees = false)
    }
}
```

---

## Test Fixtures ✅ IMPLEMENTED

> **Note:** Implemented in `crates/cdk/src/wallet/proofs.rs` test module.

### Standard Proof Sets

```rust
fn standard_proofs() -> Vec<Proof> {
    // ✅ Powers of 2: [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096]
}

fn fragmented_proofs() -> Vec<Proof> {
    // ✅ Many small: 10x1, 8x2, 6x4, 5x8, 4x16, 3x32, 2x64, 2x128, 2x256, 2x512, 2x1024, 2x2048
}

fn large_proofs() -> Vec<Proof> {
    // ✅ Few large: [4096, 2048, 1024, 512, 256]
}

fn mixed_proofs() -> Vec<Proof> {
    // ✅ Combination: [4096, 1024, 256, 256, 128, 64, 32, 16, 8, 4, 2, 1, 1]
}
```

### Fee Configurations

```rust
// ✅ Implemented via keyset_fee_and_amounts_with_fee(fee_ppk) helper function
const NO_FEE: u64 = 0;
const LOW_FEE: u64 = 100;    // 0.1 sat per proof
const MEDIUM_FEE: u64 = 200; // 0.2 sat per proof
const HIGH_FEE: u64 = 1000;  // 1 sat per proof
```

---

## Regression Tests ✅ IMPLEMENTED

| Status | Test | Location |
|--------|------|----------|
| ✅ | Swap insufficient small proof | `test_regression_swap_insufficient_small_proof` |
| ✅ | Fragmented proofs with fees | `test_regression_fragmented_proofs_with_fees` |
| ✅ | Exact amount with multiple denominations | `test_regression_exact_amount_with_multiple_denominations` |

---

## Test Utilities ✅ PARTIALLY IMPLEMENTED

```rust
// ✅ Implemented
fn keyset_fee_and_amounts_with_fee(fee_ppk: u64) -> HashMap<Id, FeeAndAmounts>;
fn proof(amount: u64) -> Proof;
fn id() -> Id;

// ❌ Not implemented (require wallet infrastructure)
/// Create a mock wallet with specific proofs
async fn wallet_with_proofs(amounts: &[u64], fee_ppk: u64) -> Wallet;

/// Verify token has correct total amount
fn assert_token_amount(token: &Token, expected: u64);

/// Verify wallet balance after operation
async fn assert_balance(wallet: &Wallet, expected: u64);

// ✅ Implemented in fees.rs
/// Calculate expected fee for proof count
fn expected_fee(num_proofs: usize, fee_ppk: u64) -> u64 {
    ((num_proofs as u64 * fee_ppk) + 999) / 1000
}
```

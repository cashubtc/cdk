# Wallet Fee Selection Bug Analysis

## Issue Summary

The wallet's `select_proofs` function with `include_fees=true` was not properly accounting for the fees of additional proofs when the initial selection was insufficient to cover the required amount after fees.

This caused melt operations to fail with "transaction unbalanced" errors even when the wallet had sufficient funds.

## The Bug

### Location
`crates/cdk/src/wallet/proofs.rs` in the `include_fees` function.

### Root Cause

When `include_fees` determined that the initially selected proofs were insufficient (because `net_amount < amount` after subtracting fees), it made a recursive call to select additional proofs:

```rust
// BUGGY CODE
selected_proofs.extend(Wallet::select_proofs(
    remaining_amount,
    remaining_proofs,
    active_keyset_ids,
    &HashMap::new(), // BUG: No fee information!
    false,           // BUG: Don't include fees for additional proofs
)?);
```

The problems:
1. `&HashMap::new()` - No fee information was passed, so the recursive call couldn't calculate fees
2. `false` - Even if fee info was passed, it wouldn't be used

This meant that when additional proofs were selected to cover a fee shortfall, the fees for those additional proofs were never accounted for.

### Example Scenario

Consider a melt operation:
- Invoice amount: **1000 sats**
- Lightning fee reserve: **10 sats** (1%)
- NUT-02 input fee: **100 ppk** (0.1 sat per proof)

**Wallet calculation:**
```
inputs_needed = amount + fee_reserve = 1000 + 10 = 1010 sats
```

**Wallet calls:** `select_proofs(1010, proofs, ..., include_fees=true)`

**Step-by-step with the bug:**

1. Initial selection: 10 proofs totaling 1010 sats
2. Calculate fee: `ceil(10 * 100 / 1000) = 1 sat`
3. Net amount: `1010 - 1 = 1009 sats`
4. Check: `1009 >= 1010`? **NO** - need more proofs
5. Shortfall: `1010 - 1009 = 1 sat`
6. **BUG:** Select 1 more proof (say 2 sats) with `include_fees=false`
7. Final proofs: 11 proofs totaling 1012 sats
8. **But actual fee is:** `ceil(11 * 100 / 1000) = ceil(1.1) = 2 sats`
9. Wallet sends 1012 sats worth of proofs

**Mint validation:**
```
required = amount + fee_reserve + input_fee
required = 1000 + 10 + 2 = 1012 sats
inputs  = 1012 sats
```

In this case it happens to work! But consider if the initial selection was different:

1. Initial selection: 10 proofs totaling 1010 sats
2. Fee: 1 sat, Net: 1009 sats
3. Need 1 more sat, select 1 proof of 1 sat
4. Total: 11 proofs, 1011 sats
5. **Actual fee:** `ceil(11 * 100 / 1000) = 2 sats`
6. **Mint requires:** `1000 + 10 + 2 = 1012 sats`
7. **Wallet provides:** `1011 sats`
8. **REJECTED:** `1011 < 1012`

## Misleading Mint Log Message

The mint's log message when rejecting the transaction was:

```
Melt request unbalanced: inputs 1010, amount 1000, fee 2
```

This log is **misleading** because:

1. It only shows `fee` (the NUT-02 input fee of 2 sats)
2. It does **NOT** show `fee_reserve` (the lightning routing fee reserve of ~10 sats)
3. The actual check is: `inputs >= amount + fee_reserve + fee`

Looking at just the log, it appears:
- inputs = 1010
- amount = 1000  
- fee = 2
- So `1010 >= 1000 + 2 = 1002` should pass ✓

But the actual calculation includes `fee_reserve`:
- inputs = 1010
- required = 1000 + 10 + 2 = 1012
- `1010 < 1012` → **FAIL** ✗

### Suggested Log Improvement

The log in `crates/cdk/src/mint/melt/melt_saga/mod.rs` should be updated from:

```rust
tracing::info!(
    "Melt request unbalanced: inputs {}, amount {}, fee {}",
    input_amount,
    quote.amount,
    fee
);
```

To:

```rust
tracing::info!(
    "Melt request unbalanced: inputs {}, amount {}, fee_reserve {}, input_fee {}, required {}",
    input_amount,
    quote.amount,
    quote.fee_reserve,
    fee,
    required_total
);
```

This would produce:
```
Melt request unbalanced: inputs 1010, amount 1000, fee_reserve 10, input_fee 2, required 1012
```

Which makes the math clear and easier to debug.

## The Fix

Changed `include_fees` to use a loop that recalculates fees after each addition of proofs:

```rust
fn include_fees(...) -> Result<Proofs, Error> {
    let keyset_fees: HashMap<Id, u64> = fees_and_keyset_amounts
        .iter()
        .map(|(key, values)| (*key, values.fee()))
        .collect();

    let mut remaining_proofs: Proofs = proofs
        .into_iter()
        .filter(|p| !selected_proofs.contains(p))
        .collect();

    loop {
        // Recalculate fee with current selection
        let fee = calculate_fee(&selected_proofs.count_by_keyset(), &keyset_fees)
            .unwrap_or_default();
        let total = selected_proofs.total_amount()?;
        let net_amount = total - fee;

        // Check if we have enough
        if net_amount >= amount {
            return Ok(selected_proofs);
        }

        // Need more proofs
        if remaining_proofs.is_empty() {
            return Err(Error::InsufficientFunds);
        }

        let shortfall = amount - net_amount;
        
        // Select additional proofs (with fee info for optimal selection)
        let additional = Wallet::select_proofs(
            shortfall,
            remaining_proofs.clone(),
            active_keyset_ids,
            fees_and_keyset_amounts,  // Pass fee info
            false,  // Don't recurse into include_fees
        )?;

        remaining_proofs.retain(|p| !additional.contains(p));
        selected_proofs.extend(additional);
        
        // Loop back to recalculate total fee with new proofs
    }
}
```

The key insight is that adding proofs to cover a fee shortfall may increase the total fee, requiring yet more proofs. The loop continues until the fee stabilizes.

## NUT-02 Spec Reference

From NUT-02:

> When constructing a transaction with ecash `inputs` (example: `/v1/swap` or `/v1/melt`), wallets **MUST** add fees to the inputs or, vice versa, subtract from the outputs.

The fee calculation:
```python
def fees(inputs: List[Proof]) -> int:
    sum_fees = 0
    for proof in inputs:
        sum_fees += keysets[proof.id].input_fee_ppk
    return (sum_fees + 999) // 1000
```

For a melt, the mint validates:
```
sum(inputs) >= amount + fee_reserve + input_fees
```

Where:
- `amount` - The invoice amount being paid
- `fee_reserve` - Lightning routing fee reserve (returned as change if not fully used)
- `input_fees` - NUT-02 fees for spending the input proofs

## Testing

Two new tests were added to verify the fix:

1. **`test_select_proofs_include_fees_accounts_for_additional_proof_fees`**
   - Tests the specific scenario from the bug report
   - Verifies that net amount after fees is >= requested amount

2. **`test_select_proofs_include_fees_iterates_until_stable`**
   - Tests that the loop correctly iterates when adding proofs increases fees
   - Uses a scenario where multiple iterations are needed

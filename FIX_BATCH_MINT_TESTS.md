# Fix Batch Mint Tests

Three tests are failing in `crates/cdk-integration-tests/tests/integration_tests_pure.rs`:
- `test_batch_mint_two_unlocked_quotes` - Error: `TransactionUnbalanced(100, 2, 0)`
- `test_batch_mint_two_locked_quotes` - Error: `NUT20(SignatureMissing)`
- `test_batch_mint_mixed_locked_unlocked` - Error: `TransactionUnbalanced(100, 2, 0)`

## Root Cause

In batch minting, the wallet creates multiple outputs via amount splitting but the server expects exactly 1 output per quote.

**Wallet** (`cdk/src/wallet/issue/batch.rs:177`):
```rust
let batch_request = BatchMintRequest {
    quote: vec![q1, q2],  // 2 quotes
    outputs: premint_secrets.blinded_messages(),  // 4 outputs (split)
};
```

**Server** (`cdk/src/mint/issue/mod.rs:878`):
```rust
for (i, quote_id) in quote_ids.iter().enumerate() {
    // Uses outputs[i] for quote[i]
    // Expects 2 outputs for 2 quotes, gets 4
}
```

## Fix

**Option A (simpler)**: In `cdk/src/wallet/issue/batch.rs`, instead of creating one `PreMintSecrets` for the total amount, create one per quote:

```rust
let mut all_outputs = Vec::new();
let mut all_premints = Vec::new();

for quote_info in &quote_infos {
    let amount = quote_info.amount_mintable();

    // Create PreMintSecrets for THIS quote's amount only
    let premint = PreMintSecrets::from_seed(
        active_keyset_id,
        counter,
        &self.seed,
        amount,  // One quote's amount, not total
        &amount_split_target,
        &fee_and_amounts,
    )?;

    all_outputs.extend(premint.blinded_messages());
    all_premints.push(premint);

    counter += number_of_secrets_for_this_amount;
}

let batch_request = BatchMintRequest {
    quote: quote_ids.clone(),
    outputs: all_outputs,
    signature: batch_signatures,
};
```

Then update signature generation to use the per-quote premints instead of the single total premint.

**Option B (harder)**: Modify server to understand multiple outputs per quote by adding output count metadata to `BatchMintRequest` and updating the handler loop.

## Verify Fix

```bash
CDK_TEST_DB_TYPE=memory cargo test --test integration_tests_pure batch_mint
```

Should pass all 3 tests. Also verify existing tests don't break:

```bash
CDK_TEST_DB_TYPE=memory cargo test --test batch_mint
CDK_TEST_DB_TYPE=memory cargo test --test integration_tests_pure
```

---
name: Mutation Testing Improvement
about: Track improvements to mutation test coverage
title: '[Mutation] '
labels: 'mutation-testing, enhancement'
assignees: ''

---

## Mutation Details

**File:** `crates/cashu/src/...`
**Line:** 123
**Mutation:** replace foo() with bar()

**Current Status:** MISSED

## Why This Matters

<!-- Explain the security/correctness implications -->

## Proposed Fix

<!-- Describe the test(s) needed to catch this mutation -->

### Test Strategy

- [ ] Add negative test case
- [ ] Add edge case test
- [ ] Add integration test
- [ ] Other: ___________

### Expected Test

```rust
#[test]
fn test_() {
    // Test that ensures this mutation would be caught
}
```

## Verification

After implementing the fix:

```bash
# Run mutation test on specific file
cargo mutants --file crates/cashu/src/...

# Or run mutation test on the specific function
cargo mutants --file crates/cashu/src/... --re "function_name"
```

Expected result: Mutation should be **CAUGHT** ✅

## Related

<!-- Link to any related issues or PRs -->

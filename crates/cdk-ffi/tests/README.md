# CDK FFI Python Tests

This directory contains Python tests for the CDK FFI (Foreign Function Interface) bindings, covering both transaction operations and wallet database operations.

## Running the Tests

### Quick Start

The easiest way to run all tests:

```bash
# From the repository root
just ffi-test
```

This command will automatically:
1. Build the FFI bindings (if needed)
2. Run all Python tests
3. Report results

Or run directly (assumes bindings are already built):

```bash
# From the repository root
python3 crates/cdk-ffi/tests/test_transactions.py
```

### Prerequisites

**Python 3.7+** is required. The `just ffi-test` command handles everything else automatically.

## How It Works

The test script automatically:
1. Locates the bindings in `target/bindings/python/`
2. Copies the shared library from `target/release/` to the bindings directory
3. Runs all tests

**No manual file copying required!**

## Test Suite

### Transaction Tests (explicit transaction management)

Tests that use `begin_db_transaction()`, `commit()`, and `rollback()`:

1. **Increment Counter with Commit** - Tests `increment_keyset_counter()` and persistence
2. **Implicit Rollback on Drop** - Verifies automatic rollback when transactions are dropped
3. **Explicit Rollback** - Tests manual `rollback()` calls
4. **Transaction Reads** - Tests reading data within active transactions
5. **Multiple Increments** - Tests sequential counter operations in one transaction
6. **Transaction Atomicity** - Tests that rollback properly reverts ALL changes
7. **Get Proofs by Y Values (Transaction)** - Tests retrieving proofs within transactions

### Wallet Tests (direct wallet methods)

Tests that use wallet database methods directly without explicit transactions:

8. **Wallet Creation** - Tests creating a wallet with SQLite backend
9. **Wallet Mint Management** - Tests adding, querying, and removing mints
10. **Wallet Keyset Management** - Tests adding and querying keysets
11. **Wallet Keyset Counter** - Tests keyset counter increment operations
12. **Wallet Quote Operations** - Tests querying mint and melt quotes
13. **Wallet Get Proofs by Y Values** - Tests retrieving proofs by Y values

### Key Features Tested

**Transaction Features:**
- ✅ **Transaction atomicity** - All-or-nothing commits/rollbacks
- ✅ **Isolation** - Uncommitted changes not visible outside transaction
- ✅ **Durability** - Committed changes persist
- ✅ **Implicit rollback** - Automatic cleanup on transaction drop
- ✅ **Explicit rollback** - Manual transaction rollback

**Wallet Features:**
- ✅ **Wallet creation** - SQLite backend initialization
- ✅ **Mint management** - Add, query, and retrieve mint URLs
- ✅ **Keyset operations** - Add keysets and query by ID or mint
- ✅ **Counter operations** - Keyset counter increment/read
- ✅ **Quote queries** - Retrieve mint and melt quotes
- ✅ **Proof retrieval** - Get proofs by Y values
- ✅ **Foreign key constraints** - Proper referential integrity

## Test Output

Expected output for successful run:

```
Starting CDK FFI Wallet and Transaction Tests
==================================================
... (test execution) ...
==================================================
Test Results: 13 passed, 0 failed
==================================================
```

## Troubleshooting

### Import Errors

If you see `ModuleNotFoundError: No module named 'cdk_ffi'`:
- Ensure FFI bindings are generated: `just ffi-generate python`
- Check that `target/bindings/python/cdk_ffi.py` exists

### Library Not Found

If you see errors about missing `.dylib` or `.so` files:
- Build the release version: `cargo build --release -p cdk-ffi`
- Check that the library exists in `target/release/`

### Test Failures

If tests fail:
- Ensure you're running from the repository root
- Check that the FFI bindings match the current code version
- Try rebuilding: `just ffi-generate python && cargo build --release -p cdk-ffi`

## Development

When adding new tests:

1. Add test function with `async def test_*()` signature
2. Add test to the `tests` list in `main()`
3. Use temporary databases for isolation
4. Follow existing patterns for setup/teardown
5. Clearly indicate if the test uses transactions or direct wallet methods

## Implementation Notes

- All tests use temporary SQLite databases
- Each test is fully isolated with its own database
- Tests clean up automatically via `finally` blocks
- The script handles path resolution and library loading automatically
- Transaction tests demonstrate ACID properties
- Wallet tests demonstrate direct database operations

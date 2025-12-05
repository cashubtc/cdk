#!/usr/bin/env python3
"""
Test suite for CDK FFI wallet and transaction operations
"""

import asyncio
import os
import sys
import tempfile
from pathlib import Path

# Setup paths before importing cdk_ffi
repo_root = Path(__file__).parent.parent.parent.parent
bindings_path = repo_root / "target" / "bindings" / "python"
lib_path = repo_root / "target" / "release"

# Copy the library to the bindings directory so Python can find it
import shutil
lib_file = "libcdk_ffi.dylib" if sys.platform == "darwin" else "libcdk_ffi.so"
src_lib = lib_path / lib_file
dst_lib = bindings_path / lib_file

if src_lib.exists() and not dst_lib.exists():
    shutil.copy2(src_lib, dst_lib)

# Add target/bindings/python to path to load cdk_ffi module
sys.path.insert(0, str(bindings_path))

import cdk_ffi


# Transaction Tests (using explicit transactions)

async def test_increment_keyset_counter_commit():
    """Test that increment_keyset_counter works and persists after commit"""
    print("\n=== Test: Increment Keyset Counter with Commit ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        keyset_id = cdk_ffi.Id(hex="004146bdf4a9afab")
        mint_url = cdk_ffi.MintUrl(url="https://testmint.example.com")
        keyset_info = cdk_ffi.KeySetInfo(
            id=keyset_id.hex,
            unit=cdk_ffi.CurrencyUnit.SAT(),
            active=True,
            input_fee_ppk=0
        )

        # Setup
        tx = await db.begin_db_transaction()
        await tx.add_mint(mint_url, None)
        await tx.add_mint_keysets(mint_url, [keyset_info])
        await tx.commit()

        # Increment counter in transaction
        tx = await db.begin_db_transaction()
        counter1 = await tx.increment_keyset_counter(keyset_id, 1)
        counter2 = await tx.increment_keyset_counter(keyset_id, 5)
        await tx.commit()

        assert counter1 == 1, f"Expected counter 1, got {counter1}"
        assert counter2 == 6, f"Expected counter 6, got {counter2}"
        print("✓ Counters incremented correctly")

        # Verify persistence
        tx_read = await db.begin_db_transaction()
        counter3 = await tx_read.increment_keyset_counter(keyset_id, 0)
        await tx_read.rollback()
        assert counter3 == 6, f"Expected persisted counter 6, got {counter3}"
        print("✓ Counter persisted after commit")

        print("✓ Test passed: Counter increments and commits work")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def test_implicit_rollback_on_drop():
    """Test that transactions are implicitly rolled back when dropped"""
    print("\n=== Test: Implicit Rollback on Drop ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        keyset_id = cdk_ffi.Id(hex="004146bdf4a9afab")
        mint_url = cdk_ffi.MintUrl(url="https://testmint.example.com")

        # Setup
        tx = await db.begin_db_transaction()
        await tx.add_mint(mint_url, None)
        keyset_info = cdk_ffi.KeySetInfo(
            id=keyset_id.hex,
            unit=cdk_ffi.CurrencyUnit.SAT(),
            active=True,
            input_fee_ppk=0
        )
        await tx.add_mint_keysets(mint_url, [keyset_info])
        await tx.commit()

        # Get initial counter
        tx_read = await db.begin_db_transaction()
        initial_counter = await tx_read.increment_keyset_counter(keyset_id, 0)
        await tx_read.rollback()
        print(f"Initial counter: {initial_counter}")

        # Increment without commit
        tx_no_commit = await db.begin_db_transaction()
        incremented = await tx_no_commit.increment_keyset_counter(keyset_id, 10)
        print(f"Counter incremented to {incremented} (not committed)")
        del tx_no_commit

        await asyncio.sleep(0.5)
        print("Transaction dropped (should trigger implicit rollback)")

        # Verify rollback
        tx_verify = await db.begin_db_transaction()
        final_counter = await tx_verify.increment_keyset_counter(keyset_id, 0)
        await tx_verify.rollback()

        assert final_counter == initial_counter, \
            f"Expected counter to rollback to {initial_counter}, got {final_counter}"
        print("✓ Implicit rollback works correctly")

        print("✓ Test passed: Implicit rollback on drop works")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def test_explicit_rollback():
    """Test explicit rollback of transaction changes"""
    print("\n=== Test: Explicit Rollback ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        keyset_id = cdk_ffi.Id(hex="004146bdf4a9afab")
        mint_url = cdk_ffi.MintUrl(url="https://testmint.example.com")

        # Setup
        tx = await db.begin_db_transaction()
        await tx.add_mint(mint_url, None)
        keyset_info = cdk_ffi.KeySetInfo(
            id=keyset_id.hex,
            unit=cdk_ffi.CurrencyUnit.SAT(),
            active=True,
            input_fee_ppk=0
        )
        await tx.add_mint_keysets(mint_url, [keyset_info])
        counter_initial = await tx.increment_keyset_counter(keyset_id, 5)
        await tx.commit()
        print(f"Initial counter: {counter_initial}")

        # Increment and rollback
        tx_rollback = await db.begin_db_transaction()
        counter_incremented = await tx_rollback.increment_keyset_counter(keyset_id, 100)
        print(f"Counter incremented to {counter_incremented} in transaction")
        await tx_rollback.rollback()
        print("Explicitly rolled back transaction")

        # Verify rollback
        tx_verify = await db.begin_db_transaction()
        counter_after = await tx_verify.increment_keyset_counter(keyset_id, 0)
        await tx_verify.rollback()

        assert counter_after == counter_initial, \
            f"Expected counter {counter_initial}, got {counter_after}"
        print("✓ Explicit rollback works correctly")

        print("✓ Test passed: Explicit rollback works")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def test_transaction_reads():
    """Test reading data within transactions"""
    print("\n=== Test: Transaction Reads ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        keyset_id = cdk_ffi.Id(hex="004146bdf4a9afab")
        mint_url = cdk_ffi.MintUrl(url="https://testmint.example.com")

        # Add keyset in transaction and read within same transaction
        tx = await db.begin_db_transaction()
        await tx.add_mint(mint_url, None)
        keyset_info = cdk_ffi.KeySetInfo(
            id=keyset_id.hex,
            unit=cdk_ffi.CurrencyUnit.SAT(),
            active=True,
            input_fee_ppk=0
        )
        await tx.add_mint_keysets(mint_url, [keyset_info])

        keyset_read = await tx.get_keyset_by_id(keyset_id)
        assert keyset_read is not None, "Should read within transaction"
        assert keyset_read.id == keyset_id.hex, "Keyset ID should match"
        print("✓ Read keyset within transaction")

        await tx.commit()

        # Read from new transaction
        tx_new = await db.begin_db_transaction()
        keyset_read2 = await tx_new.get_keyset_by_id(keyset_id)
        assert keyset_read2 is not None, "Should read committed keyset"
        await tx_new.rollback()
        print("✓ Read keyset in new transaction")

        print("✓ Test passed: Transaction reads work")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def test_multiple_increments_same_transaction():
    """Test multiple increments in same transaction"""
    print("\n=== Test: Multiple Increments in Same Transaction ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        keyset_id = cdk_ffi.Id(hex="004146bdf4a9afab")
        mint_url = cdk_ffi.MintUrl(url="https://testmint.example.com")

        # Setup
        tx = await db.begin_db_transaction()
        await tx.add_mint(mint_url, None)
        keyset_info = cdk_ffi.KeySetInfo(
            id=keyset_id.hex,
            unit=cdk_ffi.CurrencyUnit.SAT(),
            active=True,
            input_fee_ppk=0
        )
        await tx.add_mint_keysets(mint_url, [keyset_info])
        await tx.commit()

        # Multiple increments in one transaction
        tx = await db.begin_db_transaction()
        counters = []
        for i in range(1, 6):
            counter = await tx.increment_keyset_counter(keyset_id, 1)
            counters.append(counter)

        expected = list(range(1, 6))
        assert counters == expected, f"Expected {expected}, got {counters}"
        print(f"✓ Counters incremented: {counters}")

        await tx.commit()

        # Verify final value
        tx_verify = await db.begin_db_transaction()
        final = await tx_verify.increment_keyset_counter(keyset_id, 0)
        await tx_verify.rollback()
        assert final == 5, f"Expected final counter 5, got {final}"
        print("✓ Final counter value correct")

        print("✓ Test passed: Multiple increments work")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def test_transaction_atomicity():
    """Test that transaction rollback reverts all changes"""
    print("\n=== Test: Transaction Atomicity ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        mint_url1 = cdk_ffi.MintUrl(url="https://mint1.example.com")
        mint_url2 = cdk_ffi.MintUrl(url="https://mint2.example.com")
        keyset_id = cdk_ffi.Id(hex="004146bdf4a9afab")

        # Transaction with multiple operations
        tx = await db.begin_db_transaction()
        await tx.add_mint(mint_url1, None)
        await tx.add_mint(mint_url2, None)
        keyset_info = cdk_ffi.KeySetInfo(
            id=keyset_id.hex,
            unit=cdk_ffi.CurrencyUnit.SAT(),
            active=True,
            input_fee_ppk=0
        )
        await tx.add_mint_keysets(mint_url1, [keyset_info])
        await tx.increment_keyset_counter(keyset_id, 42)
        print("✓ Performed multiple operations")

        # Rollback
        await tx.rollback()
        print("✓ Rolled back transaction")

        # Verify nothing persisted
        tx_read = await db.begin_db_transaction()
        keyset_read = await tx_read.get_keyset_by_id(keyset_id)
        await tx_read.rollback()
        assert keyset_read is None, "Keyset should not exist after rollback"
        print("✓ Nothing persisted after rollback")

        # Now commit
        tx2 = await db.begin_db_transaction()
        await tx2.add_mint(mint_url1, None)
        await tx2.add_mint(mint_url2, None)
        await tx2.add_mint_keysets(mint_url1, [keyset_info])
        await tx2.increment_keyset_counter(keyset_id, 42)
        await tx2.commit()
        print("✓ Committed transaction")

        # Verify persistence
        tx_verify = await db.begin_db_transaction()
        keyset_after = await tx_verify.get_keyset_by_id(keyset_id)
        assert keyset_after is not None, "Keyset should exist after commit"
        counter_after = await tx_verify.increment_keyset_counter(keyset_id, 0)
        await tx_verify.rollback()
        assert counter_after == 42, f"Expected counter 42, got {counter_after}"
        print("✓ All operations persisted after commit")

        print("✓ Test passed: Transaction atomicity works")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)




# Wallet Tests (using direct wallet methods without explicit transactions)

async def test_wallet_creation():
    """Test creating a wallet with SQLite backend"""
    print("\n=== Test: Wallet Creation ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)
        print("✓ Wallet database created")

        # Verify database is accessible
        mint_quotes = await db.get_mint_quotes()
        assert isinstance(mint_quotes, list), "get_mint_quotes should return a list"
        print("✓ Wallet database accessible")

        print("✓ Test passed: Wallet creation works")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def test_wallet_mint_management():
    """Test adding and querying mints"""
    print("\n=== Test: Wallet Mint Management ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        mint_url = cdk_ffi.MintUrl(url="https://testmint.example.com")

        # Add mint (using transaction)
        tx = await db.begin_db_transaction()
        await tx.add_mint(mint_url, None)
        await tx.commit()
        print("✓ Added mint to wallet")

        # Get specific mint (read-only, can use db directly)
        await db.get_mint(mint_url)
        print("✓ Retrieved mint from database")

        # Remove mint (using transaction)
        tx = await db.begin_db_transaction()
        await tx.remove_mint(mint_url)
        await tx.commit()
        print("✓ Removed mint from wallet")

        # Verify removal
        mint_info_after = await db.get_mint(mint_url)
        assert mint_info_after is None, "Mint should be removed"
        print("✓ Verified mint removal")

        print("✓ Test passed: Mint management works")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def test_wallet_keyset_management():
    """Test adding and querying keysets"""
    print("\n=== Test: Wallet Keyset Management ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        mint_url = cdk_ffi.MintUrl(url="https://testmint.example.com")
        keyset_id = cdk_ffi.Id(hex="004146bdf4a9afab")

        # Add mint and keyset (using transaction)
        tx = await db.begin_db_transaction()
        await tx.add_mint(mint_url, None)
        keyset_info = cdk_ffi.KeySetInfo(
            id=keyset_id.hex,
            unit=cdk_ffi.CurrencyUnit.SAT(),
            active=True,
            input_fee_ppk=0
        )
        await tx.add_mint_keysets(mint_url, [keyset_info])
        await tx.commit()
        print("✓ Added mint and keyset")

        # Query keyset by ID (read-only)
        keyset = await db.get_keyset_by_id(keyset_id)
        assert keyset is not None, "Keyset should exist"
        assert keyset.id == keyset_id.hex, "Keyset ID should match"
        print(f"✓ Retrieved keyset: {keyset.id}")

        # Query keysets for mint (read-only)
        keysets = await db.get_mint_keysets(mint_url)
        assert keysets is not None and len(keysets) > 0, "Should have keysets for mint"
        print(f"✓ Retrieved {len(keysets)} keyset(s) for mint")

        print("✓ Test passed: Keyset management works")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def test_wallet_keyset_counter():
    """Test keyset counter operations"""
    print("\n=== Test: Wallet Keyset Counter ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        mint_url = cdk_ffi.MintUrl(url="https://testmint.example.com")
        keyset_id = cdk_ffi.Id(hex="004146bdf4a9afab")

        # Setup (using transaction)
        tx = await db.begin_db_transaction()
        await tx.add_mint(mint_url, None)
        keyset_info = cdk_ffi.KeySetInfo(
            id=keyset_id.hex,
            unit=cdk_ffi.CurrencyUnit.SAT(),
            active=True,
            input_fee_ppk=0
        )
        await tx.add_mint_keysets(mint_url, [keyset_info])
        await tx.commit()
        print("✓ Setup complete")

        # Increment counter (using transaction)
        tx = await db.begin_db_transaction()
        counter1 = await tx.increment_keyset_counter(keyset_id, 1)
        counter2 = await tx.increment_keyset_counter(keyset_id, 5)
        counter3 = await tx.increment_keyset_counter(keyset_id, 0)
        await tx.commit()

        print(f"✓ Counter after +1: {counter1}")
        assert counter1 == 1, f"Expected counter 1, got {counter1}"
        print(f"✓ Counter after +5: {counter2}")
        assert counter2 == 6, f"Expected counter 6, got {counter2}"
        print(f"✓ Current counter: {counter3}")
        assert counter3 == 6, f"Expected counter 6, got {counter3}"

        print("✓ Test passed: Keyset counter works")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def test_wallet_quotes():
    """Test mint and melt quote operations"""
    print("\n=== Test: Wallet Quote Operations ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        mint_url = cdk_ffi.MintUrl(url="https://testmint.example.com")

        # Add mint (using transaction)
        tx = await db.begin_db_transaction()
        await tx.add_mint(mint_url, None)
        await tx.commit()
        print("✓ Added mint")

        # Query quotes (read-only)
        mint_quotes = await db.get_mint_quotes()
        assert isinstance(mint_quotes, list), "get_mint_quotes should return a list"
        print(f"✓ Retrieved {len(mint_quotes)} mint quote(s)")

        melt_quotes = await db.get_melt_quotes()
        assert isinstance(melt_quotes, list), "get_melt_quotes should return a list"
        print(f"✓ Retrieved {len(melt_quotes)} melt quote(s)")

        print("✓ Test passed: Quote operations work")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def test_wallet_proofs_by_ys():
    """Test retrieving proofs by Y values"""
    print("\n=== Test: Wallet Get Proofs by Y Values ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        # Test with empty list
        proofs = await db.get_proofs_by_ys([])
        assert len(proofs) == 0, f"Expected 0 proofs, got {len(proofs)}"
        print("✓ get_proofs_by_ys returns empty for empty input")

        print("✓ Test passed: get_proofs_by_ys works")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def main():
    """Run all tests"""
    print("Starting CDK FFI Wallet and Transaction Tests")
    print("=" * 50)

    tests = [
        # Transaction tests
        ("Increment Counter with Commit", test_increment_keyset_counter_commit),
        ("Implicit Rollback on Drop", test_implicit_rollback_on_drop),
        ("Explicit Rollback", test_explicit_rollback),
        ("Transaction Reads", test_transaction_reads),
        ("Multiple Increments", test_multiple_increments_same_transaction),
        ("Transaction Atomicity", test_transaction_atomicity),
        # Wallet tests (read methods + write via transactions)
        ("Wallet Creation", test_wallet_creation),
        ("Wallet Mint Management", test_wallet_mint_management),
        ("Wallet Keyset Management", test_wallet_keyset_management),
        ("Wallet Keyset Counter", test_wallet_keyset_counter),
        ("Wallet Quote Operations", test_wallet_quotes),
        ("Wallet Get Proofs by Y Values", test_wallet_proofs_by_ys),
    ]

    passed = 0
    failed = 0

    for test_name, test_func in tests:
        try:
            await test_func()
            passed += 1
        except Exception as e:
            failed += 1
            print(f"\n✗ Test failed: {test_name}")
            print(f"Error: {e}")
            import traceback
            traceback.print_exc()

    print("\n" + "=" * 50)
    print(f"Test Results: {passed} passed, {failed} failed")
    print("=" * 50)

    return 0 if failed == 0 else 1


if __name__ == "__main__":
    exit_code = asyncio.run(main())
    sys.exit(exit_code)

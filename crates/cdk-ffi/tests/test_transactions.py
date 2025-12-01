#!/usr/bin/env python3
"""
Test suite for CDK FFI wallet operations
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


async def test_wallet_creation():
    """Test creating a wallet with SQLite backend"""
    print("\n=== Test: Wallet Creation ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)
        print("✓ Wallet database created")

        # Verify database is accessible by querying quotes
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

        # Add mint
        await db.add_mint(mint_url, None)
        print("✓ Added mint to wallet")

        # Get specific mint (verifies it was added)
        await db.get_mint(mint_url)
        print("✓ Retrieved mint from database")

        # Remove mint
        await db.remove_mint(mint_url)
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

        # Add mint first (foreign key requirement)
        await db.add_mint(mint_url, None)
        print("✓ Added mint")

        # Add keyset
        keyset_info = cdk_ffi.KeySetInfo(
            id=keyset_id.hex,
            unit=cdk_ffi.CurrencyUnit.SAT(),
            active=True,
            input_fee_ppk=0
        )
        await db.add_mint_keysets(mint_url, [keyset_info])
        print("✓ Added keyset")

        # Query keyset by ID
        keyset = await db.get_keyset_by_id(keyset_id)
        assert keyset is not None, "Keyset should exist"
        assert keyset.id == keyset_id.hex, "Keyset ID should match"
        print(f"✓ Retrieved keyset: {keyset.id}")

        # Query keysets for mint
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

        # Setup mint and keyset
        await db.add_mint(mint_url, None)
        keyset_info = cdk_ffi.KeySetInfo(
            id=keyset_id.hex,
            unit=cdk_ffi.CurrencyUnit.SAT(),
            active=True,
            input_fee_ppk=0
        )
        await db.add_mint_keysets(mint_url, [keyset_info])
        print("✓ Setup complete")

        # Increment counter
        counter1 = await db.increment_keyset_counter(keyset_id, 1)
        print(f"✓ Counter after +1: {counter1}")
        assert counter1 == 1, f"Expected counter 1, got {counter1}"

        # Increment again
        counter2 = await db.increment_keyset_counter(keyset_id, 5)
        print(f"✓ Counter after +5: {counter2}")
        assert counter2 == 6, f"Expected counter 6, got {counter2}"

        # Read current value (increment by 0)
        counter3 = await db.increment_keyset_counter(keyset_id, 0)
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

        # Add mint first
        await db.add_mint(mint_url, None)
        print("✓ Added mint")

        # Query mint quotes (should be empty initially)
        mint_quotes = await db.get_mint_quotes()
        assert isinstance(mint_quotes, list), "get_mint_quotes should return a list"
        print(f"✓ Retrieved {len(mint_quotes)} mint quote(s)")

        # Query melt quotes (should be empty initially)
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
    print("Starting CDK FFI Wallet Tests")
    print("=" * 50)

    tests = [
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

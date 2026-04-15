#!/usr/bin/env python3
"""
Test suite for CDK FFI wallet database operations
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

if src_lib.exists():
    shutil.copy2(src_lib, dst_lib)

# Add target/bindings/python to path to load cdk_ffi module
sys.path.insert(0, str(bindings_path))

import cdk_ffi


# Wallet Database Tests

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

        # Add mint
        await db.add_mint(mint_url, None)
        print("✓ Added mint to wallet")

        # Get specific mint
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

        # Add mint and keyset
        await db.add_mint(mint_url, None)
        keyset_info = cdk_ffi.KeySetInfo(
            id=keyset_id.hex,
            unit=cdk_ffi.CurrencyUnit.SAT(),
            active=True,
            input_fee_ppk=0
        )
        await db.add_mint_keysets(mint_url, [keyset_info])
        print("✓ Added mint and keyset")

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

        # Setup
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
        counter2 = await db.increment_keyset_counter(keyset_id, 5)
        counter3 = await db.increment_keyset_counter(keyset_id, 0)

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

        # Add mint
        await db.add_mint(mint_url, None)
        print("✓ Added mint")

        # Query quotes
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


async def test_wallet_proofs_by_ys_empty_errors():
    """Test that get_proofs_by_ys errors with empty list"""
    print("\n=== Test: Wallet Get Proofs by Y Values (Empty Errors) ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        try:
            await db.get_proofs_by_ys([])
            assert False, "Expected error for empty ys but got success"
        except Exception as e:
            assert "Empty IN clause" in str(e), f"Expected EmptyInClause error, got: {e}"
            print("✓ get_proofs_by_ys errors on empty input")

        print("✓ Test passed")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def test_wallet_proofs_by_ys():
    """Test retrieving proofs by Y values from the database"""
    print("\n=== Test: Wallet Get Proofs by Y Values ===")

    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        db_path = tmp.name

    try:
        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db = cdk_ffi.create_wallet_db(backend)

        # Build test proofs using JSON decode helpers
        import json
        import hashlib
        import secrets as secrets_mod

        mint_url = "https://example.com"
        keyset_id = "00deadbeef123456"

        proof_infos = []
        expected_ys = []

        for i in range(3):
            # Generate a random secret string (matching cashu secret format)
            random_hex = secrets_mod.token_hex(32)
            secret_str = json.dumps(["P2PK", {"nonce": random_hex, "data": random_hex}])

            # Use a valid secp256k1 generator point as C (just needs to be a valid pubkey)
            # secp256k1 generator point G
            c_hex = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"

            proof = cdk_ffi.Proof(
                amount=cdk_ffi.Amount(value=64),
                secret=secret_str,
                c=c_hex,
                keyset_id=keyset_id,
                witness=None,
                dleq=None,
                p2pk_e=None,
            )

            # Calculate Y from the proof
            y_hex = cdk_ffi.proof_y(proof)
            y = cdk_ffi.PublicKey(hex=y_hex)

            proof_info = cdk_ffi.ProofInfo(
                proof=proof,
                y=y,
                mint_url=cdk_ffi.MintUrl(url=mint_url),
                state=cdk_ffi.ProofState.UNSPENT,
                spending_condition=None,
                unit=cdk_ffi.CurrencyUnit.SAT(),
                used_by_operation=None,
                created_by_operation=None,
            )
            proof_infos.append(proof_info)
            expected_ys.append(y)

        # Store proofs
        await db.update_proofs(proof_infos, [])
        print("✓ Stored 3 proofs")

        # Retrieve all by Y values
        retrieved = await db.get_proofs_by_ys(expected_ys)
        assert len(retrieved) == 3, f"Expected 3 proofs, got {len(retrieved)}"
        print("✓ Retrieved all 3 proofs by Y values")

        # Retrieve subset
        subset = await db.get_proofs_by_ys([expected_ys[0]])
        assert len(subset) == 1, f"Expected 1 proof, got {len(subset)}"
        assert subset[0].y.hex == expected_ys[0].hex
        print("✓ Retrieved single proof by Y value")

        print("✓ Test passed")

    finally:
        if os.path.exists(db_path):
            os.unlink(db_path)


async def main():
    """Run all tests"""
    print("Starting CDK FFI Wallet Database Tests")
    print("=" * 50)

    tests = [
        ("Wallet Creation", test_wallet_creation),
        ("Wallet Mint Management", test_wallet_mint_management),
        ("Wallet Keyset Management", test_wallet_keyset_management),
        ("Wallet Keyset Counter", test_wallet_keyset_counter),
        ("Wallet Quote Operations", test_wallet_quotes),
        ("Wallet Get Proofs by Y Values (Empty Errors)", test_wallet_proofs_by_ys_empty_errors),
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

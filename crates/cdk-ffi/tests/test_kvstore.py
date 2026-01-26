#!/usr/bin/env python3
"""
Test suite for CDK FFI Key-Value Store operations

Tests the KVStore trait functionality exposed through the FFI bindings,
including read, write, list, and remove operations.
"""

import asyncio
import os
import sys
import tempfile
import shutil
from pathlib import Path

# Setup paths before importing cdk_ffi
repo_root = Path(__file__).parent.parent.parent.parent
bindings_path = repo_root / "target" / "bindings" / "python"
lib_path = repo_root / "target" / "release"

lib_file = "libcdk_ffi.dylib" if sys.platform == "darwin" else "libcdk_ffi.so"
src_lib = lib_path / lib_file
dst_lib = bindings_path / lib_file

if src_lib.exists() and not dst_lib.exists():
    shutil.copy2(src_lib, dst_lib)

# Add target/bindings/python to path to load cdk_ffi module
sys.path.insert(0, str(bindings_path))

import cdk_ffi

# Helper functions

def create_test_db():
    """Create a temporary SQLite database for testing"""
    tmp = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
    db_path = tmp.name
    tmp.close()
    backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
    db = cdk_ffi.create_wallet_db(backend)
    return db, db_path


def cleanup_db(db_path):
    """Clean up the temporary database file"""
    if os.path.exists(db_path):
        os.unlink(db_path)


# Basic KV Store Tests

async def test_kv_write_and_read():
    """Test basic write and read operations"""
    print("\n=== Test: KV Write and Read ===")

    db, db_path = create_test_db()

    try:
        # Write a value
        test_data = b"Hello, KVStore!"
        await db.kv_write("app", "config", "greeting", test_data)
        print("  Written value to KV store")

        # Read it back
        result = await db.kv_read("app", "config", "greeting")

        assert result is not None, "Expected to read back the value"
        assert bytes(result) == test_data, f"Expected {test_data}, got {bytes(result)}"
        print("  Read back correct value")

        print("  Test passed: KV write and read work")

    finally:
        cleanup_db(db_path)


async def test_kv_read_nonexistent():
    """Test reading a key that doesn't exist"""
    print("\n=== Test: KV Read Nonexistent Key ===")

    db, db_path = create_test_db()

    try:
        result = await db.kv_read("nonexistent", "namespace", "key")

        assert result is None, f"Expected None for nonexistent key, got {result}"
        print("  Correctly returns None for nonexistent key")

        print("  Test passed: Reading nonexistent key returns None")

    finally:
        cleanup_db(db_path)


async def test_kv_overwrite():
    """Test overwriting an existing value"""
    print("\n=== Test: KV Overwrite ===")

    db, db_path = create_test_db()

    try:
        # Write initial value
        await db.kv_write("app", "data", "counter", b"1")
        print("  Written initial value")

        # Overwrite with new value
        await db.kv_write("app", "data", "counter", b"42")
        print("  Overwrote with new value")

        # Read back
        result = await db.kv_read("app", "data", "counter")

        assert result is not None, "Expected to read back the value"
        assert bytes(result) == b"42", f"Expected b'42', got {bytes(result)}"
        print("  Read back overwritten value")

        print("  Test passed: KV overwrite works")

    finally:
        cleanup_db(db_path)


async def test_kv_remove():
    """Test removing a key"""
    print("\n=== Test: KV Remove ===")

    db, db_path = create_test_db()

    try:
        # Write a value
        await db.kv_write("app", "temp", "to_delete", b"delete me")
        print("  Written value to delete")

        # Verify it exists
        result = await db.kv_read("app", "temp", "to_delete")
        assert result is not None, "Value should exist before removal"
        print("  Verified value exists")

        # Remove it
        await db.kv_remove("app", "temp", "to_delete")
        print("  Removed value")

        # Verify it's gone
        result_after = await db.kv_read("app", "temp", "to_delete")

        assert result_after is None, f"Expected None after removal, got {result_after}"
        print("  Verified value is removed")

        print("  Test passed: KV remove works")

    finally:
        cleanup_db(db_path)


async def test_kv_list_keys():
    """Test listing keys in a namespace"""
    print("\n=== Test: KV List Keys ===")

    db, db_path = create_test_db()

    try:
        # Write multiple keys
        await db.kv_write("myapp", "settings", "theme", b"dark")
        await db.kv_write("myapp", "settings", "language", b"en")
        await db.kv_write("myapp", "settings", "timezone", b"UTC")
        await db.kv_write("myapp", "other", "unrelated", b"data")
        print("  Written multiple keys")

        # List keys in the settings namespace
        keys = await db.kv_list("myapp", "settings")

        assert len(keys) == 3, f"Expected 3 keys, got {len(keys)}"
        assert "theme" in keys, "Expected 'theme' in keys"
        assert "language" in keys, "Expected 'language' in keys"
        assert "timezone" in keys, "Expected 'timezone' in keys"
        assert "unrelated" not in keys, "'unrelated' should not be in settings namespace"
        print(f"  Listed keys: {keys}")

        print("  Test passed: KV list works")

    finally:
        cleanup_db(db_path)


async def test_kv_list_empty_namespace():
    """Test listing keys in an empty or nonexistent namespace"""
    print("\n=== Test: KV List Empty Namespace ===")

    db, db_path = create_test_db()

    try:
        keys = await db.kv_list("nonexistent", "namespace")

        assert isinstance(keys, list), "Expected a list"
        assert len(keys) == 0, f"Expected empty list, got {keys}"
        print("  Empty namespace returns empty list")

        print("  Test passed: KV list on empty namespace works")

    finally:
        cleanup_db(db_path)


# Namespace Isolation Tests

async def test_kv_namespace_isolation():
    """Test that different namespaces are isolated"""
    print("\n=== Test: KV Namespace Isolation ===")

    db, db_path = create_test_db()

    try:
        # Write same key in different namespaces
        await db.kv_write("app1", "config", "key", b"app1_value")
        await db.kv_write("app2", "config", "key", b"app2_value")
        await db.kv_write("app1", "other", "key", b"app1_other_value")
        print("  Written same key in different namespaces")

        # Read from each namespace
        result1 = await db.kv_read("app1", "config", "key")
        result2 = await db.kv_read("app2", "config", "key")
        result3 = await db.kv_read("app1", "other", "key")

        assert bytes(result1) == b"app1_value", f"Expected b'app1_value', got {bytes(result1)}"
        assert bytes(result2) == b"app2_value", f"Expected b'app2_value', got {bytes(result2)}"
        assert bytes(result3) == b"app1_other_value", f"Expected b'app1_other_value', got {bytes(result3)}"
        print("  Each namespace has correct value")

        print("  Test passed: KV namespace isolation works")

    finally:
        cleanup_db(db_path)


# Binary Data Tests

async def test_kv_binary_data():
    """Test storing and retrieving binary data"""
    print("\n=== Test: KV Binary Data ===")

    db, db_path = create_test_db()

    try:
        # Various binary data types
        test_cases = [
            ("empty", b""),
            ("null_byte", b"\x00"),
            ("all_bytes", bytes(range(256))),
            ("utf8_special", "Hello World".encode("utf-8")),
            ("random_binary", bytes([0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE])),
        ]

        for name, data in test_cases:
            await db.kv_write("binary", "test", name, data)
        print(f"  Written {len(test_cases)} binary test cases")

        # Read back and verify
        for name, expected_data in test_cases:
            result = await db.kv_read("binary", "test", name)
            assert result is not None, f"Expected data for {name}"
            actual_data = bytes(result)
            assert actual_data == expected_data, f"Mismatch for {name}: expected {expected_data!r}, got {actual_data!r}"
            print(f"    '{name}': OK ({len(actual_data)} bytes)")

        print("  Test passed: KV binary data works")

    finally:
        cleanup_db(db_path)


async def test_kv_large_value():
    """Test storing a large value"""
    print("\n=== Test: KV Large Value ===")

    db, db_path = create_test_db()

    try:
        # Create a 1MB value
        large_data = bytes([i % 256 for i in range(1024 * 1024)])

        await db.kv_write("large", "data", "megabyte", large_data)
        print(f"  Written {len(large_data)} bytes")

        # Read back
        result = await db.kv_read("large", "data", "megabyte")

        assert result is not None, "Expected to read large value"
        result_bytes = bytes(result)
        assert len(result_bytes) == len(large_data), f"Size mismatch: {len(result_bytes)} vs {len(large_data)}"
        assert result_bytes == large_data, "Data mismatch"
        print(f"  Read back {len(result_bytes)} bytes correctly")

        print("  Test passed: KV large value works")

    finally:
        cleanup_db(db_path)


# Key Name Tests

async def test_kv_special_key_names():
    """Test keys with special characters"""
    print("\n=== Test: KV Special Key Names ===")

    db, db_path = create_test_db()

    try:
        special_keys = [
            "simple",
            "with-dashes",
            "with_underscores",
            "MixedCase",
            "numbers123",
            "unicode_",  # Note: Using underscore instead of actual unicode for simplicity
            "empty_value",
        ]

        for i, key in enumerate(special_keys):
            await db.kv_write("special", "keys", key, f"value_{i}".encode())
        print(f"  Written {len(special_keys)} special keys")

        # List and verify
        keys = await db.kv_list("special", "keys")

        assert len(keys) == len(special_keys), f"Expected {len(special_keys)} keys, got {len(keys)}"
        for key in special_keys:
            assert key in keys, f"Key '{key}' not found in list"
        print(f"  All special keys stored and listed correctly")

        print("  Test passed: KV special key names work")

    finally:
        cleanup_db(db_path)


# Persistence Test

async def test_kv_persistence_across_instances():
    """Test that KV data persists when reopening the database"""
    print("\n=== Test: KV Persistence Across Instances ===")

    db_path = None
    try:
        # Create and write
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
            db_path = tmp.name

        backend = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db1 = cdk_ffi.create_wallet_db(backend)

        await db1.kv_write("persist", "test", "mykey", b"persistent_value")
        print("  Written and committed with first db instance")

        # Delete reference to first db (simulating closing)
        del db1
        await asyncio.sleep(0.1)
        print("  First db instance closed")

        # Reopen and read
        backend2 = cdk_ffi.WalletDbBackend.SQLITE(path=db_path)
        db2 = cdk_ffi.create_wallet_db(backend2)

        result = await db2.kv_read("persist", "test", "mykey")

        assert result is not None, "Data should persist across db instances"
        assert bytes(result) == b"persistent_value", f"Expected b'persistent_value', got {bytes(result)}"
        print("  Data persisted across db instances")

        print("  Test passed: KV persistence across instances works")

    finally:
        if db_path and os.path.exists(db_path):
            os.unlink(db_path)


async def main():
    """Run all KV store tests"""
    print("Starting CDK FFI Key-Value Store Tests")
    print("=" * 60)

    tests = [
        # Basic operations
        ("KV Write and Read", test_kv_write_and_read),
        ("KV Read Nonexistent", test_kv_read_nonexistent),
        ("KV Overwrite", test_kv_overwrite),
        ("KV Remove", test_kv_remove),
        ("KV List Keys", test_kv_list_keys),
        ("KV List Empty Namespace", test_kv_list_empty_namespace),
        # Namespace tests
        ("KV Namespace Isolation", test_kv_namespace_isolation),
        # Data tests
        ("KV Binary Data", test_kv_binary_data),
        ("KV Large Value", test_kv_large_value),
        ("KV Special Key Names", test_kv_special_key_names),
        # Persistence
        ("KV Persistence Across Instances", test_kv_persistence_across_instances),
    ]

    passed = 0
    failed = 0

    for test_name, test_func in tests:
        try:
            await test_func()
            passed += 1
        except Exception as e:
            failed += 1
            print(f"\n  Test failed: {test_name}")
            print(f"  Error: {e}")
            import traceback
            traceback.print_exc()

    print("\n" + "=" * 60)
    print(f"Test Results: {passed} passed, {failed} failed")
    print("=" * 60)

    return 0 if failed == 0 else 1


if __name__ == "__main__":
    exit_code = asyncio.run(main())
    sys.exit(exit_code)

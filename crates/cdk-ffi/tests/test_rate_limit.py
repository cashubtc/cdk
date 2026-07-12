#!/usr/bin/env python3
"""
Test suite for CDK FFI client-side rate-limit configuration

Covers the default, disabled, custom and invalid configurations on both a
directly constructed wallet and a repository-provided one. Every test is
offline: building a wallet or a repository makes no network request.
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

MINT_URL = "https://mint.example.com"


# Helper functions

def make_wallet(config):
    """Build a wallet backed by a fresh in-memory SQLite store"""
    return cdk_ffi.Wallet(
        mint_url=MINT_URL,
        unit=cdk_ffi.CurrencyUnit.SAT(),
        mnemonic=cdk_ffi.generate_mnemonic(),
        store=cdk_ffi.sqlite_wallet_store(":memory:"),
        config=config,
    )


def make_repository(config):
    """Build a wallet repository backed by a fresh in-memory SQLite store"""
    return cdk_ffi.WalletRepository.new_with_config(
        mnemonic=cdk_ffi.generate_mnemonic(),
        store=cdk_ffi.sqlite_wallet_store(":memory:"),
        config=config,
    )


# Wallet tests

async def test_wallet_default_rate_limit():
    """Omitting the field and asking for DEFAULT both pace the wallet"""
    # Omitting rate_limit entirely is the backwards-compatible spelling: it must
    # keep working and must select the built-in default.
    omitted = make_wallet(cdk_ffi.WalletConfig(target_proof_count=None))
    assert omitted.is_rate_limited(), "an omitted rate limit paces by default"

    explicit = make_wallet(
        cdk_ffi.WalletConfig(
            target_proof_count=None,
            rate_limit=cdk_ffi.RateLimit.DEFAULT(),
        )
    )
    assert explicit.is_rate_limited(), "DEFAULT paces the wallet"


async def test_wallet_disabled_rate_limit():
    """DISABLED turns pacing off and can be turned back on"""
    wallet = make_wallet(
        cdk_ffi.WalletConfig(
            target_proof_count=None,
            rate_limit=cdk_ffi.RateLimit.DISABLED(),
        )
    )
    assert not wallet.is_rate_limited(), "DISABLED does not pace"

    wallet.set_rate_limit(cdk_ffi.RateLimit.DEFAULT())
    assert wallet.is_rate_limited(), "a wallet built disabled can be re-enabled"


async def test_wallet_custom_rate_limit():
    """A non-zero CUSTOM burst and refill paces the wallet"""
    wallet = make_wallet(
        cdk_ffi.WalletConfig(
            target_proof_count=None,
            rate_limit=cdk_ffi.RateLimit.CUSTOM(capacity=5, refill_per_minute=30),
        )
    )
    assert wallet.is_rate_limited(), "CUSTOM paces the wallet"


async def test_wallet_invalid_rate_limit():
    """A zero in either CUSTOM field is rejected at construction"""
    for capacity, refill in [(0, 30), (5, 0), (0, 0)]:
        try:
            make_wallet(
                cdk_ffi.WalletConfig(
                    target_proof_count=None,
                    rate_limit=cdk_ffi.RateLimit.CUSTOM(
                        capacity=capacity,
                        refill_per_minute=refill,
                    ),
                )
            )
        except Exception:
            continue
        raise AssertionError(
            f"CUSTOM(capacity={capacity}, refill_per_minute={refill}) should be rejected"
        )


async def test_wallet_set_rate_limit_rejects_zero():
    """The runtime setter rejects the same zero values"""
    wallet = make_wallet(cdk_ffi.WalletConfig(target_proof_count=None))

    try:
        wallet.set_rate_limit(cdk_ffi.RateLimit.CUSTOM(capacity=0, refill_per_minute=30))
    except Exception:
        pass
    else:
        raise AssertionError("a zero capacity should be rejected")

    assert wallet.is_rate_limited(), "a rejected change leaves pacing untouched"


async def test_wallet_flush_rate_limits():
    """Flushing an untouched wallet completes"""
    wallet = make_wallet(cdk_ffi.WalletConfig(target_proof_count=None))
    await wallet.flush_rate_limits()


# Repository tests

async def test_repository_default_rate_limit():
    """Omitting the field and asking for DEFAULT both pace the repository"""
    omitted = make_repository(cdk_ffi.WalletRepositoryConfig())
    assert omitted.is_rate_limited(), "an omitted rate limit paces by default"

    explicit = make_repository(
        cdk_ffi.WalletRepositoryConfig(rate_limit=cdk_ffi.RateLimit.DEFAULT())
    )
    assert explicit.is_rate_limited(), "DEFAULT paces the repository"


async def test_repository_disabled_rate_limit():
    """DISABLED turns repository pacing off, and it reaches its wallets"""
    repo = make_repository(
        cdk_ffi.WalletRepositoryConfig(rate_limit=cdk_ffi.RateLimit.DISABLED())
    )
    assert not repo.is_rate_limited(), "DISABLED does not pace"

    wallet = await repo.get_or_create_wallet(
        cdk_ffi.MintUrl(url=MINT_URL), cdk_ffi.CurrencyUnit.SAT(), None
    )
    assert not wallet.is_rate_limited(), "a wallet inherits the repository setting"

    repo.set_rate_limit(cdk_ffi.RateLimit.DEFAULT())
    assert wallet.is_rate_limited(), "wallets share the repository limiter"


async def test_repository_custom_rate_limit():
    """A non-zero CUSTOM burst and refill paces the repository"""
    repo = make_repository(
        cdk_ffi.WalletRepositoryConfig(
            rate_limit=cdk_ffi.RateLimit.CUSTOM(capacity=5, refill_per_minute=30)
        )
    )
    assert repo.is_rate_limited(), "CUSTOM paces the repository"


async def test_repository_invalid_rate_limit():
    """A zero in either CUSTOM field is rejected, at construction and at runtime"""
    try:
        make_repository(
            cdk_ffi.WalletRepositoryConfig(
                rate_limit=cdk_ffi.RateLimit.CUSTOM(capacity=0, refill_per_minute=30)
            )
        )
    except Exception:
        pass
    else:
        raise AssertionError("a zero capacity should be rejected at construction")

    repo = make_repository(cdk_ffi.WalletRepositoryConfig())
    try:
        repo.set_rate_limit(cdk_ffi.RateLimit.CUSTOM(capacity=5, refill_per_minute=0))
    except Exception:
        pass
    else:
        raise AssertionError("a zero refill should be rejected at runtime")


async def test_repository_flush_rate_limits():
    """Flushing an untouched repository completes"""
    repo = make_repository(cdk_ffi.WalletRepositoryConfig())
    await repo.flush_rate_limits()


async def main():
    """Run all rate-limit tests"""
    print("Starting CDK FFI Rate Limit Tests")
    print("=" * 60)

    tests = [
        # Directly constructed wallets
        ("Wallet Default Rate Limit", test_wallet_default_rate_limit),
        ("Wallet Disabled Rate Limit", test_wallet_disabled_rate_limit),
        ("Wallet Custom Rate Limit", test_wallet_custom_rate_limit),
        ("Wallet Invalid Rate Limit", test_wallet_invalid_rate_limit),
        ("Wallet Set Rate Limit Rejects Zero", test_wallet_set_rate_limit_rejects_zero),
        ("Wallet Flush Rate Limits", test_wallet_flush_rate_limits),
        # Repository-provided wallets
        ("Repository Default Rate Limit", test_repository_default_rate_limit),
        ("Repository Disabled Rate Limit", test_repository_disabled_rate_limit),
        ("Repository Custom Rate Limit", test_repository_custom_rate_limit),
        ("Repository Invalid Rate Limit", test_repository_invalid_rate_limit),
        ("Repository Flush Rate Limits", test_repository_flush_rate_limits),
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

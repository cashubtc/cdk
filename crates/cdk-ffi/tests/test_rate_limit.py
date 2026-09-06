#!/usr/bin/env python3
"""Offline smoke tests for wallet workflow construction and configuration."""

import asyncio
import shutil
import sys
import tempfile
from pathlib import Path


repo_root = Path(__file__).resolve().parents[3]
bindings_path = repo_root / "target" / "bindings" / "python"
lib_path = repo_root / "target" / "release"
lib_file = "libcdk_ffi.dylib" if sys.platform == "darwin" else "libcdk_ffi.so"
src_lib = lib_path / lib_file
dst_lib = bindings_path / lib_file

if src_lib.exists() and not dst_lib.exists():
    shutil.copy2(src_lib, dst_lib)

sys.path.insert(0, str(bindings_path))

import cdk_ffi


MINT_URL = "https://mint.example.com"


def wallet_request(database_path, rate_limit=None):
    config = cdk_ffi.WalletConfig(advanced=None, rate_limit=rate_limit)
    return cdk_ffi.WalletOpenRequest(
        mint_url=MINT_URL,
        unit=cdk_ffi.CurrencyUnit.SAT(),
        mnemonic=cdk_ffi.generate_mnemonic(),
        store=cdk_ffi.WalletStore.SQLITE(path=database_path),
        config=config,
    )


async def test_wallet_open_variants(tmpdir):
    variants = [
        cdk_ffi.RateLimit.DEFAULT(),
        cdk_ffi.RateLimit.DISABLED(),
        cdk_ffi.RateLimit.CUSTOM(capacity=5, refill_per_minute=30),
    ]
    for index, rate_limit in enumerate(variants):
        wallet = cdk_ffi.Wallet.open(
            wallet_request(str(Path(tmpdir) / f"wallet-{index}.db"), rate_limit)
        )
        assert wallet.identity().mint_url.url == MINT_URL
        balance = await wallet.balance()
        assert balance.available.value == 0
        assert balance.pending.value == 0
        assert balance.reserved.value == 0


def test_invalid_rate_limit_is_rejected(tmpdir):
    for index, (capacity, refill) in enumerate(((0, 30), (5, 0), (0, 0))):
        try:
            cdk_ffi.Wallet.open(
                wallet_request(
                    str(Path(tmpdir) / f"invalid-{index}.db"),
                    cdk_ffi.RateLimit.CUSTOM(
                        capacity=capacity,
                        refill_per_minute=refill,
                    ),
                )
            )
        except Exception:
            continue
        raise AssertionError(
            f"CUSTOM(capacity={capacity}, refill_per_minute={refill}) should fail"
        )


async def test_wallet_manager(tmpdir):
    manager = cdk_ffi.WalletManager.open(
        cdk_ffi.WalletManagerOpenRequest(
            mnemonic=cdk_ffi.generate_mnemonic(),
            store=cdk_ffi.WalletStore.SQLITE(path=str(Path(tmpdir) / "root.db")),
            proxy_url=None,
            rate_limit=cdk_ffi.RateLimit.DISABLED(),
        )
    )
    wallet = await manager.open_wallet(
        cdk_ffi.WalletIdentity(
            mint_url=cdk_ffi.MintUrl(url=MINT_URL),
            unit=cdk_ffi.CurrencyUnit.SAT(),
        )
    )
    assert wallet.identity().mint_url.url == MINT_URL

    balances = await manager.balances()
    assert len(balances) == 1
    assert balances[0].wallet.mint_url.url == MINT_URL
    assert balances[0].balance.available.value == 0


def test_removed_objects_and_methods_are_not_exported():
    for name in (
        "CashuWallet",
        "CashuWalletOpenRequest",
        "WalletRepository",
        "PreparedSend",
        "PreparedMelt",
        "PendingMelt",
    ):
        assert not hasattr(cdk_ffi, name), f"removed object leaked into bindings: {name}"
    for name in ("request_minting", "prepare_send", "recover_incomplete_sagas"):
        assert not hasattr(cdk_ffi.Wallet, name), f"removed method leaked: Wallet.{name}"


async def main():
    with tempfile.TemporaryDirectory(prefix="cdk-ffi-portable-") as tmpdir:
        await test_wallet_open_variants(tmpdir)
        test_invalid_rate_limit_is_rejected(tmpdir)
        await test_wallet_manager(tmpdir)
        test_removed_objects_and_methods_are_not_exported()
    print("Wallet workflow construction tests passed")


if __name__ == "__main__":
    asyncio.run(main())

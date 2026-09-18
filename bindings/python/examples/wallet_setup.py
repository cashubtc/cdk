#!/usr/bin/env python3
"""Create wallets on an in-memory and on a file-backed SQLite store.

Runs entirely offline: no mint is contacted.
"""

import asyncio
import tempfile
from pathlib import Path

import cdk

MINT_URL = "https://testnut.cashudevkit.org"


def build_wallet(store_path):
    return cdk.Wallet(
        mint_url=MINT_URL,
        unit=cdk.CurrencyUnit.SAT(),
        mnemonic=cdk.generate_mnemonic(),
        store=cdk.sqlite_wallet_store(store_path),
        config=cdk.WalletConfig(target_proof_count=3),
    )


async def report(label, wallet):
    print(f"{label}")
    print(f"  mint:     {wallet.mint_url().url}")
    print(f"  unit:     {wallet.unit()}")
    print(f"  balance:  {(await wallet.total_balance()).value} sat")
    print(f"  pending:  {(await wallet.total_pending_balance()).value} sat")
    print(f"  reserved: {(await wallet.total_reserved_balance()).value} sat")
    print(f"  paced:    {wallet.is_rate_limited()}")


async def main():
    await report("In-memory wallet", build_wallet(":memory:"))

    with tempfile.TemporaryDirectory() as tmp:
        db_path = str(Path(tmp) / "wallet.db")
        await report(f"\nFile-backed wallet ({db_path})", build_wallet(db_path))

    mnemonic = cdk.generate_mnemonic()
    print(f"\nA fresh mnemonic has {len(mnemonic.split())} words")


if __name__ == "__main__":
    asyncio.run(main())

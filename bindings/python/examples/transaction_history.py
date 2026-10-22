#!/usr/bin/env python3
"""List and filter a wallet's transaction history.

Runs entirely offline, so the history is empty; the point is the shape of the
calls and of a Transaction record.
"""

import asyncio

import cdk

MINT_URL = "https://testnut.cashudevkit.org"


def describe(tx):
    direction = "in " if tx.direction == cdk.TransactionDirection.INCOMING else "out"
    return f"{direction} {tx.amount.value:>6} sat  fee {tx.fee.value:<4} {tx.memo or ''}"


async def main():
    wallet = cdk.Wallet(
        mint_url=MINT_URL,
        unit=cdk.CurrencyUnit.SAT(),
        mnemonic=cdk.generate_mnemonic(),
        store=cdk.sqlite_wallet_store(":memory:"),
        config=cdk.WalletConfig(target_proof_count=None),
    )

    every = await wallet.list_transactions(None)
    incoming = await wallet.list_transactions(cdk.TransactionDirection.INCOMING)
    outgoing = await wallet.list_transactions(cdk.TransactionDirection.OUTGOING)

    print(f"all:      {len(every)}")
    print(f"incoming: {len(incoming)}")
    print(f"outgoing: {len(outgoing)}")

    for tx in every:
        print(f"  {describe(tx)}")

    if not every:
        print("\nNo transactions yet. Mint or receive first, see mint_and_send.py.")


if __name__ == "__main__":
    asyncio.run(main())

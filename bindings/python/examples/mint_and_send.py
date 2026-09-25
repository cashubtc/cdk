#!/usr/bin/env python3
"""Full lifecycle against a real mint: mint, send, receive back.

Needs a reachable mint, so it is not run in CI:

    CDK_PYTHON_TEST_MINT_URL=https://testnut.cashudevkit.org \
        python examples/mint_and_send.py

testnut pays mint quotes automatically, which is why this waits instead of
asking you to pay the invoice.
"""

import asyncio
import os
import sys

import cdk

MINT_URL = os.environ.get("CDK_PYTHON_TEST_MINT_URL")
SETTLEMENT_DELAY = int(os.environ.get("CDK_PYTHON_MINT_SETTLEMENT_DELAY_SECONDS") or "3")

MINT_AMOUNT = cdk.Amount(value=64)
SEND_AMOUNT = cdk.Amount(value=16)


def send_options(memo):
    """Every SendOptions field is required: the generated bindings have no defaults."""
    return cdk.SendOptions(
        memo=cdk.SendMemo(memo=memo, include_memo=True),
        conditions=None,
        amount_split_target=cdk.SplitTarget.NONE(),
        send_kind=cdk.SendKind.ONLINE_EXACT(),
        include_fee=False,
        use_p2bk=False,
        max_proofs=None,
        metadata={},
        p2pk_signing_keys=[],
        p2pk_locked_proof_send_mode=cdk.P2pkLockedProofSendMode.SWAP,
    )


def receive_options():
    return cdk.ReceiveOptions(
        amount_split_target=cdk.SplitTarget.NONE(),
        p2pk_signing_keys=[],
        preimages=[],
        metadata={},
    )


async def main():
    if not MINT_URL:
        print("Set CDK_PYTHON_TEST_MINT_URL to run this example, for example:")
        print("  CDK_PYTHON_TEST_MINT_URL=https://testnut.cashudevkit.org \\")
        print("      python examples/mint_and_send.py")
        return 1

    wallet = cdk.Wallet(
        mint_url=MINT_URL,
        unit=cdk.CurrencyUnit.SAT(),
        mnemonic=cdk.generate_mnemonic(),
        store=cdk.sqlite_wallet_store(":memory:"),
        config=cdk.WalletConfig(target_proof_count=3),
    )

    info = await wallet.fetch_mint_info()
    print(f"mint: {info.name if info else MINT_URL}")

    quote = await wallet.mint_quote(
        cdk.PaymentMethod.BOLT11(), MINT_AMOUNT, "cdk-python example", None
    )
    print(f"quote {quote.id}")
    print(f"invoice: {quote.request}")

    print(f"waiting {SETTLEMENT_DELAY}s for the mint to settle the quote...")
    await asyncio.sleep(SETTLEMENT_DELAY)

    proofs = await wallet.mint(quote.id, cdk.SplitTarget.NONE(), None)
    print(f"minted {len(proofs)} proofs, balance {(await wallet.total_balance()).value} sat")

    prepared = await wallet.prepare_send(SEND_AMOUNT, send_options("coffee"))
    print(f"sending {prepared.amount().value} sat, fee {prepared.fee().value} sat")

    token = await prepared.confirm("coffee")
    encoded = token.encode()
    print(f"token: {encoded[:48]}...")
    print(f"balance after send: {(await wallet.total_balance()).value} sat")

    received = await wallet.receive(cdk.Token.decode(encoded), receive_options())
    print(f"received {received.value} sat back")
    print(f"final balance: {(await wallet.total_balance()).value} sat")

    for tx in await wallet.list_transactions(None):
        print(f"  {tx.direction} {tx.amount.value} sat")

    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))

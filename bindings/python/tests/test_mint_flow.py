"""Live mint flow against a real mint.

Skipped unless CDK_PYTHON_TEST_MINT_URL is set, matching how the Dart suite
gates its live tests.
"""

import asyncio

import pytest

import cdk

pytestmark = pytest.mark.live


async def test_mint_flow(live_wallet, settlement_delay):
    amount = cdk.Amount(value=100)

    quote = await live_wallet.mint_quote(
        cdk.PaymentMethod.BOLT11(), amount, "binding test", None
    )
    assert quote.request

    await asyncio.sleep(settlement_delay)

    await live_wallet.mint(quote.id, cdk.SplitTarget.NONE(), None)

    assert (await live_wallet.total_balance()).value == amount.value


async def test_send_and_receive_round_trip(live_wallet, settlement_delay):
    quote = await live_wallet.mint_quote(
        cdk.PaymentMethod.BOLT11(), cdk.Amount(value=64), "binding test", None
    )
    await asyncio.sleep(settlement_delay)
    await live_wallet.mint(quote.id, cdk.SplitTarget.NONE(), None)

    prepared = await live_wallet.prepare_send(
        cdk.Amount(value=16),
        cdk.SendOptions(
            memo=None,
            conditions=None,
            amount_split_target=cdk.SplitTarget.NONE(),
            send_kind=cdk.SendKind.ONLINE_EXACT(),
            include_fee=False,
            use_p2bk=False,
            max_proofs=None,
            metadata={},
            p2pk_signing_keys=[],
            p2pk_locked_proof_send_mode=cdk.P2pkLockedProofSendMode.SWAP,
        ),
    )
    token = await prepared.confirm("round trip")

    encoded = token.encode()
    assert encoded.startswith("cashu")

    received = await live_wallet.receive(
        cdk.Token.decode(encoded),
        cdk.ReceiveOptions(
            amount_split_target=cdk.SplitTarget.NONE(),
            p2pk_signing_keys=[],
            preimages=[],
            metadata={},
        ),
    )
    assert 0 < received.value <= 16, "receive yields the sent amount less the mint's swap fee"


async def test_transactions_are_recorded(live_wallet, settlement_delay):
    quote = await live_wallet.mint_quote(
        cdk.PaymentMethod.BOLT11(), cdk.Amount(value=32), None, None
    )
    await asyncio.sleep(settlement_delay)
    await live_wallet.mint(quote.id, cdk.SplitTarget.NONE(), None)

    incoming = await live_wallet.list_transactions(cdk.TransactionDirection.INCOMING)
    assert incoming

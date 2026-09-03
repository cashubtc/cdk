#!/usr/bin/env python3
"""Live portable-wallet test for an asynchronously finalized on-chain payment."""

import asyncio
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path


MINT_URL = "https://testnut.cashudevkit.org"
MINT_AMOUNT_SAT = 25_000
PAYMENT_AMOUNT_SAT = 1_000
MINT_QUOTE_TIMEOUT = float(os.environ.get("CDK_FFI_LIVE_MINT_TIMEOUT", "60"))
PENDING_PAYMENT_TIMEOUT = float(os.environ.get("CDK_FFI_LIVE_MELT_TIMEOUT", "30"))
POLL_INTERVAL = float(os.environ.get("CDK_FFI_LIVE_POLL_INTERVAL", "2"))

# Valid mainnet address. The test mint decides whether payment settles
# immediately or remains pending.
ONCHAIN_ADDRESS = "1BoatSLRHtKNngkdXEeobR76b53LETtpyT"


def load_bindings():
    repo_root = Path(__file__).resolve().parents[3]
    bindings_path = repo_root / "target" / "bindings" / "python"
    lib_file = "libcdk_ffi.dylib" if sys.platform == "darwin" else "libcdk_ffi.so"

    if not (bindings_path / "cdk_ffi.py").exists():
        raise SystemExit("Python bindings not found. Run: just ffi-generate python --debug")

    for profile in ("debug", "release"):
        src_lib = repo_root / "target" / profile / lib_file
        if src_lib.exists():
            shutil.copy2(src_lib, bindings_path / lib_file)
            break
    else:
        if not (bindings_path / lib_file).exists():
            raise SystemExit("FFI library not found. Run: just ffi-generate python --debug")

    sys.path.insert(0, str(bindings_path))

    import cdk_ffi  # noqa: PLC0415

    return cdk_ffi


cdk_ffi = load_bindings()


def assert_amount(amount, expected):
    assert amount.value == expected, f"expected {expected} sats, got {amount.value}"


async def wait_for_paid_mint_session(session):
    deadline = time.monotonic() + MINT_QUOTE_TIMEOUT
    last_state = session.initial_state()

    while time.monotonic() < deadline:
        last_state = await session.refresh()
        if last_state.state == cdk_ffi.MintingState.PAID:
            return last_state
        await asyncio.sleep(POLL_INTERVAL)

    raise AssertionError(f"mint session did not become paid before timeout: {last_state}")


def assert_payment_receipt(receipt, quote_id):
    assert receipt.quote_id == quote_id
    assert_amount(receipt.amount, PAYMENT_AMOUNT_SAT)
    assert receipt.fee_paid.value >= 0


async def test_live_async_onchain_payment():
    with tempfile.TemporaryDirectory(prefix="cdk-ffi-live-") as tmpdir:
        wallet = cdk_ffi.Wallet.open(
            cdk_ffi.WalletOpenRequest(
                mint_url=MINT_URL,
                unit=cdk_ffi.CurrencyUnit.SAT(),
                mnemonic=cdk_ffi.generate_mnemonic(),
                store=cdk_ffi.WalletStore.SQLITE(
                    path=str(Path(tmpdir) / "wallet.db")
                ),
                config=cdk_ffi.WalletConfig(target_proof_count=3),
            )
        )

        mint_session = await wallet.request_minting(
            cdk_ffi.MintRequest(
                method=cdk_ffi.PaymentMethod.BOLT11(),
                amount=cdk_ffi.Amount(value=MINT_AMOUNT_SAT),
            )
        )
        assert mint_session.initial_state().payment_request
        await wait_for_paid_mint_session(mint_session)
        claimed = await mint_session.claim()
        assert claimed.value > 0

        balance = await wallet.balance()
        assert balance.available.value >= MINT_AMOUNT_SAT

        sessions = await wallet.quote_payment(
            cdk_ffi.PaymentQuoteRequest(
                target=cdk_ffi.PaymentTarget.ONCHAIN(
                    address=ONCHAIN_ADDRESS,
                    amount=cdk_ffi.Amount(value=PAYMENT_AMOUNT_SAT),
                    max_fee=None,
                )
            )
        )
        assert sessions

        payment_session = sessions[0]
        quote = payment_session.quote()
        assert quote.id
        assert_amount(quote.amount, PAYMENT_AMOUNT_SAT)

        plan = await payment_session.prepare()
        assert plan.quote_id() == quote.id
        assert_amount(plan.amount(), PAYMENT_AMOUNT_SAT)
        assert plan.operation_id()

        confirmation = await plan.confirm_prefer_async()
        if confirmation.is_COMPLETED():
            assert_payment_receipt(confirmation.receipt, quote.id)
            return

        if confirmation.is_PENDING():
            pending = confirmation.payment
            assert pending.quote_id() == quote.id
            operation_id = pending.operation_id()
            assert operation_id

            # Prove the handle is reconstructable before waiting on it.
            resumed = await wallet.pending_payment(operation_id)
            assert resumed.quote_id() == quote.id
            try:
                receipt = await asyncio.wait_for(
                    resumed.wait(),
                    timeout=PENDING_PAYMENT_TIMEOUT,
                )
            except asyncio.TimeoutError:
                # Startup/resume recovery is the durable fallback when a mobile
                # process cannot keep a long-running wait alive.
                report = await wallet.synchronize(cdk_ffi.SyncPolicy.ONLINE)
                assert report.failed_operations == 0
            else:
                assert_payment_receipt(receipt, quote.id)
            return

        raise AssertionError(f"unexpected payment confirmation: {confirmation}")


async def main():
    await test_live_async_onchain_payment()
    print("Live async on-chain payment test passed")


if __name__ == "__main__":
    asyncio.run(main())

#!/usr/bin/env python3
"""
Live CDK FFI async onchain melt test.

This test uses https://testnut.cashudevkit.org and requires network access.
It intentionally lives outside the deterministic Nix FFI check.
"""

import asyncio
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path


MINT_URL = "https://testnut.cashudevkit.org"
MINT_AMOUNT_SAT = 25_000
MELT_AMOUNT_SAT = 1_000
MINT_QUOTE_TIMEOUT = float(os.environ.get("CDK_FFI_LIVE_MINT_TIMEOUT", "60"))
PENDING_MELT_TIMEOUT = float(os.environ.get("CDK_FFI_LIVE_MELT_TIMEOUT", "30"))
POLL_INTERVAL = float(os.environ.get("CDK_FFI_LIVE_POLL_INTERVAL", "2"))

# Valid mainnet address. The testnut onchain melt backend decides whether the
# payment settles immediately or remains pending.
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


async def wait_for_paid_mint_quote(wallet, quote_id):
    deadline = time.monotonic() + MINT_QUOTE_TIMEOUT
    last_quote = None

    while time.monotonic() < deadline:
        last_quote = await wallet.check_mint_quote_status(quote_id)
        if last_quote.state == cdk_ffi.QuoteState.PAID:
            return last_quote

        await asyncio.sleep(POLL_INTERVAL)

    raise AssertionError(f"mint quote did not become paid before timeout: {last_quote}")


def assert_finalized_melt(finalized, quote_id):
    assert finalized.quote_id == quote_id
    assert finalized.state == cdk_ffi.QuoteState.PAID
    assert_amount(finalized.amount, MELT_AMOUNT_SAT)
    assert finalized.fee_paid.value >= 0


async def assert_pending_remains_valid(wallet, pending):
    status = await wallet.check_melt_quote_status(pending.quote_id())

    if status.state == cdk_ffi.QuoteState.PAID:
        finalized = await wallet.finalize_pending_melts()
        for melt in finalized:
            if melt.quote_id == pending.quote_id():
                assert_finalized_melt(melt, pending.quote_id())
                return

        assert status.id == pending.quote_id()
        assert_amount(status.amount, MELT_AMOUNT_SAT)
        return

    assert status.state == cdk_ffi.QuoteState.PENDING
    assert status.id == pending.quote_id()
    assert_amount(status.amount, MELT_AMOUNT_SAT)


async def test_live_async_onchain_melt():
    with tempfile.TemporaryDirectory(prefix="cdk-ffi-live-") as tmpdir:
        db_path = str(Path(tmpdir) / "wallet.db")
        wallet = cdk_ffi.Wallet(
            mint_url=MINT_URL,
            unit=cdk_ffi.CurrencyUnit.SAT(),
            mnemonic=cdk_ffi.generate_mnemonic(),
            store=cdk_ffi.WalletStore.SQLITE(path=db_path),
            config=cdk_ffi.WalletConfig(target_proof_count=3),
        )

        mint_quote = await wallet.mint_quote(
            cdk_ffi.PaymentMethod.BOLT11(),
            cdk_ffi.Amount(value=MINT_AMOUNT_SAT),
            None,
            None,
        )
        await wait_for_paid_mint_quote(wallet, mint_quote.id)

        proofs = await wallet.mint(mint_quote.id, cdk_ffi.SplitTarget.NONE(), None)
        assert len(proofs) > 0

        balance = await wallet.total_balance()
        assert balance.value >= MINT_AMOUNT_SAT

        options = await wallet.quote_onchain_melt_options(
            ONCHAIN_ADDRESS,
            cdk_ffi.Amount(value=MELT_AMOUNT_SAT),
            None,
        )
        assert len(options) > 0

        quote = await wallet.select_onchain_melt_quote(options[0])
        assert quote.id
        assert_amount(quote.amount, MELT_AMOUNT_SAT)
        assert quote.fee_reserve.value >= 0

        prepared = await wallet.prepare_melt(quote.id)
        assert prepared.quote_id() == quote.id
        assert_amount(prepared.amount(), MELT_AMOUNT_SAT)

        outcome = await prepared.confirm_prefer_async()

        if outcome.is_PAID():
            assert_finalized_melt(outcome.finalized, quote.id)
            return

        if outcome.is_PENDING():
            pending = outcome.pending
            assert pending.quote_id() == quote.id
            assert pending.operation_id()

            try:
                finalized = await asyncio.wait_for(
                    pending.wait(),
                    timeout=PENDING_MELT_TIMEOUT,
                )
            except asyncio.TimeoutError:
                await assert_pending_remains_valid(wallet, pending)
            else:
                assert_finalized_melt(finalized, quote.id)
            return

        raise AssertionError(f"unexpected melt outcome: {outcome}")


async def main():
    await test_live_async_onchain_melt()
    print("Live async onchain melt test passed")


if __name__ == "__main__":
    asyncio.run(main())

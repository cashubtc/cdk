"""Wallet tests, ported from the Dart, Go, Kotlin and Swift binding suites."""

import asyncio

import pytest

import cdk


async def test_initial_balance_is_zero(wallet):
    assert (await wallet.total_balance()).value == 0
    assert (await wallet.total_pending_balance()).value == 0
    assert (await wallet.total_reserved_balance()).value == 0


def test_wallet_exposes_its_mint_url_and_unit(wallet, offline_mint_url):
    assert wallet.mint_url().url == offline_mint_url
    assert wallet.unit() == cdk.CurrencyUnit.SAT()


async def test_file_backed_wallet_starts_empty(file_wallet):
    assert (await file_wallet.total_balance()).value == 0


async def test_in_memory_sqlite_concurrent_access(wallet):
    balances = await asyncio.gather(*(wallet.total_balance() for _ in range(64)))

    assert len(balances) == 64
    assert all(balance.value == 0 for balance in balances)


async def test_transaction_history_starts_empty(wallet):
    assert await wallet.list_transactions(None) == []
    assert await wallet.list_transactions(cdk.TransactionDirection.INCOMING) == []
    assert await wallet.list_transactions(cdk.TransactionDirection.OUTGOING) == []


async def test_no_pending_sends_on_a_fresh_wallet(wallet):
    assert await wallet.get_pending_sends() == []


def test_wallet_rejects_an_unreachable_store(offline_mint_url):
    with pytest.raises(Exception):
        cdk.Wallet(
            mint_url=offline_mint_url,
            unit=cdk.CurrencyUnit.SAT(),
            mnemonic=cdk.generate_mnemonic(),
            store=cdk.sqlite_wallet_store("/nonexistent-directory/wallet.db"),
            config=cdk.WalletConfig(target_proof_count=None),
        )

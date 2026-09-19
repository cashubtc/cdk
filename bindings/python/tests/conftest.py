"""Shared fixtures for the cdk binding tests.

Mirrors the fixtures the Dart, Go, Kotlin and Swift binding tests use, so the
same scenarios run the same way across every language.
"""

import os

import pytest

import cdk

OFFLINE_MINT_URL = "https://mint.example.com"

LIVE_MINT_URL = os.environ.get("CDK_PYTHON_TEST_MINT_URL")

SETTLEMENT_DELAY_SECONDS = int(
    os.environ.get("CDK_PYTHON_MINT_SETTLEMENT_DELAY_SECONDS") or "3"
)

requires_mint = pytest.mark.skipif(
    not LIVE_MINT_URL,
    reason="Set CDK_PYTHON_TEST_MINT_URL to run live mint tests",
)


def build_wallet(mint_url=OFFLINE_MINT_URL, config=None, store_path=":memory:"):
    """Build a wallet backed by a fresh SQLite store."""
    return cdk.Wallet(
        mint_url=mint_url,
        unit=cdk.CurrencyUnit.SAT(),
        mnemonic=cdk.generate_mnemonic(),
        store=cdk.sqlite_wallet_store(store_path),
        config=config or cdk.WalletConfig(target_proof_count=None),
    )


@pytest.fixture
def offline_mint_url():
    return OFFLINE_MINT_URL


@pytest.fixture
def settlement_delay():
    return SETTLEMENT_DELAY_SECONDS


@pytest.fixture
def wallet():
    return build_wallet()


@pytest.fixture
def wallet_factory():
    return build_wallet


@pytest.fixture
def file_wallet(tmp_path):
    return build_wallet(store_path=str(tmp_path / "wallet.db"))


@pytest.fixture
def live_wallet():
    if not LIVE_MINT_URL:
        pytest.skip("Set CDK_PYTHON_TEST_MINT_URL to run live mint tests")
    return build_wallet(mint_url=LIVE_MINT_URL)

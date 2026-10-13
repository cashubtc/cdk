"""Client-side rate limiting, ported from the other binding suites."""

import pytest

import cdk


def test_default_paces_the_wallet(wallet_factory):
    omitted = wallet_factory(config=cdk.WalletConfig(target_proof_count=None))
    assert omitted.is_rate_limited()

    explicit = wallet_factory(
        config=cdk.WalletConfig(
            target_proof_count=None,
            rate_limit=cdk.RateLimit.DEFAULT(),
        )
    )
    assert explicit.is_rate_limited()


def test_disabled_can_be_re_enabled(wallet_factory):
    wallet = wallet_factory(
        config=cdk.WalletConfig(
            target_proof_count=None,
            rate_limit=cdk.RateLimit.DISABLED(),
        )
    )
    assert not wallet.is_rate_limited()

    wallet.set_rate_limit(cdk.RateLimit.DEFAULT())
    assert wallet.is_rate_limited()


def test_custom_paces_the_wallet(wallet_factory):
    wallet = wallet_factory(
        config=cdk.WalletConfig(
            target_proof_count=None,
            rate_limit=cdk.RateLimit.CUSTOM(capacity=5, refill_per_minute=30),
        )
    )
    assert wallet.is_rate_limited()


@pytest.mark.parametrize("capacity,refill", [(0, 30), (5, 0), (0, 0)])
def test_zero_values_are_rejected_at_construction(wallet_factory, capacity, refill):
    with pytest.raises(Exception):
        wallet_factory(
            config=cdk.WalletConfig(
                target_proof_count=None,
                rate_limit=cdk.RateLimit.CUSTOM(
                    capacity=capacity,
                    refill_per_minute=refill,
                ),
            )
        )


def test_zero_values_are_rejected_at_runtime(wallet):
    with pytest.raises(Exception):
        wallet.set_rate_limit(cdk.RateLimit.CUSTOM(capacity=0, refill_per_minute=30))

    assert wallet.is_rate_limited(), "a rejected change leaves pacing untouched"


async def test_flushing_an_untouched_wallet_completes(wallet):
    await wallet.flush_rate_limits()

#!/usr/bin/env python3
"""Offline tests through generated UniFFI bindings (no protocol encoder in the app)."""

import asyncio
import base64
import hashlib
import os
from pathlib import Path
import sys

sys.path.insert(0, os.environ.get(
    "CDK_FFI_BINDINGS_DIR",
    str(Path(__file__).resolve().parents[3] / "target/bindings/python"),
))
import cdk_ffi as cdk

# Public secp256k1 generator point; no receiving private key is passed to CDK.
PUBKEY = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
P2PK = "02" + PUBKEY
RELAY = "wss://relay.example.com"


def params(**overrides):
    values = dict(amount=None, unit="sat", description=None, pubkeys=None,
                  num_sigs=1, hash=None, preimage=None, transport="none",
                  http_url=None, nostr_relays=None, mints=None,
                  mint_preferred=None, supported_methods=[])
    values.update(overrides)
    return cdk.CreateRequestParams(**values)


def roundtrip(request):
    encoded = request.to_string_encoded()
    for wire in (encoded, request.to_bech32_string()):
        decoded = cdk.decode_payment_request(wire)
        if wire.startswith("creqA"):
            assert decoded.to_string_encoded() == encoded
        # NUT-26 decoding adds relay tags extracted from the nprofile, so
        # compare request content rather than requiring identical wire bytes.
        assert [t.target for t in decoded.transports()] == [t.target for t in request.transports()]
        assert decoded.amount() == request.amount()
        assert decoded.unit() == request.unit()
        assert decoded.supported_methods() == request.supported_methods()
        original_cbor = base64.urlsafe_b64decode(encoded[5:])
        decoded_cbor = base64.urlsafe_b64decode(decoded.to_string_encoded()[5:])
        if b"nut10" in original_cbor:
            assert original_cbor.split(b"nut10", 1)[1] == decoded_cbor.split(b"nut10", 1)[1]
        assert decoded.payment_id() == request.payment_id()
        assert decoded.mints() == request.mints()
        assert decoded.mint_preferred() == request.mint_preferred()
        assert decoded.description() == request.description()
        assert decoded.single_use() is True
    return base64.urlsafe_b64decode(encoded[5:])


async def main():
    repo = cdk.WalletRepository(cdk.generate_mnemonic(), cdk.sqlite_wallet_store(":memory:"))
    for amount in (None, 42):
        for preferred in (None, False, True):
            result = await repo.create_request(params(
                amount=amount, unit="msat", description="Receive coffee",
                payment_id="app/request/42", nostr_pubkey=PUBKEY,
                transport="nostr", nostr_relays=[RELAY],
                mints=["https://mint.example.com"], mint_preferred=preferred,
                pubkeys=[P2PK],
                supported_methods=[cdk.SupportedMethod(method="bolt11", fee=cdk.Amount(value=2))],
            ))
            req = result.payment_request
            assert result.nostr_wait_info is None
            assert req.payment_id() == "app/request/42"
            assert (req.amount().value if req.amount() else None) == amount
            assert req.unit() == cdk.CurrencyUnit.MSAT()
            assert req.description() == "Receive coffee"
            assert req.mints() == ["https://mint.example.com"]
            assert req.mint_preferred() == preferred
            assert req.supported_methods()[0].fee.value == 2
            transport = req.transports()[0]
            assert transport.target.startswith("nprofile1")
            assert transport.tags == [["n", "17"]]
            # Inspect the encoded CBOR bytes as well as the exported getters.
            wire = roundtrip(req)
            assert b"P2PK" in wire and P2PK.encode() in wire
            assert b"nut10" in wire

    # Explicit destination construction is deterministic and leaves the ID absent by default.
    explicit = params(transport="nostr", nostr_pubkey=PUBKEY, nostr_relays=[RELAY])
    one = await repo.create_request(explicit)
    two = await repo.create_request(explicit)
    assert one.payment_request.to_string_encoded() == two.payment_request.to_string_encoded()
    assert one.payment_request.payment_id() is None

    ephemeral = await repo.create_request(params(transport="nostr", nostr_relays=[RELAY]))
    assert ephemeral.nostr_wait_info is not None
    assert ephemeral.nostr_wait_info.relays() == [RELAY]
    assert len(ephemeral.nostr_wait_info.pubkey()) == 64
    roundtrip(ephemeral.payment_request)

    for transport in ("none", "http"):
        result = await repo.create_request(params(
            transport=transport, http_url="https://receiver.example.com/pay",
            hash="ab" * 32,
        ))
        assert result.nostr_wait_info is None
        assert result.payment_request.mints() == []
        wire = roundtrip(result.payment_request)
        assert b"HTLC" in wire and ("ab" * 32).encode() in wire
        targets = result.payment_request.transports()
        assert len(targets) == (1 if transport == "http" else 0)
        if targets:
            assert targets[0].target == "https://receiver.example.com/pay"

    # Preimage and multisig inputs use the same core construction path.
    result = await repo.create_request(params(preimage="00" * 32, pubkeys=[P2PK]))
    wire = roundtrip(result.payment_request)
    assert b"HTLC" in wire
    assert hashlib.sha256(bytes(32)).hexdigest().encode() in wire
    second_key = "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"
    result = await repo.create_request(params(pubkeys=[P2PK, second_key], num_sigs=2))
    wire = roundtrip(result.payment_request)
    assert P2PK.encode() in wire and second_key.encode() in wire
    assert b"n_sigs\x612" in wire

    for invalid in (
        dict(transport="invalid"), dict(transport="http"),
        dict(transport="nostr"),
        dict(transport="nostr", nostr_relays=["invalid"]),
        dict(transport="nostr", nostr_relays=[RELAY], nostr_pubkey="invalid"),
        dict(nostr_pubkey=PUBKEY), dict(mints=["not a URL"]),
        dict(pubkeys=[P2PK, P2PK], num_sigs=2),
        dict(hash="invalid"), dict(preimage="invalid"),
    ):
        try:
            await repo.create_request(params(**invalid))
        except cdk.FfiError:
            pass
        else:
            raise AssertionError(f"Expected FFI validation error for {invalid}")
    print("Payment request binding tests passed")


if __name__ == "__main__":
    asyncio.run(main())

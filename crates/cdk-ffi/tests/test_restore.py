#!/usr/bin/env python3
"""
End-to-end tests for CDK FFI seed restore options (`Nut13Options`, `RestoreStart`,
`Restored.next_keyset_indices`, and `FfiError.RestoreInterrupted`) against a
local mock HTTP mint server.
"""

import asyncio
import hashlib
import http.server
import json
import shutil
import sys
import threading
from pathlib import Path

repo_root = Path(__file__).parent.parent.parent.parent
bindings_path = repo_root / "target" / "bindings" / "python"
lib_path = repo_root / "target" / "release"

lib_file = "libcdk_ffi.dylib" if sys.platform == "darwin" else "libcdk_ffi.so"
src_lib = lib_path / lib_file
dst_lib = bindings_path / lib_file

if src_lib.exists():
    shutil.copy2(src_lib, dst_lib)

sys.path.insert(0, str(bindings_path))

import cdk_ffi

# Generator point G (private key k = 1), so C_ = 1 * B_ = B_ is a valid blind signature.
PUBKEY_G = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
PUBKEY_G_BYTES = bytes.fromhex(PUBKEY_G)


def v1_keyset_id(count: int) -> str:
    return "00" + hashlib.sha256(PUBKEY_G_BYTES * count).hexdigest()[:14]


KS1 = v1_keyset_id(1)
KS2 = v1_keyset_id(2)
KS_USD = v1_keyset_id(3)

KEYSETS = [
    {"id": KS1, "unit": "sat", "active": True, "input_fee_ppk": 0},
    {"id": KS2, "unit": "sat", "active": False, "input_fee_ppk": 0},
    {"id": KS_USD, "unit": "usd", "active": True, "input_fee_ppk": 0},
]

KEYSET_KEYS = {
    KS1: {"id": KS1, "unit": "sat", "keys": {"1": PUBKEY_G}},
    KS2: {"id": KS2, "unit": "sat", "keys": {"1": PUBKEY_G, "2": PUBKEY_G}},
    KS_USD: {
        "id": KS_USD,
        "unit": "usd",
        "keys": {"1": PUBKEY_G, "2": PUBKEY_G, "4": PUBKEY_G},
    },
}

MNEMONIC = (
    "abandon abandon abandon abandon abandon abandon "
    "abandon abandon abandon abandon abandon about"
)


class MockMintState:
    def __init__(self):
        self.lock = threading.Lock()
        self.restore_requests = []
        # Map keyset_id -> set of blinded secrets B_ that the mint will sign with amount=1.
        self.signed_b_: dict[str, set[str]] = {KS1: set(), KS2: set()}
        # Optional set of B_ values whose batch triggers an HTTP 500 error.
        self.fail_on_b_: set[str] = set()

    def clear_requests(self):
        with self.lock:
            self.restore_requests.clear()


def start_mock_mint(state: MockMintState):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, format, *args):
            pass

        def _send_json(self, status: int, body: dict):
            payload = json.dumps(body).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def do_GET(self):
            if self.path == "/v1/info":
                self._send_json(
                    200,
                    {
                        "name": "Mock FFI Mint",
                        "version": "mock/0.1.0",
                        "nuts": {
                            "7": {"supported": True},
                            "9": {"supported": True},
                        },
                    },
                )
            elif self.path == "/v1/keysets":
                self._send_json(200, {"keysets": KEYSETS})
            elif self.path == "/v1/keys":
                self._send_json(200, {"keysets": list(KEYSET_KEYS.values())})
            elif self.path.startswith("/v1/keys/"):
                ks_id = self.path.split("/v1/keys/", 1)[1]
                if ks_id in KEYSET_KEYS:
                    self._send_json(200, {"keysets": [KEYSET_KEYS[ks_id]]})
                else:
                    self._send_json(404, {"detail": "Unknown keyset", "code": 12001})
            else:
                self._send_json(404, {"detail": f"Unhandled GET {self.path}", "code": 0})

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length).decode("utf-8"))
            if self.path == "/v1/restore":
                outputs = body.get("outputs", [])
                with state.lock:
                    state.restore_requests.append(outputs)
                    should_fail = any(o["B_"] in state.fail_on_b_ for o in outputs)
                    matched_outputs = []
                    matched_sigs = []
                    if not should_fail:
                        for o in outputs:
                            ks_id = o["id"]
                            if o["B_"] in state.signed_b_.get(ks_id, set()):
                                matched_outputs.append(o)
                                matched_sigs.append(
                                    {
                                        "id": ks_id,
                                        "amount": 1,
                                        "C_": o["B_"],
                                    }
                                )
                if should_fail:
                    self._send_json(500, {"detail": "Simulated mint failure", "code": 0})
                else:
                    self._send_json(
                        200,
                        {"outputs": matched_outputs, "signatures": matched_sigs},
                    )
            elif self.path == "/v1/checkstate":
                ys = body.get("Ys", [])
                self._send_json(
                    200,
                    {"states": [{"Y": y, "state": "UNSPENT"} for y in ys]},
                )
            else:
                self._send_json(404, {"detail": f"Unhandled POST {self.path}", "code": 0})

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address
    return server, f"http://{host}:{port}"


def make_wallet(mint_url: str, db=None):
    store = (
        cdk_ffi.custom_wallet_store(db)
        if db is not None
        else cdk_ffi.sqlite_wallet_store(":memory:")
    )
    return cdk_ffi.Wallet(
        mint_url=mint_url,
        unit=cdk_ffi.CurrencyUnit.SAT(),
        mnemonic=MNEMONIC,
        store=store,
        config=cdk_ffi.WalletConfig(
            target_proof_count=None,
            rate_limit=cdk_ffi.RateLimit.DISABLED(),
        ),
    )


async def test_restore_with_all_start_variants_and_interruption():
    state = MockMintState()
    server, mint_url = start_mock_mint(state)
    try:
        # 1) Discovery pass using omitted `start` (backwards-compatible default = None -> FromZero)
        #    to capture deterministic blinded secrets B_(0..12) for both Sat keysets.
        discovery_wallet = make_wallet(mint_url)
        omitted_opts = cdk_ffi.Nut13Options(batch_size=12, max_gap=1)
        assert omitted_opts.start is None, f"expected default start=None, got {omitted_opts.start}"
        discovery = await discovery_wallet.restore_with_opts(omitted_opts)
        assert discovery.unspent.value == 0
        assert discovery.next_keyset_indices == {KS1: 0, KS2: 0}, discovery.next_keyset_indices

        b_by_ks: dict[str, list[str]] = {}
        for batch in state.restore_requests:
            ks_id = batch[0]["id"]
            b_by_ks[ks_id] = [o["B_"] for o in batch]
        assert set(b_by_ks.keys()) == {KS1, KS2}
        assert len(b_by_ks[KS1]) == 12 and len(b_by_ks[KS2]) == 12

        # Stage proofs: KS1 has proofs at indices 2 and 6; KS2 has a proof at index 5.
        state.signed_b_[KS1] = {b_by_ks[KS1][2], b_by_ks[KS1][6]}
        state.signed_b_[KS2] = {b_by_ks[KS2][5]}

        # 2) Explicit FROM_ZERO recovers all staged proofs and reports next_keyset_indices.
        db = cdk_ffi.create_wallet_db(cdk_ffi.WalletDbBackend.SQLITE(path=":memory:"))
        wallet = make_wallet(mint_url, db=db)
        state.clear_requests()
        full = await wallet.restore_with_opts(
            cdk_ffi.Nut13Options(
                batch_size=4,
                max_gap=2,
                start=cdk_ffi.RestoreStart.FROM_ZERO(),
            )
        )
        assert full.unspent.value == 3, full
        assert full.next_keyset_indices == {KS1: 7, KS2: 6}, full.next_keyset_indices
        assert (await wallet.total_balance()).value == 3
        assert await db.increment_keyset_counter(cdk_ffi.Id(hex=KS1), 0) == 7
        assert await db.increment_keyset_counter(cdk_ffi.Id(hex=KS2), 0) == 6

        # 3) LOOKBACK(count=2) starts KS1 at 5 (7-2) and KS2 at 4 (6-2).
        state.clear_requests()
        lookback = await wallet.restore_with_opts(
            cdk_ffi.Nut13Options(
                batch_size=4,
                max_gap=1,
                start=cdk_ffi.RestoreStart.LOOKBACK(count=2),
            )
        )
        assert lookback.unspent.value == 2, lookback  # KS1@6 and KS2@5
        assert lookback.next_keyset_indices == {KS1: 7, KS2: 6}, lookback.next_keyset_indices
        first_batch_for = {}
        for batch in state.restore_requests:
            ks_id = batch[0]["id"]
            first_batch_for.setdefault(ks_id, batch[0]["B_"])
        assert first_batch_for[KS1] == b_by_ks[KS1][5]
        assert first_batch_for[KS2] == b_by_ks[KS2][4]

        # 4) KEYSET_INDICES map applies independently per keyset (missing keyset defaults to 0).
        fresh_wallet = make_wallet(mint_url)
        state.clear_requests()
        explicit = await fresh_wallet.restore_with_opts(
            cdk_ffi.Nut13Options(
                batch_size=4,
                max_gap=2,
                start=cdk_ffi.RestoreStart.KEYSET_INDICES(indices={KS1: 6}),
            )
        )
        assert explicit.unspent.value == 2, explicit  # KS1@6 and KS2@5 (KS1@2 skipped, KS2 starts at 0)
        assert explicit.next_keyset_indices == {KS1: 7, KS2: 6}, explicit.next_keyset_indices
        first_batch_for = {}
        for batch in state.restore_requests:
            ks_id = batch[0]["id"]
            first_batch_for.setdefault(ks_id, batch[0]["B_"])
        assert first_batch_for[KS1] == b_by_ks[KS1][6]
        assert first_batch_for[KS2] == b_by_ks[KS2][0]

        state.clear_requests()
        explicit_both = await fresh_wallet.restore_with_opts(
            cdk_ffi.Nut13Options(
                batch_size=4,
                max_gap=2,
                start=cdk_ffi.RestoreStart.KEYSET_INDICES(indices={KS1: 6, KS2: 5}),
            )
        )
        assert explicit_both.unspent.value == 2, explicit_both
        assert explicit_both.next_keyset_indices == {KS1: 7, KS2: 6}, explicit_both.next_keyset_indices
        first_batch_for = {}
        for batch in state.restore_requests:
            ks_id = batch[0]["id"]
            first_batch_for.setdefault(ks_id, batch[0]["B_"])
        assert first_batch_for[KS1] == b_by_ks[KS1][6]
        assert first_batch_for[KS2] == b_by_ks[KS2][5]

        # 5) Interruption mid-scan raises FfiError.RestoreInterrupted with partial progress,
        #    and passing err.partial.next_keyset_indices into KEYSET_INDICES resumes cleanly.
        interrupted_wallet = make_wallet(mint_url)
        state.clear_requests()
        state.fail_on_b_ = {b_by_ks[KS1][4]}  # KS1 batch [4..8) fails after [0..4) succeeds!
        try:
            await interrupted_wallet.restore_with_opts(
                cdk_ffi.Nut13Options(
                    batch_size=4,
                    max_gap=2,
                    start=cdk_ffi.RestoreStart.FROM_ZERO(),
                )
            )
            raise AssertionError("Expected RestoreInterrupted error")
        except cdk_ffi.FfiError.RestoreInterrupted as err:
            partial = err.partial
            assert partial.unspent.value >= 1, partial
            assert partial.next_keyset_indices[KS1] == 4, partial.next_keyset_indices
            assert set(partial.next_keyset_indices.keys()) == {KS1, KS2}
            resume_indices = partial.next_keyset_indices

        state.fail_on_b_.clear()
        state.clear_requests()
        resumed = await interrupted_wallet.restore_with_opts(
            cdk_ffi.Nut13Options(
                batch_size=4,
                max_gap=2,
                start=cdk_ffi.RestoreStart.KEYSET_INDICES(indices=resume_indices),
            )
        )
        assert resumed.next_keyset_indices == {KS1: 7, KS2: 6}, resumed.next_keyset_indices
        assert (await interrupted_wallet.total_balance()).value == 3
        first_batch_for = {}
        for batch in state.restore_requests:
            ks_id = batch[0]["id"]
            first_batch_for.setdefault(ks_id, batch[0]["B_"])
        assert first_batch_for[KS1] == b_by_ks[KS1][4], "KS1 must resume at batch 4"

        # 6) Validation and error cases.
        state.clear_requests()
        error_cases = [
            (
                "duplicate case-variant keyset IDs",
                cdk_ffi.Nut13Options(
                    batch_size=4,
                    max_gap=1,
                    start=cdk_ffi.RestoreStart.KEYSET_INDICES(
                        indices={KS1.lower(): 1, KS1.upper(): 2}
                    ),
                ),
                "Duplicate keyset ID",
            ),
            (
                "invalid keyset ID",
                cdk_ffi.Nut13Options(
                    batch_size=4,
                    max_gap=1,
                    start=cdk_ffi.RestoreStart.KEYSET_INDICES(indices={"not-hex": 1}),
                ),
                "Invalid keyset ID",
            ),
            (
                "u32::MAX start index",
                cdk_ffi.Nut13Options(
                    batch_size=4,
                    max_gap=1,
                    start=cdk_ffi.RestoreStart.KEYSET_INDICES(indices={KS1: 2**32 - 1}),
                ),
                "u32::MAX",
            ),
            (
                "unknown keyset ID",
                cdk_ffi.Nut13Options(
                    batch_size=4,
                    max_gap=1,
                    start=cdk_ffi.RestoreStart.KEYSET_INDICES(
                        indices={"00ffffffffffffff": 1}
                    ),
                ),
                "00ffffffffffffff",
            ),
            (
                "other-unit keyset ID",
                cdk_ffi.Nut13Options(
                    batch_size=4,
                    max_gap=1,
                    start=cdk_ffi.RestoreStart.KEYSET_INDICES(indices={KS_USD: 1}),
                ),
                KS_USD,
            ),
            (
                "zero batch_size",
                cdk_ffi.Nut13Options(batch_size=0, max_gap=1),
                "batch_size",
            ),
            (
                "zero max_gap",
                cdk_ffi.Nut13Options(batch_size=1, max_gap=0),
                "max_gap",
            ),
        ]
        for label, opts, expected_substr in error_cases:
            try:
                await wallet.restore_with_opts(opts)
            except cdk_ffi.FfiError as err:
                assert expected_substr in str(err), f"{label}: {expected_substr!r} not in {err}"
            else:
                raise AssertionError(f"{label} should have failed")
        assert state.restore_requests == [], "Validation failures must not call /v1/restore"

        print("✓ All FFI restore_with_opts tests passed")
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    asyncio.run(test_restore_with_all_start_variants_and_interruption())

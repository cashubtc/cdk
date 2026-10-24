# BIP353 optional LNURL regression

The production change is in `cdk::wallet::bip321`. Both v0.18.2 (`9f6d3a37`)
and main (`87c2bf97`) use bitcoin-payment-instructions 0.7.1 and reproduce the
reported native exception. Its top-level parser splits the scheme before `:`,
but compares that component with `bitcoin:`. Its intended concrete-URI branch
therefore never runs: a bech32 LNURL anywhere in a bitcoin URI overrides its
other instructions and reaches CDK's DummyHrnResolver.

The local adapter routes concrete bitcoin URIs through the dependency's existing
already-resolved HRN result path. The internal placeholder name performs no
network access, is discarded on conversion, and confers no authentication. The
complete original URI is parsed, with no parameter removal or rewriting.
Standalone inputs retain their original parser. The adapter can be removed when
the dependency provides a corrected concrete-instruction entry point.

[BIP321 forward compatibility](https://github.com/bitcoin/bips/blob/master/bip-0321.mediawiki#forward-compatibility)
allows unsupported optional parameters to be ignored; unknown `req-` parameters
invalidate the entire URI. `lnurl` is unsupported by this concrete parser, while
LN-address support remains available through its existing wallet API.
[BIP353 resolution](https://github.com/bitcoin/bips/blob/master/bip-0353.mediawiki#resolution)
requires local DNSSEC validation and rejection of multiple bitcoin TXT records.
Those operations remain before URI parsing. Mint errors remain after parsing and
are not relabeled as DNS/recipient errors. Resolver preference is unchanged:
BOLT12-capable mints try BIP353, falling back only on `Bip353Resolve`; other mints
use LN-address directly.

Affected Rust APIs are `parse_payment_instruction`,
`resolve_bip353_payment_instruction`, `Wallet::melt_bip353_quote`,
`Wallet::melt_human_readable_quote`, and its `melt_human_readable` alias. Their
Wallet-trait and UniFFI wrappers inherit the change without signature changes.

## Rust validation

From the repository root (the pinned Nix development shell is preferred):

```sh
cargo test --locked -p cdk --no-default-features --features wallet,bip353 --lib
cargo clippy --locked -p cdk --no-default-features --features wallet,bip353 --lib -- -D warnings
cargo check --locked -p cdk --no-default-features --features wallet
cargo fmt --all -- --check
```

The Rust tests cover BOLT12-only, BOLT12 plus LNURL, all three methods, an
unreachable LNURL endpoint, unsupported standalone/URI LNURL-only inputs,
malformed offers, unknown required parameters (including required LNURL), amount
and network validation, conflicting TXT records, mint error propagation, and
existing resolver preferences. Mock connectors supply controlled TXT records;
they do **not** establish DNSSEC validity. That is tested separately below.

## Native binding validation (macOS)

Requires a C compiler and a Python interpreter that permits
`DYLD_INSERT_LIBRARIES` (for example Homebrew, Nix, or Conda Python, rather than
the SIP-protected `/usr/bin/python3`). Run from the repository root:

```sh
export CARGO_TARGET_DIR="$PWD/target/bip353"
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_DEV_INCREMENTAL=false
cargo build --locked -p cdk-ffi --no-default-features --features bip353 --lib --bin uniffi-bindgen
"$CARGO_TARGET_DIR/debug/uniffi-bindgen" generate \
  --library "$CARGO_TARGET_DIR/debug/libcdk_ffi.dylib" \
  --language python --out-dir "$CARGO_TARGET_DIR/bindings"
python3 crates/cdk-ffi/tests/bip353/run.py \
  --bindings "$CARGO_TARGET_DIR/bindings" \
  --library "$CARGO_TARGET_DIR/debug/libcdk_ffi.dylib"
```

This builds and loads an actual native library through generated UniFFI Python
bindings. It tests the controlled mixed-method parsing matrix, resolves a signed
BIP353 fixture through the production TCP DNS proof builder and root-anchored
validator, and creates a BOLT12 quote against a localhost stub mint. Separate
child processes test a mint error, corrupted signed TXT data, expired proofs,
and a genuinely signed record with an invalid offer. No melt/payment endpoint
is implemented or invoked. No public recipient is contacted.

`interpose.c` is a test-only macOS shim, never linked into CDK. It redirects only
the production Google DNS TCP destinations to the local fixture server, fixes
wall time within the historical signatures' validity window, and rejects other
remote connections. Monotonic time is unaffected. The parent verifies DNS replay
was exercised, the expected number of mint requests occurred, and no remote
recipient connection was attempted. It does not replace the parser, DNSSEC
validator, quote implementation, or FFI exceptions.

The committed DNS wire transcripts were captured from Google DNS on 2026-10-18:

- `twelve.json`: `test@twelve.cash`, on-chain + BOLT12 + LNURL. This reproduces
  the original exception with `--baseline`. After the fix, its offer is still
  rejected as `InvalidBolt12(InvalidSemantics(InvalidAmount))`: it encodes an
  explicit zero amount. [BOLT12 offer requirements](https://github.com/lightning/bolts/blob/master/12-offer-encoding.md#requirements-1)
  prohibit responding to such offers. Do not weaken this check to make this
  particular public record pass.
- `matt.json`: `matt@mattcorallo.com`, on-chain + a valid BOLT12 offer. This is
  the genuine DNSSEC-to-stub-mint success fixture. Its unrelated TXT record is
  ignored and the split TXT strings are reconstructed by the real resolver.

Both transcripts include the DNSKEY/DS/RRSIG chain to the real DNS root. Their
fixed test time is `1792348800` (2026-10-18 18:40 UTC). Expiry testing advances that
clock by 30 days; signature-failure testing changes signed TXT RDATA without
changing its signatures. Normal runs are entirely offline except localhost.
They must not be updated by changing signed RDATA to create desired instructions.

For pre-fix reproduction, build the **unmodified** v0.18.2 tag or main
`87c2bf97` in a separate checkout using the same build/bindgen commands. Run this
checkout's `run.py` with `--baseline`, pointing `--bindings` and `--library` at
those baseline artifacts. The signed twelve fixture must yield
`HrnResolutionError("LNURL resolution not supported")`, and the mint must receive
zero requests. Validation here used Rust 1.99.0 for both baseline builds and the
fix; v0.18.2's own toolchain pin is 1.97.1.

No extra CA-loading features are needed for these tests: DNS uses TCP and the
stub mint uses plain HTTP. This build enables only `cdk-ffi/bip353`, rather than
the default Nostr feature set; it is a test artifact, not a release artifact.
Passing these tests does not establish HTTPS CA loading for shipped binaries,
mobile packaging, a working real LNURL service, or an actual Lightning payment.
The native socket/clock replay is currently macOS-only and manual; the Rust
regressions run with the ordinary wallet library tests.

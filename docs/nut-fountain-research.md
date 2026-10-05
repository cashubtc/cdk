# nut-fountain and CDK animated QR integration

Inspected 2026-10-05, upstream tag `v0.1.0-alpha.0` at commit [`55fe48b20627a19d3152a16d628f4fb98731a1f4`](https://github.com/Egge21M/nut-fountain/commit/55fe48b20627a19d3152a16d628f4fb98731a1f4). The implementation is pinned to this tagged revision. Its TypeScript core tests were run during the port.

## Package status

This repository supplies a **TypeScript package, not a Rust crate**. The library is in `packages/nut-fountain`, named `nut-fountain`, version `0.1.0-alpha.0`, with an MIT license and public package configuration. The wire format remains experimental. It builds ESM and TypeScript declarations. Runtime dependencies are `@cashu/cashu-ts` 4.11.0, `cborg` 4.3.2, and `@noble/hashes` 2.4.0. `@gandlaf21/bc-ur` 1.1.12 is only a development dependency for interoperability fixtures. [Package manifest](https://github.com/Egge21M/nut-fountain/blob/55fe48b20627a19d3152a16d628f4fb98731a1f4/packages/nut-fountain/package.json), [library README](https://github.com/Egge21M/nut-fountain/blob/55fe48b20627a19d3152a16d628f4fb98731a1f4/packages/nut-fountain/README.md).

## Code map and APIs

| Location | Responsibility |
| --- | --- |
| `packages/nut-fountain/src/core.ts` | `FountainEncoder(message, {fragmentSize?})`, `nextFrame()`, `fragmentCount`; `FountainDecoder.receive(frame)`, `reset()`, `isComplete`, `result`, `independentFrames`, `fragmentCount`, `progress` |
| `packages/nut-fountain/src/internal/core/` | Binary wire framing and deterministic equation selection |
| `packages/nut-fountain/src/internal/fountain.ts` | Shared GF(2) Gaussian-elimination solver |
| `packages/nut-fountain/src/ur.ts` | `UrDecoder`: inbound complete `ur:bytes` strings, single-part or multipart |
| `packages/nut-fountain/src/internal/ur/` | Bytewords and MUR fragment chooser |
| `packages/nut-fountain/src/auto.ts` | `AutoDecoder`: route scanned raw bytes or UR strings to one decoder per transfer |
| `packages/nut-fountain/src/cashu.ts` | `tokenToBytes`, `bytesToToken`, `bytesToTokenString` |
| `apps/playground/src/qr.ts`, `useCamera.ts`, `App.tsx` | Separate QR rendering, scanning, camera, and animated sending UI |

Sources: [core](https://github.com/Egge21M/nut-fountain/blob/55fe48b20627a19d3152a16d628f4fb98731a1f4/packages/nut-fountain/src/core.ts), [UR](https://github.com/Egge21M/nut-fountain/blob/55fe48b20627a19d3152a16d628f4fb98731a1f4/packages/nut-fountain/src/ur.ts), [auto](https://github.com/Egge21M/nut-fountain/blob/55fe48b20627a19d3152a16d628f4fb98731a1f4/packages/nut-fountain/src/auto.ts), [Cashu](https://github.com/Egge21M/nut-fountain/blob/55fe48b20627a19d3152a16d628f4fb98731a1f4/packages/nut-fountain/src/cashu.ts), [playground](https://github.com/Egge21M/nut-fountain/tree/55fe48b20627a19d3152a16d628f4fb98731a1f4/apps/playground/src).

The core transports arbitrary bytes. The Cashu adapter maps `cashuB` or token objects to `crawB` + CBOR. `cashuA` is unsupported. Imported `nut-fountain/core` avoids Cashu and UR dependencies. QR images and camera capture are outside the codec library. [Library README](https://github.com/Egge21M/nut-fountain/blob/55fe48b20627a19d3152a16d628f4fb98731a1f4/packages/nut-fountain/README.md).

## Wire compatibility

The new transport is experimental dense random linear fountain coding over GF(2), **not the UR fountain algorithm or UR output**. It emits systematic source fragments first, then deterministic XOR repair frames using Mulberry32-derived selection. Binary frames begin `NF`, carry version 1, big-endian sequence/count/message-length/checksum fields, raw fragment bytes, and a frame checksum: 24 bytes overhead. Fragment size defaults to 128 bytes, maximum 4096; maximum 256 source fragments and 1 MiB message. The protocol includes a standalone known-answer frame vector. [Wire specification](https://github.com/Egge21M/nut-fountain/blob/55fe48b20627a19d3152a16d628f4fb98731a1f4/packages/nut-fountain/docs/protocol.md).

UR support is a separate reader: Bytewords/CBOR framing, SHA-256-seeded Xoshiro256**, Walker-Vose degree sampling, and MUR shuffle. It shares the Gaussian solver with the binary codec. Supported Cashu conventions are `ur:bytes` wrapping a CBOR byte string containing UTF-8 `cashuB` or raw `crawB`; there is **no UR encoder**. Those tested conventions do not establish compatibility with every NUT-16 wallet. [UR adapter](https://github.com/Egge21M/nut-fountain/blob/55fe48b20627a19d3152a16d628f4fb98731a1f4/packages/nut-fountain/src/ur.ts), [wire specification](https://github.com/Egge21M/nut-fountain/blob/55fe48b20627a19d3152a16d628f4fb98731a1f4/packages/nut-fountain/docs/protocol.md).

## CDK integration implications (inferences)

- Native CDK integration needs a Rust implementation of the binary codec/spec, rather than adding this repository as a Cargo dependency.
- Keep the byte transport separate from Cashu token serialization and application QR rendering. Existing UR remains a distinct format; changing an existing UR encoder to emit `NF` would break its contract.
- Expose byte frames to consumers: converting arbitrary binary frames through a text scanner interface would lose the intended representation.
- Use cross-language fixtures against this pinned version, especially source-loss recovery, repair-only recovery, duplicates, CRC/padding checks, and exact wrapping arithmetic. The upstream tests include core, Cashu, auto routing, UR interoperability, and independent UR vectors; they are useful starting references, not validation of a future Rust implementation.
- The binary codec is MIT-licensed; CDK retains its notice in `crates/cashu/NOTICE-nut-fountain`. The separate upstream `NOTICE.md` covers Blockchain Commons UR material. [Third-party notice](https://github.com/Egge21M/nut-fountain/blob/55fe48b20627a19d3152a16d628f4fb98731a1f4/packages/nut-fountain/NOTICE.md).
- Do not infer improved scan performance from the earlier proof of concept: the README explicitly says its measurements do not establish this implementation's performance. [Library README](https://github.com/Egge21M/nut-fountain/blob/55fe48b20627a19d3152a16d628f4fb98731a1f4/packages/nut-fountain/README.md).

## Current CDK implementation

Inspected local CDK commit `abe3827340f1d436f6366410e5c35ae1c40149c6`.

| Location | Responsibility |
| --- | --- |
| [Workspace manifest](../Cargo.toml) and [cashu manifest](../crates/cashu/Cargo.toml) | External `ur = "0.5.2"` dependency, also resolved to 0.5.2 in Cargo.lock |
| [NUT-16 module](../crates/cashu/src/nuts/nut16.rs) | `TokenUrEncoder`, `TokenUrDecoder`, and `Token::ur_encoder`; wraps `ur::Encoder` / `ur::Decoder` |
| [NUT exports](../crates/cashu/src/nuts/mod.rs), [common exports](../crates/cdk-common/src/lib.rs), [SDK exports](../crates/cdk/src/lib.rs) | Expose the types through `cashu::nuts` and `cdk::nuts` |
| [FFI types](../crates/cdk-ffi/src/types/ur.rs) and [FFI token](../crates/cdk-ffi/src/token.rs) | UniFFI encoder/decoder objects and token encoder factory |
| [CLI send command](../crates/cdk-cli/src/sub_commands/send.rs) | `send --animate`; `display_animated_qr` renders uppercase UR with `qrcode`, 100-byte fragment budget, 250 ms frame interval |
| [Example](../crates/cashu/examples/nut16_animated_qr.rs) | Encoding, reconstruction, and dropped-frame recovery without a graphical UI |
| [Decoder fuzz target](../fuzz/fuzz_targets/fuzz_nut16_ur_decode.rs) and [multipart fuzz target](../fuzz/fuzz_targets/fuzz_nut16_ur_multipart.rs) | Existing UR parsing and round-trip fuzz coverage |

There is no dedicated CDK animated-QR crate. The Cashu-facing layer belongs to the existing `cashu` crate; generic UR/fountain operations come from the external `ur` crate. No separately named MUR implementation was found in the checkout. Multipart UR is handled through `ur::Decoder`.

Current NUT-16 encoding normalizes V3 tokens to V4, serializes `cashuB` text as a CBOR byte string, and emits `ur:bytes` strings. Its decoder unwraps CBOR and parses UTF-8 token text. The UR decoder does not accept binary `NF` frames or raw `crawB` payloads inside UR; the added `TokenFountainDecoder` handles the binary `NF` format separately. [NUT-16 implementation](../crates/cashu/src/nuts/nut16.rs).

The binary Cashu serialization already exists: `Token::to_raw_bytes()` and `TokenV4::to_raw_bytes()` produce `crawB` plus CBOR, and `TryFrom<&Vec<u8>> for Token` reads it. `Token::to_raw_bytes()` rejects V3, so a future wrapper must either normalize V3 explicitly or document V4-only input. [Token implementation](../crates/cashu/src/nuts/nut00/token.rs).

Implemented boundary: the generic byte codec and token adapters live in `cashu::nuts::nut16::fountain`, with token types also re-exported through `cashu::nuts` and `cdk::nuts`. Matching UniFFI types live in `crates/cdk-ffi/src/types/fountain.rs`. Existing UR APIs are unchanged. Binary frames use `Vec<u8>` / `&[u8]`. Automatic format routing and changes to the CLI renderer are outside this addition.

Interoperability fixtures in `crates/cashu/src/nuts/nut16/fountain/testdata/fountain.json` are generated directly from the pinned TypeScript implementation; the adjacent Bun script reproduces them. Rust tests compare exact emitted frames, accepted equation counts, repair-only recovery, and high sequence numbers. Additional tests cover malformed frames, transfer switching, checksum/padding failures, bounds, and token conversion. See `crates/cashu/examples/nut16_binary_fountain.rs` for usage.

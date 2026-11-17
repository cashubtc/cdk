# Cashu cryptography bindings

`cashu-ffi` exposes the existing `cashu` cryptographic primitives through UniFFI.
It contains no wallet state, networking, storage, or consumer-specific output
strategy. It can be built as a shared library, static library, or Rust library.

The interface covers random secret generation, message blinding/unblinding,
DLEQ verification, deterministic derivation, and explicit-slot public-key
blinding. Public keys and scalars use hex; secrets use bytes. Secret-bearing
records redact their `Debug` output. Returned secret material is visible to the
calling language; this is not a keystore.

The core's contracts are preserved:

- NUT-13 takes a 64-byte seed and a `u32` counter, with the core's keyset-specific
  limits. Broader consumer input domains belong in a compatibility adapter.
- Public-key parsing accepts the core's compressed representation. Consumer
  conveniences such as x-only key normalization belong in the adapter.
- DLEQ verification rejects missing proofs. A consumer that allows missing
  proofs must explicitly choose when to invoke verification.
- NUT-28 callers supply the canonical slot and ephemeral secret. Output grouping,
  locking-tag assembly, and ephemeral-key sharing are the consumer's responsibility.

This crate adapts representations and calls `cashu`; it does not implement a
second derivation algorithm or add consumer requirements to the core. A binding
consumer may batch calls in native code without routing each primitive through
the foreign-language runtime.

```sh
cargo test -p cashu-ffi --locked
cargo clippy -p cashu-ffi --all-targets --locked -- -D warnings
cargo build -p cashu-ffi --locked
```

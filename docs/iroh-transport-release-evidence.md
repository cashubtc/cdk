# Iroh transport hardening and release evidence

Evidence date: 2026-08-01.

This record covers `iroh = 1.0.3`, `iroh-tickets = 1.0.0`, and CDK's declared
Rust 1.91 MSRV. It complements the milestone-by-milestone evidence in
`docs/iroh-transport-implementation-plan.md` and the redacted Nocean drill in
`docs/federated-cashu-nocean-multi-operator-drill.md`.

## Reproducible transport checks

The complete standalone suite is:

```console
cargo test -p cdk-iroh --offline -- --test-threads=1
cargo clippy -p cdk-iroh --all-targets --offline -- -D warnings
cargo fmt -p cdk-iroh -- --check
```

It passes three unit tests, three generic bridge tests, thirteen transport
conformance tests, and doc tests. The coverage includes:

- direct static-ticket requests and a locally hosted current-Iroh relay;
- relay-only bootstrap, migration of the same authenticated connection to a
  selected direct path, and continued requests after relay loss;
- an unchanged endpoint identity moving to a refreshed direct address, plus
  fail-closed offline discovery and stale-ticket behavior;
- malformed HTTP, wrong ALPN, unknown IDs, authority mismatch, oversized
  headers, request bodies, and streamed responses;
- bounded partial and slow streams, connection churn, global/per-peer
  connection admission, global/per-connection stream admission, and bounded
  shutdown with active HTTP and WebSocket streams;
- raw, JSON, form, auth, status/error body, and WebSocket behavior;
- 24 concurrent short requests over one pooled connection while the bridge
  suite separately keeps a WebSocket open; and
- an intentionally ambiguous non-idempotent POST: the server receives the
  body once, the connection closes before response headers, the caller gets an
  error, and the transport performs no replay.

The relay test uses Iroh 1.0.3's own test relay server and TLS test support.
Those extra server dependencies are dev-only; the production Iroh feature set
remains `tls-ring` without Iroh's default metrics, port-mapping, or fast Apple
datapath features.

## Measurements

One debug-build local relay run reported:

| Sample | Result |
|---|---:|
| Relay-ticket connection setup | 5,144 microseconds |
| First request during relay bootstrap | 1,705 microseconds |
| Warm request on the migrated direct connection after relay loss | 657 microseconds |
| Relay server traffic during bootstrap and the first request | 22,000 bytes |
| Process file descriptors before / peak / after | 10 / 21 / 15 |

These are reproducible engineering samples, not release-mode benchmarks or
latency SLOs. The relay-byte figure includes transport setup and control
traffic, not only application payload. File-descriptor snapshots include the
test runtime and relay fixture; separate shutdown tests are the lifecycle
assertion and observe zero active CDK connection/stream tasks.

The one-server Nocean threshold-3-of-5 drill completed both dealer-free BLS
and FROST DKG in 21 seconds. Sampled peak DKG RSS was 295,004 KiB across all
five independently operated processes and 59,544 KiB for one process. The
complete deployment, restart, outage/catch-up, and backup/restore run took 938
seconds including a clean release build and binary transfers.

Production metrics expose the fixed discovery class, connection attempts and
failures, initial direct/relay/unknown path outcomes, reconnects, pool hits,
active connections and streams, request/status counts, a saturating aggregate
request-duration counter, admission rejections, and timeout count. They have
no peer, address, ticket, path, or credential labels.

## Compatibility matrix

The following checks pass:

```console
cargo check -p cdk --no-default-features --features wallet,iroh,tor --offline
cargo check -p cdk-ffi --offline
cargo check -p cdk-mintd --features federation,iroh --offline
```

The feature-disabled native wallet dependency tree contains no `iroh` or
`iroh-tickets` package. An official Linux Rust 1.91 environment passes the
locked production `cdk-iroh` check and compiles every `cdk-iroh` test target.
With only the Rust 1.91 `wasm32-unknown-unknown` standard library and Clang
added, this feature-disabled wallet check also passes:

```console
cargo check -p cdk --target wasm32-unknown-unknown \
  --no-default-features --features wallet --locked
```

The broader regression record remains:

- 335 Iroh-enabled wallet library tests;
- 542 default CDK library tests;
- 222 federation+Iroh mintd library tests;
- 576 federation+Bark+Iroh Axum library tests, with three manual benchmarks
  ignored; and
- the successful all-Iroh and mixed HTTP/Iroh federation suites described in
  the implementation plan.

## HTTPS/Iroh semantic comparison

The shared connector conformance runner uses the same `HttpClient<T>` route
mapping against HTTPS dispatch and Iroh. It compares status codes, structured
Cashu error bodies, authentication headers, JSON/form/raw bodies, NUT-19
retries, and NUT-17 WebSockets. Both listeners receive the same final Axum
router and middleware.

Iroh bridge version 1 does not advertise `Accept-Encoding`, so Axum returns an
ordinary uncompressed representation on that path. This avoids returning
compressed bytes to `RawResponse`, which intentionally carries only status
and body. HTTPS may negotiate compression through its existing HTTP client;
the decoded application body is the same. Compression negotiation itself is
therefore transport-specific and is not claimed to be byte-identical.

## Dependency advisory review

The official RustSec
[`cargo-audit`](https://github.com/RustSec/rustsec/tree/main/cargo-audit)
0.22.2 scan loaded the current advisory database and scanned the complete
workspace lockfile. None of the four packages reported as vulnerable occurs
in the normal `cdk-iroh`, wallet+Iroh, or mintd federation+Iroh dependency
graphs.

The complete lockfile does contain four existing vulnerabilities in optional
or otherwise inactive non-Iroh graphs:

- [`RUSTSEC-2026-0216`](https://rustsec.org/advisories/RUSTSEC-2026-0216)
  and [`RUSTSEC-2026-0219`](https://rustsec.org/advisories/RUSTSEC-2026-0219)
  affect `nostr 0.44.4`;
- [`RUSTSEC-2026-0224`](https://rustsec.org/advisories/RUSTSEC-2026-0224)
  affects `nostr-relay-pool 0.44.1`; and
- [`RUSTSEC-2023-0071`](https://rustsec.org/advisories/RUSTSEC-2023-0071)
  affects `rsa 0.9.10` and has no fixed release.

The three Nostr advisories have fixed versions and require a separate
workspace dependency update and regression review. Cargo-audit also reports
allowed maintenance/unsound/yanked warnings. This Iroh milestone neither
suppresses those results nor expands scope into unrelated optional features;
the workspace release process must track them explicitly.

## Security review

No release-blocking issue was found in the Iroh transport scope of the
Milestone 5 review. The separate workspace advisory findings above remain
visible release follow-up.

- `IrohStream` contains no unsafe code and delegates bounded Tokio I/O to the
  authenticated Iroh send/receive halves. Hyper framing, request bodies,
  response bodies, timeouts, and admission are bounded at named layers.
- The endpoint ID in an `iroh://` URL remains the TLS authentication target.
  Static tickets and discovery can change reachability but not identity.
  Wrong URL/ticket IDs, missing discovery, wrong ALPN, and listener-ID/key
  mismatch fail closed.
- Persistent endpoint keys use protected create-new files. Missing, corrupt,
  weak-permission, symlinked, or identity-mismatched files/configurations are
  rejected before runtime traffic. Keys, tickets, addresses, auth values, and
  payloads are excluded from normal logs and `Debug` output.
- URL parsing accepts only explicit supported schemes, binds HTTP authority to
  the authenticated endpoint, rejects transport-owned and unsafe hop-by-hop
  headers, and never falls back across transports.
- The pool serializes only a duplicate dial for the same endpoint/ALPN key;
  peer dials and request streams remain concurrent. Pool, connection, peer,
  stream, header, body, and shutdown limits remain finite.
- Iroh transport authentication does not grant federation authorization.
  Signed member envelopes, replay fences, DKG/FROST recipient protection, and
  consensus/checkpoint validation remain unchanged above the transport.
- A POST that may have reached its handler is returned as an ambiguous
  transport error and is never replayed below the existing application layer.

## Accurately unsupported or unverified combinations

- Iroh is native-only in this release. Browser/WASM builds must leave the
  feature disabled; Android and iOS need target-specific compile and lifecycle
  evidence before being declared supported.
- The Nocean drill intentionally used one affordable server with five
  independent directories, processes, tmux sessions, persistent identities,
  and public DNS subdomains. It proves operator/process isolation and protocol
  concurrency, not five-machine network or fault-domain independence. A true
  multi-host drill remains future release evidence when infrastructure is
  available.
- The Nocean wallet check was read-only because the Signet payment backend was
  not funded. Paid state transitions are covered by local federation suites,
  not attributed to that deployment.
- Tor and Iroh compile together, but Tor continues to apply to the ordinary
  HTTP(S) transport. Iroh-over-Tor is not implemented.
- The local custom relay uses a self-signed test certificate. Nocean exercises
  N0 production discovery configuration and reachability but does not claim
  which path class Iroh selected; an operator-hosted public relay must provide
  its own normally trusted TLS deployment.
- There is no automatic HTTPS/Iroh fallback, transport-level POST replay, or
  0-RTT request delivery.

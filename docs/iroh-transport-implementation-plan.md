# Iroh transport for CDK: implementation plan

> **Status — implementation in progress 2026-08-01:** This document is the
> implementation and review contract for adding Iroh beside HTTP(S).
> Milestones 0 through 3 are complete. It does not authorize an
> endpoint-by-endpoint reimplementation of the Cashu API. If the generic
> HTTP/WebSocket-over-Iroh bridge described below proves infeasible, stop and
> amend this plan before widening the integration surface.

## Outcome

CDK wallets and federation members can use either ordinary HTTP(S) URLs or
Iroh URLs:

```text
https://mint.example
iroh://<endpoint-id>
```

The URL scheme selects the transport for each request. An Iroh-enabled client
remains an HTTP(S) client too, so one wallet can talk to HTTPS and Iroh mints
and one federation can contain HTTPS and Iroh members at the same time. There
is no automatic fallback from one scheme to the other.

The implementation lives primarily in a new `crates/cdk-iroh` crate. It plugs
into the existing `cdk_http_client::Transport` boundary and forwards inbound
Iroh requests to the same fully assembled Axum router used by the HTTP server.
The existing wallet `HttpClient<T>` remains the one mapping of Cashu routes.
The existing federation peer client remains the one mapping of private
federation routes, including DKG, FROST, consensus, checkpoint, catch-up, and
Bark traffic.

The target shape is:

```text
MintUrl
  |
  v
HttpClient<IrohTransport<Async>>
  |-- http / https / ws / wss --> existing Async transport
  `-- iroh ---------------------> cached Iroh connection
                                      |
                                      v
                              HTTP/1.1 or WebSocket
                              on an Iroh QUIC stream
                                      |
                    +-----------------+-----------------+
                    |                                   |
                 HTTPS TCP                         Iroh endpoint
                    |                                   |
                    +---------- same Axum Router -------+
```

## Complexity assessment

This is a medium-to-high complexity transport addition, but it should have a
small product-code integration surface.

- The generic REST client integration is medium complexity because
  `cdk_http_client::Transport` already exists and the wallet already maps every
  Cashu endpoint through it.
- The server bridge and native NUT-17 WebSocket upgrade are the main technical
  risks. They must be proven with a small end-to-end spike before integration.
- Mixed-transport federation is medium complexity once one shared transport is
  injected at the federation runtime boundary. The route mapping itself must
  not change.
- Operational identity persistence, discovery, resource bounds, shutdown, and
  adverse-network testing make the total effort larger than a single-file
  patch even though normal Cashu handlers should not change.
- Iroh's current stable release has an MSRV conflict with CDK that must be
  resolved before implementation starts.

The expected review shape is one substantial new crate plus small, mechanical
changes at a few named seams. Endpoint handlers, mint business logic, wallet
sagas, federation state machines, database schemas, and Cashu wire types are
outside the permitted change surface.

## Goals

- Keep HTTP(S) behavior unchanged and available alongside Iroh.
- Make `iroh://<endpoint-id>` work transparently with the ordinary wallet
  builder and every native wallet-to-mint Cashu operation.
- Carry REST, authentication headers, status codes, errors, bounded responses,
  and NUT-17 subscriptions without a second Cashu API implementation.
- Let a mint expose HTTP(S), Iroh, or both, with at least one listener required.
- Let every federation member independently advertise an HTTPS or Iroh private
  federation URL.
- Support all-Iroh federations with no HTTPS certificates or DNS names.
- Support mixed federations in which an HTTPS member calls an Iroh member and
  an Iroh member calls an HTTPS member.
- Reuse one persistent Iroh endpoint per mint for inbound and outbound traffic.
- Preserve existing federation request signatures, replay protection,
  idempotency, admission limits, and DKG/FROST state machines.
- Keep builds that do not enable the `iroh` feature free of Iroh dependencies
  and behavior changes.
- Make future Axum routes available over Iroh automatically.

## Non-goals

- Replacing HTTP(S), Tor, Axum, or `cdk-http-client`.
- Defining new Cashu request or response payloads.
- Adding an Iroh alternative for every method in `MintConnector` or every
  federation endpoint.
- Encoding relay URLs, socket addresses, or discovery policy in `MintUrl`.
- Silently falling back from Iroh to HTTPS, or from HTTPS to Iroh.
- Treating Iroh connectivity as consensus, authorization, or replay
  protection.
- Removing application-layer encryption from DKG or FROST private messages.
- Routing unrelated third-party protocols such as LNURL callbacks or an
  external OIDC provider through Iroh. Their own URLs remain authoritative.
- Browser/WASM support in version 1. Native mobile targets may be enabled only
  after the selected Iroh version and CDK bindings compile and pass lifecycle
  tests on those targets.
- Transport redundancy under one mint URL. If CDK later needs ordered fallback
  URLs, that is a separate URL/configuration design.

## Thin-surface review contract

The following are hard implementation constraints:

1. `crates/cdk-iroh` owns all Iroh endpoint, connection, discovery, stream,
   HTTP bridge, WebSocket bridge, error, and metrics code.
2. `crates/cdk/src/wallet/mint_connector/http_client.rs` remains the only
   wallet-side Cashu REST mapping. No Iroh-specific route is added there.
3. `crates/cdk-axum` continues to build one Axum router. No second Iroh router
   and no copied handler table are allowed.
4. `FederationRestPeerClient` remains the only federation route mapping. DKG,
   FROST, AlephBFT, checkpoints, catch-up, wallet liveness, and Bark do not get
   Iroh-specific clients.
5. Existing source changes are limited to dependency/feature declarations and
   these integration seams:
   - the default wallet transport alias/factory;
   - preservation of the `iroh` scheme for NUT-17 and one generic
     WebSocket-over-caller-stream constructor;
   - mint listener construction, shared endpoint injection, and shutdown;
   - federation default-transport construction; and
   - URL/security policy validation for `iroh` as a secure transport.
6. No wallet saga, mint operation, quote, proof, signing, consensus, DKG,
   FROST, Bark, or persistence behavior may branch on `iroh`.
7. An integration test with a route known only to a test Axum router must prove
   that an unlisted future route works over Iroh. This is the regression test
   against endpoint duplication.
8. Any production-code change outside this list requires a written amendment
   to this document explaining why the generic boundary was insufficient.

Mechanical replacement of scattered `Async::default()` calls with a cloned
runtime-owned default transport is permitted. It must be one transport
injection refactor, not local Iroh logic in each caller.

## Current CDK boundaries to reuse

### Wallet client

`cdk/src/wallet/mint_connector/http_client.rs` is already generic over
`cdk_http_client::Transport`. All Cashu route joining, JSON serialization,
authentication token selection, OIDC transport reuse, status mapping, NUT-19
retry policy, and bounded response handling flow through that transport.

The ordinary wallet type alias currently fixes the generic client to
`transport::Async`. With `iroh` enabled, that alias should instead select the
hybrid Iroh transport. Wallet construction and public wallet APIs should not
change.

### Mint server

`cdk-axum` already produces an Axum `Router` containing `/v1`, authentication,
custom payment-method routes, `/v1/ws`, and, when enabled,
`/federation/v1`. `cdk-mintd` then adds body limits, compression, tracing, and
custom routers before starting the TCP listener.

The Iroh listener must receive a clone of that final router after all layers
and merges. Passing an earlier router would create observable differences in
limits, compression, tracing, custom routes, or authentication.

### Federation client

`cdk-axum/src/federation/peer_client.rs` is already generic over the same
`Transport` trait. Every current federation REST operation joins its target
member's `federation_api_url` and calls that transport. Scheme-aware dispatch
therefore belongs below this client.

The current direct construction of `Async::default()` in federation runtime
and operator modules must be replaced by a small centralized transport factory
or an injected cloned handle. No endpoint method should be edited merely to
support Iroh.

## Crate design: `cdk-iroh`

The new crate has the same optional transport role that Tor has today, but is
isolated more strongly in its own crate.

Suggested modules:

```text
crates/cdk-iroh/
  Cargo.toml
  README.md
  src/
    lib.rs
    address.rs
    client.rs
    config.rs
    connection_pool.rs
    endpoint.rs
    error.rs
    io.rs
    server.rs
    transport.rs
  tests/
    http_bridge.rs
    websocket_bridge.rs
    discovery.rs
    resource_limits.rs
```

The exact module split can change, but the responsibilities cannot leak into
Cashu endpoint code.

### Public types

- `IrohTransport<H = cdk_http_client::Async>` implements
  `cdk_http_client::Transport` and contains an ordinary HTTP transport plus a
  cloneable Iroh client handle.
- `IrohNode` owns one Iroh `Endpoint`, its connection pool, address book, and
  lifecycle. Mintd constructs it with a persistent key; a wallet can construct
  a client-only node with an ephemeral key retained for the node's lifetime.
- `IrohServer` accepts Iroh connections and serves a provided Axum/Tower
  service until shutdown.
- `IrohConfig` contains discovery, relay, bind, timeouts, admission limits, and
  static peer hints without depending on mint or federation configuration.
- `IrohConnectionInfo` contains the authenticated remote Iroh endpoint ID and
  is inserted into request extensions for diagnostics and future policy. It is
  not a replacement for Cashu auth or signed federation envelopes.

Every public type needs redacted `Debug`, bounded errors, documentation, and
focused unit tests. Secret keys, full endpoint tickets, request bodies, auth
headers, and peer addresses must not appear in logs or errors.

`IrohTransport::default()` must remain synchronous because the existing wallet
client requires `Transport: Default`. It therefore creates a cheap,
clone-shared lazy node whose endpoint binds on the first Iroh operation. The
mint runtime does not use that lazy random identity: it explicitly constructs
the persistent node before starting listeners or federation workers and
injects an `IrohTransport` backed by that node.

### Dependency direction

The dependency direction must remain acyclic:

```text
cdk-http-client <--- cdk-iroh <--- cdk / cdk-axum / cdk-mintd
                         |
                         `--------> iroh, hyper, tower, tokio
```

`cdk-iroh` may depend on `cdk-http-client` to implement its existing transport
trait. `cdk-http-client` must not depend on `cdk-iroh`. The only expected
`cdk-http-client` source change is a transport-neutral native helper that wraps
a caller-provided async byte stream as its existing `WsSender`/`WsReceiver`.

## URL, addressing, and discovery

### Canonical mint URL

Version 1 uses exactly:

```text
iroh://<endpoint-id>
```

`<endpoint-id>` is Iroh's canonical textual endpoint ID. A base mint URL:

- must have the `iroh` scheme;
- must contain exactly one valid endpoint ID as its host;
- must not contain user information, a port, query, or fragment; and
- may contain a normalized base path only where CDK already supports base
  paths.

`MintUrl::join_paths` continues to create `/v1/...` and
`/federation/v1/...` paths. Parsing tests must prove that normalization does
not corrupt an endpoint ID and that path joining preserves the `iroh` scheme.

Do not place an `EndpointTicket`, relay URL, IP address, or discovery query in
the mint URL. The endpoint ID is stable identity; network coordinates are
mutable reachability data.

### Discovery policy

`iroh://<endpoint-id>` is sufficient only when the client can resolve current
reachability information. Support these explicit modes:

- `n0`: use Iroh's N0 preset and its address lookup/relay defaults;
- `custom`: use an operator-configured Iroh address lookup and relay policy;
  and
- `static`: disable network lookup and seed an in-memory address book with
  standard Iroh `EndpointTicket` values.

Use `iroh-tickets::EndpointTicket` as the standard out-of-band bootstrap
format. Configuration and CLI flags may accept endpoint tickets, but the
ticket's endpoint ID must equal the corresponding `iroh://` URL. Persist only
operator-approved hints; learned relay/direct addresses are mutable cache, not
federation consensus or Cashu token identity.

This provides a domain-free self-hosted option: exchange endpoint tickets
through the operator setup channel and use static/custom relay configuration.
It does not require inventing a second mint URL syntax.

Failure to discover an endpoint must return a specific, bounded connection
error identifying only the endpoint ID fingerprint and discovery mode. It must
not trigger HTTPS fallback.

### Persistent mint identity

- Mintd generates or imports one Iroh secret key and writes it with the same
  protected, atomic secret-file rules used for other mint private material.
- Restarting a mint preserves its endpoint ID.
- A configured public or federation Iroh URL must match the derived endpoint
  ID before listeners or federation workers start.
- One process reuses one endpoint for accepting and dialing. Do not create an
  endpoint in every `Async::default()` call.
- Wallet client endpoints may be ephemeral, but live for the complete wallet
  transport/node lifetime so connections can be reused.
- Rotating a mint Iroh key is an explicit URL/roster change. It is not an
  automatic recovery action.

One endpoint can serve both `/v1` and `/federation/v1`; the route namespaces
and existing authentication separate them. Setup validation must therefore
allow a member's public and federation base URL to be the same Iroh endpoint.
Separate public/private Iroh identities can be a later isolation feature, not
a requirement for version 1.

## Transport protocol

### Use standard HTTP and WebSocket framing

Do not define a second JSON/CBOR request envelope. Use a versioned Iroh ALPN,
provisionally:

```text
cashu-cdk-http/1
```

After the Iroh connection is authenticated, open one bidirectional QUIC stream
per HTTP transaction and run an HTTP/1.1 connection on that stream. Reuse the
same Iroh QUIC connection for concurrent streams. A NUT-17 WebSocket performs
the standard HTTP upgrade and keeps its stream open.

Requests use origin-form path/query targets and an authority derived from the
endpoint ID. The server rejects an authority that names a different endpoint.
The client-side WebSocket helper accepts a prebuilt handshake request so it
does not need to reinterpret `iroh` as a DNS WebSocket scheme.

This deliberately carries familiar HTTP semantics over Iroh rather than
implementing HTTP/3 or a CDK-specific RPC protocol:

- method, path, query, headers, status, and body semantics stay identical;
- Axum routing and middleware are reused directly;
- Cashu JSON bytes are not translated by the Iroh layer;
- WebSocket JSON-RPC messages remain unchanged; and
- future routes need no transport registration.

The Milestone 0 spike must prove that Hyper HTTP/1 upgrades and Axum WebSocket
handling work over the safe Iroh send/receive stream adapter. If they do not,
the implementation must pause for design review. Polling is acceptable as a
temporary development fallback, not as the completed NUT-17 implementation.

### Client request rules

For an `iroh` URL, `IrohTransport`:

1. validates and extracts the endpoint ID;
2. preserves the path and query as the HTTP request target;
3. resolves mutable address hints according to configured discovery policy;
4. obtains or creates a cached authenticated Iroh connection for the endpoint
   ID and ALPN;
5. opens a fresh bidirectional stream;
6. sends the ordinary HTTP request, including content type and Cashu auth
   headers; and
7. maps the HTTP status/body to `RawResponse` and the existing `HttpError`
   behavior.

`http_get`, `http_get_raw`, `http_post`, `http_post_bounded`, and
`http_post_form_raw` are thin serialization helpers around one internal raw
request function. `ws_connect` uses the same connection pool and a long-lived
stream.

For `http`, `https`, `ws`, and `wss`, the hybrid transport delegates unchanged
to its inner `Async` transport. DNS TXT resolution and proxy configuration also
delegate to `Async`; proxy configuration does not silently proxy or disable
Iroh.

Unsupported schemes fail explicitly. The scheme of the request, not which
listener happens to be reachable, is authoritative.

### Retry and ambiguity

- Reuse healthy Iroh connections and evict them after terminal connection
  errors.
- A connection attempt may be retried only before request bytes are accepted
  by a stream.
- Do not automatically replay an ambiguous POST. Existing NUT-19 retry logic
  and federation idempotency own application retries.
- Disable 0-RTT for Cashu mutations, DKG, FROST, Bark, and federation
  operations in version 1.
- Preserve existing request deadlines. Connection establishment, stream open,
  header read, body read, and graceful close also need bounded deadlines.

### Server request rules

- Accept only the versioned Cashu ALPN.
- Apply global and per-connection limits before spawning stream work.
- Serve each admitted stream through the same final Axum router used by the
  TCP listener.
- Insert authenticated remote endpoint information into request extensions.
- Strip or reject malformed authority, hop-by-hop headers outside the required
  HTTP upgrade flow, oversized headers, invalid methods, and invalid paths.
- Preserve the router's request body limit, decompression, compression,
  tracing, authentication, and error response behavior.
- Bound response collection/streaming exactly as the corresponding HTTP client
  method requires; `http_post_bounded` must reject while reading, not after an
  unbounded allocation.
- Cancel stream tasks on connection close and drain them on graceful shutdown.

## Wallet integration

Add a native `iroh` feature to `cdk` and wire it to the optional `cdk-iroh`
dependency. Under that feature, the ordinary wallet `HttpClient` alias uses
`IrohTransport<Async>`. Because it is a hybrid transport, HTTPS support remains
enabled and applications do not select a different wallet type.

The wallet builder continues to call `HttpClient::new(mint_url, auth_wallet)`.
No public wallet method, FFI method, or Cashu endpoint implementation changes.
Applications opt in at build time and select Iroh at runtime only by supplying
an `iroh://` mint URL.

NUT-17 currently rewrites HTTPS to WSS and all other schemes to WS. Change this
small scheme switch so:

- `https` becomes `wss`;
- `http` becomes `ws`;
- `iroh` remains `iroh`; and
- any other scheme returns `NotSupported`.

`IrohTransport::ws_connect` then performs the standard WebSocket handshake on
an Iroh stream. Existing polling fallback stays available for transient or
unsupported streaming, but successful native Iroh deployments must use and
test the streaming path.

## Mint listener integration

Add an optional mintd Iroh configuration section containing:

- enabled/disabled;
- secret key file or protected generated-key location;
- discovery mode and custom lookup/relay settings;
- static peer endpoint tickets;
- bind addresses where applicable;
- connection, stream, request, header, and shutdown timeouts; and
- global/per-peer connection and stream admission limits.

Refactor the existing single-listener tail of mintd into a small supervised
listener set:

- HTTP(S) listener, unchanged when enabled;
- Iroh listener, using the same final router clone when enabled; and
- shared shutdown that signals, drains, and joins both.

At least one public listener must be enabled. Iroh-only operation must not
require an HTTPS certificate, TCP public bind, or DNS name. Starting both is a
normal supported configuration.

The Iroh node is constructed once before federation workers start. Its cloned
transport handle is injected into those workers, while the server half accepts
inbound streams. Listener startup fails closed if the configured URL, key,
discovery publication, or route service cannot be initialized.

## Federation and DKG integration

### Scheme-aware peer dispatch

Every outbound federation request continues to use the target member's
`federation_api_url`:

- `https://...` is delegated to the existing HTTP transport;
- `iroh://<endpoint-id>` is sent over Iroh; and
- any other production scheme is rejected.

One roster may mix schemes in any order. No federation-wide transport mode is
introduced. A member that advertises HTTPS can still dial another member's
Iroh URL using its configured local Iroh node; its existing signed federation
request authenticates the sender. A member that advertises Iroh must use the
persistent endpoint matching that URL.

The Iroh handshake authenticates the dialed receiver endpoint. Existing
`FederationSignedRequest` validation authenticates the sender, receiver,
federation, message kind, sequence, and body. Keep both layers. Do not replace
the secp256k1 federation identity with the Iroh Ed25519 endpoint identity in
version 1.

### One injected transport

Create the default federation transport once at the federation runtime or
mintd composition boundary and clone it into peer clients and background
workers. Replace direct `Async::default()` construction in DKG, sync,
AlephBFT, wallet liveness, checkpoint, payment observation, Bark, smoke, and
operator modules with that shared boundary.

This is an intentional prerequisite refactor because a production mint must
not create multiple random Iroh identities or independent connection pools.
The refactor must pass all HTTP-only federation tests before Iroh behavior is
enabled.

### Secure transport policy

Rename misleading HTTPS-only policy names such as
`RequireHttps` to `RequireSecureTransport` while federation formats are still
undeployed. The production secure set is:

```text
https
iroh
```

Plain HTTP remains allowed only for explicit loopback development policy.
Apply the same rule to multi-operator setup URLs, enrollment validation, DKG
secret-share delivery, setup coordinator calls, and mintd CLI validation.

Iroh transport encryption does not justify removing signed envelopes,
recipient binding, durable replay fences, or application-layer protection of
private DKG/FROST shares. Those properties keep the ceremony transport-neutral
and protect stored/forwarded payloads.

### Coverage requirement

The all-Iroh federation test must complete the real setup and DKG path, FROST
rounds, AlephBFT message exchange, operation submission, checkpoint creation,
catch-up, restart, and wallet mint/swap/melt flow without an HTTPS listener or
DNS name. If `federation-bark` is enabled, its complete transport surface must
run through the same test harness.

## Feature and compatibility policy

- Add `cdk-iroh` as a workspace dependency, disabled by default.
- Add native `iroh` features to `cdk`, `cdk-axum`, and `cdk-mintd` with clear
  feature propagation.
- `iroh` and `tor` are independent opt-ins and must compile together. The
  ordinary Iroh-enabled alias handles HTTP(S) and Iroh; the explicit Tor wallet
  client remains available for Tor. Cargo feature unification must not silently
  route Iroh through Tor or make the explicit Tor type the default.
- Federation without `iroh` remains HTTP-only and behaviorally unchanged.
- Wallet builds without `iroh` retain the existing `Async` alias.
- `wasm32-unknown-unknown` without `iroh` must keep passing. Version 1 should
  reject the unsupported `iroh`+browser feature combination clearly.
- No Cashu protocol version, NUT payload, token format, or database migration
  is introduced by this transport.

## MSRV and dependency gate

At the start of this plan CDK declared Rust 1.85.0, while current `iroh` 1.0.3
declared Rust 1.91. This was a release-policy decision, not a detail to
discover after the transport was written. Milestone 0 resolved it by raising
the CDK workspace MSRV to Rust 1.91.0.

Milestone 0 must produce a small dependency report covering:

- the newest Iroh release compatible with CDK's current MSRV;
- API and wire differences between that release and current stable Iroh;
- the transitive dependency and binary-size impact;
- native Linux, macOS, Android, and iOS support relevant to CDK; and
- maintenance/security consequences of pinning a pre-1.0 Iroh release.

Then choose and document exactly one path:

1. raise the workspace MSRV to the current stable Iroh requirement in a
   separate, explicitly reviewed policy change; or
2. pin the newest acceptable MSRV-compatible Iroh release with an upgrade plan
   and compatibility tests.

Do not pretend an optional feature avoids Cargo's package `rust-version`
contract, and do not patch out an upstream MSRV declaration.

**Milestone 0 decision:** use path 1. The workspace MSRV is raised explicitly
to Rust 1.91.0, with exact pins to `iroh = 1.0.3` and
`iroh-tickets = 1.0.0`. The attempted Rust-1.85 alternative is documented in
`crates/cdk-iroh/docs/dependency-decision.md`: Iroh 0.95.1 has no compatible
standard ticket crate, Iroh 0.93.2 conflicts with CDK's existing stable
Salsa20 dependency graph, and Iroh 0.35 is too old to accept as a new
production transport dependency.

Relevant upstream references:

- [Iroh crate and address lookup](https://docs.rs/iroh/latest/iroh/index.html)
- [Iroh endpoint connection behavior](https://docs.rs/iroh/latest/iroh/endpoint/struct.Endpoint.html)
- [Iroh endpoint tickets](https://docs.rs/iroh-tickets/latest/iroh_tickets/endpoint/struct.EndpointTicket.html)
- [Current Iroh manifest and MSRV](https://docs.rs/crate/iroh/latest/source/Cargo.toml)

## Expected existing-code changes

This table is a review budget, not permission for unrelated cleanup.

| Area | Permitted change | Explicitly unchanged |
|---|---|---|
| Root manifests | workspace dependency and lockfile | unrelated dependency versions |
| `cdk-http-client` | generic native WebSocket wrapper for a caller-provided stream | `Transport` REST methods and HTTP backends |
| `cashu::MintUrl` | Iroh parsing/path-join tests; minimal validation fix only if tests require it | serialized URL shape and Cashu types |
| `cdk` wallet | feature/dependency, default transport alias, NUT-17 scheme switch | endpoint mapping, sagas, wallet API, FFI API |
| `cdk-axum` | feature/dependency, final-router handoff, shared federation transport injection, secure-scheme policy | handlers, route table, mint state machines |
| `cdk-common` | transport-neutral secure URL policy accepting `https` and `iroh` | federation payloads, signatures, DKG/FROST types |
| `cdk-mintd` | Iroh config, persistent key lifecycle, listener supervision, transport construction | payment/database/signatory setup |
| Tests/docs | transport conformance and deployment documentation | weakening or skipping existing suites |

Keep the `cdk-iroh` pull request reviewable by separating prerequisite
transport injection from functional Iroh support. Avoid formatting or renaming
unrelated federation code while replacing default transport construction.

## Milestone sequence

Each milestone is independently reviewable and must record exact commands,
results, and tests not run.

### Milestone 0 — dependency and stream-bridge spike

- [x] Resolve the MSRV decision and pin exact Iroh/Iroh-ticket versions.
- [x] Build a standalone safe adapter joining Iroh send/receive halves into the
  async byte stream required by Hyper and Tokio Tungstenite.
- [x] Prove GET, POST, status/body errors, concurrent requests, and an Axum
  WebSocket upgrade over a local Iroh connection.
- [x] Prove one QUIC connection can carry concurrent short HTTP streams and a
  long-lived WebSocket stream.
- [x] Confirm graceful cancellation and no task leak when either endpoint
  closes.
- [x] Freeze the ALPN, HTTP version, authority rules, limits, and error mapping
  in short protocol documentation and golden tests.

**Gate:** The test router serves an arbitrary route and NUT-17-shaped echo
WebSocket over Iroh with no route-specific bridge code. If this gate fails,
stop; do not begin endpoint mapping.

**Progress:** complete in the Milestone 0 commit. The new `cdk-iroh` spike owns
a safe `IrohStream` adapter that delegates Tokio reads and writes to Iroh's
separate receive/send halves without unsafe code or buffering. Its synthetic
Axum router is passed generically through Hyper HTTP/1.1 with upgrades. One
authenticated Iroh connection carried a long-lived NUT-17-shaped echo
WebSocket while sixteen short requests ran concurrently; GET, POST, arbitrary
future route, query, status/body preservation, and wrong-authority rejection
all passed. Separate cancellation cases closed the client and server around
partial HTTP streams and observed zero remaining stream or WebSocket tasks.

The frozen `cashu-cdk-http/1` ALPN, origin-form/authority rules, limits,
timeouts, non-replay rule, and error mapping are recorded in
`crates/cdk-iroh/docs/http-bridge-v1.md` and fixed by a golden test. The bridge
contains no `/v1` or `/federation/v1` route list and no Cashu payload mapping.

Verification evidence:

- `cargo test -p cdk-iroh --offline`: all three spike tests and doc tests pass
  on the macOS host;
- `cargo clippy -p cdk-iroh --all-targets --offline -- -D warnings`: passes;
- `cargo fmt -p cdk-iroh -- --check`: passes with only the repository's
  stable-rustfmt warnings about nightly formatting options;
- an official Rust 1.91 Linux container, with the workspace and required Bark
  sibling mounted read-only, passes
  `cargo check -p cdk-iroh --locked`, proving the declared MSRV and Linux
  dependency graph;
- the normal `cdk-iroh` graph resolves to 259 unique runtime package versions;
  the optimized, unstripped end-to-end bridge test executable is 15,066,832
  bytes, a preliminary upper-bound artifact rather than a final mintd size
  delta; and
- `cargo test -p cdk-iroh --release --no-run --offline` passes.

`cargo tree -p cdk --no-default-features --features wallet` contains zero Iroh
packages, confirming that adding the workspace crate and dependency pins did
not pull Iroh into the existing feature-disabled wallet graph.

The attempted `aarch64-apple-ios` library check stopped before CDK/Iroh code
because the host's non-rustup toolchain does not have that standard-library
target installed. Android and iOS compilation/lifecycle tests remain required
before those native targets are enabled. `cargo audit` was not run because the
command is not installed; advisory review remains part of Milestone 5. No
Nocean deployment is claimed for this local feasibility milestone.

### Milestone 1 — standalone `cdk-iroh` crate

- [x] Add endpoint construction for ephemeral client and persistent server
  identities.
- [x] Add N0, custom, and static-ticket discovery configuration.
- [x] Add connection pooling keyed by endpoint ID and ALPN.
- [x] Add one-stream-per-transaction HTTP client and bounded response reads.
- [x] Add WebSocket-over-stream support.
- [x] Add generic Axum/Tower server support with request extensions.
- [x] Add admission, timeouts, shutdown, redacted errors, tracing, and metrics.
- [x] Implement `IrohTransport<Async>` with explicit scheme dispatch.
- [x] Add a transport conformance suite covering raw, JSON, form, auth,
  bounded, error, and streaming behavior.

**Gate:** `cdk-iroh` passes its complete tests against a synthetic router and
an in-process Iroh network without depending on `cdk`, `cdk-axum`, or mint
business logic.

**Progress:** complete in the Milestone 1 commit. `IrohNode` now binds either a
generated ephemeral identity or an operator-provided persistent `SecretKey`.
Its discovery policy is explicit: current N0 discovery/default relays,
operator-provided current `RelayUrl` values, or direct/static discovery seeded
with canonical `iroh_tickets::endpoint::EndpointTicket` values. Configuration
and node `Debug` output reports only modes, counts, and shortened endpoint
fingerprints; it does not render relay URLs, tickets, bind addresses, request
bodies, or credentials.

The clone-shared connection pool is keyed by authenticated endpoint ID plus
the frozen ALPN. Only concurrent duplicate dials for the same key share a
short per-key critical section; different peers and all opened streams remain
concurrent. Its configured upper bound rejects new peer keys when all pooled
connections are live and reclaims terminal entries before admitting new ones.
Every HTTP transaction and WebSocket opens its own bidirectional stream and
ambiguous requests are never replayed. HTTP response bodies are read
frame-by-frame with both progress deadlines and allocation bounds;
declared or observed oversize responses fail through CDK's existing bounded
response error. JSON, form, raw status/body, Cashu authentication headers, and
WebSocket framing preserve the existing `Transport` behavior.

`IrohServer` accepts the final cloneable Axum/Tower service generically. It
converts and limits the incoming body, injects the TLS-authenticated remote
endpoint as `IrohConnectionInfo`, binds the HTTP authority to the local
endpoint ID, rejects non-WebSocket hop-by-hop headers, and closes every
ordinary response after one transaction. Global/per-peer connection and
global/per-connection stream semaphores reject excess work without queueing or
serializing admitted streams. Accept, connection, and stream tasks are
supervised and drained on bounded shutdown. Transport counters have no
peer/request labels, tracing uses only short endpoint fingerprints, and
upstream errors are mapped to bounded redacted variants.

`IrohTransport<Async>` lazily creates a default ephemeral endpoint or accepts
a runtime-owned node. It dispatches only `iroh` to Iroh, delegates `http` and
`https` byte-for-byte to the existing transport, delegates `ws`/`wss` for
WebSockets, and rejects every other scheme. An Iroh failure never falls back
to HTTPS. The native caller-provided-stream WebSocket constructor originally
listed in Milestone 2 landed as the minimal shared prerequisite for this
milestone's WebSocket gate; it contains no Iroh routing or endpoint knowledge.

The user-requested upstream recheck found that the current stable packages are
still exact `iroh = 1.0.3` and `iroh-tickets = 1.0.0`, both requiring Rust
1.91. CDK therefore keeps the explicit MSRV increase and current standalone
ticket type rather than adding a deprecated pre-1.0 compatibility layer.

Verification evidence:

- `cargo test -p cdk-iroh --offline` with loopback socket permission passes
  one unit test, three bridge/golden/cancellation tests, nine standalone
  conformance tests, and doc tests. Coverage includes ephemeral/persistent
  identity, all discovery policies, canonical tickets, raw/JSON/form/auth,
  status errors, fragmented bounded reads, pre-dial request limits, endpoint
  authority binding, generic future routes, authenticated request extensions,
  protected framing headers, redacted unknown endpoint errors, WebSockets, 24
  concurrent requests over one pooled connection, outbound pool reclamation,
  global/per-peer connection saturation, stream saturation with an upgraded
  WebSocket holding its admission lease, recovery after every saturation,
  graceful zero-activity shutdown, exact HTTP delegation, and no fallback;
- `cargo test -p cdk-http-client --offline` with loopback permission passes 21
  unit tests, 32 integration tests, and one doc test after adding the generic
  native WebSocket stream wrapper;
- `cargo clippy -p cdk-iroh -p cdk-http-client --all-targets --offline --
  -D warnings` passes, and targeted formatting passes with only the
  repository's stable-rustfmt warnings for nightly-only options;
- `cargo check --workspace --all-targets --offline --quiet` passes, confirming
  the MSRV/dependency policy change did not break another workspace target;
- an official Linux Rust 1.91 container passes `cargo check -p cdk-iroh -p
  cdk-http-client --locked --quiet`, testing the declared MSRV instead of the
  newer host compiler; and
- the normal dependency tree contains neither `cdk` nor `cdk-axum`; the
  synthetic router and in-process Iroh network are the only server fixtures.

Mobile cross-compilation, mint/wallet integration, and Nocean relay testing are
not claimed by this standalone milestone. They remain assigned to Milestones
2, 3, and 5 rather than being inferred from local transport conformance.

### Milestone 2 — transparent wallet integration

- [x] Add feature propagation and the hybrid default wallet transport alias.
- [x] Preserve `iroh` in the NUT-17 URL scheme switch.
- [x] Add the generic native WebSocket stream constructor to
  `cdk-http-client`.
- [x] Prove one wallet application can use an HTTPS mint and an Iroh mint in the
  same process.
- [x] Run one reusable `MintConnector` conformance suite unchanged against
  HTTP and Iroh, covering every existing connector method and payment-method
  variant, including batch operations, auth, custom methods, NUT-19 retries,
  and NUT-17 subscriptions. Do not maintain a second Iroh endpoint list.
- [x] Prove an Iroh connection error does not attempt HTTPS fallback.

**Gate:** No production endpoint method changes, and a test-only future route
is reachable automatically through the bridge.

**Progress:** complete in the Milestone 2 commit. The native `cdk/iroh`
feature selects `HttpClient<IrohTransport<Async>>` and the matching auth alias;
without that feature the aliases remain exactly `HttpClient<Async>`. The
hybrid transport now has one process-shared lazy default client endpoint, so
ordinary independently constructed wallet connectors do not create separate
random identities or pools. Runtime-owned nodes remain injectable and are
clone-shared explicitly. Browser/WASM builds reject `iroh` clearly, while a
wallet build without the feature resolves no Iroh packages. `iroh` and `tor`
compile together and the explicit Tor aliases remain independent.

CDK's `Wallet` type is intentionally bound to one mint URL. The original
"one wallet instance" checklist wording is therefore clarified above to the
actual invariant: one wallet application and one hybrid transport runtime can
serve ordinary mint-bound connectors for HTTPS and Iroh concurrently, without
different connector APIs or types. The test uses one clone-shared hybrid
transport for both mint URLs.

NUT-17 URL construction is now one fallible helper: `https` becomes `wss`,
`http` becomes `ws`, `iroh` remains `iroh`, and every other scheme returns
`NotSupported`. The existing subscription state machine and polling fallback
are unchanged. The generic caller-stream WebSocket wrapper remains the only
shared HTTP-client addition.

One reusable conformance runner executes the existing generic `HttpClient<T>`
against HTTPS dispatch and live in-process Iroh using the same synthetic Axum
router. It covers every `MintConnector` and auth-connector method, Bolt11,
Bolt12, onchain, and custom mint/melt variants, quote status, batch check and
batch mint, auth headers, NUT-19 retry, OIDC construction, LNURL and DNS
delegation, NUT-17 WebSocket framing, and wallet auth state. HTTPS behavior is
tested at the generic dispatch boundary with a fixture inner transport that
forwards to the loopback router; TLS behavior itself remains owned by the
unchanged existing HTTP transport. A path present only in the fixture router
works over Iroh, proving there is no second endpoint table. Closing a ticketed
endpoint before a request produces an Iroh error and zero inner HTTPS requests.

Verification evidence:

- `cargo test -p cdk --no-default-features --features wallet,iroh --lib
  --offline` passes all 335 tests, including the shared HTTPS/Iroh connector
  conformance runner and NUT-17 scheme policy;
- the unchanged default `cargo test -p cdk --lib --offline` passes all 542
  tests;
- `cargo test -p cdk-iroh --offline` passes two unit tests, three bridge tests,
  nine transport conformance tests, and doc tests; the added unit test proves
  independent defaults share one process endpoint;
- strict Clippy passes for the changed production libraries and the Iroh
  wallet tests. The test-target command needs a narrow allowance for the
  pre-existing `manual_is_multiple_of` warning in `cdk-sql-common` on the newer
  host Clippy; this milestone does not modify that unrelated crate;
- `cargo check -p cdk --no-default-features --features wallet,iroh,tor
  --offline` passes;
- `cargo tree -p cdk --no-default-features --features wallet` contains no Iroh
  package;
- an official Linux Rust 1.91 container passes the native Iroh-enabled wallet
  check with the locked dependency graph; and
- after installing only the WASM standard library and `clang` in that
  disposable container, the feature-disabled `wasm32-unknown-unknown` wallet
  check passes. Enabling `iroh` on the same target fails at CDK's explicit
  compile-time policy message, before CDK attempts to compile Iroh code.

### Milestone 3 — mint listener and operator configuration

- [x] Add protected persistent endpoint-key management.
- [x] Add discovery, relay, ticket, bind, limit, and timeout configuration.
- [x] Hand the final layered router to `IrohServer`.
- [x] Supervise HTTP and Iroh listeners together with graceful shutdown.
- [x] Support HTTP-only, Iroh-only, and dual-listener mintd startup.
- [x] Validate configured Iroh URLs against the derived endpoint ID.
- [x] Document endpoint ticket export/import and N0 versus self-hosted
  discovery.

**Gate:** The same mint state is reachable over HTTPS and Iroh, custom routes
and middleware behave identically, and Iroh-only startup uses no certificate or
DNS name.

**Progress:** complete in the Milestone 3 commit. The native `cdk-mintd/iroh`
feature now propagates through `cdk-axum` and `cdk`, while feature-disabled
mintd retains its original dependency graph and HTTP listener behavior. Mintd
creates one persistent Iroh endpoint before database, mint, or federation
workers start. The default 32-byte endpoint secret is created with protected
directory/file permissions and create-new semantics; reload rejects weak
permissions, corruption, and symlinks. `cdk-mintd iroh init` lets an operator
create or inspect the stable endpoint identity before placing the resulting
`iroh://<endpoint-id>` in configuration. Startup rejects a mismatched URL and
atomically exports the current standard Iroh endpoint ticket without logging
the key, ticket, relay addresses, or configured paths.

The optional `[iroh]` section and matching environment variables expose N0,
static, or custom-relay discovery, imported current
`iroh_tickets::endpoint::EndpointTicket` strings, a UDP bind address, all
bridge timeouts, and every connection/stream/header/body limit. Zero timeouts,
invalid tickets/relays, an empty custom-relay set, and configurations with no
public listener fail closed. Iroh-only mode requires an Iroh public URL and
does not parse or bind the TCP listener. The ordinary HTTP listener remains
enabled by default.

The final router is cloned only after `/v1`, auth, federation routes, body
limits, compression, tracing, and caller-supplied routers have all been
assembled. A small listener supervisor starts HTTP, Iroh, or both, observes an
unexpected listener exit, broadcasts shutdown to auxiliary services, drains
both listeners, and joins their tasks before mint/RPC teardown. The live gate
test reaches a custom layered route in all three modes and observes one shared
atomic state advance from the HTTP request to the Iroh request in dual mode.
The repository's HTTP listener remains plain TCP behind any existing HTTPS
terminator, so the unchanged HTTPS/TLS behavior stays owned outside this
listener refactor.

Verification evidence:

- `cargo test -p cdk-iroh --offline` passes 2 unit, 3 bridge, and 9 transport
  conformance tests plus doc tests;
- `cargo test -p cdk --no-default-features --features wallet,iroh --lib
  --offline` passes all 335 Iroh-enabled wallet tests;
- `cargo test -p cdk-mintd --lib --offline -- --test-threads=1` passes all 55
  feature-disabled mintd tests;
- `cargo test -p cdk-mintd --features iroh --lib --offline --
  --test-threads=1` passes all 63 tests, including protected key restart,
  current ticket export, CLI parsing, URL mismatch, and the three live
  listener modes;
- `cargo test -p cdk --lib --offline` passes all 542 default CDK tests;
- `cargo test -p cdk-mintd --features federation --lib --offline --
  --test-threads=1` passes all 212 pre-Iroh federation tests, including the
  independent multi-operator HTTP setup/certification path;
- `cargo check -p cdk-mintd --features federation,iroh --offline` passes,
  proving the next mixed-transport integration feature set compiles before
  Milestone 4 changes it;
- `cargo clippy -p cdk-iroh -p cdk-mintd -p cdk-axum --features
  cdk-mintd/iroh,cdk-axum/iroh --lib --offline -- -D warnings -A
  clippy::manual_is_multiple_of` passes. The narrow allowance remains for the
  unrelated pre-existing `cdk-sql-common` warning on the newer host Clippy;
  and
- the exact current dependency pins remain `iroh = 1.0.3` and
  `iroh-tickets = 1.0.0`, under the already-verified Rust 1.91 MSRV gate from
  Milestones 0 and 2.

### Milestone 4 — heterogeneous federation and DKG

- [x] Land the HTTP-only shared federation transport-injection refactor.
- [x] Use the hybrid transport for every federation runtime and operator
  client.
- [x] Accept `iroh` in setup, enrollment, mintd, and DKG secure URL policies.
- [x] Allow one Iroh endpoint to be both the public and federation base URL.
- [x] Add exact roster URL/endpoint-ID startup validation.
- [ ] Run a mixed HTTPS/Iroh federation through normal operation, restart, and
  catch-up.
- [ ] Run an all-Iroh federation through setup, DKG, FROST, AlephBFT,
  checkpoint, wallet, and recovery flows.
- [x] Confirm signed request and replay rejection tests are byte-for-byte
  transport independent.
- [x] Include Bark transport coverage when that feature is enabled.

**Gate:** No DKG, FROST, consensus, checkpoint, catch-up, or Bark state-machine
source branches on transport scheme.

**Progress:** the integration and local protocol coverage are complete; the
Nocean deployment gate is in progress. `FederationTransport` is constructed
once and cloned through setup, DKG, FROST signing, AlephBFT, peer sync,
checkpoint/catch-up, wallet liveness, Bark, and every operator client. URL
scheme dispatch remains below `FederationRestPeerClient`; none of those state
machines branches on transport scheme.

The mint runtime owns one protected persistent endpoint for serving and
dialing. Setup and operator CLI processes deliberately use short-lived
ephemeral dialing endpoints with the same discovery/ticket policy so they do
not claim the live service identity or bind address. Startup separately loads
the protected endpoint ID and checks the activated roster before any operator
request. N0 discovery needs no tickets; static/custom discovery requires the
exact set of remote roster tickets and rejects missing, duplicate, extraneous,
cross-member duplicate, and local-key-mismatch configurations. Setup clients
skip activated-roster validation because those files do not exist before DKG;
the setup server and activated runtime still own the protected identity.

Local verification evidence:

- `cargo test -p cdk-common --features federation --test
  federation_setup_domain --offline` passes all 12 secure-URL/setup policy
  tests;
- `RUST_MIN_STACK=8388608 cargo test -p cdk-mintd --features
  federation,iroh --lib --offline -- --test-threads=1` passes all 222 tests,
  while the HTTP-only federation build passes all 212 and default mintd passes
  all 55;
- `RUST_MIN_STACK=8388608 cargo test -p cdk-axum --features
  federation-bark,iroh --lib --offline -- --test-threads=1` passes 576 tests
  with three manual benchmarks ignored. This includes mixed live dialing,
  all-Iroh signed DKG and replay rejection, FROST, AlephBFT, checkpoint,
  catch-up, restart, wallet-liveness, and Bark coverage;
- without the explicit 8 MiB test-thread stack, one pre-existing deep AlephBFT
  harness case overflows its test thread in both HTTP-only and Iroh feature
  builds. The case passes at 8 MiB and this is recorded as test-harness stack
  sensitivity rather than a transport assertion failure; and
- the focused operator-identity test proves a CLI client uses a different
  ephemeral endpoint and neither overwrites nor claims the persistent endpoint
  ticket/key, while exact roster tests continue to bind the advertised local
  URL to that protected key.

The remaining Milestone 4 proof is the one-server Nocean drill requested for
the available infrastructure: five independent protected directories and tmux
operator sessions, five existing public HTTPS DNS subdomains, five persistent
Iroh federation endpoints, and all user-entered setup/DKG/runtime steps over
Iroh N0. This exercises operational isolation on one machine; it does not
claim five-machine fault-domain independence.

### Milestone 5 — hardening and release evidence

- [ ] Test direct path, relay path, address change, offline discovery, stale
  ticket, relay loss, and direct/relay migration.
- [ ] Test malformed HTTP, invalid ALPN, unknown endpoint ID, oversized
  headers/body/response, slow streams, connection churn, admission saturation,
  and shutdown under load.
- [ ] Test request ambiguity without unsafe POST replay.
- [ ] Test endpoint key mismatch and key-file loss/corruption fail closed.
- [ ] Compare status codes, Cashu error bodies, compression, auth, and
  WebSocket behavior between HTTPS and Iroh.
- [ ] Measure connection setup, warm-request latency, stream concurrency,
  memory, file descriptors, and relay bandwidth.
- [ ] Run standalone, federation, Tor, FFI/native, MSRV, and WASM-without-Iroh
  regression builds.
- [ ] Complete security review of the stream adapter, key storage, discovery,
  URL parsing, resource bounds, and mixed-transport federation assumptions.

**Gate:** Release documentation contains reproducible local, relay, and
multi-host results and accurately lists unsupported target combinations.

## Verification matrix

### Transport selection

| Caller | Target URL | Expected path |
|---|---|---|
| Iroh-enabled wallet | `https://...` | existing HTTP transport only |
| Iroh-enabled wallet | `iroh://...` | Iroh transport only |
| non-Iroh wallet | `https://...` | existing HTTP transport only |
| non-Iroh wallet | `iroh://...` | explicit unsupported-scheme error |
| federation member | peer HTTPS URL | existing HTTP transport only |
| federation member | peer Iroh URL | shared Iroh node and pool only |

### Listener modes

| HTTP listener | Iroh listener | Expected result |
|---|---|---|
| enabled | disabled | current behavior |
| disabled | enabled | domain-free Iroh-only mint |
| enabled | enabled | both URLs serve the same mint/router |
| disabled | disabled | startup failure |

### Federation topologies

| Topology | Required proof |
|---|---|
| all HTTPS | existing suites unchanged |
| all Iroh | complete setup, DKG, runtime, wallet, restart, catch-up |
| alternating HTTPS/Iroh | every directed member pair communicates |
| public HTTPS/private Iroh | wallet over HTTPS, all federation traffic over Iroh |
| public Iroh/private HTTPS | wallet over Iroh, federation traffic over HTTPS |

### Failure cases

- Unknown or malformed Iroh endpoint ID.
- Endpoint ticket ID differs from URL ID.
- Configured secret key derives a different advertised ID.
- No discovery result in N0, custom, or static mode.
- Stale direct address with working relay, and stale relay with working direct
  path.
- Peer supports Iroh but not the Cashu ALPN.
- HTTP parse failure or WebSocket upgrade rejection on an Iroh stream.
- Request or response exceeds the correct bound while streaming.
- Connection closes before any request bytes versus after ambiguous request
  delivery.
- Saturated global/per-peer stream admission.
- Shutdown during REST, DKG, AlephBFT, and WebSocket traffic.
- HTTPS peer failure does not try Iroh, and Iroh peer failure does not try
  HTTPS.

## Security invariants

- The endpoint ID in the URL is the Iroh transport authentication target.
- Discovery data changes reachability, never peer identity.
- Relay operators can forward encrypted traffic but cannot terminate the Iroh
  connection or change the authenticated endpoint ID.
- The Iroh secret key is persistent mint private material and follows CDK's
  protected file, backup, redaction, and rotation rules.
- Federation secp256k1 identities and signed request envelopes remain the
  application authorization roots.
- Cashu auth headers have identical scope over HTTPS and Iroh and are never
  logged.
- DKG/FROST recipient binding and private payload protection remain enabled.
- No non-idempotent request is replayed after ambiguous delivery by the
  transport layer.
- All parsing and buffering are size bounded before allocation or task spawn.
- Per-peer limits use authenticated Iroh endpoint IDs where available, while
  existing federation member admission continues to use signed member IDs.
- An authenticated transport peer is not automatically allowed to call
  federation routes; the existing route authentication decides access.

## Observability

Expose bounded metrics for:

- endpoint online/offline state without secret or address labels;
- connection attempts and outcomes by discovery mode and direct/relay class;
- active connections and streams;
- request count, duration, status class, and bounded failure kind;
- pool hits, reconnects, admission rejection, and timeout stage; and
- HTTP versus Iroh federation request outcomes by non-secret member ID.

Logs may include a shortened endpoint ID fingerprint, route template, method,
status class, transport, timeout stage, and direct/relay classification. They
must not include secret keys, full tickets, IP addresses by default, auth
headers, request/response bodies, DKG/FROST private payloads, payment secrets,
local paths, or operator-identifying data.

## Prohibited shortcuts

- Do not copy the `/v1` or `/federation/v1` route list into `cdk-iroh`.
- Do not add `iroh_get_keys`, `iroh_swap`, `iroh_dkg_*`, or equivalent methods.
- Do not create a second mint router or bypass its middleware.
- Do not make polling the permanent NUT-17 implementation.
- Do not use a process-global random Iroh endpoint to avoid transport
  injection.
- Do not create a fresh endpoint or connection for every request.
- Do not put endpoint tickets, relay URLs, or IP addresses into Cashu mint
  URLs, tokens, or federation consensus hashes.
- Do not infer fallback URLs or rewrite `iroh://` to `https://`.
- Do not accept any authenticated Iroh endpoint as a federation member.
- Do not remove signed federation envelopes because Iroh authenticates its
  connection.
- Do not retry ambiguous mutations below the existing application retry layer.
- Do not raise limits or timeouts merely to make load tests pass.
- Do not raise CDK's MSRV or pin an obsolete Iroh release without the explicit
  Milestone 0 decision.

## Definition of done

The Iroh transport is complete only when:

- an application using the ordinary native wallet API can change only a mint
  URL from HTTPS to `iroh://<endpoint-id>` and use every Cashu operation;
- the same wallet instance can communicate with HTTPS and Iroh mints;
- REST and NUT-17 use the existing Cashu JSON and WebSocket protocols through
  one generic bridge;
- a mint can run HTTP-only, Iroh-only, or both against the same final router;
- a future test route works over both listeners without registering it in
  `cdk-iroh`;
- an all-Iroh federation completes setup, real DKG/FROST, consensus, wallet
  operations, checkpointing, restart, and catch-up without HTTPS or DNS names;
- a mixed HTTPS/Iroh federation passes directed connectivity and failure
  tests;
- existing HTTP-only, standalone, Tor, and federation tests remain unchanged
  in behavior;
- feature-disabled builds contain no Iroh dependency or runtime behavior;
- persistent identity, discovery, bounds, ambiguity, observability, and
  shutdown requirements are verified; and
- reviewers can identify every existing-code modification as one of the thin
  integration seams in this plan, with the Cashu endpoint and state-machine
  implementations untouched.

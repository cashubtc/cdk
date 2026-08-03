# CDK Iroh transport

`cdk-iroh` owns CDK's optional Iroh endpoint, discovery, pooled connections,
and generic HTTP/WebSocket bridge. It is intentionally transport-only: Cashu
routes remain in their existing crates.

The crate provides:

- `IrohNode` with separate outgoing client and listener-capable server roles;
- N0, operator-provided relay, and static-ticket discovery policies;
- `IrohClient` for generic bounded HTTP/1.1 and WebSocket transactions;
- `IrohServer` for serving a final Axum/Tower service with authenticated
  `IrohConnectionInfo` request extensions; and
- `IrohTransport<Async>` for explicit `iroh` versus HTTP(S) scheme dispatch.

One authenticated Iroh connection is pooled per endpoint ID and ALPN. Each
HTTP request or WebSocket uses a separate bidirectional stream, so long-lived
subscriptions do not serialize ordinary requests. The server has global and
per-peer connection limits, global and per-connection stream limits, bounded
headers and bodies, supervised shutdown, redacted errors, tracing, and
bounded-cardinality metrics. QUIC enforces the per-connection stream cap. Idle
server connections are reaped, ordinary HTTP requests have a total deadline,
and failed outbound dials use bounded exponential backoff with jitter. The
outbound pool is bounded and promptly evicts closed connections.

Default hybrid client transports lazily share one process-wide ephemeral Iroh
endpoint and connection pool. N0 clients resolve mint addresses and retain relay
reachability without publishing their own address. Backend runtimes that own a
persistent identity should construct one `IrohNode` and pass clones through
`IrohTransport::with_node`.

For ticket-only local operation, construct a server node, export its standard
ticket, and install the ticket in the client configuration:

```rust,no_run
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use cdk_iroh::{IrohConfig, IrohNode};

# async fn example() -> Result<(), cdk_iroh::Error> {
let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
let server = IrohNode::ephemeral(IrohConfig::static_only().with_bind_addr(bind_addr)).await?;
let client = IrohNode::client(
    IrohConfig::static_only()
        .with_bind_addr(bind_addr)
        .with_ticket(server.endpoint_ticket()),
)
.await?;

assert_ne!(server.endpoint_id(), client.endpoint_id());
client.close().await;
server.close().await;
# Ok(())
# }
```

Production servers should pass a securely loaded `SecretKey` to
`IrohNode::persistent`. `cdk-mintd` owns protected persistent key files,
operator configuration, endpoint-ticket export, URL validation, and listener
supervision; this transport-only crate deliberately does not read operator
files itself.

The crate pins stable Iroh 1.0.3 and `iroh-tickets` 1.0.0. This requires CDK's
Rust 1.91 MSRV policy change; see `docs/dependency-decision.md` for the rejected
pre-1.0 alternative and upgrade policy.

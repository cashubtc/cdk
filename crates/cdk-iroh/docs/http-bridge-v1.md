# `cashu-cdk-http/1` bridge

This document freezes the Milestone 0 wire contract for carrying ordinary
HTTP/1.1 and WebSocket traffic over Iroh bidirectional QUIC streams.

- ALPN: the exact bytes `cashu-cdk-http/1`.
- One authenticated Iroh QUIC connection may carry many concurrent streams.
- One bidirectional stream carries one HTTP/1.1 connection and one transaction.
  The client never sends a second request, and the server closes every
  non-upgraded response.
- A successful WebSocket upgrade owns that stream until the WebSocket closes.
- Requests use origin-form path/query targets and a `Host` authority equal to
  the dialed Iroh identity. The server returns 421 for another authority.
- 0-RTT is disabled. A stream may be retried only before request bytes are
  accepted; ambiguous mutations are never replayed by this layer.
- Request headers are limited to 64 KiB, request bodies to 1 MiB, and buffered
  responses to 16 MiB. The final Axum router may enforce smaller route limits.
- Connection establishment is bounded to 15 seconds, stream opening to 10
  seconds, header receipt to 15 seconds, body progress to 30 seconds, and
  graceful drain to 10 seconds.
- Transport or discovery failures produce transport errors, malformed HTTP
  produces 400, oversized headers produce 431, authority mismatch produces
  421, and router status/body responses pass through unchanged. A response
  exceeding the configured bound maps to a transport error.
- Inbound hop-by-hop headers are rejected except for the standard WebSocket
  upgrade flow. Non-upgrade responses discard application hop-by-hop headers
  and set `Connection: close`. Authentication headers and payload bodies are
  never logged by the bridge.

The spike test intentionally serves a route known only to its Axum router. The
bridge has no Cashu route table and performs no JSON translation.

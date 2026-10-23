# Wallet subscription transport control

`Wallet::subscribe` keeps its existing automatic WebSocket/HTTP fallback.
`Wallet::subscribe_with_options(params, false)` is socket-only: no status HTTP
requests are made by the consumer, including during reconnects. This allows a host
with a durable reconciliation/retry scheduler to own the HTTP fallback without a
second hidden polling loop. The wallet and FFI Wallet trait expose both methods.

Socket-only subscriptions share connections per mint and subscription kind, so a
rejected/unsupported kind does not tear down another kind's working stream. They
attempt WebSockets even when the default consumer prefers HTTP. Failed attempts,
including unsupported endpoints, retry at 2, 4, 8, 16, then 30 seconds. Connection,
send and subscription acknowledgement waits have 10 second deadlines.

Rust consumers expose a separate `stream_readiness()` handle. FFI consumers expose
`is_streaming()`, which is safe to call while `recv()` is waiting. Readiness means
all of this subscription's filters have matching successful acknowledgements on
the current connection. It clears on disconnect and unsubscribe; the existence of
a subscription handle does not imply a working socket. Hosts should still retain
a slow safety reconciliation for missed events or silent connections.

FFI `stop()` is idempotent, interrupts a pending `recv()`, and drops the active
subscription. Mobile hosts should call it on background and wallet replacement.
Socket-only hosts must schedule their own HTTP recovery, coalesce notifications,
and respect durable issuance failure deadlines. Notifications are wakeups, not
proof of successful settlement.

For ordinary subscriptions, HTTP poll failures now back off independently up to
300 seconds; successful polls restore the existing 2 second interval. Socket
retries remain independent. Quote HTTP errors are propagated to this scheduler
rather than silently treated as successful polls.

`wait_pending_melt` uses socket-only updates to wake the existing saga recovery,
with a 60 second safety check on a ready stream and 1–30 second exponential
fallback while disconnected. Its saga validation and finalization remain the
source of payment truth.

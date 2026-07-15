# Federated BDK chain sources

Each federation member configures its watch-only BDK chain source locally. The
endpoint, credentials, retry policy, and storage path never enter federation
consensus. Members may independently choose Bitcoin Core RPC or an
Esplora-compatible server.

## Esplora

A public mempool.space-compatible deployment can use:

```toml
[federation.onchain_wallet]
enabled = true
chain_source_type = "esplora"
chain_source_scope = "shared"
chain_source_id = "public-mempool-space"
esplora_url = "https://mempool.space/api"
esplora_parallel_requests = 1
request_timeout_secs = 15
esplora_max_retries = 3
sync_interval_secs = 30
```

The same configuration works with a self-hosted Esplora API by changing only
the local URL, source scope, and opaque source ID. Use `independent` only when
the member controls an upstream independently of the other federation members.
Public Esplora is optional; it is not part of the public federation config and
is never a federation-wide dependency.

`esplora_parallel_requests` is restricted to 1 through 16, request timeouts to
1 through 120 seconds, and retries to at most 6. Retryable HTTP responses use
the Esplora client's bounded retry policy. Repeated synchronization failures
also use exponential backoff capped at five minutes. Federated wallets query
only scripts already revealed by accepted consensus allocation counters, so
their unallocated scan lookahead is zero and cannot grow through an upstream
response.

An Esplora service learns the scripts or addresses queried by that member and
can correlate them by network identity and timing. A public service can also
omit transactions, report stale data, rate-limit requests, or become
unavailable. Prefer a member-operated service, network privacy controls, and
different infrastructure for different members. Do not place API tokens or
credentials in `chain_source_id`; it is returned by private operator status.

## Bitcoin Core RPC

A member-operated Bitcoin Core source uses:

```toml
[federation.onchain_wallet]
enabled = true
chain_source_type = "bitcoinrpc"
chain_source_scope = "independent"
chain_source_id = "member-bitcoin-core"
bitcoind_rpc_host = "127.0.0.1"
bitcoind_rpc_port = 18443
bitcoind_rpc_user = "rpc-user"
bitcoind_rpc_password = "rpc-password"
request_timeout_secs = 15
sync_interval_secs = 30
```

RPC credentials remain member-local and are redacted from debug and status
output. Synchronous RPC transport work runs on bounded blocking workers; BDK
and database locks are held only while snapshotting or applying local state,
never during network calls.

## Independence and consensus

Consensus authorization remains member-based. Declaring sources as shared does
not lower or raise the configured member threshold, but it does reduce the
practical independence of observations. Members using the same provider should
use the same opaque `chain_source_id` and `shared` scope. Private status then
emits an explicit correlated-source warning, while accepted observation state
reports distinct independent and shared source counts.

A timeout, malformed reply, stale tip, or omitted transaction degrades only the
affected member's local wallet. The last persisted BDK graph remains intact,
and that member fails closed for consensus-dependent actions until a successful
sync recovers it. Confirmation, reorg, and terminal transaction decisions still
require accepted federation operations; no chain backend is consensus
authority by itself.

## Optional verification improvements

Bitcoin Core RPC already validates the local chain through the member's full
node. Esplora members may add a local header chain and verify reported block
headers, proof-of-work, transaction Merkle inclusion, and confirmation ancestry
before proposing observations. This is a future defense-in-depth layer: it
cannot prove that an upstream has not omitted a transaction, and it must not
replace member quorum or the existing fail-closed confirmation rules.

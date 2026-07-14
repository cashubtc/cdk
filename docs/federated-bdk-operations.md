# Federated BDK operations

This runbook covers the member-local watch-only BDK deployment boundary. The
shared wallet descriptor, confirmation depth, and fee-policy version are
consensus configuration. Each member independently configures its chain source
and stores only its sealed FROST root share. A standalone BDK mnemonic is never
valid in this mode.

## Configuration

Select the explicit payment-interface adapter and enable the federation wallet:

```toml
[onchain]
onchain_backend = "federated-bdk"

[federation.onchain_wallet]
enabled = true
network = "bitcoin"
minimum_confirmations = 6
fee_policy_version = 1
chain_source_type = "bitcoinrpc"
chain_source_scope = "independent"
chain_source_id = "member-core-primary"
bitcoind_rpc_host = "127.0.0.1"
bitcoind_rpc_port = 8332
bitcoind_rpc_user = "<from-secret-store>"
bitcoind_rpc_password = "<from-secret-store>"
request_timeout_secs = 15
sync_interval_secs = 15
```

The network, confirmation depth, and fee-policy version must exactly match the
active public FROST application. Timeouts, retry counts, and Esplora concurrency
are bounded. Configuration fails closed when the public wallet, active
application, local sealed share, or chain source is missing; when standalone
`[bdk]` configuration is present; or when `onchain_backend = "bdk"` is mixed
with an active federated wallet.

Environment overrides use the `CDK_MINTD_FEDERATION_ONCHAIN_WALLET_*` variables.
Credentials and endpoints are redacted from debug output and status.

## Chain-source choice

Use independently operated Bitcoin Core RPC sources where possible. A
self-hosted Esplora instance can simplify deployment but adds indexing
infrastructure. Public mempool.space is convenient for test or low-assurance
observation but is shared, rate-limited, and correlates members. See the
[chain-source guide](federated-bdk-chain-sources.md) for the complete trust,
privacy, timeout, and outage tradeoffs. Shared sources produce an explicit
startup and status warning; they do not reduce the consensus observation
threshold.

## Diagnosis

All operator requests are authenticated by a federation member identity:

```console
cdk-mintd federation dkg-status
cdk-mintd federation frost-status --require-ready --require-converged
cdk-mintd federation wallet-status --require-ready
cdk-mintd federation transaction-status --proposal-id <proposal-id>
```

Wallet status reports fixed lifecycle counts only: chain observation, proposal,
signing, broadcast authorization, broadcast, confirmation, and terminal. It
also reports public root/application epochs and commitments, chain tip health,
revealed child counts, UTXO counts and values, sticky reserved value, liveness
attempts, and broadcast health. It does not report addresses, scripts,
transaction contents, secret shares, nonces, credentials, or sensitive local
paths. Prometheus uses only member IDs and fixed state, phase, keychain, and
health labels.

If signing cannot reach the configured threshold, the liveness worker remains
fail-closed and status shows pending batches and signer failures. During a new
ceremony startup warns that both the independent BLS and FROST results remain
incomplete until the same threshold quorum confirms them.

## Safe rebroadcast

An immediate retry requires an explicit acknowledgement:

```console
cdk-mintd federation rebroadcast \
  --proposal-id <proposal-id> \
  --acknowledge-exact-byte-retry
```

The route targets one accepted proposal, rereads the exact durably persisted
signed bytes, verifies their txid, and uses the normal consensus broadcast
intent/observation state machine. It cannot construct a new spend, change an
output, release a reservation, skip policy validation, or bypass threshold
signing.

## Rotation, replacement, and abandonment

Submit a typed lifecycle operation only after checking the active epochs:

```console
cdk-mintd federation frost-rotate \
  --operation rotation.json \
  --expected-root-epoch <epoch> \
  --expected-application-epoch <epoch> \
  --acknowledge-guarded-rotation
```

The command rejects a stale active-epoch expectation. Consensus then enforces
the registered-application usage contract, old-descriptor sweep requirements,
unresolved transaction guards, and root-share retirement rules described in
the [rotation runbook](federated-frost-key-rotation.md).

Abandonment and replacement are typed consensus operations. Accepted inputs
remain sticky because an older transaction may still reach Bitcoin; replacement
must use the reviewed fee-bump policy and a fresh FROST signing session. There
is intentionally no force-spend, force-release, share export, nonce export, or
private-key export command.

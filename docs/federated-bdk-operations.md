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

### Reorg recovery

One accepted reorg observation immediately moves the affected UTXO to
`reorged`. Before Cashu liability is issued, members can recover a false or
transient reorg only by reconfirming the exact original block in a new
observation epoch. Old confirmations do not count: the configured payment
observation quorum must confirm again before the UTXO becomes spendable.

Post-liability reorgs remain sticky and raise a liability alarm. Re-inclusion
in a different block is not recovered automatically. Do not edit the
chain-observation database, force a UTXO spendable, or run a fresh DKG to repair
either case.

Observation-epoch support changes federation consensus payloads. Upgrade and
restart all members together. For a pre-liability UTXO stuck by an older
epoch-zero reorg, upgraded members automatically submit epoch-one evidence on
their next successful same-block scan; no new Bitcoin deposit is required.
Confirm that the journal receives the configured quorum of new deposit
observations and that status moves from `reorged` to `spendable` without a
liability alarm.

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

## Replacement and abandonment

Abandonment and replacement are typed consensus operations. FROST root and
application key rotation is intentionally unsupported. Accepted inputs
remain sticky because an older transaction may still reach Bitcoin; replacement
must use the reviewed fee-bump policy and a fresh FROST signing session. There
is intentionally no force-spend, force-release, share export, nonce export, or
private-key export command.

Automatic RBF compares the accepted transaction's fee with a fresh, bounded
estimate for its confirmation target. Successful rebroadcasts do not trigger a
replacement by attempt count, and an unavailable or invalid estimate does not
trigger a fee bump using a fallback. Intent fee reserves and the consensus
incremental relay fee continue to bound every replacement.

An accepted replacement retains the original's rebroadcast eligibility until a
descendant has accepted successful broadcast evidence. Signing, broadcast
authorization, or a rejected replacement broadcast alone does not stop the
original. Confirmation polling continues for every signed lineage transaction,
so either version can settle the payment exactly once.

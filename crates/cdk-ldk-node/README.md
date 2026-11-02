# CDK LDK Node

CDK lightning backend for ldk-node, providing Lightning Network functionality for CDK with support for Cashu operations.

## Features

- Lightning Network payments (Bolt11 and Bolt12)
- Channel management
- Payment processing for Cashu mint operations
- Web management interface
- Support for multiple Bitcoin networks (Mainnet, Testnet, Signet/Mutinynet, Regtest)
- Esplora, Electrum, and Bitcoin Core RPC chain sources
- RGS (Rapid Gossip Sync) and P2P gossip support

## Quick Start

### Mutinynet (Recommended for Testing)

```bash
# Using environment variables (simplest)
export CDK_MINTD_PAYMENT_BACKEND="ldk-node"
export CDK_MINTD_LDK_NODE_BITCOIN_NETWORK="signet"
export CDK_MINTD_LDK_NODE_ESPLORA_URL="https://mutinynet.com/api"
export CDK_MINTD_LDK_NODE_RGS_URL="https://rgs.mutinynet.com/snapshot/0"
export CDK_MINTD_LDK_NODE_GOSSIP_SOURCE_TYPE="rgs"

cdk-mintd
```

After starting:
- Mint API: <http://127.0.0.1:8085>
- LDK management interface: <http://127.0.0.1:8091>
- Get test sats: [mutinynet.com](https://mutinynet.com)

**For complete network configuration examples, Docker setup, and production deployment, see [NETWORK_GUIDE.md](./NETWORK_GUIDE.md).**

## Web Management Interface

The CDK LDK Node includes a built-in web management interface accessible at `http://127.0.0.1:8091` by default.

⚠️ **SECURITY WARNING**: The web management interface has **NO AUTHENTICATION** and allows sending funds and managing channels. **NEVER expose it publicly** without proper authentication/authorization in front of it. Only bind to localhost (`127.0.0.1`) for security, or put it behind VPN, SSH tunneling, or authenticated reverse-proxy access. The dashboard includes CSRF protection and does not enable permissive browser CORS, but those browser hardening measures are not a substitute for access control.

### Key Features
- **Dashboard**: Node status, balance, and recent activity
- **Channel Management**: Open and close Lightning channels
- **Payment Management**: Create invoices, send payments, view history with pagination
- **On-chain Operations**: View balances and manage transactions

### Configuration

```toml
[ldk_node]
webserver_host = "127.0.0.1"  # IMPORTANT: Only localhost for security
webserver_port = 8091  # 0 = auto-assign port
```

Or via environment variables:
- `CDK_MINTD_LDK_NODE_WEBSERVER_HOST`
- `CDK_MINTD_LDK_NODE_WEBSERVER_PORT`

### Open a private channel from the command line

`POST /channels/open/private` opens an unannounced channel on the running node
with zero push to the peer. It accepts URL-encoded `_csrf`, `node_id`, `address`,
`port`, and `amount_sats` fields only. The amount is in sats; the port and amount
must be positive. IPv6 hosts must be bracketed, for example `[::1]`. Unknown or
duplicate fields are rejected, including visibility and push fields.

The existing `/channels/open` HTML form continues to open **public** channels.
Adding `visibility=private` to that old endpoint does not make it private.
Servers without the new endpoint return HTTP 404; never fall back to the old
endpoint.

| HTTP status | JSON response | Meaning |
| --- | --- | --- |
| 202 | `{"status":"initiated","user_channel_id":"12345678901234567890","announce_channel":false}` | Initiation only; wait for the channel to become ready. The local ID is a decimal string, not a funding transaction ID. |
| 400 | `{"error":"invalid_request","message":"...","outcome":"not_started"}` | Invalid request, rejected before calling LDK. |
| 403 | `{"error":"csrf_failed","message":"...","outcome":"not_started"}` | Missing or mismatched CSRF credentials. |
| 500 | `{"error":"channel_open_failed","message":"...","outcome":"unknown"}` | LDK returned an error; channel creation may already have started. |

After HTTP 500, an unexpected response, a timeout, or connection loss, inspect
channel state and node logs before considering another request. This operation
is not idempotent. It never retries or falls back to a public channel.

This example requires Bash, curl, awk, and jq. Set the peer and amount before
running it. Use localhost or an SSH tunnel to the management listener. Both curl
calls put `--disable` first to ignore personal `.curlrc` settings, disable retries
explicitly, bypass proxies, and do not follow redirects. CSRF protection is not
authentication; the management listener must remain access-restricted.

```bash
set -euo pipefail
umask 077
management_url='http://127.0.0.1:8091'
peer_id='<compressed-peer-public-key>'
peer_address='<peer-host>'
peer_port='9735'
channel_sats='500000'
private_open_tmp="$(mktemp -d)"
trap 'rm -rf -- "$private_open_tmp"' EXIT

# Obtain the existing server-issued CSRF cookie. Do not synthesize a token.
curl --disable --retry 0 --noproxy '*' --silent --show-error --fail --max-time 10 \
  --cookie-jar "$private_open_tmp/cookies" \
  --output /dev/null "$management_url/channels/open"
csrf_token="$(awk '$6 == "ldk_node_dashboard_csrf" { print $7 }' "$private_open_tmp/cookies")"
[[ "$csrf_token" =~ ^[0-9A-Fa-f]{64}$ ]]

# One POST only. A transport error may occur after initiation; never retry here.
if ! http_status="$(curl --disable --retry 0 --noproxy '*' --silent --show-error --max-time 90 \
  --cookie "$private_open_tmp/cookies" \
  --data-urlencode "_csrf=$csrf_token" \
  --data-urlencode "node_id=$peer_id" \
  --data-urlencode "address=$peer_address" \
  --data-urlencode "port=$peer_port" \
  --data-urlencode "amount_sats=$channel_sats" \
  --output "$private_open_tmp/response" --write-out '%{http_code}' \
  "$management_url/channels/open/private")"; then
  printf '%s\n' 'Outcome unknown: inspect channels and logs before any retry.' >&2
  exit 1
fi
cat "$private_open_tmp/response"
case "$http_status" in
  202)
    jq -e '.status == "initiated" and .announce_channel == false and
      (.user_channel_id | type == "string" and test("^[0-9]+$"))' \
      "$private_open_tmp/response" >/dev/null || {
        printf '%s\n' 'Unexpected receipt: inspect channels before any retry.' >&2
        exit 1
      }
    printf '\n%s\n' 'Initiated. Wait for the channel to become ready.'
    ;;
  404)
    printf '\n%s\n' 'Endpoint unavailable. Stop; do not use the public opening route.' >&2
    exit 1
    ;;
  400|403)
    printf '\n%s\n' 'Request rejected. Review the error; no automatic retry.' >&2
    exit 1
    ;;
  *)
    printf '\n%s\n' 'Outcome unknown: inspect channels and logs before any retry.' >&2
    exit 1
    ;;
esac
```

Private opening does not change existing public channels, disable forwarding
globally, or change invoice route-hint selection. Receiving over a private
channel also requires sufficient inbound liquidity and a discoverable route.

## Basic Configuration

### Config File Example

```toml
[payment_backend]
backend = "ldk-node"

[ldk_node]
bitcoin_network = "signet"  # mainnet, testnet, signet, regtest
chain_source_type = "esplora"  # esplora, electrum, or bitcoinrpc
esplora_url = "https://mutinynet.com/api"
rgs_url = "https://rgs.mutinynet.com/snapshot/0"
gossip_source_type = "rgs"  # rgs or p2p
webserver_port = 8091
```

### Environment Variables

All options can be set with `CDK_MINTD_LDK_NODE_` prefix:
- `CDK_MINTD_LDK_NODE_BITCOIN_NETWORK`
- `CDK_MINTD_LDK_NODE_CHAIN_SOURCE_TYPE`
- `CDK_MINTD_LDK_NODE_ESPLORA_URL`
- `CDK_MINTD_LDK_NODE_ELECTRUM_URL`
- `CDK_MINTD_LDK_NODE_RGS_URL`
- `CDK_MINTD_LDK_NODE_GOSSIP_SOURCE_TYPE`

**For detailed network configurations, Docker setup, production deployment, and troubleshooting, see [NETWORK_GUIDE.md](./NETWORK_GUIDE.md).**

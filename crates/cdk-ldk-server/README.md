# CDK LDK Server

Lightning backend for using a separate
[LDK Server](https://github.com/lightningdevkit/ldk-server) daemon with
`cdk-mintd`.

`ldk-server` exposes a TLS gRPC API with HMAC authentication. This backend uses
the upstream `ldk-server-client` crate and implements CDK's `MintPayment`
interface for BOLT11 and BOLT12 mint/melt flows.

## Configuration

```toml
[ln]
ln_backend = "ldk-server"
unit = "sat"

[ldk_server]
address = "127.0.0.1:3536"
api_key = "<hex API key>"
cert_path = "/path/to/ldk-server/tls.crt"
fee_percent = 0.02
reserve_fee_min = 2
```

## Environment Variables

| Variable | Description | Required |
| --- | --- | --- |
| `CDK_MINTD_LN_BACKEND` | Set to `ldk-server` | Yes |
| `CDK_MINTD_LDK_SERVER_ADDRESS` | `ldk-server` host and port, without scheme | Yes |
| `CDK_MINTD_LDK_SERVER_API_KEY` | HMAC API key as expected by `ldk-server-client` | Yes |
| `CDK_MINTD_LDK_SERVER_CERT_PATH` | Path to the pinned TLS certificate PEM | Yes |
| `CDK_MINTD_LDK_SERVER_FEE_PERCENT` | Fee reserve percentage, default `0.02` | No |
| `CDK_MINTD_LDK_SERVER_RESERVE_FEE_MIN` | Minimum fee reserve in sats, default `2` | No |
| `CDK_MINTD_LDK_SERVER_MAX_PAYMENT_SCAN_PAGES` | Max `ListPayments` pages scanned for incoming status lookup, default `32` | No |

## Notes

`ldk-server` currently looks up payment details by `payment_id`, while CDK
incoming mint quotes are keyed by BOLT11 payment hash or BOLT12 offer ID. This
backend therefore uses event notifications and bounded `ListPayments` scans for
incoming quote status checks.

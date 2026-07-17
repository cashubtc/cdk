# cdk-enclavia

`cdk-enclavia` connects a CDK wallet to a mint running inside an
[Enclavia](https://enclavia.io) enclave. It establishes Enclavia's encrypted
Noise channel and verifies the enclave's Nitro attestation against pinned
PCR0, PCR1, and PCR2 values before constructing a CDK mint connector.

Mint requests fail closed: a request addressed to the configured mint is never
retried over ordinary HTTP if the attested channel fails. Requests to unrelated
services, such as LNURL callbacks or an external OIDC issuer, continue to use
CDK's regular HTTP transport.

```rust,no_run
use cdk::mint_url::MintUrl;
use cdk::wallet::WalletBuilder;
use cdk_enclavia::{EnclaviaClientBuilder, Pcrs};

# async fn example(mint_url: MintUrl) -> Result<(), Box<dyn std::error::Error>> {
let pcrs = Pcrs::from_hex(
    &"00".repeat(48),
    &"11".repeat(48),
    &"22".repeat(48),
)?;

let client = EnclaviaClientBuilder::new(
    mint_url.clone(),
    "wss://example.enclaves.beta.enclavia.io",
    pcrs,
)
.build()
.await?;

let _builder = WalletBuilder::new()
    .mint_url(mint_url)
    .client(client)
    // Enclavia WebSocket upgrades are not adapted to CDK subscriptions yet.
    .use_http_subscription();
# Ok(())
# }
```

## WebSocket subscriptions

The initial implementation supports all wallet HTTP and auth operations.
NUT-17 WebSocket connections are rejected rather than opened outside the
attested tunnel. Configure the wallet with `use_http_subscription()` until an
Enclavia upgraded stream adapter is added to `cdk-http-client`.

## MSRV

The `enclavia` 0.1.0 dependency requires Rust 1.88, so this crate currently has
an MSRV of 1.88. The rest of CDK retains its Rust 1.85 MSRV.

# Mint Connectors

This module provides different ways to connect wallets to Cashu mints.

## HTTP Connector

The standard `HttpClient` provides direct HTTP communication with mints:

```rust
use cdk::wallet::mint_connector::HttpClient;
use cdk::mint_url::MintUrl;

let mint_url = MintUrl::from("https://mint.example.com")?;
let client = HttpClient::new(mint_url);
```

## OHTTP Connector

The `OhttpClient` provides privacy-enhanced communication through OHTTP gateways or relays:

### Using an OHTTP Gateway

```rust
use cdk::wallet::mint_connector::OhttpClient;
use cdk::mint_url::MintUrl;
use url::Url;

let mint_url = MintUrl::from("https://mint.example.com")?;
let gateway_url = Url::parse("https://gateway.example.com")?;
let client = OhttpClient::new_with_gateway(mint_url, gateway_url);
```

### Using an OHTTP Relay

```rust
use cdk::wallet::mint_connector::OhttpClient;
use cdk::mint_url::MintUrl;
use url::Url;

let mint_url = MintUrl::from("https://mint.example.com")?;
let relay_url = Url::parse("https://relay.example.com")?;
let gateway_url = Some(Url::parse("https://gateway.example.com")?);
let client = OhttpClient::new_with_relay(mint_url, relay_url, gateway_url);
```

### Using Pre-loaded OHTTP Keys

```rust
use cdk::wallet::mint_connector::OhttpClient;
use cdk::mint_url::MintUrl;
use url::Url;

let mint_url = MintUrl::from("https://mint.example.com")?;
let target_url = Url::parse("https://gateway.example.com")?;
let keys_source_url = Url::parse("https://gateway.example.com")?;
let ohttp_keys = std::fs::read("ohttp_keys.bin")?;

let client = OhttpClient::new_with_keys(
    mint_url,
    target_url,
    false, // not a relay
    None,  // no relay gateway URL
    ohttp_keys,
    keys_source_url,
);
```

## Usage

All connectors implement the `MintConnector` trait, so they can be used interchangeably:

```rust
use cdk::wallet::mint_connector::MintConnector;

async fn mint_info(client: &dyn MintConnector) -> Result<(), Error> {
    let info = client.get_mint_info().await?;
    println!("Mint: {}", info.name.unwrap_or_default());
    Ok(())
}
```

## Features

- **HTTP Connector**: Direct, fast communication
- **OHTTP Connector**: Privacy-enhanced communication through OHTTP protocol
  - Gateway mode: Direct connection to OHTTP gateway
  - Relay mode: Connection through OHTTP relay to gateway
  - Pre-loaded keys: Use cached OHTTP keys for faster initialization

## Privacy Benefits of OHTTP

The OHTTP (Oblivious HTTP) protocol provides:

1. **Request Privacy**: The gateway cannot see request contents
2. **Response Privacy**: The gateway cannot see response contents
3. **Metadata Protection**: Connection metadata is separated from request data
4. **Forward Secrecy**: Each request uses fresh encryption keys

This makes it much harder for network observers to correlate wallet activities with specific users.

## Performance Considerations

- OHTTP adds some latency due to encryption/decryption overhead
- Network topology (gateway/relay locations) affects performance
- Pre-loading OHTTP keys can reduce initialization time
- Consider caching strategies for frequently accessed data

## Examples

See the `examples/` directory for complete usage examples:

- `ohttp_mint_connector.rs`: Basic OHTTP connector usage

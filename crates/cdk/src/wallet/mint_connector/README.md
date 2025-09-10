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

The OHTTP connector provides privacy-enhanced communication through OHTTP gateways or relays using the same `HttpClient` with an OHTTP transport:

### Using an OHTTP Gateway

```rust
use cdk::wallet::mint_connector::{http_client::HttpClient, ohttp_transport::OhttpTransport};
use cdk::mint_url::MintUrl;
use url::Url;

let mint_url = MintUrl::from("https://mint.example.com")?;
let gateway_url = Url::parse("https://gateway.example.com")?;
let keys_source_url = gateway_url.clone(); // Keys fetched from same gateway

// Create OHTTP transport
let transport = OhttpTransport::new_with_gateway(gateway_url, keys_source_url);

// Create HTTP client with OHTTP transport  
let client = HttpClient::with_transport(mint_url, transport);
```

### Using an OHTTP Relay

```rust
use cdk::wallet::mint_connector::{http_client::HttpClient, ohttp_transport::OhttpTransport};
use cdk::mint_url::MintUrl;
use url::Url;

let mint_url = MintUrl::from("https://mint.example.com")?;
let gateway_url = Url::parse("https://gateway.example.com")?;
let relay_url = Url::parse("https://relay.example.com")?;
let keys_source_url = gateway_url.clone();

// Create OHTTP transport with relay
let transport = OhttpTransport::new(mint_url.as_url().clone(), gateway_url, relay_url, keys_source_url);

// Create HTTP client with OHTTP transport
let client = HttpClient::with_transport(mint_url, transport);
```

### Using Pre-loaded OHTTP Keys

```rust
use cdk::wallet::mint_connector::{http_client::HttpClient, ohttp_transport::OhttpTransport};
use cdk::mint_url::MintUrl;
use url::Url;

let mint_url = MintUrl::from("https://mint.example.com")?;
let gateway_url = Url::parse("https://gateway.example.com")?;
let ohttp_keys = std::fs::read("ohttp_keys.bin")?;

// Create OHTTP transport with pre-loaded keys
let transport = OhttpTransport::new_with_keys(gateway_url, ohttp_keys);

// Create HTTP client with OHTTP transport
let client = HttpClient::with_transport(mint_url, transport);
```

### Convenient Type Alias

For easier usage, you can also use the type alias:

```rust
use cdk::wallet::mint_connector::OhttpHttpClient;
use cdk::mint_url::MintUrl;
use url::Url;

let mint_url = MintUrl::from("https://mint.example.com")?;
let gateway_url = Url::parse("https://gateway.example.com")?;
let keys_source_url = gateway_url.clone();

// Create OHTTP transport
let transport = ohttp_transport::OhttpTransport::new_with_gateway(gateway_url, keys_source_url);

// Use the convenient type alias
let client: OhttpHttpClient = HttpClient::with_transport(mint_url, transport);
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

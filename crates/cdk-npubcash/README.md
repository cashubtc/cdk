# cdk-npubcash

Rust client SDK for the NpubCash v2 API.

## Features

- **HTTP Client**: Fetch quotes with automatic pagination
- **NIP-98 Authentication**: Sign requests using Nostr keys
- **JWT Token Caching**: Automatic token refresh and caching
- **WebSocket Subscriptions**: Real-time quote updates
- **Settings Management**: Configure mint URL and quote locking

## Usage

```rust
use cdk_npubcash::{NpubCashClient, JwtAuthProvider};
use nostr_sdk::Keys;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base_url = "https://npubx.cash".to_string();
    let keys = Keys::generate();
    
    let auth_provider = JwtAuthProvider::new(base_url.clone(), keys);
    let client = NpubCashClient::new(base_url, std::sync::Arc::new(auth_provider));
    
    // Fetch all quotes
    let quotes = client.get_all_quotes().await?;
    println!("Found {} quotes", quotes.len());
    
    // Fetch quotes since timestamp
    let recent_quotes = client.get_quotes_since(1234567890).await?;
    
    // Update mint URL setting
    client.settings.set_mint_url("https://example-mint.tld").await?;
    
    Ok(())
}
```

## WebSocket Subscriptions

```rust
use cdk_npubcash::SubscriptionManager;

let ws_url = "wss://npubx.cash/api/v2/wallet/quotes/subscribe".to_string();
let subscription_manager = SubscriptionManager::new(ws_url, auth_provider);

let handle = subscription_manager.subscribe(|quote_id| {
    println!("Quote updated: {}", quote_id);
}).await?;

// Keep handle alive to maintain subscription
// Drop handle to cancel subscription
```

## Examples

The SDK includes several examples to help you get started:

- **`basic_usage.rs`** - Demonstrates fetching quotes and basic client usage
- **`subscribe_quotes.rs`** - Shows how to subscribe to real-time quote updates
- **`manage_settings.rs`** - Illustrates settings management (mint URL)
- **`create_and_wait_payment.rs`** - Demonstrates creating a npub.cash address and monitoring for payments

**Note:** Quotes are always locked by default on the NPubCash server for security. The ability to toggle quote locking has been removed from the SDK.

Run an example with:
```bash
cargo run --example basic_usage
```

Set environment variables to customize behavior:
```bash
export NPUBCASH_URL=https://npubx.cash
export NOSTR_NSEC=nsec1...  # Optional: use specific Nostr keys
cargo run --example create_and_wait_payment
```

## Authentication

The SDK uses NIP-98 (Nostr HTTP Auth) to authenticate with the NpubCash service:

1. Creates a NIP-98 signed event with the request URL and method
2. Exchanges the NIP-98 token for a JWT token
3. Caches the JWT token for subsequent requests
4. Automatically refreshes the token when it expires

## License

MIT

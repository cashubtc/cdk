# CDK Nostr

Nostr support for the Cashu Development Kit in a single crate.

## Modules

- **`keys`** — secret key generation/parsing and x-only public key derivation.
- **`nip44`** — NIP-44 v2 authenticated encryption between Nostr keys.
- **`inbox`** — standing NIP-17 inbox listener: subscribes relays for gift
  wraps addressed to an identity, unwraps them, and delivers the rumors to a
  `NostrInboxListener` callback.
- **`nwc`** (feature `nwc`) — NIP-47 Nostr Wallet Connect wallet service.
- **`npubcash`** (feature `npubcash`) — npub.cash API client (quotes, settings,
  NIP-98/JWT auth).

## Example

```rust,no_run
use std::sync::Arc;

use cdk_nostr::inbox::{Nip17Event, NostrInbox, NostrInboxListener};
use cdk_nostr::keys;
use cdk_nostr::nostr_sdk::{RelayUrl, Timestamp};

struct Printer;

impl NostrInboxListener for Printer {
    fn on_event(&self, event: Nip17Event) {
        println!("gift wrap {} from {}", event.wrap_id, event.sender);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let secret_key = keys::generate_secret_key();
    let pubkey = keys::public_key(&secret_key);
    println!("npub: {pubkey}");

    let one_week_ago = Timestamp::from_secs(
        Timestamp::now().as_secs().saturating_sub(7 * 24 * 60 * 60),
    );
    let inbox = NostrInbox::new(
        secret_key,
        vec![RelayUrl::parse("wss://relay.damus.io")?],
        Some(one_week_ago),
    )?;
    inbox.start(Arc::new(Printer)).await?;

    // ... run until done, then:
    inbox.stop();
    Ok(())
}
```

## License

MIT

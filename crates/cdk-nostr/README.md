# CDK Nostr

Nostr support for the Cashu Development Kit in a single crate.

## Modules

- **`keys`** — secret key generation/parsing, x-only public key derivation,
  and default NIP-06 identity derivation (`m/44'/1237'/0'/0/0`) from a BIP-39
  mnemonic or seed.
- **`nip44`** — NIP-44 v2 authenticated encryption between Nostr keys.
- **`inbox`** — standing NIP-17 inbox listener: subscribes relays for gift
  wraps addressed to an identity, strictly validates every NIP-59 layer, and
  delivers verified rumors to a `NostrInboxListener` callback. Start, stop,
  and restart are idempotent; async stop waits until callbacks are quiescent.
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
    let identity = keys::derive_nip06_keys_from_mnemonic(
        "leader monkey parrot ring guide accident before fence cannon height naive bean",
        None,
    )?;
    let pubkey = identity.public_key();
    println!("npub: {pubkey}");

    let one_week_ago = Timestamp::from_secs(
        Timestamp::now().as_secs().saturating_sub(7 * 24 * 60 * 60),
    );
    let inbox = NostrInbox::from_keys(
        identity,
        vec![RelayUrl::parse("wss://relay.damus.io")?],
        Some(one_week_ago),
    )?;
    inbox.start(Arc::new(Printer)).await?;

    // ... run until done, then:
    inbox.stop().await;
    Ok(())
}
```

The `since` value is a fixed floor for the inbox lifetime. Keep a generous
lookback because NIP-59 intentionally randomizes and backdates gift-wrap
timestamps. Relay reconnection uses bounded adaptive backoff and reuses the
same subscription filter.

## License

MIT

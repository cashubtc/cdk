//! Use one typed Rust-owned identity for mobile-facing Nostr operations.

use std::sync::Arc;

use cdk_ffi::{NostrSigner, NostrUnsignedEvent, NpubCashClient};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Public NIP-06 test vector. A real wallet passes its own BIP-39 mnemonic;
    // the seed is calculated inside Rust and never crosses the FFI boundary.
    let signer = Arc::new(NostrSigner::from_mnemonic(
        "leader monkey parrot ring guide accident before fence cannon height naive bean"
            .to_string(),
        None,
    )?);

    let event = signer.sign_event(NostrUnsignedEvent {
        created_at: 1_700_000_000,
        kind: 27_235,
        tags: vec![
            vec!["u".to_string(), "https://example.com/api".to_string()],
            vec!["method".to_string(), "POST".to_string()],
        ],
        content: String::new(),
    })?;
    println!("signed NIP-98 event {} as {}", event.id, event.pubkey);
    println!("Cashu P2PK key: {}", signer.cashu_p2pk_public_key());

    let npubcash = NpubCashClient::with_signer("https://npub.cash".to_string(), signer);
    println!("npub.cash identity: {}", npubcash.identity_pubkey());

    Ok(())
}

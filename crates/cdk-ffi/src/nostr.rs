//! FFI bindings for Nostr key management, NIP-44 encryption and the NIP-17
//! inbox listener.
//!
//! These functions cover the secp256k1-based cryptography a wallet needs for
//! Nostr, so foreign-language wallets do not have to bundle a separate
//! secp256k1 library.

use std::sync::Arc;

use cdk_nostr::inbox::{Nip17Event, NostrInbox as CdkNostrInbox};
use cdk_nostr::nostr::prelude::{RelayUrl, Timestamp};
use cdk_nostr::{keys as nostr_keys, nip44 as cdk_nip44};

use crate::error::FfiError;

/// Generate a new random Nostr secret key
///
/// # Returns
///
/// The hex-encoded secret key (64 characters)
#[uniffi::export]
pub fn nostr_generate_secret_key() -> String {
    nostr_keys::generate_secret_key().to_secret_hex()
}

/// Get the public key for a Nostr secret key
///
/// # Arguments
///
/// * `nostr_secret_key` - Nostr secret key. Accepts either:
///   - Hex-encoded secret key (64 characters)
///   - Bech32 `nsec` format (e.g., "nsec1...")
///
/// # Returns
///
/// The hex-encoded x-only public key (64 characters)
///
/// # Errors
///
/// Returns an error if the secret key is invalid
#[uniffi::export]
pub fn nostr_get_pubkey(nostr_secret_key: String) -> Result<String, FfiError> {
    let secret_key = nostr_keys::parse_secret_key(&nostr_secret_key)
        .map_err(|e| FfiError::internal(e.to_string()))?;
    Ok(nostr_keys::public_key(&secret_key).to_hex())
}

/// Encrypt a message with NIP-44 v2
///
/// Derives the conversation key via secp256k1 ECDH between `nostr_secret_key`
/// and `recipient_pubkey` and returns the base64-encoded payload.
///
/// # Arguments
///
/// * `nostr_secret_key` - Sender secret key (hex or bech32 `nsec`)
/// * `recipient_pubkey` - Recipient x-only public key (hex, 64 characters)
/// * `plaintext` - Message to encrypt
///
/// # Errors
///
/// Returns an error if a key is invalid or encryption fails
#[uniffi::export]
pub fn nip44_encrypt(
    nostr_secret_key: String,
    recipient_pubkey: String,
    plaintext: String,
) -> Result<String, FfiError> {
    let secret_key = nostr_keys::parse_secret_key(&nostr_secret_key)
        .map_err(|e| FfiError::internal(e.to_string()))?;
    let public_key = nostr_keys::parse_public_key(&recipient_pubkey)
        .map_err(|e| FfiError::internal(e.to_string()))?;
    cdk_nip44::encrypt(&secret_key, &public_key, &plaintext)
        .map_err(|e| FfiError::internal(e.to_string()))
}

/// Decrypt a NIP-44 v2 payload
///
/// Derives the conversation key via secp256k1 ECDH between `nostr_secret_key`
/// and `sender_pubkey` and decrypts the base64-encoded payload.
///
/// # Arguments
///
/// * `nostr_secret_key` - Recipient secret key (hex or bech32 `nsec`)
/// * `sender_pubkey` - Sender x-only public key (hex, 64 characters)
/// * `payload` - Base64-encoded NIP-44 v2 payload
///
/// # Errors
///
/// Returns an error if a key is invalid, the payload is malformed, or MAC
/// verification fails
#[uniffi::export]
pub fn nip44_decrypt(
    nostr_secret_key: String,
    sender_pubkey: String,
    payload: String,
) -> Result<String, FfiError> {
    let secret_key = nostr_keys::parse_secret_key(&nostr_secret_key)
        .map_err(|e| FfiError::internal(e.to_string()))?;
    let public_key = nostr_keys::parse_public_key(&sender_pubkey)
        .map_err(|e| FfiError::internal(e.to_string()))?;
    cdk_nip44::decrypt(&secret_key, &public_key, &payload)
        .map_err(|e| FfiError::internal(e.to_string()))
}

/// An unwrapped NIP-17 gift wrap received by a [`NostrInbox`]
///
/// All IDs and keys are hex-encoded; timestamps are unix seconds.
#[derive(uniffi::Record)]
pub struct NostrInboxEvent {
    /// ID of the (ephemeral) kind `1059` gift wrap event. Use it to
    /// de-duplicate deliveries across relay reconnects and restarts.
    pub wrap_id: String,
    /// `created_at` of the gift wrap (NIP-59 randomizes/backdates it)
    pub wrap_created_at: u64,
    /// Author of the verified seal — the real sender of the rumor
    pub sender_pubkey: String,
    /// ID of the rumor, if the sender included one
    pub rumor_id: Option<String>,
    /// Kind of the rumor (commonly `14` for chat/DM payloads)
    pub rumor_kind: u16,
    /// Content of the rumor (e.g. a NUT-18 payment request payload for
    /// kind `14` rumors)
    pub rumor_content: String,
    /// `created_at` of the rumor
    pub rumor_created_at: u64,
    /// Tags of the rumor
    pub rumor_tags: Vec<Vec<String>>,
}

impl From<Nip17Event> for NostrInboxEvent {
    fn from(event: Nip17Event) -> Self {
        Self {
            wrap_id: event.wrap_id.to_hex(),
            wrap_created_at: event.wrap_created_at.as_secs(),
            sender_pubkey: event.sender.to_hex(),
            rumor_id: event.rumor.id.map(|id| id.to_hex()),
            rumor_kind: event.rumor.kind.as_u16(),
            rumor_content: event.rumor.content,
            rumor_created_at: event.rumor.created_at.as_secs(),
            rumor_tags: event
                .rumor
                .tags
                .iter()
                .map(|tag| tag.as_slice().to_vec())
                .collect(),
        }
    }
}

/// Callback interface for [`NostrInbox`] events
///
/// Implementations must be non-blocking; hand expensive work (token claims,
/// database writes) off to a separate task.
#[uniffi::export(with_foreign)]
pub trait NostrInboxListener: Send + Sync {
    /// Called once per successfully unwrapped gift wrap
    fn on_event(&self, event: NostrInboxEvent);
}

/// Bridge from the Rust listener trait to the FFI callback interface
struct FfiInboxListener {
    ffi_listener: Arc<dyn NostrInboxListener>,
}

impl cdk_nostr::inbox::NostrInboxListener for FfiInboxListener {
    fn on_event(&self, event: Nip17Event) {
        self.ffi_listener.on_event(event.into());
    }
}

/// A standing NIP-17 inbox listener for a single Nostr identity
///
/// Subscribes the given relays for gift wraps addressed to the identity's
/// public key and delivers unwrapped rumors to a [`NostrInboxListener`].
#[derive(uniffi::Object)]
pub struct NostrInbox {
    inner: CdkNostrInbox,
}

#[uniffi::export(async_runtime = "tokio")]
impl NostrInbox {
    /// Create a new inbox listener
    ///
    /// # Arguments
    ///
    /// * `nostr_secret_key` - The identity's secret key (hex or bech32 `nsec`)
    /// * `relays` - Relay URLs (`ws://`/`wss://`) to subscribe; must be
    ///   non-empty
    /// * `since` - Optional unix timestamp lower bound for the relay `since`
    ///   filter. Because NIP-59 backdates gift wraps, pick a generous lookback
    ///   window instead of "now".
    ///
    /// # Errors
    ///
    /// Returns an error if the secret key or a relay URL is invalid, or no
    /// relays are configured.
    #[uniffi::constructor]
    pub fn new(
        nostr_secret_key: String,
        relays: Vec<String>,
        since: Option<u64>,
    ) -> Result<Self, FfiError> {
        let secret_key = nostr_keys::parse_secret_key(&nostr_secret_key)
            .map_err(|e| FfiError::internal(e.to_string()))?;
        let relay_urls = relays
            .iter()
            .map(|relay| {
                RelayUrl::parse(relay)
                    .map_err(|e| FfiError::internal(format!("invalid relay {relay}: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let since = since.map(Timestamp::from_secs);

        let inner = CdkNostrInbox::new(secret_key, relay_urls, since)
            .map_err(|e| FfiError::internal(e.to_string()))?;
        Ok(Self { inner })
    }

    /// Public key of the inbox identity (hex-encoded, x-only)
    pub fn pubkey(&self) -> String {
        self.inner.pubkey().to_hex()
    }

    /// Connect to the relays, activate the subscription and start delivering
    /// events to `listener` on a background task
    ///
    /// Returns once the subscription is active. Events are delivered until
    /// [`NostrInbox::stop`] is called.
    ///
    /// # Errors
    ///
    /// Returns an error if a relay cannot be added or the subscription cannot
    /// be created.
    pub async fn start(&self, listener: Arc<dyn NostrInboxListener>) -> Result<(), FfiError> {
        self.inner
            .start(Arc::new(FfiInboxListener {
                ffi_listener: listener,
            }))
            .await
            .map_err(|e| FfiError::internal(e.to_string()))
    }

    /// Stop listening: cancels the background pump and disconnects from the
    /// relays
    pub fn stop(&self) {
        self.inner.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nostr_get_pubkey_derives_xonly_hex() {
        let pubkey = nostr_get_pubkey(
            "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        )
        .expect("valid secret key");
        assert_eq!(
            pubkey,
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        );
    }

    #[test]
    fn nip44_roundtrip_via_ffi() {
        let alice = nostr_generate_secret_key();
        let bob = nostr_generate_secret_key();
        let bob_pub = nostr_get_pubkey(bob.clone()).expect("bob pubkey");
        let alice_pub = nostr_get_pubkey(alice.clone()).expect("alice pubkey");

        let payload = nip44_encrypt(alice, bob_pub, "hello cashu".to_string()).expect("encrypt");
        let decrypted = nip44_decrypt(bob, alice_pub, payload).expect("decrypt");
        assert_eq!(decrypted, "hello cashu");
    }

    #[test]
    fn inbox_rejects_invalid_relay_url() {
        let secret = nostr_generate_secret_key();
        let result = NostrInbox::new(secret, vec!["not-a-url".to_string()], None);
        assert!(result.is_err());
    }

    #[test]
    fn nostr_get_pubkey_rejects_invalid_key() {
        assert!(nostr_get_pubkey("definitely-not-a-key".to_string()).is_err());
    }

    #[test]
    fn nip44_encrypt_rejects_invalid_recipient_pubkey() {
        let alice = nostr_generate_secret_key();
        assert!(nip44_encrypt(alice, "not-a-pubkey".to_string(), "hi".to_string()).is_err());
    }

    #[test]
    fn nip44_decrypt_rejects_wrong_recipient_key() {
        let alice = nostr_generate_secret_key();
        let bob = nostr_generate_secret_key();
        let eve = nostr_generate_secret_key();
        let bob_pub = nostr_get_pubkey(bob).expect("bob pubkey");
        let alice_pub = nostr_get_pubkey(alice.clone()).expect("alice pubkey");

        let payload = nip44_encrypt(alice, bob_pub, "secret".to_string()).expect("encrypt");
        assert!(nip44_decrypt(eve, alice_pub, payload).is_err());
    }

    #[test]
    fn inbox_requires_at_least_one_relay() {
        let secret = nostr_generate_secret_key();
        assert!(NostrInbox::new(secret, vec![], None).is_err());
    }

    #[test]
    fn inbox_rejects_invalid_secret_key() {
        assert!(NostrInbox::new(
            "bad-key".to_string(),
            vec!["wss://relay.example.com".to_string()],
            None
        )
        .is_err());
    }

    #[test]
    fn inbox_pubkey_matches_identity_key() {
        let secret = nostr_generate_secret_key();
        let expected = nostr_get_pubkey(secret.clone()).expect("pubkey derives");
        let inbox = NostrInbox::new(
            secret,
            vec!["wss://relay.example.com".to_string()],
            Some(1_700_000_000),
        )
        .expect("valid inbox builds without connecting");
        assert_eq!(inbox.pubkey(), expected);
    }
}

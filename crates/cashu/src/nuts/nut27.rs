//! NUT-27: Nostr Mint Backup
//!
//! <https://github.com/cashubtc/nuts/blob/main/27.md>
//!
//! This NUT describes a method for wallets to backup their mint list as Nostr events.
//! The backup keys are deterministically derived from the wallet's mnemonic seed phrase.

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use nostr_sdk::nips::nip44::{self, Version};
use nostr_sdk::{Event, EventBuilder, Keys, Kind, Tag, TagKind};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::mint_url::MintUrl;

/// Domain separator for mint backup key derivation
const DOMAIN_SEPARATOR: &[u8] = b"cashu-mint-backup";

/// Event kind for addressable events (NIP-78)
const KIND_APPLICATION_SPECIFIC_DATA: u16 = 30078;

/// The "d" tag identifier for mint list backup
const MINT_LIST_IDENTIFIER: &str = "mint-list";

/// NUT-27 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Nostr key error
    #[error(transparent)]
    NostrKey(#[from] nostr_sdk::key::Error),
    /// Nostr event builder error
    #[error(transparent)]
    NostrEventBuilder(#[from] nostr_sdk::event::builder::Error),
    /// NIP-44 encryption error
    #[error(transparent)]
    Nip44(#[from] nip44::Error),
    /// JSON serialization error
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Invalid event kind
    #[error("Invalid event kind: expected {expected}, got {got}")]
    InvalidEventKind {
        /// Expected kind
        expected: u16,
        /// Actual kind
        got: u16,
    },
    /// Missing "d" tag
    #[error("Missing 'd' tag with identifier '{0}'")]
    MissingIdentifierTag(String),
}

/// Mint backup data structure
///
/// This represents the plaintext data that gets encrypted in the Nostr event content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintBackup {
    /// List of mint URLs
    pub mints: Vec<MintUrl>,
    /// Unix timestamp of when the backup was created
    pub timestamp: u64,
}

impl MintBackup {
    /// Create a new mint backup with the current timestamp
    pub fn new(mints: Vec<MintUrl>) -> Self {
        let timestamp = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self { mints, timestamp }
    }

    /// Create a new mint backup with a specific timestamp
    pub fn with_timestamp(mints: Vec<MintUrl>, timestamp: u64) -> Self {
        Self { mints, timestamp }
    }
}

/// Derive Nostr keys for mint backup from a BIP39 seed
///
/// The derivation follows the NUT-27 specification:
/// 1. Concatenate the seed with the domain separator "cashu-mint-backup"
/// 2. Hash the combined data with SHA256 to produce the private key
/// 3. Derive the public key from the private key
///
/// # Arguments
///
/// * `seed` - A 64-byte seed derived from a BIP39 mnemonic
///
/// # Returns
///
/// A nostr_sdk `Keys` struct containing the derived secret and public keys
///
/// # Example
///
/// ```
/// use std::str::FromStr;
///
/// use bip39::Mnemonic;
/// use cashu::nuts::nut27::derive_nostr_keys;
///
/// let mnemonic = Mnemonic::from_str(
///     "half depart obvious quality work element tank gorilla view sugar picture humble",
/// )
/// .unwrap();
/// let seed: [u8; 64] = mnemonic.to_seed("");
/// let keys = derive_nostr_keys(&seed).unwrap();
/// ```
pub fn derive_nostr_keys(seed: &[u8; 64]) -> Result<Keys, Error> {
    let mut combined_data = Vec::with_capacity(seed.len() + DOMAIN_SEPARATOR.len());
    combined_data.extend_from_slice(seed);
    combined_data.extend_from_slice(DOMAIN_SEPARATOR);

    let hash = Sha256Hash::hash(&combined_data);
    let private_key_bytes = hash.to_byte_array();

    let secret_key = nostr_sdk::SecretKey::from_slice(&private_key_bytes)?;
    let keys = Keys::new(secret_key);

    Ok(keys)
}

/// Create a Nostr backup event for the mint list
///
/// This creates a NIP-78 addressable event (kind 30078) with the mint list
/// encrypted using NIP-44. The event is self-encrypted using the same key
/// for both sender and receiver.
///
/// # Arguments
///
/// * `keys` - The Nostr keys derived from the wallet seed
/// * `backup` - The mint backup data to encrypt
/// * `client` - Optional client name to include in the event tags
///
/// # Returns
///
/// A signed Nostr event ready to be published to relays
pub fn create_backup_event(
    keys: &Keys,
    backup: &MintBackup,
    client: Option<&str>,
) -> Result<Event, Error> {
    let plaintext = serde_json::to_string(backup)?;

    // Self-encryption: same key for sender and receiver per NIP-44
    let encrypted_content = nip44::encrypt(
        keys.secret_key(),
        &keys.public_key(),
        plaintext,
        Version::V2,
    )?;

    let mut builder = EventBuilder::new(
        Kind::Custom(KIND_APPLICATION_SPECIFIC_DATA),
        encrypted_content,
    )
    .tag(Tag::identifier(MINT_LIST_IDENTIFIER));

    if let Some(client_name) = client {
        builder = builder.tag(Tag::custom(
            nostr_sdk::TagKind::Custom(std::borrow::Cow::Borrowed("client")),
            [client_name],
        ));
    }

    let event = builder.sign_with_keys(keys)?;

    Ok(event)
}

/// Decrypt and parse a mint backup event
///
/// This decrypts a NIP-78 event containing an encrypted mint list and
/// returns the parsed backup data.
///
/// # Arguments
///
/// * `keys` - The Nostr keys derived from the wallet seed
/// * `event` - The Nostr event to decrypt
///
/// # Returns
///
/// The decrypted mint backup data
pub fn decrypt_backup_event(keys: &Keys, event: &Event) -> Result<MintBackup, Error> {
    let expected_kind = Kind::Custom(KIND_APPLICATION_SPECIFIC_DATA);
    if event.kind != expected_kind {
        return Err(Error::InvalidEventKind {
            expected: KIND_APPLICATION_SPECIFIC_DATA,
            got: event.kind.as_u16(),
        });
    }

    let has_mint_list_tag = event
        .tags
        .iter()
        .any(|tag| tag.kind() == TagKind::d() && tag.content() == Some(MINT_LIST_IDENTIFIER));

    if !has_mint_list_tag {
        return Err(Error::MissingIdentifierTag(
            MINT_LIST_IDENTIFIER.to_string(),
        ));
    }

    let decrypted = nip44::decrypt(keys.secret_key(), &keys.public_key(), &event.content)?;

    let backup: MintBackup = serde_json::from_str(&decrypted)?;

    Ok(backup)
}

/// Create a Nostr filter for discovering mint backup events
///
/// This creates filter parameters that can be used to query relays
/// for mint backup events created by the given public key.
///
/// # Arguments
///
/// * `keys` - The Nostr keys derived from the wallet seed
///
/// # Returns
///
/// A tuple of (kind, authors, d_tag) that can be used to construct a filter
pub fn backup_filter_params(keys: &Keys) -> (Kind, nostr_sdk::PublicKey, &'static str) {
    (
        Kind::Custom(KIND_APPLICATION_SPECIFIC_DATA),
        keys.public_key(),
        MINT_LIST_IDENTIFIER,
    )
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bip39::Mnemonic;

    use super::*;

    fn test_keys() -> Keys {
        let mnemonic = Mnemonic::from_str(
            "half depart obvious quality work element tank gorilla view sugar picture humble",
        )
        .unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");
        derive_nostr_keys(&seed).unwrap()
    }

    #[test]
    fn test_derive_nostr_keys_from_seed() {
        let keys = test_keys();

        // Verify the keys are valid
        let secret_key = keys.secret_key();
        let public_key = keys.public_key();

        // Secret key should be 32 bytes
        assert_eq!(secret_key.as_secret_bytes().len(), 32);

        // Public key in nostr is x-only (32 bytes), displayed as 64 hex chars
        assert_eq!(public_key.to_hex().len(), 64);
    }

    /// Test vector for key derivation from BIP39 mnemonic.
    ///
    /// Mnemonic: "half depart obvious quality work element tank gorilla view sugar picture humble"
    /// Expected secret key: e7ca79469a270b36617e4227ff2f068d3bcbb6b072c8584190b0203597c53c0d
    /// Expected public key: 0767277aaed200af7a8843491745272fc1ad2c7bfe340225e6f34f3a9a273aed
    #[test]
    fn test_key_derivation_vector() {
        use crate::util::hex;

        let keys = test_keys();

        // Test vector: secret key
        let expected_secret_key =
            "e7ca79469a270b36617e4227ff2f068d3bcbb6b072c8584190b0203597c53c0d";
        assert_eq!(
            hex::encode(keys.secret_key().as_secret_bytes()),
            expected_secret_key
        );

        // Test vector: public key
        let expected_public_key =
            "0767277aaed200af7a8843491745272fc1ad2c7bfe340225e6f34f3a9a273aed";
        assert_eq!(keys.public_key().to_hex(), expected_public_key);
    }

    #[test]
    fn test_mint_backup_new() {
        let mints = vec![
            MintUrl::from_str("https://mint.example.com").unwrap(),
            MintUrl::from_str("https://another-mint.org").unwrap(),
        ];

        let backup = MintBackup::new(mints.clone());

        assert_eq!(backup.mints, mints);
        assert!(backup.timestamp > 0);
    }

    #[test]
    fn test_mint_backup_serialization() {
        let mints = vec![
            MintUrl::from_str("https://mint.example.com").unwrap(),
            MintUrl::from_str("https://another-mint.org").unwrap(),
        ];
        let backup = MintBackup::with_timestamp(mints, 1703721600);

        let json = serde_json::to_string(&backup).unwrap();
        let parsed: MintBackup = serde_json::from_str(&json).unwrap();

        assert_eq!(backup, parsed);
    }

    #[test]
    fn test_create_and_decrypt_backup_event() {
        let keys = test_keys();
        let mints = vec![
            MintUrl::from_str("https://mint.example.com").unwrap(),
            MintUrl::from_str("https://another-mint.org").unwrap(),
        ];
        let backup = MintBackup::with_timestamp(mints.clone(), 1703721600);

        // Create the backup event
        let event = create_backup_event(&keys, &backup, Some("cashu-test")).unwrap();

        // Verify event properties
        assert_eq!(event.kind, Kind::Custom(KIND_APPLICATION_SPECIFIC_DATA));
        assert_eq!(event.pubkey, keys.public_key());

        // Verify tags
        let has_d_tag = event
            .tags
            .iter()
            .any(|tag| tag.kind() == TagKind::d() && tag.content() == Some(MINT_LIST_IDENTIFIER));
        assert!(has_d_tag, "Event should have 'd' tag with 'mint-list'");

        // Decrypt and verify content
        let decrypted = decrypt_backup_event(&keys, &event).unwrap();
        assert_eq!(decrypted.mints, mints);
        assert_eq!(decrypted.timestamp, 1703721600);
    }

    #[test]
    fn test_create_backup_event_without_client() {
        let keys = test_keys();
        let backup = MintBackup::with_timestamp(vec![], 1703721600);

        let event = create_backup_event(&keys, &backup, None).unwrap();

        // Should not have a client tag
        let has_client_tag = event.tags.iter().any(
            |tag| matches!(tag.kind(), nostr_sdk::TagKind::Custom(cow) if cow.as_ref() == "client"),
        );
        assert!(!has_client_tag);
    }

    #[test]
    fn test_decrypt_wrong_event_kind() {
        let keys = test_keys();

        // Create an event with wrong kind
        let event = EventBuilder::new(Kind::TextNote, "test")
            .tag(Tag::identifier(MINT_LIST_IDENTIFIER))
            .sign_with_keys(&keys)
            .unwrap();

        let result = decrypt_backup_event(&keys, &event);
        assert!(matches!(result, Err(Error::InvalidEventKind { .. })));
    }

    #[test]
    fn test_decrypt_missing_d_tag() {
        let keys = test_keys();
        let backup = MintBackup::with_timestamp(vec![], 1703721600);
        let plaintext = serde_json::to_string(&backup).unwrap();
        let encrypted = nip44::encrypt(
            keys.secret_key(),
            &keys.public_key(),
            plaintext,
            Version::V2,
        )
        .unwrap();

        // Create event without the d tag
        let event = EventBuilder::new(Kind::Custom(KIND_APPLICATION_SPECIFIC_DATA), encrypted)
            .sign_with_keys(&keys)
            .unwrap();

        let result = decrypt_backup_event(&keys, &event);
        assert!(matches!(result, Err(Error::MissingIdentifierTag(_))));
    }

    #[test]
    fn test_backup_filter_params() {
        let keys = test_keys();
        let (kind, pubkey, d_tag) = backup_filter_params(&keys);

        assert_eq!(kind, Kind::Custom(KIND_APPLICATION_SPECIFIC_DATA));
        assert_eq!(pubkey, keys.public_key());
        assert_eq!(d_tag, MINT_LIST_IDENTIFIER);
    }
}

//! Typed FFI bindings for the wallet's Nostr identity, event signing, NIP-44,
//! and the restartable NIP-17/NIP-59 inbox.

use std::sync::Arc;

use cdk_nostr::inbox::{Nip17Event, NostrInbox as CdkNostrInbox};
use cdk_nostr::nostr_sdk::{Event, Keys, Kind, RelayUrl, Tag, Timestamp, ToBech32, UnsignedEvent};
use cdk_nostr::{keys as nostr_keys, nip44 as cdk_nip44};

use crate::error::FfiError;
use crate::wallet_repository::WalletRepository;

/// A wallet-facing Nostr signer and identity.
///
/// The object owns one Nostr keypair and is intended to be shared by Nostr
/// event signing, NIP-44, the NIP-17 inbox, npub.cash, and the application's
/// primary Cashu P2PK identity. It is deliberately not used for NWC, NUT-27,
/// proof derivation, or other purpose-specific CDK key domains.
#[derive(uniffi::Object)]
pub struct NostrSigner {
    keys: Keys,
}

impl NostrSigner {
    fn from_keys(keys: Keys) -> Self {
        Self { keys }
    }

    pub(crate) fn parse_secret_key(secret_key: &str) -> Result<Self, FfiError> {
        let secret_key = nostr_keys::parse_secret_key(secret_key)
            .map_err(|e| FfiError::internal(e.to_string()))?;
        Ok(Self::from_keys(Keys::new(secret_key)))
    }

    pub(crate) fn keys(&self) -> &Keys {
        &self.keys
    }
}

/// An unsigned Nostr event supplied by a foreign-language caller.
///
/// The signer supplies `pubkey`; CDK uses the Rust Nostr implementation to
/// perform canonical serialization, event-ID calculation, and signing.
#[derive(Debug, Clone, uniffi::Record)]
pub struct NostrUnsignedEvent {
    /// Unix timestamp in seconds.
    pub created_at: u64,
    /// Numeric Nostr event kind. NIP-98 uses kind `27235`.
    pub kind: u16,
    /// Nostr tags represented as arrays of strings.
    pub tags: Vec<Vec<String>>,
    /// Event content.
    pub content: String,
}

/// A canonically encoded and BIP-340-signed Nostr event.
#[derive(Debug, Clone, uniffi::Record)]
pub struct NostrSignedEvent {
    /// Hex-encoded event ID.
    pub id: String,
    /// Hex-encoded x-only author public key.
    pub pubkey: String,
    /// Unix timestamp in seconds.
    pub created_at: u64,
    /// Numeric Nostr event kind.
    pub kind: u16,
    /// Nostr tags represented as arrays of strings.
    pub tags: Vec<Vec<String>>,
    /// Event content.
    pub content: String,
    /// Hex-encoded BIP-340 Schnorr signature.
    pub sig: String,
}

impl From<Event> for NostrSignedEvent {
    fn from(event: Event) -> Self {
        Self {
            id: event.id.to_hex(),
            pubkey: event.pubkey.to_hex(),
            created_at: event.created_at.as_secs(),
            kind: event.kind.as_u16(),
            tags: event
                .tags
                .iter()
                .map(|tag| tag.as_slice().to_vec())
                .collect(),
            content: event.content,
            sig: event.sig.to_string(),
        }
    }
}

#[uniffi::export]
impl NostrSigner {
    /// Derive the default wallet identity from a BIP-39 mnemonic using NIP-06
    /// path `m/44'/1237'/0'/0/0`.
    ///
    /// The mnemonic is parsed and expanded to its seed inside Rust. Callers
    /// never calculate or pass seed bytes.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid mnemonic or failed derivation.
    #[uniffi::constructor]
    pub fn from_mnemonic(mnemonic: String, passphrase: Option<String>) -> Result<Self, FfiError> {
        let keys = nostr_keys::derive_nip06_keys_from_mnemonic(&mnemonic, passphrase.as_deref())
            .map_err(|e| FfiError::internal(e.to_string()))?;
        Ok(Self::from_keys(keys))
    }

    /// Derive the default NIP-06 identity from an existing wallet repository.
    ///
    /// This is the preferred constructor when the repository already parsed
    /// the wallet mnemonic: its BIP-39 seed never leaves Rust.
    ///
    /// # Errors
    ///
    /// Returns an error if NIP-06 key derivation fails.
    #[uniffi::constructor]
    pub fn from_wallet_repository(repository: Arc<WalletRepository>) -> Result<Self, FfiError> {
        let secret_key = nostr_keys::derive_nip06_secret_key_from_seed(repository.inner().seed())
            .map_err(|e| FfiError::internal(e.to_string()))?;
        Ok(Self::from_keys(Keys::new(secret_key)))
    }

    /// Construct an identity from a 64-character secret-key hex string.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed hex, zero, or an out-of-range scalar.
    #[uniffi::constructor]
    pub fn from_secret_key_hex(secret_key_hex: String) -> Result<Self, FfiError> {
        if secret_key_hex.len() != 64
            || !secret_key_hex.as_bytes().iter().all(u8::is_ascii_hexdigit)
        {
            return Err(FfiError::internal(
                "Nostr secret-key hex must contain exactly 64 hexadecimal characters",
            ));
        }
        Self::parse_secret_key(&secret_key_hex)
    }

    /// Construct an identity from a bech32 `nsec`.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed bech32 or an invalid scalar.
    #[uniffi::constructor]
    pub fn from_nsec(nsec: String) -> Result<Self, FfiError> {
        if !nsec
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("nsec1"))
        {
            return Err(FfiError::internal("Nostr nsec must start with 'nsec1'"));
        }
        Self::parse_secret_key(&nsec)
    }

    /// Generate a cryptographically secure random Nostr identity.
    #[uniffi::constructor]
    pub fn generate() -> Self {
        Self::from_keys(Keys::generate())
    }

    /// Return the 64-character secret-key hex string.
    pub fn secret_key_hex(&self) -> String {
        self.keys.secret_key().to_secret_hex()
    }

    /// Return the bech32 `nsec`.
    ///
    /// # Errors
    ///
    /// Returns an error if bech32 encoding fails.
    pub fn nsec(&self) -> Result<String, FfiError> {
        self.keys
            .secret_key()
            .to_bech32()
            .map_err(FfiError::internal)
    }

    /// Return the 64-character x-only public-key hex string.
    pub fn public_key_hex(&self) -> String {
        self.keys.public_key().to_hex()
    }

    /// Explicit alias for [`Self::public_key_hex`].
    pub fn x_only_public_key_hex(&self) -> String {
        self.public_key_hex()
    }

    /// Return the bech32 `npub`.
    ///
    /// # Errors
    ///
    /// Returns an error if bech32 encoding fails.
    pub fn npub(&self) -> Result<String, FfiError> {
        self.keys
            .public_key()
            .to_bech32()
            .map_err(FfiError::internal)
    }

    /// Return the compressed even-parity Cashu P2PK public key.
    ///
    /// BIP-340 x-only public keys use the even-Y representative, so its
    /// compressed encoding is the `02` prefix followed by the x-only key.
    pub fn cashu_p2pk_public_key(&self) -> String {
        format!("02{}", self.public_key_hex())
    }

    /// Canonically serialize, hash, and BIP-340 sign an unsigned Nostr event.
    ///
    /// This generic record supports NIP-98 (kind `27235`) as well as other
    /// event kinds.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty tag or if event signing fails.
    pub fn sign_event(&self, event: NostrUnsignedEvent) -> Result<NostrSignedEvent, FfiError> {
        let tags = event
            .tags
            .into_iter()
            .map(Tag::parse)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| FfiError::internal(format!("invalid Nostr tag: {e}")))?;
        let event = UnsignedEvent::new(
            self.keys.public_key(),
            Timestamp::from_secs(event.created_at),
            Kind::from(event.kind),
            tags,
            event.content,
        )
        .sign_with_keys(&self.keys)
        .map_err(|e| FfiError::internal(format!("failed to sign Nostr event: {e}")))?;
        Ok(event.into())
    }

    /// Encrypt plaintext for `recipient_pubkey` using NIP-44 v2.
    ///
    /// `recipient_pubkey` accepts x-only hex or `npub`.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid public key or encryption failure.
    pub fn nip44_encrypt(
        &self,
        recipient_pubkey: String,
        plaintext: String,
    ) -> Result<String, FfiError> {
        let recipient = nostr_keys::parse_public_key(&recipient_pubkey)
            .map_err(|e| FfiError::internal(e.to_string()))?;
        cdk_nip44::encrypt(self.keys.secret_key(), &recipient, &plaintext)
            .map_err(|e| FfiError::internal(e.to_string()))
    }

    /// Decrypt a NIP-44 v2 payload from `sender_pubkey`.
    ///
    /// `sender_pubkey` accepts x-only hex or `npub`.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid public key, malformed payload, or MAC
    /// verification failure.
    pub fn nip44_decrypt(
        &self,
        sender_pubkey: String,
        payload: String,
    ) -> Result<String, FfiError> {
        let sender = nostr_keys::parse_public_key(&sender_pubkey)
            .map_err(|e| FfiError::internal(e.to_string()))?;
        cdk_nip44::decrypt(self.keys.secret_key(), &sender, &payload)
            .map_err(|e| FfiError::internal(e.to_string()))
    }
}

/// Generate a new random Nostr secret key.
///
/// This compatibility helper returns hex. New mobile code should keep a
/// [`NostrSigner`] object instead.
#[uniffi::export]
pub fn nostr_generate_secret_key() -> String {
    nostr_keys::generate_secret_key().to_secret_hex()
}

/// Get the x-only public key for a Nostr secret key in hex or `nsec` form.
///
/// # Errors
///
/// Returns an error if the secret key is invalid.
#[uniffi::export]
pub fn nostr_get_pubkey(nostr_secret_key: String) -> Result<String, FfiError> {
    Ok(NostrSigner::parse_secret_key(&nostr_secret_key)?.public_key_hex())
}

/// Return whether a x-only hex or bech32 `npub` public key is valid.
#[uniffi::export]
pub fn nostr_is_valid_public_key(public_key: String) -> bool {
    nostr_keys::parse_public_key(&public_key).is_ok()
}

/// Encrypt with NIP-44 v2 using a secret key in hex or `nsec` form.
///
/// This compatibility helper is retained for existing callers. New code
/// should call [`NostrSigner::nip44_encrypt`].
///
/// # Errors
///
/// Returns an error for invalid keys or encryption failure.
#[uniffi::export]
pub fn nip44_encrypt(
    nostr_secret_key: String,
    recipient_pubkey: String,
    plaintext: String,
) -> Result<String, FfiError> {
    NostrSigner::parse_secret_key(&nostr_secret_key)?.nip44_encrypt(recipient_pubkey, plaintext)
}

/// Decrypt with NIP-44 v2 using a secret key in hex or `nsec` form.
///
/// This compatibility helper is retained for existing callers. New code
/// should call [`NostrSigner::nip44_decrypt`].
///
/// # Errors
///
/// Returns an error for invalid keys or decryption failure.
#[uniffi::export]
pub fn nip44_decrypt(
    nostr_secret_key: String,
    sender_pubkey: String,
    payload: String,
) -> Result<String, FfiError> {
    NostrSigner::parse_secret_key(&nostr_secret_key)?.nip44_decrypt(sender_pubkey, payload)
}

/// An unwrapped, fully validated NIP-17 gift wrap.
///
/// All IDs and keys are hex-encoded; timestamps are Unix seconds.
#[derive(Debug, Clone, uniffi::Record)]
pub struct NostrInboxEvent {
    /// ID of the outer kind `1059` gift wrap, for persistent deduplication.
    pub wrap_id: String,
    /// Randomized/backdated `created_at` of the outer gift wrap.
    pub wrap_created_at: u64,
    /// Author of the verified kind `13` seal.
    pub sender_pubkey: String,
    /// Verified ID of the kind `14` rumor.
    pub rumor_id: String,
    /// Kind of the rumor (currently required to be `14`).
    pub rumor_kind: u16,
    /// Content of the rumor.
    pub rumor_content: String,
    /// `created_at` of the rumor.
    pub rumor_created_at: u64,
    /// Tags of the rumor.
    pub rumor_tags: Vec<Vec<String>>,
}

impl From<Nip17Event> for NostrInboxEvent {
    fn from(event: Nip17Event) -> Self {
        Self {
            wrap_id: event.wrap_id.to_hex(),
            wrap_created_at: event.wrap_created_at.as_secs(),
            sender_pubkey: event.sender.to_hex(),
            rumor_id: event.rumor_id.to_hex(),
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

/// Callback interface for [`NostrInbox`] events.
///
/// Implementations must be non-blocking; hand expensive work such as token
/// claiming or database writes to a separate task.
#[uniffi::export(with_foreign)]
pub trait NostrInboxListener: Send + Sync {
    /// Called once per successfully validated and unwrapped gift wrap.
    fn on_event(&self, event: NostrInboxEvent);
}

struct FfiInboxListener {
    ffi_listener: Arc<dyn NostrInboxListener>,
}

impl cdk_nostr::inbox::NostrInboxListener for FfiInboxListener {
    fn on_event(&self, event: Nip17Event) {
        self.ffi_listener.on_event(event.into());
    }
}

/// A restartable NIP-17/NIP-59 inbox bound to a typed [`NostrSigner`].
#[derive(uniffi::Object)]
pub struct NostrInbox {
    inner: CdkNostrInbox,
    signer: Arc<NostrSigner>,
}

impl NostrInbox {
    fn build(
        signer: Arc<NostrSigner>,
        relays: Vec<String>,
        since: Option<u64>,
    ) -> Result<Self, FfiError> {
        let relay_urls = relays
            .iter()
            .map(|relay| {
                RelayUrl::parse(relay)
                    .map_err(|e| FfiError::internal(format!("invalid relay {relay}: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let inner = CdkNostrInbox::from_keys(
            signer.keys().clone(),
            relay_urls,
            since.map(Timestamp::from_secs),
        )
        .map_err(|e| FfiError::internal(e.to_string()))?;
        Ok(Self { inner, signer })
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl NostrInbox {
    /// Compatibility constructor accepting secret-key hex or `nsec`.
    ///
    /// New mobile code should construct one [`NostrSigner`] and pass it to
    /// [`Self::with_signer`].
    ///
    /// `since` is a fixed subscription floor for this object's entire
    /// lifetime, including relay reconnects and explicit restarts. Use a
    /// generous lookback because NIP-59 gift wraps are intentionally
    /// backdated.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid secret key or relay configuration.
    #[uniffi::constructor]
    pub fn new(
        nostr_secret_key: String,
        relays: Vec<String>,
        since: Option<u64>,
    ) -> Result<Self, FfiError> {
        Self::build(
            Arc::new(NostrSigner::parse_secret_key(&nostr_secret_key)?),
            relays,
            since,
        )
    }

    /// Create an inbox using the shared active Nostr identity.
    ///
    /// This is the recommended mobile constructor. The configured `since`
    /// value remains the fixed subscription floor through reconnects and
    /// explicit restarts.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid/empty relay configuration.
    #[uniffi::constructor]
    pub fn with_signer(
        signer: Arc<NostrSigner>,
        relays: Vec<String>,
        since: Option<u64>,
    ) -> Result<Self, FfiError> {
        Self::build(signer, relays, since)
    }

    /// Public key of the inbox identity (hex-encoded, x-only).
    pub fn pubkey(&self) -> String {
        self.signer.public_key_hex()
    }

    /// Start delivering events. Calling this while running is a no-op.
    ///
    /// # Errors
    ///
    /// Returns an error if relay setup or subscription fails.
    pub async fn start(&self, listener: Arc<dyn NostrInboxListener>) -> Result<(), FfiError> {
        self.inner
            .start(Arc::new(FfiInboxListener {
                ffi_listener: listener,
            }))
            .await
            .map_err(|e| FfiError::internal(e.to_string()))
    }

    /// Stop the current run, then start a new one with `listener`.
    ///
    /// # Errors
    ///
    /// Returns an error if relay setup or subscription fails.
    pub async fn restart(&self, listener: Arc<dyn NostrInboxListener>) -> Result<(), FfiError> {
        self.inner
            .restart(Arc::new(FfiInboxListener {
                ffi_listener: listener,
            }))
            .await
            .map_err(|e| FfiError::internal(e.to_string()))
    }

    /// Stop listening and await shutdown.
    ///
    /// After this method completes, the stopped run is disconnected and can
    /// never invoke another callback. Calling it while stopped is a no-op.
    pub async fn stop(&self) {
        self.inner.stop().await;
    }

    /// Return whether the inbox currently has an active relay pump.
    pub async fn is_running(&self) -> bool {
        self.inner.is_running().await
    }
}

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "npubcash", feature = "nwc"))]
    use bip39::Mnemonic;
    use cdk_nostr::nostr_sdk::{JsonUtil, ToBech32};

    use super::*;
    use crate::database::custom_wallet_store;
    use crate::sqlite::WalletSqliteDatabase;

    const NIP06_MNEMONIC: &str =
        "leader monkey parrot ring guide accident before fence cannon height naive bean";
    const NIP06_SECRET_KEY: &str =
        "7f7ff03d123792d6ac594bfa67bf6d0c0ab55b6b1fdb6249303fe861f1ccba9a";
    const ONE_SECRET_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";
    const ONE_PUBLIC_KEY: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    #[test]
    fn public_bip39_vector_derives_expected_nip06_identity() {
        let signer =
            NostrSigner::from_mnemonic(NIP06_MNEMONIC.to_string(), None).expect("mnemonic derives");
        assert_eq!(signer.secret_key_hex(), NIP06_SECRET_KEY);
    }

    #[test]
    #[cfg(feature = "npubcash")]
    fn mnemonic_api_matches_existing_npubcash_derivation() {
        let mnemonic = Mnemonic::parse_normalized(NIP06_MNEMONIC).expect("valid mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let signer =
            NostrSigner::from_mnemonic(NIP06_MNEMONIC.to_string(), None).expect("mnemonic derives");
        let npubcash =
            cdk::wallet::derive_npubcash_secret_key_from_seed(&seed).expect("npub.cash derives");

        assert_eq!(
            signer.keys().secret_key().as_secret_bytes(),
            &npubcash.to_secret_bytes()
        );
    }

    #[test]
    fn repository_identity_matches_direct_mnemonic_identity() {
        let database =
            WalletSqliteDatabase::new_in_memory().expect("in-memory wallet database opens");
        let repository = Arc::new(
            WalletRepository::new(NIP06_MNEMONIC.to_string(), custom_wallet_store(database))
                .expect("repository builds"),
        );
        let from_repository =
            NostrSigner::from_wallet_repository(repository).expect("repository identity derives");
        let from_mnemonic = NostrSigner::from_mnemonic(NIP06_MNEMONIC.to_string(), None)
            .expect("mnemonic identity derives");

        assert_eq!(
            from_repository.secret_key_hex(),
            from_mnemonic.secret_key_hex()
        );
    }

    #[test]
    fn secret_hex_and_nsec_parsing_reject_invalid_scalars() {
        let signer =
            NostrSigner::from_secret_key_hex(ONE_SECRET_KEY.to_string()).expect("valid scalar");
        let nsec = signer.nsec().expect("nsec encodes");
        assert_eq!(
            NostrSigner::from_nsec(nsec)
                .expect("nsec parses")
                .secret_key_hex(),
            ONE_SECRET_KEY
        );
        assert_eq!(
            NostrSigner::from_nsec(signer.nsec().expect("nsec encodes").to_uppercase())
                .expect("uppercase nsec parses")
                .secret_key_hex(),
            ONE_SECRET_KEY
        );

        assert!(NostrSigner::from_secret_key_hex("00".repeat(32)).is_err());
        assert!(NostrSigner::from_secret_key_hex("ff".repeat(32)).is_err());
        assert!(NostrSigner::from_nsec("npub1not-an-nsec".to_string()).is_err());
    }

    #[test]
    fn generated_identity_and_public_formats_are_valid() {
        let signer = NostrSigner::generate();
        assert!(NostrSigner::from_secret_key_hex(signer.secret_key_hex()).is_ok());
        assert!(NostrSigner::from_nsec(signer.nsec().expect("nsec encodes")).is_ok());
        assert!(nostr_is_valid_public_key(signer.public_key_hex()));
        assert!(nostr_is_valid_public_key(
            signer.npub().expect("npub encodes")
        ));
        assert_eq!(signer.public_key_hex().len(), 64);
        assert_eq!(signer.cashu_p2pk_public_key().len(), 66);
        assert!(signer.cashu_p2pk_public_key().starts_with("02"));
    }

    #[test]
    fn public_key_of_one_has_expected_hex_npub_and_p2pk_encoding() {
        let signer =
            NostrSigner::from_secret_key_hex(ONE_SECRET_KEY.to_string()).expect("valid scalar");
        assert_eq!(signer.public_key_hex(), ONE_PUBLIC_KEY);
        assert_eq!(
            signer.npub().expect("npub encodes"),
            signer.keys().public_key().to_bech32().expect("npub")
        );
        assert_eq!(
            signer.cashu_p2pk_public_key(),
            format!("02{ONE_PUBLIC_KEY}")
        );
    }

    #[test]
    fn signed_event_id_and_signature_verify() {
        let signer =
            NostrSigner::from_secret_key_hex(ONE_SECRET_KEY.to_string()).expect("valid scalar");
        let signed = signer
            .sign_event(NostrUnsignedEvent {
                created_at: 1_700_000_000,
                kind: 27_235,
                tags: vec![
                    vec!["u".to_string(), "https://example.com/api".to_string()],
                    vec!["method".to_string(), "POST".to_string()],
                ],
                content: String::new(),
            })
            .expect("event signs");
        let json = serde_json::json!({
            "id": signed.id,
            "pubkey": signed.pubkey,
            "created_at": signed.created_at,
            "kind": signed.kind,
            "tags": signed.tags,
            "content": signed.content,
            "sig": signed.sig,
        })
        .to_string();
        let event = Event::from_json(json).expect("signed event parses");
        event.verify().expect("event ID and signature verify");
    }

    #[test]
    fn nip44_official_vector_and_cross_party_roundtrip() {
        let receiver = NostrSigner::from_secret_key_hex(
            "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        )
        .expect("receiver key");
        let sender = NostrSigner::from_secret_key_hex(
            "0000000000000000000000000000000000000000000000000000000000000002".to_string(),
        )
        .expect("sender key");
        let vector = "AgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABee0G5VSK0/9YypIObAtDKfYEAjD35uVkHyB0F4DwrcNaCXlCWZKaArsGrY6M9wnuTMxWfp1RTN9Xga8no+kF5Vsb";
        assert_eq!(
            receiver
                .nip44_decrypt(sender.public_key_hex(), vector.to_string())
                .expect("official vector decrypts"),
            "a"
        );

        let payload = sender
            .nip44_encrypt(receiver.public_key_hex(), "hello cashu".to_string())
            .expect("encrypts");
        assert_eq!(
            receiver
                .nip44_decrypt(sender.public_key_hex(), payload)
                .expect("decrypts"),
            "hello cashu"
        );
    }

    #[test]
    #[cfg(feature = "nwc")]
    fn domain_separated_identities_remain_distinct() {
        let mnemonic = Mnemonic::parse_normalized(NIP06_MNEMONIC).expect("valid mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let signer =
            NostrSigner::from_mnemonic(NIP06_MNEMONIC.to_string(), None).expect("mnemonic derives");
        let nwc = cdk::wallet::derive_nwc_secret_key_from_seed(&seed).expect("NWC derives");
        let backup = cdk::nuts::nut27::derive_nostr_keys(&seed).expect("NUT-27 derives");

        assert_ne!(
            signer.keys().secret_key().as_secret_bytes(),
            &nwc.to_secret_bytes()
        );
        assert_ne!(signer.secret_key_hex(), backup.secret_key().to_secret_hex());
    }

    #[test]
    fn compatibility_helpers_still_work() {
        assert_eq!(
            nostr_get_pubkey(ONE_SECRET_KEY.to_string()).expect("public key derives"),
            ONE_PUBLIC_KEY
        );
        let alice = nostr_generate_secret_key();
        let bob = nostr_generate_secret_key();
        let payload = nip44_encrypt(
            alice.clone(),
            nostr_get_pubkey(bob.clone()).expect("bob pubkey"),
            "hello".to_string(),
        )
        .expect("encrypts");
        assert_eq!(
            nip44_decrypt(bob, nostr_get_pubkey(alice).expect("alice pubkey"), payload,)
                .expect("decrypts"),
            "hello"
        );
    }

    #[test]
    fn inbox_constructors_validate_inputs_and_share_signer() {
        let signer = Arc::new(NostrSigner::generate());
        assert!(NostrInbox::with_signer(signer.clone(), Vec::new(), None).is_err());
        assert!(
            NostrInbox::with_signer(signer.clone(), vec!["not-a-url".to_string()], None).is_err()
        );
        let inbox = NostrInbox::with_signer(
            signer.clone(),
            vec!["wss://relay.example.com".to_string()],
            Some(1_700_000_000),
        )
        .expect("inbox builds without connecting");
        assert_eq!(inbox.pubkey(), signer.public_key_hex());

        let compatibility = NostrInbox::new(
            signer.secret_key_hex(),
            vec!["wss://relay.example.com".to_string()],
            None,
        )
        .expect("legacy constructor remains available");
        assert_eq!(compatibility.pubkey(), signer.public_key_hex());
    }
}

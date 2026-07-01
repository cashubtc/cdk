//! C2SP checkpoint, signed-note, and cosignature formats.
//!
//! Implements <https://c2sp.org/tlog-checkpoint>, the "note" signature
//! format from <https://c2sp.org/signed-note>, and Ed25519 cosignatures
//! from <https://c2sp.org/tlog-cosignature>. These are the exact formats
//! Sigsum, Tessera, and Sigstore's `sunlight`/static-ct logs all use, which
//! is what lets a mint-native checkpoint be cosigned by third-party
//! witnesses (and lets a mint's own built-in witness, see the `witness`
//! module, cosign checkpoints from other systems) without inventing a new
//! wire format.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use bitcoin::hashes::sha256;
use bitcoin::hashes::Hash as BitcoinHash;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};

use crate::merkle::Hash;

/// Ed25519 signed-note signature, as specified by signed-note.md §"Signature types".
const SIG_TYPE_ED25519: u8 = 0x01;
/// Timestamped Ed25519 checkpoint cosignature, per c2sp.org/tlog-cosignature.
const SIG_TYPE_ED25519_COSIGNATURE: u8 = 0x04;
/// Domain separation header for the Ed25519 cosigned message.
const COSIGNATURE_HEADER: &str = "cosignature/v1";

/// Errors from parsing or verifying checkpoints, notes, and cosignatures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The note text did not have the minimum three lines a checkpoint requires.
    #[error("checkpoint note text must have at least 3 lines")]
    TruncatedCheckpoint,
    /// The tree size line was not a valid non-negative decimal integer.
    #[error("invalid tree size line: {0}")]
    InvalidSize(String),
    /// The root hash line was not valid base64, or wasn't 32 bytes.
    #[error("invalid root hash line: {0}")]
    InvalidRootHash(String),
    /// The note had no blank line separating text from signatures.
    #[error("note is missing the blank line separating text from signatures")]
    MissingSignatureSeparator,
    /// A signature line didn't match `— name base64(...)`.
    #[error("malformed signature line: {0:?}")]
    MalformedSignatureLine(String),
    /// A signature line's base64 payload was shorter than the 4-byte key ID.
    #[error("signature line payload too short to contain a key ID")]
    SignatureTooShort,
    /// Base64 decoding failed.
    #[error("invalid base64: {0}")]
    InvalidBase64(String),
    /// No signature line matched the expected name and key ID.
    #[error("no signature found from the expected key")]
    NoMatchingSignature,
    /// A signature line matched the expected name and key ID but failed to verify.
    #[error("signature failed to verify")]
    InvalidSignature,
}

/// A [tlog-checkpoint](https://c2sp.org/tlog-checkpoint): an origin, a tree
/// size, and a root hash, optionally followed by opaque extension lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// Unique identifier for the log that issued this checkpoint (e.g.
    /// `<mint-domain>/transparency-log`).
    pub origin: String,
    /// Number of leaves in the tree at this checkpoint.
    pub size: u64,
    /// RFC 6962 Merkle root hash of the tree at `size`.
    pub root_hash: Hash,
    /// Opaque extension lines. Not recommended by the spec; kept for
    /// forward compatibility with witnesses/logs that use them.
    pub extra_lines: Vec<String>,
}

impl Checkpoint {
    /// Creates a checkpoint with no extension lines.
    pub fn new(origin: impl Into<String>, size: u64, root_hash: Hash) -> Self {
        Self {
            origin: origin.into(),
            size,
            root_hash,
            extra_lines: Vec::new(),
        }
    }

    /// The note text for this checkpoint: origin, size, and base64 root
    /// hash, each newline-terminated, followed by any extension lines.
    /// This is exactly what gets signed.
    pub fn note_text(&self) -> String {
        let mut text = format!(
            "{}\n{}\n{}\n",
            self.origin,
            self.size,
            BASE64.encode(self.root_hash)
        );
        for line in &self.extra_lines {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// Parses the note text (without any signature lines) back into a
    /// [`Checkpoint`].
    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut lines = text.lines();
        let origin = lines
            .next()
            .filter(|l| !l.is_empty())
            .ok_or(Error::TruncatedCheckpoint)?
            .to_string();
        let size_line = lines.next().ok_or(Error::TruncatedCheckpoint)?;
        let size = size_line
            .parse::<u64>()
            .map_err(|_| Error::InvalidSize(size_line.to_string()))?;
        let root_line = lines.next().ok_or(Error::TruncatedCheckpoint)?;
        let root_hash =
            decode_hash(root_line).map_err(|_| Error::InvalidRootHash(root_line.to_string()))?;
        let extra_lines = lines.map(str::to_string).collect();

        Ok(Self {
            origin,
            size,
            root_hash,
            extra_lines,
        })
    }
}

fn decode_hash(b64: &str) -> Result<Hash, Error> {
    let bytes = BASE64
        .decode(b64)
        .map_err(|e| Error::InvalidBase64(e.to_string()))?;
    bytes
        .try_into()
        .map_err(|_| Error::InvalidBase64("expected 32 decoded bytes".to_string()))
}

/// One `— <name> base64(keyID || signature)` line of a signed note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureLine {
    /// The signer's key name.
    pub name: String,
    /// The 4-byte key ID identifying which key/algorithm signed.
    pub key_id: [u8; 4],
    /// The raw signature bytes (algorithm-dependent length).
    pub signature: Vec<u8>,
}

impl SignatureLine {
    /// Formats this as a single signed-note signature line, including the
    /// trailing newline.
    pub fn to_line(&self) -> String {
        let mut payload = Vec::with_capacity(4 + self.signature.len());
        payload.extend_from_slice(&self.key_id);
        payload.extend_from_slice(&self.signature);
        format!("\u{2014} {} {}\n", self.name, BASE64.encode(payload))
    }

    /// Parses a single signature line of the form `— name base64(...)`.
    /// The em dash and trailing newline are both optional on input.
    ///
    /// # Panics
    ///
    /// Never panics: `payload.split_at(4)` is only reached after checking
    /// `payload.len() >= 4` above, so the key-ID slice is always exactly 4
    /// bytes.
    pub fn parse_line(line: &str) -> Result<Self, Error> {
        let line = line.trim_end_matches('\n');
        let rest = line
            .strip_prefix("\u{2014} ")
            .ok_or_else(|| Error::MalformedSignatureLine(line.to_string()))?;
        let (name, b64) = rest
            .split_once(' ')
            .ok_or_else(|| Error::MalformedSignatureLine(line.to_string()))?;
        let payload = BASE64
            .decode(b64)
            .map_err(|e| Error::InvalidBase64(e.to_string()))?;
        if payload.len() < 4 {
            return Err(Error::SignatureTooShort);
        }
        let (key_id, signature) = payload.split_at(4);
        Ok(Self {
            name: name.to_string(),
            key_id: key_id.try_into().expect("split_at(4) yields 4 bytes"),
            signature: signature.to_vec(),
        })
    }
}

/// A checkpoint together with zero or more signed-note signature lines
/// (the checkpoint's own signature, plus any witness cosignatures) — a
/// complete, self-contained, offline-verifiable "signed note".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedCheckpoint {
    /// The checkpoint being signed/cosigned.
    pub checkpoint: Checkpoint,
    /// All signature lines currently attached, in the order they should be
    /// rendered.
    pub signatures: Vec<SignatureLine>,
}

impl SignedCheckpoint {
    /// Renders the full signed note: checkpoint text, a blank line, then
    /// every signature line.
    pub fn to_note(&self) -> String {
        let mut note = self.checkpoint.note_text();
        note.push('\n');
        for sig in &self.signatures {
            note.push_str(&sig.to_line());
        }
        note
    }

    /// Parses a full signed note (checkpoint text, blank line, signature
    /// lines) as produced by [`Self::to_note`] or returned by a witness's
    /// `add-checkpoint` endpoint.
    pub fn parse(note: &str) -> Result<Self, Error> {
        let (text, sig_block) = note
            .split_once("\n\n")
            .ok_or(Error::MissingSignatureSeparator)?;
        let checkpoint = Checkpoint::parse(&format!("{text}\n"))?;
        let signatures = sig_block
            .lines()
            .filter(|l| !l.is_empty())
            .map(SignatureLine::parse_line)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            checkpoint,
            signatures,
        })
    }
}

/// Computes a signed-note key ID: `SHA256(name || 0x0A || sig_type ||
/// pubkey)[:4]`.
fn key_id(name: &str, sig_type: u8, pubkey: &[u8]) -> [u8; 4] {
    let mut buf = Vec::with_capacity(name.len() + 1 + 1 + pubkey.len());
    buf.extend_from_slice(name.as_bytes());
    buf.push(b'\n');
    buf.push(sig_type);
    buf.extend_from_slice(pubkey);
    let digest = sha256::Hash::hash(&buf).to_byte_array();
    digest[..4].try_into().expect("4 bytes")
}

/// Signs `checkpoint` as its origin log would: a plain Ed25519 signature
/// (signed-note type `0x01`) directly over the checkpoint's note text.
pub fn sign_checkpoint(checkpoint: Checkpoint, name: &str, key: &SigningKey) -> SignedCheckpoint {
    let text = checkpoint.note_text();
    let signature = key.sign(text.as_bytes()).to_bytes().to_vec();
    let sig_line = SignatureLine {
        name: name.to_string(),
        key_id: key_id(name, SIG_TYPE_ED25519, key.verifying_key().as_bytes()),
        signature,
    };
    SignedCheckpoint {
        checkpoint,
        signatures: vec![sig_line],
    }
}

/// Verifies that `line` is a valid signed-note signature (type `0x01`)
/// over `checkpoint` from `name`/`key`. Returns `Ok(())` only if the key
/// ID matches (the caller is responsible for only calling this once it
/// has already decided `name`/`key` is who it expects the signer to be —
/// per the spec, verifiers must ignore signatures from unknown keys
/// rather than erroring on them).
pub fn verify_checkpoint_signature(
    checkpoint: &Checkpoint,
    name: &str,
    key: &VerifyingKey,
    line: &SignatureLine,
) -> Result<(), Error> {
    if line.name != name || line.key_id != key_id(name, SIG_TYPE_ED25519, key.as_bytes()) {
        return Err(Error::NoMatchingSignature);
    }
    let signature_bytes: [u8; 64] = line
        .signature
        .as_slice()
        .try_into()
        .map_err(|_| Error::InvalidSignature)?;
    let signature = ed25519_dalek::Signature::from_bytes(&signature_bytes);
    key.verify_strict(checkpoint.note_text().as_bytes(), &signature)
        .map_err(|_| Error::InvalidSignature)
}

/// Produces an Ed25519 cosignature (signed-note type `0x04`) over
/// `checkpoint`, asserting that as of `timestamp` (seconds since the Unix
/// epoch) this is the largest consistent tree the cosigner has observed
/// for `checkpoint.origin`.
pub fn cosign(
    checkpoint: &Checkpoint,
    timestamp: u64,
    name: &str,
    key: &SigningKey,
) -> SignatureLine {
    let message = cosignature_message(checkpoint, timestamp);
    let signature = key.sign(&message).to_bytes().to_vec();
    SignatureLine {
        name: name.to_string(),
        key_id: key_id(
            name,
            SIG_TYPE_ED25519_COSIGNATURE,
            key.verifying_key().as_bytes(),
        ),
        signature,
    }
}

/// Verifies an Ed25519 cosignature line against `checkpoint`, given the
/// `timestamp` the caller believes was used (e.g. because it was just
/// returned in response to a request the caller made with that
/// timestamp, or extracted out-of-band).
pub fn verify_cosignature_with_timestamp(
    checkpoint: &Checkpoint,
    timestamp: u64,
    name: &str,
    key: &VerifyingKey,
    line: &SignatureLine,
) -> Result<(), Error> {
    if line.name != name
        || line.key_id != key_id(name, SIG_TYPE_ED25519_COSIGNATURE, key.as_bytes())
    {
        return Err(Error::NoMatchingSignature);
    }
    let message = cosignature_message(checkpoint, timestamp);
    let signature_bytes: [u8; 64] = line
        .signature
        .as_slice()
        .try_into()
        .map_err(|_| Error::InvalidSignature)?;
    let signature = ed25519_dalek::Signature::from_bytes(&signature_bytes);
    key.verify_strict(&message, &signature)
        .map_err(|_| Error::InvalidSignature)
}

fn cosignature_message(checkpoint: &Checkpoint, timestamp: u64) -> Vec<u8> {
    let mut message = format!("{COSIGNATURE_HEADER}\ntime {timestamp}\n").into_bytes();
    message.extend_from_slice(checkpoint.note_text().as_bytes());
    message
}

#[cfg(test)]
mod tests {
    use rand_core::OsRng;

    use super::*;

    fn sample_checkpoint() -> Checkpoint {
        Checkpoint::new("example.com/behind-the-sofa", 20852163, [7u8; 32])
    }

    #[test]
    fn note_text_round_trips_through_parse() {
        let checkpoint = sample_checkpoint();
        let text = checkpoint.note_text();
        assert_eq!(Checkpoint::parse(&text).expect("parses"), checkpoint);
    }

    #[test]
    fn note_text_matches_c2sp_field_order() {
        let checkpoint = sample_checkpoint();
        let text = checkpoint.note_text();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("example.com/behind-the-sofa"));
        assert_eq!(lines.next(), Some("20852163"));
        assert_eq!(lines.next(), Some(BASE64.encode([7u8; 32]).as_str()));
    }

    #[test]
    fn sign_and_verify_checkpoint_signature() {
        let key = SigningKey::generate(&mut OsRng);
        let checkpoint = sample_checkpoint();
        let signed = sign_checkpoint(checkpoint.clone(), "example.com/behind-the-sofa", &key);

        verify_checkpoint_signature(
            &checkpoint,
            "example.com/behind-the-sofa",
            &key.verifying_key(),
            &signed.signatures[0],
        )
        .expect("signature should verify");
    }

    #[test]
    fn signed_note_round_trips_through_to_note_and_parse() {
        let key = SigningKey::generate(&mut OsRng);
        let checkpoint = sample_checkpoint();
        let signed = sign_checkpoint(checkpoint, "example.com/behind-the-sofa", &key);

        let note = signed.to_note();
        let parsed = SignedCheckpoint::parse(&note).expect("parses");
        assert_eq!(parsed, signed);
    }

    #[test]
    fn tampered_checkpoint_fails_signature_verification() {
        let key = SigningKey::generate(&mut OsRng);
        let checkpoint = sample_checkpoint();
        let signed = sign_checkpoint(checkpoint, "example.com/behind-the-sofa", &key);

        let mut tampered = signed.checkpoint.clone();
        tampered.size += 1;

        assert_eq!(
            verify_checkpoint_signature(
                &tampered,
                "example.com/behind-the-sofa",
                &key.verifying_key(),
                &signed.signatures[0],
            ),
            Err(Error::InvalidSignature)
        );
    }

    #[test]
    fn signature_from_wrong_key_is_ignored_not_erroring_ambiguously() {
        let key = SigningKey::generate(&mut OsRng);
        let other_key = SigningKey::generate(&mut OsRng);
        let checkpoint = sample_checkpoint();
        let signed = sign_checkpoint(checkpoint.clone(), "example.com/behind-the-sofa", &key);

        // Verifying against a different trusted key must not match this
        // signature's key ID, per the "ignore unknown keys" rule.
        assert_eq!(
            verify_checkpoint_signature(
                &checkpoint,
                "example.com/behind-the-sofa",
                &other_key.verifying_key(),
                &signed.signatures[0],
            ),
            Err(Error::NoMatchingSignature)
        );
    }

    #[test]
    fn cosign_and_verify_round_trip() {
        let witness_key = SigningKey::generate(&mut OsRng);
        let checkpoint = sample_checkpoint();
        let timestamp = 1_679_315_147;

        let cosig = cosign(&checkpoint, timestamp, "witness.example/w1", &witness_key);
        verify_cosignature_with_timestamp(
            &checkpoint,
            timestamp,
            "witness.example/w1",
            &witness_key.verifying_key(),
            &cosig,
        )
        .expect("cosignature should verify");
    }

    #[test]
    fn cosignature_does_not_verify_as_a_plain_checkpoint_signature() {
        // Domain separation check: type 0x01 and type 0x04 key IDs for the
        // same key/name must differ, and a cosignature line must not be
        // mistaken for a primary checkpoint signature.
        let key = SigningKey::generate(&mut OsRng);
        let checkpoint = sample_checkpoint();
        let cosig = cosign(&checkpoint, 1_679_315_147, "log.example", &key);

        assert_eq!(
            verify_checkpoint_signature(&checkpoint, "log.example", &key.verifying_key(), &cosig),
            Err(Error::NoMatchingSignature)
        );
    }

    #[test]
    fn cosignature_with_wrong_timestamp_fails() {
        let witness_key = SigningKey::generate(&mut OsRng);
        let checkpoint = sample_checkpoint();
        let cosig = cosign(
            &checkpoint,
            1_679_315_147,
            "witness.example/w1",
            &witness_key,
        );

        assert_eq!(
            verify_cosignature_with_timestamp(
                &checkpoint,
                1_679_315_148,
                "witness.example/w1",
                &witness_key.verifying_key(),
                &cosig,
            ),
            Err(Error::InvalidSignature)
        );
    }

    #[test]
    fn signature_line_round_trips() {
        let line = SignatureLine {
            name: "example.com/foo".to_string(),
            key_id: [0x53, 0x0d, 0x90, 0x3a],
            signature: vec![1, 2, 3, 4, 5],
        };
        let rendered = line.to_line();
        assert!(rendered.starts_with("\u{2014} example.com/foo "));
        let parsed = SignatureLine::parse_line(rendered.trim_end()).expect("parses");
        assert_eq!(parsed, line);
    }
}

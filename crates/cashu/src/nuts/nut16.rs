//! NUT-16: Animated QR codes
//!
//! <https://github.com/cashubtc/nuts/blob/main/16.md>
//!
//! Tokens that are too large for a single QR code are shared as an animated
//! QR code based on the [UR](https://developer.blockchaincommons.com/ur/)
//! protocol. The sender splits the token into a fountain-coded sequence of UR
//! fragments ([`TokenUrEncoder`]) and displays each fragment as one QR frame.
//! The receiver scans the frames and feeds them into a [`TokenUrDecoder`]
//! until the token is reassembled.
//!
//! Fragments are `ur:bytes` URs whose payload is the serialized token
//! (`cashuB…`) encoded as a CBOR byte string, matching the de-facto standard
//! used by existing NUT-16 implementations (e.g. cashu.me).
//!
//! # Example
//!
//! ```
//! use std::str::FromStr;
//!
//! use cashu::nuts::nut16::DEFAULT_MAX_FRAGMENT_LENGTH;
//! use cashu::nuts::{Token, TokenUrDecoder};
//!
//! let token = Token::from_str("cashuBpGF0gaJhaUgArSaMTR9YJmFwgaNhYQFhc3hAOWE2ZGJiODQ3YmQyMzJiYTc2ZGIwZGYxOTcyMTZiMjlkM2I4Y2MxNDU1M2NkMjc4MjdmYzFjYzk0MmZlZGI0ZWFjWCEDhhhUP_trhpXfStS6vN6So0qWvc2X3O4NfM-Y1HISZ5JhZGlUaGFuayB5b3VhbXVodHRwOi8vbG9jYWxob3N0OjMzMzhhdWNzYXQ=")?;
//!
//! // Sender: display each fragment as one QR frame
//! let mut encoder = token.ur_encoder(DEFAULT_MAX_FRAGMENT_LENGTH)?;
//! let mut decoder = TokenUrDecoder::default();
//!
//! // Receiver: feed scanned frames until the token is reassembled
//! while !decoder.complete() {
//!     decoder.receive(&encoder.next_part()?)?;
//! }
//!
//! assert_eq!(decoder.token()?, Some(token));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;
use std::str::FromStr;
use std::string::FromUtf8Error;

use thiserror::Error;

use crate::nuts::nut00::{Token, TokenV4};

/// UR type used for Cashu tokens: `ur:bytes`
///
/// `bytes` is the de-facto standard used by existing NUT-16 wallets: the UR
/// payload is the serialized token encoded as a CBOR byte string.
pub const TOKEN_UR_TYPE: &str = "bytes";

/// Default maximum fragment length, in payload bytes per QR frame
///
/// 200 bytes is the largest fragment length recommended for broad QR scanner
/// compatibility; smaller fragments (50–100 bytes) produce less dense frames
/// that scan more reliably at the cost of a longer animation.
pub const DEFAULT_MAX_FRAGMENT_LENGTH: usize = 200;

/// Maximum encoded length accepted for one UR part
///
/// This is the maximum alphanumeric capacity of a version 40 QR code. Checking
/// it before decoding bounds allocations caused by untrusted scanner or FFI
/// input.
pub const MAX_UR_PART_LENGTH: usize = 4_296;

/// Maximum number of source fragments accepted by the decoder
///
/// Fountain decoding work can grow with the declared fragment count, so this
/// limit is checked before a part is passed to the underlying decoder.
pub const MAX_UR_FRAGMENT_COUNT: usize = 4_096;

/// Maximum reconstructed UR message length accepted by the decoder
///
/// One mebibyte leaves ample room for large Cashu tokens while bounding memory
/// retained during reconstruction.
pub const MAX_UR_MESSAGE_LENGTH: usize = 1024 * 1024;

/// NUT-16 Error
#[derive(Debug, Error)]
pub enum Error {
    /// UR encoding or decoding error
    #[error("UR error: {0}")]
    Ur(ur::ur::Error),
    /// CBOR serialization error
    #[error(transparent)]
    CiboriumSer(#[from] ciborium::ser::Error<std::io::Error>),
    /// CBOR deserialization error
    #[error(transparent)]
    CiboriumDe(#[from] ciborium::de::Error<std::io::Error>),
    /// UR payload is not valid UTF-8
    #[error(transparent)]
    Utf8(#[from] FromUtf8Error),
    /// Token error
    #[error(transparent)]
    Token(#[from] crate::nuts::nut00::Error),
    /// Received a UR of an unexpected type
    #[error("unexpected UR type: expected `{TOKEN_UR_TYPE}`, got `{0}`")]
    UnexpectedUrType(String),
    /// An encoded UR part exceeds the decoder limit
    #[error("UR part too large: {actual} bytes, maximum is {max}")]
    PartTooLarge {
        /// Actual encoded part length
        actual: usize,
        /// Maximum accepted encoded part length
        max: usize,
    },
    /// A multipart UR declares too many source fragments
    #[error("too many UR fragments: {actual}, maximum is {max}")]
    TooManyFragments {
        /// Declared source fragment count
        actual: usize,
        /// Maximum accepted source fragment count
        max: usize,
    },
    /// A UR declares a reconstructed message that is too large
    #[error("UR message too large: {actual} bytes, maximum is {max}")]
    MessageTooLarge {
        /// Declared reconstructed message length
        actual: usize,
        /// Maximum accepted reconstructed message length
        max: usize,
    },
}

impl From<ur::ur::Error> for Error {
    fn from(err: ur::ur::Error) -> Self {
        Self::Ur(err)
    }
}

/// Encodes a [`Token`] into UR fragments for display as an animated QR code
///
/// Each call to [`next_part`](Self::next_part) returns one fragment
/// (`ur:bytes/…`) to be displayed as a single QR frame. The first
/// [`fragment_count`](Self::fragment_count) frames cover the whole token; the
/// stream is unbounded and frames beyond that are redundant fountain parts,
/// so a receiver can complete from any sufficiently large subset of frames.
/// The sender typically loops the frames until the receiver signals
/// completion.
///
/// If the token fits into a single frame, the single-part form
/// (`ur:bytes/<payload>`, without fragment indices) is returned, matching
/// the reference implementations.
///
/// V3 tokens are normalized to V4 before serialization, so the encoded token
/// payload always uses the `cashuB…` format.
pub struct TokenUrEncoder {
    encoder: ur::Encoder<'static>,
    /// Full CBOR payload, used to emit the single-part form
    cbor: Vec<u8>,
}

impl TokenUrEncoder {
    /// Creates a new encoder for `token`
    ///
    /// `max_fragment_length` is the maximum number of payload bytes per QR
    /// frame; [`DEFAULT_MAX_FRAGMENT_LENGTH`] is a sane default.
    ///
    /// # Errors
    ///
    /// Returns an error if `max_fragment_length` is zero or the token cannot
    /// be converted to V4 or serialized.
    pub fn new(token: &Token, max_fragment_length: usize) -> Result<Self, Error> {
        let cbor = token_to_cbor(token)?;
        let encoder = ur::Encoder::bytes(&cbor, max_fragment_length)?;
        Ok(Self { encoder, cbor })
    }

    /// Returns the next UR fragment to display as a QR frame
    ///
    /// # Errors
    ///
    /// Returns an error if the fragment cannot be encoded.
    pub fn next_part(&mut self) -> Result<String, Error> {
        if self.fragment_count() == 1 {
            // Advance the index, then emit the single-part form, matching the
            // reference implementations. The type is the constant
            // `TOKEN_UR_TYPE`, which is always valid.
            let _part = self.encoder.next_part()?;
            return Ok(ur::encode(&self.cbor, &ur::Type::Bytes));
        }

        Ok(self.encoder.next_part()?)
    }

    /// Returns the number of fragments emitted so far
    pub fn current_index(&self) -> usize {
        self.encoder.current_index()
    }

    /// Returns the number of fragments the token was split into
    pub fn fragment_count(&self) -> usize {
        self.encoder.fragment_count()
    }

    /// Returns whether the token fits into a single QR frame
    pub fn is_single_fragment(&self) -> bool {
        self.fragment_count() == 1
    }
}

impl fmt::Debug for TokenUrEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenUrEncoder")
            .field("current_index", &self.current_index())
            .field("fragment_count", &self.fragment_count())
            .finish()
    }
}

impl Token {
    /// Creates a [`TokenUrEncoder`] for displaying this token as an animated
    /// QR code
    ///
    /// `max_fragment_length` is the maximum number of payload bytes per QR
    /// frame; [`DEFAULT_MAX_FRAGMENT_LENGTH`] is a sane default. Each
    /// [`TokenUrEncoder::next_part`] fragment is displayed as one QR frame.
    ///
    /// # Errors
    ///
    /// Returns an error if `max_fragment_length` is zero or the token cannot
    /// be converted to V4 or serialized.
    pub fn ur_encoder(&self, max_fragment_length: usize) -> Result<TokenUrEncoder, Error> {
        TokenUrEncoder::new(self, max_fragment_length)
    }
}

/// Reassembles a [`Token`] from scanned UR fragments
///
/// QR frames are fed with [`receive`](Self::receive) as they are scanned, in
/// any order; [`complete`](Self::complete) reports when enough frames have
/// been seen and [`token`](Self::token) returns the reassembled token.
#[derive(Default)]
pub struct TokenUrDecoder {
    decoder: ur::Decoder,
    /// Payload of a received single-part UR, if any
    single_part: Option<Vec<u8>>,
}

impl TokenUrDecoder {
    /// Feeds one scanned QR frame into the decoder
    ///
    /// Accepts both multi-part fragments (`ur:bytes/<seq>-<len>/<payload>`)
    /// and the single-part form (`ur:bytes/<payload>`).
    ///
    /// # Errors
    ///
    /// Returns an error if the frame is not a well-formed `ur:bytes` UR, is
    /// inconsistent with previously received frames, exceeds the decoder
    /// resource limits, or fails checksum validation.
    pub fn receive(&mut self, part: &str) -> Result<(), Error> {
        if part.len() > MAX_UR_PART_LENGTH {
            return Err(Error::PartTooLarge {
                actual: part.len(),
                max: MAX_UR_PART_LENGTH,
            });
        }

        let ur_type = parse_ur_type(part)?;
        if !ur_type.eq_ignore_ascii_case(TOKEN_UR_TYPE) {
            return Err(Error::UnexpectedUrType(ur_type.to_string()));
        }

        let (kind, payload) = ur::decode(part)?;
        match kind {
            ur::ur::Kind::MultiPart => {
                validate_multi_part_metadata(&payload)?;
                self.decoder.receive(part)?;
            }
            ur::ur::Kind::SinglePart => {
                if payload.len() > MAX_UR_MESSAGE_LENGTH {
                    return Err(Error::MessageTooLarge {
                        actual: payload.len(),
                        max: MAX_UR_MESSAGE_LENGTH,
                    });
                }
                self.single_part = Some(payload);
            }
        }

        Ok(())
    }

    /// Returns whether the token has been fully reassembled
    pub fn complete(&self) -> bool {
        self.single_part.is_some() || self.decoder.complete()
    }

    /// Returns the reassembled [`Token`] once [`complete`](Self::complete)
    ///
    /// Returns `None` while decoding is incomplete.
    ///
    /// # Errors
    ///
    /// Returns an error if the reassembled payload is not a valid token.
    pub fn token(&self) -> Result<Option<Token>, Error> {
        let message = match &self.single_part {
            Some(payload) => Some(payload.clone()),
            None => self.decoder.message()?,
        };

        message.map(|m| token_from_cbor(&m)).transpose()
    }

    /// Returns the total number of fragments the token was split into
    ///
    /// This is `0` until the first multi-part fragment is received.
    pub fn fragment_count(&self) -> usize {
        self.decoder.fragment_count()
    }

    /// Returns the number of fragments resolved so far, either received
    /// directly or recovered via the fountain code
    ///
    /// Useful for progress indication. Returns `None` before any fragment
    /// has been received.
    pub fn resolved_fragment_count(&self) -> Option<usize> {
        match &self.single_part {
            Some(_) => Some(1),
            None => self.decoder.resolved_fragment_count(),
        }
    }
}

impl fmt::Debug for TokenUrDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenUrDecoder")
            .field("complete", &self.complete())
            .field("fragment_count", &self.fragment_count())
            .field("resolved_fragment_count", &self.resolved_fragment_count())
            .finish()
    }
}

/// Serializes a token to the CBOR payload carried by the UR fragments: the
/// serialized token (`cashuB…`) as a CBOR byte string
fn token_to_cbor(token: &Token) -> Result<Vec<u8>, Error> {
    let serialized = match token {
        Token::TokenV3(token) => TokenV4::try_from(token.clone())?.to_string(),
        Token::TokenV4(token) => token.to_string(),
    };
    let mut cbor = Vec::new();
    ciborium::ser::into_writer(&serde_bytes::Bytes::new(serialized.as_bytes()), &mut cbor)?;
    Ok(cbor)
}

/// Deserializes a token from the CBOR payload of a reassembled UR
fn token_from_cbor(cbor: &[u8]) -> Result<Token, Error> {
    let bytes: serde_bytes::ByteBuf = ciborium::de::from_reader(cbor)?;
    let token_str = String::from_utf8(bytes.into_vec())?;
    Ok(Token::from_str(&token_str)?)
}

/// Extracts the type component of a UR string (`ur:<type>/…`)
fn parse_ur_type(part: &str) -> Result<&str, Error> {
    let (scheme, without_scheme) = part.split_once(':').ok_or(ur::ur::Error::InvalidScheme)?;
    if !scheme.eq_ignore_ascii_case("ur") {
        return Err(ur::ur::Error::InvalidScheme.into());
    }

    let (ur_type, _) = without_scheme
        .split_once('/')
        .ok_or(ur::ur::Error::TypeUnspecified)?;
    Ok(ur_type)
}

/// Validates the resource dimensions declared by a CBOR-encoded fountain part
fn validate_multi_part_metadata(payload: &[u8]) -> Result<(), Error> {
    type FountainPart = (u32, u32, u32, u32, serde_bytes::ByteBuf);

    let (_, fragment_count, message_length, _, _): FountainPart =
        ciborium::de::from_reader(payload)?;
    let fragment_count = fragment_count as usize;
    let message_length = message_length as usize;

    if fragment_count > MAX_UR_FRAGMENT_COUNT {
        return Err(Error::TooManyFragments {
            actual: fragment_count,
            max: MAX_UR_FRAGMENT_COUNT,
        });
    }
    if message_length > MAX_UR_MESSAGE_LENGTH {
        return Err(Error::MessageTooLarge {
            actual: message_length,
            max: MAX_UR_MESSAGE_LENGTH,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Token from the NUT-00 test vectors
    const TOKEN_STR: &str = "cashuBpGF0gaJhaUgArSaMTR9YJmFwgaNhYQFhc3hAOWE2ZGJiODQ3YmQyMzJiYTc2ZGIwZGYxOTcyMTZiMjlkM2I4Y2MxNDU1M2NkMjc4MjdmYzFjYzk0MmZlZGI0ZWFjWCEDhhhUP_trhpXfStS6vN6So0qWvc2X3O4NfM-Y1HISZ5JhZGlUaGFuayB5b3VhbXVodHRwOi8vbG9jYWxob3N0OjMzMzhhdWNzYXQ=";
    const TOKEN_V3_STR: &str = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91LiJ9";
    const TOKEN_V4_FROM_V3_STR: &str = "cashuBpGFtd2h0dHBzOi8vODMzMy5zcGFjZTozMzM4YXVjc2F0YWRqVGhhbmsgeW91LmF0gaJhaUgAmh8pMlPkHmFwgqRhYQJhc3hANDA3OTE1YmMyMTJiZTYxYTc3ZTNlNmQyYWViNGM3Mjc5ODBiZGE1MWNkMDZhNmFmYzI5ZTI4NjE3NjhhNzgzN2FjWCECvJCXmX2Br7LMc0a15DRak0a9KlBut5WFmKcvDPhRY-phZPakYWEIYXN4QGZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmVhY1ghAp6OUFC4kKfWwJaNsWvB1dX6BA6h3ihPbsadYSmfZxBZYWT2";

    fn test_token() -> Token {
        Token::from_str(TOKEN_STR).expect("valid test token")
    }

    fn multi_part(sequence: u32, fragment_count: u32, message_length: u32, data: &[u8]) -> String {
        let metadata = (
            sequence,
            fragment_count,
            message_length,
            0_u32,
            serde_bytes::Bytes::new(data),
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&metadata, &mut cbor).expect("valid fountain part CBOR");
        let encoded = ur::encode(&cbor, &ur::Type::Bytes);
        let payload = encoded
            .strip_prefix("ur:bytes/")
            .expect("encoded bytes UR has expected prefix");
        format!("ur:bytes/{sequence}-{fragment_count}/{payload}")
    }

    #[test]
    fn test_single_fragment_roundtrip() {
        let token = test_token();
        let mut encoder = TokenUrEncoder::new(&token, 1000).expect("valid encoder");
        assert!(encoder.is_single_fragment());

        let part = encoder.next_part().expect("valid part");
        assert!(part.starts_with("ur:bytes/"));
        // Single-part form carries no fragment indices (`ur:bytes/<payload>`)
        assert_eq!(part.matches('/').count(), 1);

        let mut decoder = TokenUrDecoder::default();
        decoder.receive(&part).expect("valid receive");
        assert!(decoder.complete());
        assert_eq!(decoder.token().expect("valid token"), Some(token));
    }

    #[test]
    fn test_v3_token_is_encoded_as_v4() {
        let token = Token::from_str(TOKEN_V3_STR).expect("valid V3 token");
        let expected = Token::from_str(TOKEN_V4_FROM_V3_STR).expect("valid V4 token");
        let mut encoder = token.ur_encoder(1000).expect("valid encoder");
        let part = encoder.next_part().expect("valid part");

        let mut decoder = TokenUrDecoder::default();
        decoder.receive(&part).expect("valid receive");

        assert_eq!(decoder.token().expect("valid token"), Some(expected));
    }

    #[test]
    fn test_token_ur_encoder_method() {
        let token = Token::from_str(TOKEN_STR).expect("valid token");
        let mut encoder = token.ur_encoder(100).expect("encoder");

        let mut decoder = TokenUrDecoder::default();
        while !decoder.complete() {
            let part = encoder.next_part().expect("part");
            decoder.receive(&part).expect("receive");
        }

        assert_eq!(decoder.token().expect("token"), Some(token));
    }

    #[test]
    fn test_multi_fragment_roundtrip() {
        let token = test_token();
        let mut encoder = TokenUrEncoder::new(&token, 20).expect("valid encoder");
        assert!(encoder.fragment_count() > 1);

        let mut decoder = TokenUrDecoder::default();
        let mut parts = 0;
        while !decoder.complete() {
            let part = encoder.next_part().expect("valid part");
            assert!(part.starts_with("ur:bytes/"));
            // Multi-part form carries `<seq>-<len>` fragment indices
            // (`ur:bytes/<seq>-<len>/<payload>`)
            assert_eq!(part.matches('/').count(), 2);
            decoder.receive(&part).expect("valid receive");
            parts += 1;
            assert!(parts <= 100, "decoder should complete");
        }

        assert!(parts >= encoder.fragment_count());
        assert_eq!(decoder.token().expect("valid token"), Some(token));
    }

    #[test]
    fn test_multi_fragment_with_dropped_frames() {
        let token = test_token();
        let mut encoder = TokenUrEncoder::new(&token, 20).expect("valid encoder");
        let mut decoder = TokenUrDecoder::default();

        // Drop every other frame; the fountain code must recover
        for _ in 0..200 {
            if decoder.complete() {
                break;
            }
            let part = encoder.next_part().expect("valid part");
            if encoder.current_index() % 2 == 0 {
                decoder.receive(&part).expect("valid receive");
            }
        }

        assert!(decoder.complete(), "decoder must tolerate dropped frames");
        assert_eq!(decoder.token().expect("valid token"), Some(token));
    }

    #[test]
    fn test_rejects_wrong_ur_type() {
        let part = ur::encode(b"not a token", &ur::Type::Custom("crypto-psbt"));
        let mut decoder = TokenUrDecoder::default();
        let err = decoder.receive(&part).expect_err("must reject wrong type");
        assert!(matches!(err, Error::UnexpectedUrType(t) if t == "crypto-psbt"));
    }

    #[test]
    fn test_rejects_non_ur_frame() {
        let mut decoder = TokenUrDecoder::default();
        assert!(decoder.receive(TOKEN_STR).is_err());
    }

    #[test]
    fn test_accepts_uppercase_ur() {
        let token = test_token();
        let mut encoder = TokenUrEncoder::new(&token, 1000).expect("valid encoder");
        let part = encoder
            .next_part()
            .expect("valid part")
            .to_ascii_uppercase();

        let mut decoder = TokenUrDecoder::default();
        decoder.receive(&part).expect("valid uppercase receive");
        assert_eq!(decoder.token().expect("valid token"), Some(token));
    }

    #[test]
    fn test_rejects_oversized_part_before_decoding() {
        let part = "x".repeat(MAX_UR_PART_LENGTH + 1);
        let mut decoder = TokenUrDecoder::default();
        let err = decoder.receive(&part).expect_err("oversized part");

        assert!(matches!(
            err,
            Error::PartTooLarge {
                actual,
                max: MAX_UR_PART_LENGTH,
            } if actual == MAX_UR_PART_LENGTH + 1
        ));
    }

    #[test]
    fn test_rejects_excessive_fragment_count() {
        let fragment_count =
            u32::try_from(MAX_UR_FRAGMENT_COUNT + 1).expect("fragment limit fits in u32");
        let part = multi_part(fragment_count + 1, fragment_count, 1, &[0]);
        let mut decoder = TokenUrDecoder::default();
        let err = decoder.receive(&part).expect_err("too many fragments");

        assert!(matches!(
            err,
            Error::TooManyFragments {
                actual,
                max: MAX_UR_FRAGMENT_COUNT,
            } if actual == MAX_UR_FRAGMENT_COUNT + 1
        ));
    }

    #[test]
    fn test_rejects_excessive_message_length() {
        let message_length =
            u32::try_from(MAX_UR_MESSAGE_LENGTH + 1).expect("message limit fits in u32");
        let part = multi_part(1, 1, message_length, &[0]);
        let mut decoder = TokenUrDecoder::default();
        let err = decoder.receive(&part).expect_err("message too large");

        assert!(matches!(
            err,
            Error::MessageTooLarge {
                actual,
                max: MAX_UR_MESSAGE_LENGTH,
            } if actual == MAX_UR_MESSAGE_LENGTH + 1
        ));
    }

    #[test]
    fn test_payload_is_cbor_byte_string_of_token() {
        // The UR payload must be the serialized token as a CBOR byte string
        // (de-facto NUT-16 encoding used by e.g. cashu.me)
        let token = test_token();
        let cbor = token_to_cbor(&token).expect("valid cbor");
        let bytes: serde_bytes::ByteBuf =
            ciborium::de::from_reader(&cbor[..]).expect("valid cbor byte string");
        assert_eq!(bytes.into_vec(), token.to_string().as_bytes());
    }
}

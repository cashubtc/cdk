//! Experimental binary animated QR transport compatible with nut-fountain v0.1.0-alpha.0.
//!
//! This is an additional transport, not the NUT-16 UR wire format. Each part is
//! a complete binary `NF` version-1 frame: render it in QR byte mode and pass
//! the scanner's raw bytes to the decoder without text conversion.
//! Tokens use the existing `crawB` binary serialization; V3 tokens are
//! normalized to V4. The generic [`FountainEncoder`] and [`FountainDecoder`]
//! can also transport arbitrary bytes.
//!
//! The [upstream specification] defines the framing and fragment selection.
//! CRCs detect accidental corruption, not authenticity. See
//! `crates/cashu/NOTICE-nut-fountain` for attribution and license terms.
//!
//! [upstream specification]: https://github.com/Egge21M/nut-fountain/blob/v0.1.0-alpha.0/packages/nut-fountain/docs/protocol.md
//!
//! ```no_run
//! use cashu::nuts::{Token, TokenFountainDecoder};
//! use cashu::nuts::nut16::fountain::DEFAULT_MAX_FRAGMENT_LENGTH;
//!
//! # fn transfer(token: Token) -> Result<(), Box<dyn std::error::Error>> {
//! let mut encoder = token.fountain_encoder(DEFAULT_MAX_FRAGMENT_LENGTH)?;
//! let mut decoder = TokenFountainDecoder::default();
//! while !decoder.complete() {
//!     decoder.receive(&encoder.next_part()?)?;
//! }
//! let recovered = decoder.token()?.expect("completed transfer");
//! # Ok(())
//! # }
//! ```

mod codec;

#[cfg(test)]
mod tests;

pub use self::codec::{FountainDecoder, FountainEncoder};
use crate::nuts::nut00::{Token, TokenV4};

/// Default number of payload bytes in each binary frame (excluding overhead).
pub const DEFAULT_MAX_FRAGMENT_LENGTH: usize = 128;
/// Largest payload accepted by version 1. The full frame may exceed QR capacity.
pub const MAX_FRAGMENT_LENGTH: usize = 4096;
/// Maximum number of source fragments, bounding decoder memory and work.
pub const MAX_FRAGMENT_COUNT: usize = 256;
/// Maximum unpadded message length (one mebibyte).
pub const MAX_MESSAGE_LENGTH: usize = MAX_FRAGMENT_LENGTH * MAX_FRAGMENT_COUNT;
/// Header and checksum bytes added to each fragment.
pub const FRAME_OVERHEAD: usize = 24;

/// Binary fountain transport error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The requested payload size is outside the protocol limits.
    #[error("invalid fragment length {actual}; expected 1..={MAX_FRAGMENT_LENGTH}")]
    InvalidFragmentLength {
        /// Requested payload size.
        actual: usize,
    },
    /// The message exceeds the protocol limit.
    #[error("message too large: {actual} bytes, maximum is {MAX_MESSAGE_LENGTH}")]
    MessageTooLarge {
        /// Requested message length.
        actual: usize,
    },
    /// The selected fragment size requires too many source fragments.
    #[error("too many fragments: {actual}, maximum is {MAX_FRAGMENT_COUNT}")]
    TooManyFragments {
        /// Required source fragment count.
        actual: usize,
    },
    /// A frame cannot contain a valid bounded payload.
    #[error("invalid fountain frame length: {actual}")]
    InvalidFrameLength {
        /// Received frame length.
        actual: usize,
    },
    /// The magic, version, or reserved flags are unsupported.
    #[error("unsupported fountain frame format, version, or flags")]
    UnsupportedFormat,
    /// Sequence, count, or message length is invalid or inconsistent.
    #[error("invalid fountain frame metadata")]
    InvalidMetadata,
    /// A frame's checksum is incorrect.
    #[error("fountain frame checksum mismatch")]
    FrameChecksumMismatch,
    /// A frame describes another transfer; reset before changing transfers.
    #[error("frame belongs to another message; reset the decoder first")]
    InconsistentMessage,
    /// The reconstructed message checksum or zero padding is incorrect.
    #[error("reconstructed message checksum or padding mismatch; reset may be required")]
    MessageIntegrityMismatch,
    /// All nonzero 32-bit sequence numbers have been used.
    #[error("fountain sequence exhausted; create a new encoder")]
    SequenceExhausted,
    /// Token serialization or parsing failed.
    #[error(transparent)]
    Token(#[from] crate::nuts::nut00::Error),
}

/// Encodes a token into raw binary fountain frames for animated QR display.
///
/// The first [`fragment_count`](Self::fragment_count) parts contain each source
/// fragment in order. Later parts repair missing fragments. Each frame has
/// exactly `max_fragment_length + FRAME_OVERHEAD` bytes, including padding.
#[derive(Debug)]
pub struct TokenFountainEncoder {
    encoder: FountainEncoder,
}

impl TokenFountainEncoder {
    /// Creates an encoder, normalizing V3 tokens to V4 before serialization.
    ///
    /// # Errors
    /// Returns an error for an invalid fragment size, an oversized message,
    /// or a token that cannot be converted to V4 or serialized.
    pub fn new(token: &Token, max_fragment_length: usize) -> Result<Self, Error> {
        let message = match token {
            Token::TokenV3(token) => TokenV4::try_from(token.clone())?.to_raw_bytes()?,
            Token::TokenV4(token) => token.to_raw_bytes()?,
        };
        Ok(Self {
            encoder: FountainEncoder::new(&message, max_fragment_length)?,
        })
    }

    /// Returns the next complete binary frame, including header and checksum.
    ///
    /// # Errors
    /// Returns an error when the 32-bit sequence is exhausted; it never wraps.
    pub fn next_part(&mut self) -> Result<Vec<u8>, Error> {
        self.encoder.next_part()
    }

    /// Returns the number of frames emitted so far.
    pub fn current_index(&self) -> u32 {
        self.encoder.current_index()
    }

    /// Returns the number of source fragments.
    pub fn fragment_count(&self) -> usize {
        self.encoder.fragment_count()
    }

    /// Returns whether one frame can carry the entire token.
    pub fn is_single_fragment(&self) -> bool {
        self.fragment_count() == 1
    }
}

impl Token {
    /// Creates an experimental binary fountain encoder for animated QR display.
    ///
    /// `max_fragment_length` counts payload bytes; each frame adds
    /// [`FRAME_OVERHEAD`] bytes. Use [`DEFAULT_MAX_FRAGMENT_LENGTH`] as a default.
    ///
    /// # Errors
    /// Returns an error if the fragment size or token is unsupported or the
    /// serialized token exceeds the transport limits.
    pub fn fountain_encoder(
        &self,
        max_fragment_length: usize,
    ) -> Result<TokenFountainEncoder, Error> {
        TokenFountainEncoder::new(self, max_fragment_length)
    }
}

/// Reassembles a token from binary fountain frames, in any order.
///
/// Duplicate or dependent frames do not advance progress. A completed transfer
/// must be reset before receiving a different message. Transport completion
/// validates checksums and padding; [`token`](Self::token) validates the token.
#[derive(Debug, Default)]
pub struct TokenFountainDecoder {
    decoder: FountainDecoder,
}

impl TokenFountainDecoder {
    /// Feeds one complete binary frame from a QR scanner.
    ///
    /// # Errors
    /// Returns an error for invalid frames, mixed transfers, or a failed
    /// reconstructed checksum/padding check. Invalid frames do not change
    /// state. Reconstruction failure discards only the last inserted equation;
    /// earlier equations may require a [`reset`](Self::reset).
    pub fn receive(&mut self, part: &[u8]) -> Result<(), Error> {
        self.decoder.receive(part)?;
        Ok(())
    }

    /// Returns whether the binary message has been fully reconstructed.
    pub fn complete(&self) -> bool {
        self.decoder.complete()
    }

    /// Returns the token, or `None` until the transfer is complete.
    ///
    /// # Errors
    /// Returns an error if the reconstructed message is not a binary V4 token.
    pub fn token(&self) -> Result<Option<Token>, Error> {
        self.decoder
            .message()
            .map(|bytes| Token::try_from(&bytes.to_vec()).map_err(Error::from))
            .transpose()
    }

    /// Returns the source fragment count, or zero before the first valid frame.
    pub fn fragment_count(&self) -> usize {
        self.decoder.fragment_count()
    }

    /// Returns the number of individually recoverable source fragments.
    ///
    /// Returns `None` before the first valid frame. This differs from equation
    /// rank: independent mixed frames can advance progress without resolving
    /// an individual source fragment.
    pub fn resolved_fragment_count(&self) -> Option<usize> {
        self.decoder.resolved_fragment_count()
    }

    /// Returns the number of independent equations retained for this transfer.
    pub fn independent_fragment_count(&self) -> usize {
        self.decoder.independent_fragment_count()
    }

    /// Returns retained information as a fraction from zero to one.
    pub fn progress(&self) -> f64 {
        self.decoder.progress()
    }

    /// Clears the transfer and all accumulated frames.
    pub fn reset(&mut self) {
        self.decoder.reset();
    }
}

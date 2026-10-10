//! FFI-compatible experimental binary fountain transport for Cashu tokens.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::FfiError;
use crate::token::Token;

/// Encodes a token as binary `NF` version-1 frames for QR byte mode.
#[derive(Debug, uniffi::Object)]
pub struct TokenFountainEncoder {
    inner: Mutex<cdk::nuts::TokenFountainEncoder>,
}

impl TokenFountainEncoder {
    pub(crate) fn from_inner(inner: cdk::nuts::TokenFountainEncoder) -> Self {
        Self {
            inner: Mutex::new(inner),
        }
    }

    fn lock(&self) -> MutexGuard<'_, cdk::nuts::TokenFountainEncoder> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

#[uniffi::export]
impl TokenFountainEncoder {
    /// Returns the next complete binary frame, including header and checksum.
    /// Sequence numbers never wrap; exhaustion returns an error.
    pub fn next_part(&self) -> Result<Vec<u8>, FfiError> {
        Ok(self.lock().next_part()?)
    }

    /// Returns the number of emitted frames.
    pub fn current_index(&self) -> u32 {
        self.lock().current_index()
    }

    /// Returns the number of source fragments.
    pub fn fragment_count(&self) -> u32 {
        self.lock().fragment_count() as u32
    }

    /// Returns whether a single frame can carry the whole token.
    pub fn is_single_fragment(&self) -> bool {
        self.lock().is_single_fragment()
    }
}

/// Reconstructs a token from raw binary fountain frames supplied by a scanner.
/// UR strings are handled by the separate `TokenUrDecoder`.
#[derive(Debug, uniffi::Object)]
pub struct TokenFountainDecoder {
    inner: Mutex<cdk::nuts::TokenFountainDecoder>,
}

impl Default for TokenFountainDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenFountainDecoder {
    fn lock(&self) -> MutexGuard<'_, cdk::nuts::TokenFountainDecoder> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

#[uniffi::export]
impl TokenFountainDecoder {
    /// Creates an empty decoder.
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(cdk::nuts::TokenFountainDecoder::default()),
        }
    }

    /// Feeds a complete binary frame, preserving all scanner bytes.
    /// Invalid frames and frames from another transfer return errors.
    pub fn receive(&self, part: Vec<u8>) -> Result<(), FfiError> {
        Ok(self.lock().receive(&part)?)
    }

    /// Returns whether the message has passed reconstruction and integrity checks.
    pub fn complete(&self) -> bool {
        self.lock().complete()
    }

    /// Returns the token after completion, or `None` while incomplete.
    /// A reconstructed message that is not a binary V4 token returns an error.
    pub fn token(&self) -> Result<Option<Arc<Token>>, FfiError> {
        Ok(self.lock().token()?.map(|token| Arc::new(token.into())))
    }

    /// Returns the source fragment count, or zero before a valid frame arrives.
    pub fn fragment_count(&self) -> u32 {
        self.lock().fragment_count() as u32
    }

    /// Returns the number of individually recoverable source fragments, or
    /// `None` before the first valid frame.
    pub fn resolved_fragment_count(&self) -> Option<u32> {
        self.lock()
            .resolved_fragment_count()
            .map(|count| count as u32)
    }

    /// Returns the number of retained independent equations.
    pub fn independent_fragment_count(&self) -> u32 {
        self.lock().independent_fragment_count() as u32
    }

    /// Returns retained information as a fraction from zero to one.
    pub fn progress(&self) -> f64 {
        self.lock().progress()
    }

    /// Clears the current transfer so another message can be received.
    pub fn reset(&self) {
        self.lock().reset();
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    const TOKEN: &str = "cashuBpGF0gaJhaUgArSaMTR9YJmFwgaNhYQFhc3hAOWE2ZGJiODQ3YmQyMzJiYTc2ZGIwZGYxOTcyMTZiMjlkM2I4Y2MxNDU1M2NkMjc4MjdmYzFjYzk0MmZlZGI0ZWFjWCEDhhhUP_trhpXfStS6vN6So0qWvc2X3O4NfM-Y1HISZ5JhZGlUaGFuayB5b3VhbXVodHRwOi8vbG9jYWxob3N0OjMzMzhhdWNzYXQ=";

    #[test]
    fn binary_fountain_bindings_recover_missing_frames_and_reset() {
        let token = Token::from_str(TOKEN).expect("token");
        let encoder = token.fountain_encoder(Some(32)).expect("encoder");
        let decoder = TokenFountainDecoder::new();
        assert_eq!(decoder.resolved_fragment_count(), None);
        assert!(decoder.token().expect("incomplete").is_none());
        for _ in 0..encoder.fragment_count() {
            encoder.next_part().expect("skip source frames");
        }
        for _ in 0..512 {
            decoder
                .receive(encoder.next_part().expect("repair frame"))
                .expect("receive");
            if decoder.complete() {
                break;
            }
        }
        assert!(decoder.complete());
        assert_eq!(decoder.progress(), 1.0);
        assert_eq!(
            decoder.independent_fragment_count(),
            encoder.fragment_count()
        );
        assert_eq!(
            decoder.resolved_fragment_count(),
            Some(encoder.fragment_count())
        );
        assert_eq!(
            decoder.token().expect("token").expect("complete").encode(),
            token.encode()
        );
        decoder.reset();
        assert!(!decoder.complete());
        assert_eq!(decoder.fragment_count(), 0);
        assert_eq!(decoder.progress(), 0.0);
        assert!(token.fountain_encoder(Some(0)).is_err());
        assert!(decoder.receive(b"ur:bytes/invalid".to_vec()).is_err());
        let single = token.fountain_encoder(Some(1024)).expect("single");
        assert!(single.is_single_fragment());
        decoder
            .receive(single.next_part().expect("frame"))
            .expect("receive");
        assert!(decoder.complete());
        assert_eq!(single.current_index(), 1);
        assert!(token.fountain_encoder(None).is_ok());
    }
}

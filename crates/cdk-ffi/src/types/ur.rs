//! FFI-compatible NUT-16 animated QR code types

use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::FfiError;
use crate::token::Token;

/// FFI-compatible NUT-16 UR encoder for animated QR codes
///
/// Splits a token into a fountain-coded sequence of UR fragments
/// (`ur:bytes/…`), one per QR frame. The first `fragment_count` frames cover
/// the whole token; the stream is unbounded and frames beyond that are
/// redundant fountain parts, so a receiver can complete from any
/// sufficiently large subset of frames. Senders typically loop the frames
/// until the receiver signals completion.
///
/// If the token fits into a single frame, the single-part form
/// (`ur:bytes/<payload>`, without fragment indices) is emitted.
#[derive(uniffi::Object)]
pub struct TokenUrEncoder {
    inner: Mutex<cdk::nuts::nut16::TokenUrEncoder>,
}

impl std::fmt::Debug for TokenUrEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenUrEncoder")
            .field("current_index", &self.current_index())
            .field("fragment_count", &self.fragment_count())
            .finish()
    }
}

impl TokenUrEncoder {
    /// Create from an inner encoder
    pub(crate) fn from_inner(inner: cdk::nuts::nut16::TokenUrEncoder) -> Self {
        Self {
            inner: Mutex::new(inner),
        }
    }

    /// Lock the inner encoder, recovering from a poisoned mutex
    ///
    /// A panic in another thread while holding the lock does not corrupt
    /// encoder state (frame emission is atomic from the caller's
    /// perspective), so recovery is safe.
    fn lock(&self) -> MutexGuard<'_, cdk::nuts::nut16::TokenUrEncoder> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[uniffi::export]
impl TokenUrEncoder {
    /// Returns the next UR fragment to display as a QR frame
    pub fn next_part(&self) -> Result<String, FfiError> {
        Ok(self.lock().next_part()?)
    }

    /// Returns the number of fragments emitted so far
    pub fn current_index(&self) -> u32 {
        self.lock().current_index() as u32
    }

    /// Returns the number of fragments the token was split into
    pub fn fragment_count(&self) -> u32 {
        self.lock().fragment_count() as u32
    }

    /// Returns whether the token fits into a single QR frame
    pub fn is_single_fragment(&self) -> bool {
        self.lock().is_single_fragment()
    }
}

/// FFI-compatible NUT-16 UR decoder for animated QR codes
///
/// Reassembles a token from scanned UR fragments. Feed each scanned QR
/// frame with `receive` in any order until `complete` returns true, then
/// read the token with `token`.
#[derive(uniffi::Object)]
pub struct TokenUrDecoder {
    inner: Mutex<cdk::nuts::nut16::TokenUrDecoder>,
}

impl std::fmt::Debug for TokenUrDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenUrDecoder")
            .field("complete", &self.complete())
            .field("fragment_count", &self.fragment_count())
            .field("resolved_fragment_count", &self.resolved_fragment_count())
            .finish()
    }
}

impl Default for TokenUrDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenUrDecoder {
    /// Lock the inner decoder, recovering from a poisoned mutex
    fn lock(&self) -> MutexGuard<'_, cdk::nuts::nut16::TokenUrDecoder> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[uniffi::export]
impl TokenUrDecoder {
    /// Create a new decoder
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(cdk::nuts::nut16::TokenUrDecoder::default()),
        }
    }

    /// Feed one scanned QR frame into the decoder
    ///
    /// Accepts both multi-part fragments (`ur:bytes/<seq>-<len>/<payload>`)
    /// and the single-part form (`ur:bytes/<payload>`).
    pub fn receive(&self, part: String) -> Result<(), FfiError> {
        Ok(self.lock().receive(&part)?)
    }

    /// Returns whether the token has been fully reassembled
    pub fn complete(&self) -> bool {
        self.lock().complete()
    }

    /// Returns the reassembled token once `complete`, `None` while decoding
    /// is incomplete
    pub fn token(&self) -> Result<Option<Arc<Token>>, FfiError> {
        let token = self.lock().token()?;
        Ok(token.map(|t| Arc::new(t.into())))
    }

    /// Returns the total number of fragments the token was split into
    ///
    /// This is `0` until the first multi-part fragment is received.
    pub fn fragment_count(&self) -> u32 {
        self.lock().fragment_count() as u32
    }

    /// Returns the number of fragments resolved so far, either received
    /// directly or recovered via the fountain code
    ///
    /// Useful for progress indication. Returns `None` before any fragment
    /// has been received.
    pub fn resolved_fragment_count(&self) -> Option<u32> {
        self.lock().resolved_fragment_count().map(|c| c as u32)
    }
}

/// Returns whether scanned QR text is a NUT-16 fountain frame
///
/// Route such text to `TokenFountainDecoder` only, even if it then fails;
/// other text goes to the token or UR parser.
#[uniffi::export]
pub fn is_fountain_frame(text: String) -> bool {
    cdk::nuts::nut16::FountainDecoder::is_frame(&text)
}

/// FFI-compatible NUT-16 binary fountain encoder for animated QR codes
///
/// Each `next_frame` is Base45 text for one alphanumeric-mode QR symbol.
/// Loop the frames until the receiver completes.
#[derive(uniffi::Object)]
pub struct TokenFountainEncoder {
    inner: Mutex<cdk::nuts::nut16::FountainEncoder>,
}

impl std::fmt::Debug for TokenFountainEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&*self.lock(), f)
    }
}

impl TokenFountainEncoder {
    /// Create from an inner encoder
    pub(crate) fn from_inner(inner: cdk::nuts::nut16::FountainEncoder) -> Self {
        Self {
            inner: Mutex::new(inner),
        }
    }

    /// Lock the inner encoder, recovering from a poisoned mutex
    fn lock(&self) -> MutexGuard<'_, cdk::nuts::nut16::FountainEncoder> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[uniffi::export]
impl TokenFountainEncoder {
    /// Create an encoder for a `cashuB` string, keeping its exact bytes
    ///
    /// `fragment_size` is payload bytes per frame; `None` fits a version 10-M
    /// QR symbol.
    #[uniffi::constructor]
    pub fn from_token_string(
        token: String,
        fragment_size: Option<u32>,
    ) -> Result<Arc<Self>, FfiError> {
        let encoder = cdk::nuts::nut16::FountainEncoder::from_token_str(
            &token,
            fragment_size_or_default(fragment_size),
        )?;
        Ok(Arc::new(Self::from_inner(encoder)))
    }

    /// Returns the next frame as QR text
    pub fn next_frame(&self) -> Result<String, FfiError> {
        Ok(self.lock().next_frame()?)
    }

    /// Returns the number of source fragments
    pub fn fragment_count(&self) -> u32 {
        self.lock().fragment_count() as u32
    }

    /// Returns whether the token fits one frame, which may be shown as a static QR
    pub fn is_single_fragment(&self) -> bool {
        self.lock().is_single_fragment()
    }
}

/// Fragment size from FFI input, defaulting to the version 10-M fit
pub(crate) fn fragment_size_or_default(fragment_size: Option<u32>) -> usize {
    fragment_size
        .map(|s| s as usize)
        .unwrap_or(cdk::nuts::nut16::DEFAULT_FRAGMENT_SIZE)
}

/// FFI-compatible NUT-16 binary fountain decoder for animated QR codes
///
/// Feed each scanned frame's exact text with `receive` until `complete`,
/// then read the token with `token`. A failed reassembly resets the decoder.
#[derive(uniffi::Object)]
pub struct TokenFountainDecoder {
    inner: Mutex<cdk::nuts::nut16::FountainDecoder>,
}

impl std::fmt::Debug for TokenFountainDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&*self.lock(), f)
    }
}

impl Default for TokenFountainDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenFountainDecoder {
    /// Lock the inner decoder, recovering from a poisoned mutex
    fn lock(&self) -> MutexGuard<'_, cdk::nuts::nut16::FountainDecoder> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[uniffi::export]
impl TokenFountainDecoder {
    /// Create a new decoder
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(cdk::nuts::nut16::FountainDecoder::default()),
        }
    }

    /// Feed one scanned frame, as the exact QR text
    ///
    /// Returns whether the frame added information. Errors on an invalid
    /// frame or one from a different transfer (call `reset` to switch).
    pub fn receive(&self, text: String) -> Result<bool, FfiError> {
        Ok(self.lock().receive(&text)?)
    }

    /// Returns whether the token has been fully reassembled
    pub fn complete(&self) -> bool {
        self.lock().complete()
    }

    /// Returns the reassembled token once `complete`, `None` before
    pub fn token(&self) -> Result<Option<Arc<Token>>, FfiError> {
        let token = self.lock().token()?;
        Ok(token.map(|t| Arc::new(t.into())))
    }

    /// Returns the number of source fragments, `0` before the first frame
    pub fn fragment_count(&self) -> u32 {
        self.lock().fragment_count() as u32
    }

    /// Returns the number of fragments resolved so far
    pub fn resolved_fragment_count(&self) -> u32 {
        self.lock().resolved_fragment_count() as u32
    }

    /// Discards all frames, ready for a new transfer
    pub fn reset(&self) {
        self.lock().reset()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    const TOKEN_STR: &str = "cashuBpGF0gaJhaUgArSaMTR9YJmFwgaNhYQFhc3hAOWE2ZGJiODQ3YmQyMzJiYTc2ZGIwZGYxOTcyMTZiMjlkM2I4Y2MxNDU1M2NkMjc4MjdmYzFjYzk0MmZlZGI0ZWFjWCEDhhhUP_trhpXfStS6vN6So0qWvc2X3O4NfM-Y1HISZ5JhZGlUaGFuayB5b3VhbXVodHRwOi8vbG9jYWxob3N0OjMzMzhhdWNzYXQ=";

    fn test_token() -> Arc<Token> {
        Arc::new(Token::from_str(TOKEN_STR).expect("valid token"))
    }

    #[test]
    fn test_encoder_decoder_roundtrip() {
        let encoder = test_token().ur_encoder(Some(100)).expect("encoder");
        assert!(!encoder.is_single_fragment());
        assert!(encoder.fragment_count() > 1);

        let decoder = TokenUrDecoder::new();
        let mut frames = 0;
        while !decoder.complete() {
            let part = encoder.next_part().expect("part");
            assert!(part.starts_with("ur:bytes/"));
            decoder.receive(part).expect("receive");
            frames += 1;
            // Fountain parts guarantee completion shortly after fragment_count
            assert!(frames <= encoder.fragment_count() * 2);
        }

        let token = decoder.token().expect("token").expect("complete");
        // `Token` re-serialization is not byte-identical to the input
        // string (key ordering), so compare against the re-encoded token
        assert_eq!(token.encode(), test_token().encode());
    }

    #[test]
    fn test_single_fragment_roundtrip() {
        // Fragment budget large enough for the token to fit one frame
        let encoder = test_token().ur_encoder(Some(1000)).expect("encoder");
        assert!(encoder.is_single_fragment());

        let part = encoder.next_part().expect("part");
        let decoder = TokenUrDecoder::new();
        assert_eq!(decoder.resolved_fragment_count(), None);
        decoder.receive(part).expect("receive");

        assert!(decoder.complete());
        assert_eq!(decoder.resolved_fragment_count(), Some(1));
        let token = decoder.token().expect("token").expect("complete");
        assert_eq!(token.encode(), test_token().encode());
    }

    #[test]
    fn test_decoder_rejects_wrong_ur_type() {
        let decoder = TokenUrDecoder::new();
        assert!(decoder
            .receive("ur:crypto-psbt/1-1/lpadaxcswtcyztyalnwe".to_string())
            .is_err());
    }

    #[test]
    fn test_token_before_complete_is_none() {
        let encoder = test_token().ur_encoder(Some(50)).expect("encoder");
        let decoder = TokenUrDecoder::new();

        // A single frame of a multi-part token cannot complete the decode
        let part = encoder.next_part().expect("part");
        decoder.receive(part).expect("receive");

        assert!(!decoder.complete());
        assert!(decoder.fragment_count() > 1);
        assert!(decoder.token().expect("token").is_none());
    }

    #[test]
    fn test_fountain_roundtrip() {
        let encoder = test_token().fountain_encoder(Some(32)).expect("encoder");
        assert!(encoder.fragment_count() > 1);

        let decoder = TokenFountainDecoder::new();
        while !decoder.complete() {
            let frame = encoder.next_frame().expect("frame");
            assert!(is_fountain_frame(frame.clone()));
            decoder.receive(frame).expect("receive");
        }
        let token = decoder.token().expect("token").expect("complete");
        assert_eq!(token.encode(), test_token().encode());
    }

    #[test]
    fn test_fountain_from_string_keeps_bytes() {
        let encoder = TokenFountainEncoder::from_token_string(TOKEN_STR.to_string(), Some(1000))
            .expect("encoder");
        assert!(encoder.is_single_fragment());
        let decoder = TokenFountainDecoder::new();
        decoder
            .receive(encoder.next_frame().expect("frame"))
            .expect("receive");
        let token = decoder.token().expect("token").expect("complete");
        assert_eq!(token.encode(), test_token().encode());
        assert!(!is_fountain_frame(TOKEN_STR.to_string()));
    }
}

//! Domain-based rate-limiting for the `add-leaf` endpoint (section 4 of
//! the log server protocol).
//!
//! A public Sigsum log that accepts submissions from anyone is expected to
//! require proof that the submitter controls a DNS domain, via a
//! `_sigsum_v1.<domain>` TXT record. This module implements the
//! submitter's side of that setup: generating the rate-limit keypair and
//! producing the per-log submit token that gets sent as the `sigsum-token`
//! HTTP header.

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rand_core::OsRng;

use crate::hashing::NAMESPACE_SUBMIT_TOKEN;

/// The Ed25519 keypair a submitter registers via a `_sigsum_v1.<domain>`
/// DNS TXT record, used only to prove domain ownership for rate-limiting
/// purposes. Per the spec this should be a different key from the one
/// used to sign leaves.
#[derive(Debug)]
pub struct RateLimitKeyPair {
    signing_key: SigningKey,
}

impl RateLimitKeyPair {
    /// Generates a new rate-limit keypair. Persist the result if you plan
    /// to keep using the same DNS TXT record; there is no requirement that
    /// this key be long-lived (see spec section 4.1).
    pub fn generate() -> Self {
        Self {
            signing_key: SigningKey::generate(&mut OsRng),
        }
    }

    /// Restores a rate-limit keypair from raw Ed25519 secret key bytes.
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(bytes),
        }
    }

    /// The hex-encoded public key to publish as the value of the
    /// `_sigsum_v1.<domain>` TXT record.
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.signing_key.verifying_key().as_bytes())
    }

    /// Produces the submit token for `log_public_key`, to be sent as the
    /// `sigsum-token` header on every `add-leaf` request to that log. The
    /// token is only ever valid for the specific log it was created for.
    pub fn submit_token(&self, domain: &str, log_public_key: &VerifyingKey) -> SubmitToken {
        let mut signing_bytes = Vec::with_capacity(NAMESPACE_SUBMIT_TOKEN.len() + 1 + 32);
        signing_bytes.extend_from_slice(NAMESPACE_SUBMIT_TOKEN);
        signing_bytes.push(0);
        signing_bytes.extend_from_slice(log_public_key.as_bytes());

        let signature = self.signing_key.sign(&signing_bytes).to_bytes();
        SubmitToken {
            domain: domain.to_string(),
            signature,
        }
    }
}

/// A ready-to-send `sigsum-token` header value: the domain the rate-limit
/// key was registered under, plus a signature proving control of the
/// corresponding private key, scoped to one specific log.
#[derive(Debug, Clone)]
pub struct SubmitToken {
    domain: String,
    signature: [u8; 64],
}

impl SubmitToken {
    /// Formats the token as the literal value of the `sigsum-token` HTTP
    /// header: `<domain> <hex signature>`.
    pub fn header_value(&self) -> String {
        format!("{} {}", self.domain, hex::encode(self.signature))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_token_is_scoped_to_one_log() {
        let rate_limit_key = RateLimitKeyPair::generate();
        let log_a = SigningKey::generate(&mut OsRng).verifying_key();
        let log_b = SigningKey::generate(&mut OsRng).verifying_key();

        let token_a = rate_limit_key.submit_token("mint.example.com", &log_a);
        let token_b = rate_limit_key.submit_token("mint.example.com", &log_b);

        assert_ne!(token_a.header_value(), token_b.header_value());
    }
}

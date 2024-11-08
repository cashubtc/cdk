//! Mint Quote Signatures

use std::str::FromStr;

use bitcoin::secp256k1::schnorr::Signature;
use thiserror::Error;

use super::{MintBolt11Request, PublicKey, SecretKey};

/// Nut19 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Witness not provided
    #[error("Witness not provided")]
    WitnessMissing,
    /// Quote witness invalid signature
    #[error("Quote witness invalid signature")]
    InvalidWitness,
    /// Nut01 error
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut01::Error),
}

impl MintBolt11Request {
    /// Constructs the message to be signed according to NUT-19 specification.
    ///
    /// The message is constructed by concatenating:
    /// 1. The quote ID
    /// 2. All blinded secrets (B_0 through B_n)
    ///
    /// Format: `quote_id || B_0 || B_1 || ... || B_n`
    pub fn msg_to_sign(&self) -> String {
        // Pre-calculate capacity to avoid reallocations
        let capacity = self.quote.len() + (self.outputs.len() * 66);
        let mut msg = String::with_capacity(capacity);

        msg.push_str(&self.quote);
        for output in &self.outputs {
            msg.push_str(&output.blinded_secret.to_hex());
        }
        msg
    }

    /// Sign [`MintBolt11Request`]
    pub fn sign(&mut self, secret_key: SecretKey) -> Result<(), Error> {
        let msg = self.msg_to_sign();

        let signature: Signature = secret_key.sign(msg.as_bytes())?;

        self.witness = Some(signature.to_string());

        Ok(())
    }

    /// Verify signature on [`MintBolt11Request`]
    pub fn verify_witness(&self, pubkey: PublicKey) -> Result<(), Error> {
        let witness = self.witness.as_ref().ok_or(Error::WitnessMissing)?;

        let signature = Signature::from_str(witness).map_err(|_| Error::InvalidWitness)?;

        let msg_to_sign = self.msg_to_sign();

        pubkey.verify(msg_to_sign.as_bytes(), &signature)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_msg_to_sign() {
        let request: MintBolt11Request = serde_json::from_str(r#"{"quote":"9d745270-1405-46de-b5c5-e2762b4f5e00","outputs":[{"amount":1,"id":"00456a94ab4e1c46","B_":"0342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834"},{"amount":1,"id":"00456a94ab4e1c46","B_":"032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4"},{"amount":1,"id":"00456a94ab4e1c46","B_":"033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c311"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b53"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79"}],"witness":"cb2b8e7ea69362dfe2a07093f2bbc319226db33db2ef686c940b5ec976bcbfc78df0cd35b3e998adf437b09ee2c950bd66dfe9eb64abd706e43ebc7c669c36c3"}"#).unwrap();

        let expected_msg_to_sign = "9d745270-1405-46de-b5c5-e2762b4f5e000342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c31102be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b5302209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79";

        let request_msg_to_sign = request.msg_to_sign();

        assert_eq!(expected_msg_to_sign, request_msg_to_sign);
    }

    #[test]
    fn test_valid_signature() {
        let pubkey = PublicKey::from_hex(
            "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
        )
        .unwrap();

        let request: MintBolt11Request = serde_json::from_str(r#"{"quote":"9d745270-1405-46de-b5c5-e2762b4f5e00","outputs":[{"amount":1,"id":"00456a94ab4e1c46","B_":"0342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834"},{"amount":1,"id":"00456a94ab4e1c46","B_":"032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4"},{"amount":1,"id":"00456a94ab4e1c46","B_":"033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c311"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b53"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79"}], "witness": "d4b386f21f7aa7172f0994ee6e4dd966539484247ea71c99b81b8e09b1bb2acbc0026a43c221fd773471dc30d6a32b04692e6837ddaccf0830a63128308e4ee0"}"#).unwrap();

        assert!(request.verify_witness(pubkey).is_ok());
    }

    #[test]
    fn test_mint_request_signature() {
        let mut request: MintBolt11Request = serde_json::from_str(r#"{"quote":"9d745270-1405-46de-b5c5-e2762b4f5e00","outputs":[{"amount":1,"id":"00456a94ab4e1c46","B_":"0342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834"},{"amount":1,"id":"00456a94ab4e1c46","B_":"032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4"},{"amount":1,"id":"00456a94ab4e1c46","B_":"033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c311"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b53"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79"}]}"#).unwrap();

        let secret =
            SecretKey::from_hex("50d7fd7aa2b2fe4607f41f4ce6f8794fc184dd47b8cdfbe4b3d1249aa02d35aa")
                .unwrap();

        request.sign(secret.clone()).unwrap();

        assert!(request.verify_witness(secret.public_key()).is_ok());
    }

    #[test]
    fn test_invalid_signature() {
        let pubkey = PublicKey::from_hex(
            "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
        )
        .unwrap();

        let request: MintBolt11Request = serde_json::from_str(r#"{"quote":"9d745270-1405-46de-b5c5-e2762b4f5e00","outputs":[{"amount":1,"id":"00456a94ab4e1c46","B_":"0342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834"},{"amount":1,"id":"00456a94ab4e1c46","B_":"032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4"},{"amount":1,"id":"00456a94ab4e1c46","B_":"033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c311"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b53"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79"}],"witness":"cb2b8e7ea69362dfe2a07093f2bbc319226db33db2ef686c940b5ec976bcbfc78df0cd35b3e998adf437b09ee2c950bd66dfe9eb64abd706e43ebc7c669c36c3"}"#).unwrap();

        // Signature is on a different quote id verification should fail
        assert!(request.verify_witness(pubkey).is_err());
    }
}

//! Mint Quote Signatures

use std::str::FromStr;

use bitcoin::secp256k1::schnorr::Signature;
use thiserror::Error;

use super::{MintBolt11Request, PublicKey, SecretKey};

/// Nut19 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Signature not provided
    #[error("Signature not provided")]
    SignatureMissing,
    /// Quote signature invalid signature
    #[error("Quote signature invalid signature")]
    InvalidSignature,
    /// Nut01 error
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut01::Error),
}

impl<Q> MintBolt11Request<Q>
where
    Q: ToString,
{
    /// Constructs the message to be signed according to NUT-20 specification.
    ///
    /// The message is constructed by concatenating (as UTF-8 encoded bytes):
    /// 1. The quote ID (as UTF-8)
    /// 2. All blinded secrets (B_0 through B_n) converted to hex strings (as UTF-8)
    ///
    /// Format: `quote_id || B_0 || B_1 || ... || B_n`
    /// where each component is encoded as UTF-8 bytes
    pub fn msg_to_sign(&self) -> Vec<u8> {
        // Pre-calculate capacity to avoid reallocations
        let quote_id = self.quote.to_string();
        let capacity = quote_id.len() + (self.outputs.len() * 66);
        let mut msg = Vec::with_capacity(capacity);
        msg.append(&mut quote_id.clone().into_bytes()); // String.into_bytes() produces UTF-8
        for output in &self.outputs {
            // to_hex() creates a hex string, into_bytes() converts it to UTF-8 bytes
            msg.append(&mut output.blinded_secret.to_hex().into_bytes());
        }
        msg
    }

    /// Sign [`MintBolt11Request`]
    pub fn sign(&mut self, secret_key: SecretKey) -> Result<(), Error> {
        let msg = self.msg_to_sign();

        let signature: Signature = secret_key.sign(&msg)?;

        self.signature = Some(signature.to_string());

        Ok(())
    }

    /// Verify signature on [`MintBolt11Request`]
    pub fn verify_signature(&self, pubkey: PublicKey) -> Result<(), Error> {
        let signature = self.signature.as_ref().ok_or(Error::SignatureMissing)?;

        let signature = Signature::from_str(signature).map_err(|_| Error::InvalidSignature)?;

        let msg_to_sign = self.msg_to_sign();

        pubkey.verify(&msg_to_sign, &signature)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use uuid::Uuid;

    use super::*;

    #[test]
    fn test_msg_to_sign() {
        let request: MintBolt11Request<String> = serde_json::from_str(r#"{"quote":"9d745270-1405-46de-b5c5-e2762b4f5e00","outputs":[{"amount":1,"id":"00456a94ab4e1c46","B_":"0342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834"},{"amount":1,"id":"00456a94ab4e1c46","B_":"032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4"},{"amount":1,"id":"00456a94ab4e1c46","B_":"033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c311"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b53"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79"}],"signature":"cb2b8e7ea69362dfe2a07093f2bbc319226db33db2ef686c940b5ec976bcbfc78df0cd35b3e998adf437b09ee2c950bd66dfe9eb64abd706e43ebc7c669c36c3"}"#).unwrap();

        // let expected_msg_to_sign = "9d745270-1405-46de-b5c5-e2762b4f5e000342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c31102be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b5302209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79";

        let expected_msg_to_sign = [
            57, 100, 55, 52, 53, 50, 55, 48, 45, 49, 52, 48, 53, 45, 52, 54, 100, 101, 45, 98, 53,
            99, 53, 45, 101, 50, 55, 54, 50, 98, 52, 102, 53, 101, 48, 48, 48, 51, 52, 50, 101, 53,
            98, 99, 99, 55, 55, 102, 53, 98, 50, 97, 51, 99, 50, 97, 102, 98, 52, 48, 98, 98, 53,
            57, 49, 97, 49, 101, 50, 55, 100, 97, 56, 51, 99, 100, 100, 99, 57, 54, 56, 97, 98,
            100, 99, 48, 101, 99, 52, 57, 48, 52, 50, 48, 49, 97, 50, 48, 49, 56, 51, 52, 48, 51,
            50, 102, 100, 51, 99, 52, 100, 99, 52, 57, 97, 50, 56, 52, 52, 97, 56, 57, 57, 57, 56,
            100, 53, 101, 57, 100, 53, 98, 48, 102, 48, 98, 48, 48, 100, 100, 101, 57, 51, 49, 48,
            48, 54, 51, 97, 99, 98, 56, 97, 57, 50, 101, 50, 102, 100, 97, 102, 97, 52, 49, 50, 54,
            100, 52, 48, 51, 51, 98, 54, 102, 100, 101, 53, 48, 98, 54, 97, 48, 100, 102, 101, 54,
            49, 97, 100, 49, 52, 56, 102, 102, 102, 49, 54, 55, 97, 100, 57, 99, 102, 56, 51, 48,
            56, 100, 101, 100, 53, 102, 54, 102, 54, 98, 50, 102, 101, 48, 48, 48, 97, 48, 51, 54,
            99, 52, 54, 52, 99, 51, 49, 49, 48, 50, 98, 101, 53, 97, 53, 53, 102, 48, 51, 101, 53,
            99, 48, 97, 97, 101, 97, 55, 55, 53, 57, 53, 100, 53, 55, 52, 98, 99, 101, 57, 50, 99,
            54, 100, 53, 55, 97, 50, 97, 48, 102, 98, 50, 98, 53, 57, 53, 53, 99, 48, 98, 56, 55,
            101, 52, 53, 50, 48, 101, 48, 54, 98, 53, 51, 48, 50, 50, 48, 57, 102, 99, 50, 56, 55,
            51, 102, 50, 56, 53, 50, 49, 99, 98, 100, 100, 101, 55, 102, 55, 98, 51, 98, 98, 49,
            53, 50, 49, 48, 48, 50, 52, 54, 51, 102, 53, 57, 55, 57, 54, 56, 54, 102, 100, 49, 53,
            54, 102, 50, 51, 102, 101, 54, 97, 56, 97, 97, 50, 98, 55, 57,
        ]
        .to_vec();

        let request_msg_to_sign = request.msg_to_sign();

        assert_eq!(expected_msg_to_sign, request_msg_to_sign);
    }

    #[test]
    fn test_valid_signature() {
        let pubkey = PublicKey::from_hex(
            "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
        )
        .unwrap();

        let request: MintBolt11Request<Uuid> = serde_json::from_str(r#"{"quote":"9d745270-1405-46de-b5c5-e2762b4f5e00","outputs":[{"amount":1,"id":"00456a94ab4e1c46","B_":"0342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834"},{"amount":1,"id":"00456a94ab4e1c46","B_":"032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4"},{"amount":1,"id":"00456a94ab4e1c46","B_":"033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c311"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b53"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79"}], "signature": "d4b386f21f7aa7172f0994ee6e4dd966539484247ea71c99b81b8e09b1bb2acbc0026a43c221fd773471dc30d6a32b04692e6837ddaccf0830a63128308e4ee0"}"#).unwrap();

        assert!(request.verify_signature(pubkey).is_ok());
    }

    #[test]
    fn test_mint_request_signature() {
        let mut request: MintBolt11Request<String> = serde_json::from_str(r#"{"quote":"9d745270-1405-46de-b5c5-e2762b4f5e00","outputs":[{"amount":1,"id":"00456a94ab4e1c46","B_":"0342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834"},{"amount":1,"id":"00456a94ab4e1c46","B_":"032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4"},{"amount":1,"id":"00456a94ab4e1c46","B_":"033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c311"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b53"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79"}]}"#).unwrap();

        let secret =
            SecretKey::from_hex("50d7fd7aa2b2fe4607f41f4ce6f8794fc184dd47b8cdfbe4b3d1249aa02d35aa")
                .unwrap();

        request.sign(secret.clone()).unwrap();

        assert!(request.verify_signature(secret.public_key()).is_ok());
    }

    #[test]
    fn test_invalid_signature() {
        let pubkey = PublicKey::from_hex(
            "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
        )
        .unwrap();

        let request: MintBolt11Request<String> = serde_json::from_str(r#"{"quote":"9d745270-1405-46de-b5c5-e2762b4f5e00","outputs":[{"amount":1,"id":"00456a94ab4e1c46","B_":"0342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834"},{"amount":1,"id":"00456a94ab4e1c46","B_":"032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4"},{"amount":1,"id":"00456a94ab4e1c46","B_":"033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c311"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b53"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79"}],"signature":"cb2b8e7ea69362dfe2a07093f2bbc319226db33db2ef686c940b5ec976bcbfc78df0cd35b3e998adf437b09ee2c950bd66dfe9eb64abd706e43ebc7c669c36c3"}"#).unwrap();

        // Signature is on a different quote id verification should fail
        assert!(request.verify_signature(pubkey).is_err());
    }
}

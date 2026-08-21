//! Mint Quote Signatures

use std::str::FromStr;

use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::Network;
use thiserror::Error;

use super::{BlindedMessage, MintRequest, PublicKey, SecretKey};

/// NUT-20 Error
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
    /// BIP32 derivation error
    #[error(transparent)]
    Bip32(#[from] bitcoin::bip32::Error),
}

const MINT_QUOTE_SIG_DOMAIN_TAG: &[u8] = b"Cashu_MintQuoteSig_v1";
const QUOTE_LOCKING_PURPOSE: u32 = 129_373;
const QUOTE_LOCKING_ACCOUNT: u32 = 20;

/// Derive a deterministic NUT-20 quote-locking key from a wallet seed.
///
/// Keys use the BIP32 path `m/129373'/20'/0'/0'/{counter}`, where `counter`
/// is a non-hardened child index independent from all other wallet counters.
pub fn derive_quote_locking_key(seed: &[u8; 64], counter: u32) -> Result<SecretKey, Error> {
    let path = DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(QUOTE_LOCKING_PURPOSE)?,
        ChildNumber::from_hardened_idx(QUOTE_LOCKING_ACCOUNT)?,
        ChildNumber::from_hardened_idx(0)?,
        ChildNumber::from_hardened_idx(0)?,
        ChildNumber::from_normal_idx(counter)?,
    ]);
    let xpriv = Xpriv::new_master(Network::Bitcoin, seed)?;
    let derived_key = xpriv.derive_priv(&crate::SECP256K1, &path)?.private_key;

    Ok(derived_key.into())
}

pub(crate) fn amount_to_minimal_bytes(amount: crate::Amount) -> Vec<u8> {
    let value = u64::from(amount);
    if value == 0 {
        return Vec::new();
    }

    let bytes = value.to_be_bytes();
    let first_non_zero = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len());
    bytes[first_non_zero..].to_vec()
}

pub(crate) fn append_len_prefixed(msg: &mut Vec<u8>, bytes: &[u8]) {
    msg.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    msg.extend_from_slice(bytes);
}

pub(crate) fn mint_quote_msg_to_sign(quote_id: &str, outputs: &[BlindedMessage]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(
        MINT_QUOTE_SIG_DOMAIN_TAG.len() + 4 + quote_id.len() + outputs.len() * (4 + 8 + 4 + 33),
    );

    msg.extend_from_slice(MINT_QUOTE_SIG_DOMAIN_TAG);
    append_len_prefixed(&mut msg, quote_id.as_bytes());

    for output in outputs {
        append_len_prefixed(&mut msg, &amount_to_minimal_bytes(output.amount));
        append_len_prefixed(&mut msg, &output.blinded_secret.to_bytes());
    }

    msg
}

pub(crate) fn legacy_mint_quote_msg_to_sign(quote_id: &str, outputs: &[BlindedMessage]) -> Vec<u8> {
    let capacity = quote_id.len() + (outputs.len() * 66);
    let mut msg = Vec::with_capacity(capacity);

    msg.extend_from_slice(quote_id.as_bytes());
    for output in outputs {
        msg.extend_from_slice(output.blinded_secret.to_hex().as_bytes());
    }

    msg
}

impl<Q> MintRequest<Q>
where
    Q: ToString,
{
    /// Constructs the message to be signed according to NUT-20 specification.
    ///
    /// The message is domain-separated and length-prefixed, committing to the
    /// quote ID, output amounts, and blinded messages in request order.
    pub fn msg_to_sign(&self) -> Vec<u8> {
        let quote_id = self.quote.to_string();
        mint_quote_msg_to_sign(&quote_id, &self.outputs)
    }

    fn legacy_msg_to_sign(&self) -> Vec<u8> {
        let quote_id = self.quote.to_string();
        legacy_mint_quote_msg_to_sign(&quote_id, &self.outputs)
    }

    /// Sign [`MintRequest`]
    pub fn sign(&mut self, secret_key: &SecretKey) -> Result<(), Error> {
        let msg = self.msg_to_sign();

        let signature: Signature = secret_key.sign(&msg)?;

        self.signature = Some(signature.to_string());

        Ok(())
    }

    /// Sign [`MintRequest`] using the legacy NUT-20 message format.
    ///
    /// This is only for wallet compatibility retries against mints that have
    /// not yet upgraded to the domain-separated quote signature message.
    pub fn sign_legacy(&mut self, secret_key: SecretKey) -> Result<(), Error> {
        let msg = self.legacy_msg_to_sign();

        let signature: Signature = secret_key.sign(&msg)?;

        self.signature = Some(signature.to_string());

        Ok(())
    }

    /// Verify signature on [`MintRequest`]
    pub fn verify_signature(&self, pubkey: PublicKey) -> Result<(), Error> {
        let signature = self.signature.as_ref().ok_or(Error::SignatureMissing)?;

        let signature = Signature::from_str(signature).map_err(|_| Error::InvalidSignature)?;

        let quote_id = self.quote.to_string();
        let msg_to_sign = mint_quote_msg_to_sign(&quote_id, &self.outputs);

        match pubkey.verify(&msg_to_sign, &signature) {
            Ok(()) => return Ok(()),
            Err(_) => {
                let legacy_msg = legacy_mint_quote_msg_to_sign(&quote_id, &self.outputs);
                pubkey.verify(&legacy_msg, &signature)?;
                tracing::warn!(
                    quote_id = %quote_id,
                    output_count = self.outputs.len(),
                    "Accepted legacy mint quote signature format"
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bip39::Mnemonic;

    use super::*;

    #[test]
    fn quote_locking_key_derivation_matches_test_vectors() {
        let mnemonic = Mnemonic::from_str(
            "half depart obvious quality work element tank gorilla view sugar picture humble",
        )
        .expect("valid mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let expected = [
            "03062837166e56114b59a4d1fd3a5a812bf7aadc1dde758428cf943d80acd41539",
            "02b47d9d41725f5ce6f08c874835cef25376cb1e95f6cb073fef52ca8fd986cf15",
            "029acbd3a46fd75bc05ba0226d0b4d909b2fb6e96c80544a094a1a3567737e44d3",
            "0373e4a42fbe0a4e18aadb57cf500b655f2446b4071ee579121d2ed8905bcc49c2",
            "02b8709bfce17c10f1864f5218844533ae60930d52089669b317d8b5f474eec071",
        ];

        for (counter, expected_pubkey) in expected.into_iter().enumerate() {
            let secret_key = derive_quote_locking_key(&seed, counter as u32)
                .expect("test vector key should derive");
            assert_eq!(secret_key.public_key().to_hex(), expected_pubkey);
        }
    }

    #[test]
    fn quote_locking_key_derivation_rejects_hardened_counter() {
        let seed = [0; 64];

        assert!(derive_quote_locking_key(&seed, 1 << 31).is_err());
    }

    #[test]
    fn amount_encoding_is_minimal_big_endian() {
        assert_eq!(
            amount_to_minimal_bytes(crate::Amount::ZERO),
            Vec::<u8>::new()
        );
        assert_eq!(
            amount_to_minimal_bytes(crate::Amount::from(256)),
            vec![1, 0]
        );
    }

    #[test]
    fn test_msg_to_sign() {
        let request: MintRequest<String> = serde_json::from_str(r#"{"quote":"0192d3c0-7e8a-7c3d-8e9f-1a2b3c4d5e6f","outputs":[{"amount":1,"id":"009a1f293253e41e","B_":"036d6caac248af96f6afa7f904f550253a0f3ef3f5aa2fe6838a95b216691468e2"},{"amount":1,"id":"009a1f293253e41e","B_":"021f8a566c205633d029094747d2e18f44e05993dda7a5f88f496078205f656e59"}],"signature":"4881093a332ff7c79f3e598ce5b249d64978b47165a0b19c18adf0ced0246228e61e702f0abaf1bf27b92be4336bdbabacfbe4c914076386b3c66fdcd0b3480e"}"#).unwrap();

        let expected_msg_to_sign = crate::util::hex::decode("43617368755f4d696e7451756f74655369675f76310000002430313932643363302d376538612d376333642d386539662d316132623363346435653666000000010100000021036d6caac248af96f6afa7f904f550253a0f3ef3f5aa2fe6838a95b216691468e2000000010100000021021f8a566c205633d029094747d2e18f44e05993dda7a5f88f496078205f656e59").expect("valid hex");

        let request_msg_to_sign = request.msg_to_sign();

        assert_eq!(expected_msg_to_sign, request_msg_to_sign);

        use bitcoin::hashes::sha256::Hash as Sha256Hash;
        use bitcoin::hashes::Hash;

        assert_eq!(
            Sha256Hash::hash(&request_msg_to_sign).to_string(),
            "c164fd384879f74ab6ea2e7cf13d90ed42e6df9d5de607eeb5c9cc7d36fb1c21"
        );
    }

    #[cfg(feature = "mint")]
    #[test]
    fn test_valid_signature_from_test_vector() {
        let pubkey = PublicKey::from_hex(
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        )
        .expect("valid pubkey");

        let request: MintRequest<String> = serde_json::from_str(r#"{"quote":"0192d3c0-7e8a-7c3d-8e9f-1a2b3c4d5e6f","outputs":[{"amount":1,"id":"009a1f293253e41e","B_":"036d6caac248af96f6afa7f904f550253a0f3ef3f5aa2fe6838a95b216691468e2"},{"amount":1,"id":"009a1f293253e41e","B_":"021f8a566c205633d029094747d2e18f44e05993dda7a5f88f496078205f656e59"}],"signature":"4881093a332ff7c79f3e598ce5b249d64978b47165a0b19c18adf0ced0246228e61e702f0abaf1bf27b92be4336bdbabacfbe4c914076386b3c66fdcd0b3480e"}"#).expect("valid request");

        assert!(request.verify_signature(pubkey).is_ok());
    }

    #[cfg(feature = "mint")]
    #[test]
    fn test_valid_legacy_signature_fallback() {
        use uuid::Uuid;

        let pubkey = PublicKey::from_hex(
            "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
        )
        .expect("valid pubkey");

        let request: MintRequest<Uuid> = serde_json::from_str(r#"{"quote":"9d745270-1405-46de-b5c5-e2762b4f5e00","outputs":[{"amount":1,"id":"00456a94ab4e1c46","B_":"0342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834"},{"amount":1,"id":"00456a94ab4e1c46","B_":"032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4"},{"amount":1,"id":"00456a94ab4e1c46","B_":"033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c311"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b53"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79"}], "signature": "d4b386f21f7aa7172f0994ee6e4dd966539484247ea71c99b81b8e09b1bb2acbc0026a43c221fd773471dc30d6a32b04692e6837ddaccf0830a63128308e4ee0"}"#).unwrap();

        assert!(request.verify_signature(pubkey).is_ok());
    }

    #[test]
    fn test_sign_legacy_produces_verifiable_signature() {
        let mut request: MintRequest<String> = serde_json::from_str(r#"{"quote":"legacy-quote","outputs":[{"amount":2,"id":"00456a94ab4e1c46","B_":"0342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834"}]}"#).unwrap();
        let secret =
            SecretKey::from_hex("50d7fd7aa2b2fe4607f41f4ce6f8794fc184dd47b8cdfbe4b3d1249aa02d35aa")
                .expect("valid secret key");
        let pubkey = secret.public_key();

        request.sign_legacy(secret).unwrap();

        assert!(request.verify_signature(pubkey).is_ok());
    }

    #[test]
    fn test_mint_request_signature() {
        let mut request: MintRequest<String> = serde_json::from_str(r#"{"quote":"9d745270-1405-46de-b5c5-e2762b4f5e00","outputs":[{"amount":1,"id":"00456a94ab4e1c46","B_":"0342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834"},{"amount":1,"id":"00456a94ab4e1c46","B_":"032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4"},{"amount":1,"id":"00456a94ab4e1c46","B_":"033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c311"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b53"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79"}]}"#).unwrap();

        let secret =
            SecretKey::from_hex("50d7fd7aa2b2fe4607f41f4ce6f8794fc184dd47b8cdfbe4b3d1249aa02d35aa")
                .expect("valid secret key");

        request.sign(&secret).unwrap();

        assert!(request.verify_signature(secret.public_key()).is_ok());
    }

    #[test]
    fn test_invalid_signature() {
        let pubkey = PublicKey::from_hex(
            "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
        )
        .expect("valid pubkey");

        let request: MintRequest<String> = serde_json::from_str(r#"{"quote":"9d745270-1405-46de-b5c5-e2762b4f5e00","outputs":[{"amount":1,"id":"00456a94ab4e1c46","B_":"0342e5bcc77f5b2a3c2afb40bb591a1e27da83cddc968abdc0ec4904201a201834"},{"amount":1,"id":"00456a94ab4e1c46","B_":"032fd3c4dc49a2844a89998d5e9d5b0f0b00dde9310063acb8a92e2fdafa4126d4"},{"amount":1,"id":"00456a94ab4e1c46","B_":"033b6fde50b6a0dfe61ad148fff167ad9cf8308ded5f6f6b2fe000a036c464c311"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02be5a55f03e5c0aaea77595d574bce92c6d57a2a0fb2b5955c0b87e4520e06b53"},{"amount":1,"id":"00456a94ab4e1c46","B_":"02209fc2873f28521cbdde7f7b3bb1521002463f5979686fd156f23fe6a8aa2b79"}],"signature":"cb2b8e7ea69362dfe2a07093f2bbc319226db33db2ef686c940b5ec976bcbfc78df0cd35b3e998adf437b09ee2c950bd66dfe9eb64abd706e43ebc7c669c36c3"}"#).unwrap();

        // Signature is on a different quote id verification should fail
        assert!(request.verify_signature(pubkey).is_err());
    }
}

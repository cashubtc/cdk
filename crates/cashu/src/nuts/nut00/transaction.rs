//! Transaction transcripts (NUT-00, v3 keysets)
//!
//! Every spending operation is a transaction: inputs (proofs, paid mint
//! quotes) and outputs (blinded messages, melt quotes). The transcript
//! serializes it as one TLV container record per element; every v3 input
//! signs its own [input digest](Transaction::input_digest).

use std::collections::HashSet;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use thiserror::Error;

use super::tlv::{self, encode_uint, write_record};
use super::{BlindedMessage, Proof};
use crate::nuts::nut01::{DhkePoint, ProofId, PublicKey};
use crate::nuts::nut02::Id;
use crate::util::tagged_hash::tagged_hash_parts;
use crate::Amount;

/// Tag of the per-input digest
pub const TRANSACTION_INPUT_TAG: &str = "Cashu_TransactionInput";

/// Container type of a proof input
pub const PROOF_INPUT: u8 = 0x11;
/// Container type of a mint quote input
pub const MINT_QUOTE_INPUT: u8 = 0x12;
/// Container type of a blinded message output
pub const BLINDED_MESSAGE_OUTPUT: u8 = 0x21;
/// Container type of a melt quote output
pub const MELT_QUOTE_OUTPUT: u8 = 0x22;

/// A SHA-256 digest
pub type Digest = [u8; 32];

/// Transaction error
#[derive(Debug, Error)]
pub enum Error {
    /// A transaction needs at least one input
    #[error("Transaction has no inputs")]
    NoInputs,
    /// A transaction needs at least one output
    #[error("Transaction has no outputs")]
    NoOutputs,
    /// The same proof `Y` appears twice
    #[error("Duplicate proof in transaction")]
    DuplicateProof,
    /// The same quote id appears twice
    #[error("Duplicate quote id in transaction")]
    DuplicateQuote,
    /// Input index out of range
    #[error("No transaction input at index {0}")]
    InputIndex(usize),
    /// A v3 batch mint needs `quote_amounts`
    #[error("A v3 batch mint needs quote_amounts")]
    MissingQuoteAmounts,
    /// Quotes and their states or amounts differ in number
    #[error("{quotes} quotes but {states} quote states or amounts")]
    QuoteCountMismatch {
        /// Number of quotes
        quotes: usize,
        /// Number of states or amounts
        states: usize,
    },
    /// Amount overflow
    #[error("Amount overflow")]
    AmountOverflow,
    /// Proofs and their `Y`s differ in number
    #[error("{proofs} proofs but {ys} Ys")]
    YCountMismatch {
        /// Number of proofs
        proofs: usize,
        /// Number of `Y`s
        ys: usize,
    },
    /// TLV encoding error
    #[error(transparent)]
    Tlv(#[from] tlv::Error),
    /// DHKE error computing a proof's `Y`
    #[error(transparent)]
    Dhke(#[from] crate::dhke::Error),
}

/// A transaction input
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionInput {
    /// A proof (container `0x11`)
    Proof {
        /// Amount
        amount: Amount,
        /// Keyset id
        keyset_id: Id,
        /// `Y`: the keyset's hash to curve of the secret
        y: ProofId,
        /// Unblinded signature
        c: DhkePoint,
    },
    /// A paid mint quote (container `0x12`)
    MintQuote {
        /// Amount this transaction issues against the quote
        amount: Amount,
        /// Quote id
        quote_id: String,
        /// The quote's lock key (its `pubkey`)
        lock_key: PublicKey,
        /// The quote's `amount_issued` when the mint accepts the transaction
        issued: Amount,
    },
}

impl TransactionInput {
    /// The input of a proof, computing its `Y`
    pub fn from_proof(proof: &Proof) -> Result<Self, Error> {
        Ok(Self::Proof {
            amount: proof.amount,
            keyset_id: proof.keyset_id,
            y: crate::dhke::hash_secret_to_curve(&proof.secret, proof.keyset_id.get_version())?
                .into(),
            c: proof.c,
        })
    }

    /// The input of a proof whose `Y` is already known
    pub fn from_proof_with_y(proof: &Proof, y: ProofId) -> Self {
        Self::Proof {
            amount: proof.amount,
            keyset_id: proof.keyset_id,
            y,
            c: proof.c,
        }
    }

    fn container_type(&self) -> u8 {
        match self {
            Self::Proof { .. } => PROOF_INPUT,
            Self::MintQuote { .. } => MINT_QUOTE_INPUT,
        }
    }

    fn fields(&self) -> Result<Vec<u8>, Error> {
        let mut fields = Vec::new();
        match self {
            Self::Proof {
                amount,
                keyset_id,
                y,
                c,
            } => {
                write_record(&mut fields, 0x01, &encode_uint(amount.to_u64()))?;
                write_record(&mut fields, 0x02, &keyset_id.to_bytes())?;
                write_record(&mut fields, 0x03, y.as_bytes())?;
                write_record(&mut fields, 0x04, &c.to_bytes())?;
            }
            Self::MintQuote {
                amount,
                quote_id,
                lock_key,
                issued,
            } => {
                write_record(&mut fields, 0x01, &encode_uint(amount.to_u64()))?;
                write_record(&mut fields, 0x02, quote_id.as_bytes())?;
                write_record(&mut fields, 0x03, &lock_key.to_bytes())?;
                write_record(&mut fields, 0x04, &encode_uint(issued.to_u64()))?;
            }
        }
        Ok(fields)
    }
}

/// A transaction output
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionOutput {
    /// A blinded message (container `0x21`)
    BlindedMessage {
        /// Amount (zero for a blank change output)
        amount: Amount,
        /// Keyset id
        keyset_id: Id,
        /// `SHA256(B_)` over the point in its keyset's encoding
        b_hash: Digest,
    },
    /// A melt quote (container `0x22`)
    MeltQuote {
        /// The quote's amount plus the fee reserve the request selects
        amount: Amount,
        /// Quote id
        quote_id: String,
    },
}

impl TransactionOutput {
    /// The output of a blinded message, exactly as it appears in the request
    pub fn from_blinded_message(output: &BlindedMessage) -> Self {
        Self::BlindedMessage {
            amount: output.amount,
            keyset_id: output.keyset_id,
            b_hash: Sha256Hash::hash(&output.blinded_secret.to_bytes()).to_byte_array(),
        }
    }

    fn container_type(&self) -> u8 {
        match self {
            Self::BlindedMessage { .. } => BLINDED_MESSAGE_OUTPUT,
            Self::MeltQuote { .. } => MELT_QUOTE_OUTPUT,
        }
    }

    fn fields(&self) -> Result<Vec<u8>, Error> {
        let mut fields = Vec::new();
        match self {
            Self::BlindedMessage {
                amount,
                keyset_id,
                b_hash,
            } => {
                write_record(&mut fields, 0x01, &encode_uint(amount.to_u64()))?;
                write_record(&mut fields, 0x02, &keyset_id.to_bytes())?;
                write_record(&mut fields, 0x03, b_hash)?;
            }
            Self::MeltQuote { amount, quote_id } => {
                write_record(&mut fields, 0x01, &encode_uint(amount.to_u64()))?;
                write_record(&mut fields, 0x02, quote_id.as_bytes())?;
            }
        }
        Ok(fields)
    }
}

/// Proof inputs for `proofs`, computing each `Y`
pub fn proof_inputs(proofs: &[Proof]) -> Result<Vec<TransactionInput>, Error> {
    proofs.iter().map(TransactionInput::from_proof).collect()
}

/// Proof inputs for `proofs` whose `Y`s are already known, in the same order
pub fn proof_inputs_with_ys(
    proofs: &[Proof],
    ys: &[ProofId],
) -> Result<Vec<TransactionInput>, Error> {
    if proofs.len() != ys.len() {
        return Err(Error::YCountMismatch {
            proofs: proofs.len(),
            ys: ys.len(),
        });
    }
    Ok(proofs
        .iter()
        .zip(ys)
        .map(|(proof, y)| TransactionInput::from_proof_with_y(proof, *y))
        .collect())
}

/// Blinded message outputs, exactly as they appear in the request
pub fn blinded_outputs(outputs: &[BlindedMessage]) -> Vec<TransactionOutput> {
    outputs
        .iter()
        .map(TransactionOutput::from_blinded_message)
        .collect()
}

/// A mint quote's state as the mint reads it when accepting a transaction
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintQuoteState {
    /// The quote's lock key (its `pubkey`)
    pub lock_key: PublicKey,
    /// The quote's `amount_issued`
    pub issued: Amount,
}

/// A transaction with its transcript and digest
///
/// Inputs and outputs keep request order; the transcript groups their
/// containers by ascending type, request order within a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    inputs: Vec<TransactionInput>,
    outputs: Vec<TransactionOutput>,
    /// Container record of each input, in `inputs` order
    input_containers: Vec<Vec<u8>>,
    transcript: Vec<u8>,
    digest: Digest,
}

impl Transaction {
    /// Build a transaction, rejecting empty sections and duplicates
    pub fn new(
        inputs: Vec<TransactionInput>,
        outputs: Vec<TransactionOutput>,
    ) -> Result<Self, Error> {
        if inputs.is_empty() {
            return Err(Error::NoInputs);
        }
        if outputs.is_empty() {
            return Err(Error::NoOutputs);
        }

        let mut ys = HashSet::new();
        let mut mint_quotes = HashSet::new();
        let mut melt_quotes = HashSet::new();
        for input in &inputs {
            let fresh = match input {
                TransactionInput::Proof { y, .. } => ys.insert(y.as_bytes()),
                TransactionInput::MintQuote { quote_id, .. } => {
                    mint_quotes.insert(quote_id.as_str())
                }
            };
            if !fresh {
                return Err(match input {
                    TransactionInput::Proof { .. } => Error::DuplicateProof,
                    TransactionInput::MintQuote { .. } => Error::DuplicateQuote,
                });
            }
        }
        for output in &outputs {
            if let TransactionOutput::MeltQuote { quote_id, .. } = output {
                if !melt_quotes.insert(quote_id.as_str()) {
                    return Err(Error::DuplicateQuote);
                }
            }
        }

        let input_containers = inputs
            .iter()
            .map(|input| container(input.container_type(), &input.fields()?))
            .collect::<Result<Vec<_>, Error>>()?;
        let output_containers = outputs
            .iter()
            .map(|output| {
                Ok((
                    output.container_type(),
                    container(output.container_type(), &output.fields()?)?,
                ))
            })
            .collect::<Result<Vec<_>, Error>>()?;

        // Stable sorts keep request order within a container type.
        let mut ordered: Vec<(u8, &[u8])> = inputs
            .iter()
            .zip(&input_containers)
            .map(|(input, bytes)| (input.container_type(), bytes.as_slice()))
            .chain(
                output_containers
                    .iter()
                    .map(|(container_type, bytes)| (*container_type, bytes.as_slice())),
            )
            .collect();
        ordered.sort_by_key(|(container_type, _)| *container_type);
        let transcript: Vec<u8> = ordered
            .into_iter()
            .flat_map(|(_, bytes)| bytes.iter().copied())
            .collect();
        let digest = Sha256Hash::hash(&transcript).to_byte_array();

        Ok(Self {
            inputs,
            outputs,
            input_containers,
            transcript,
            digest,
        })
    }

    /// Inputs, in request order
    pub fn inputs(&self) -> &[TransactionInput] {
        &self.inputs
    }

    /// Outputs, in request order
    pub fn outputs(&self) -> &[TransactionOutput] {
        &self.outputs
    }

    /// The TLV transcript
    pub fn transcript(&self) -> &[u8] {
        &self.transcript
    }

    /// `transaction_digest = SHA256(transcript)`
    pub fn digest(&self) -> Digest {
        self.digest
    }

    /// `input_id = SHA256(input container record)` of input `index`
    pub fn input_id(&self, index: usize) -> Result<Digest, Error> {
        let container = self
            .input_containers
            .get(index)
            .ok_or(Error::InputIndex(index))?;
        Ok(Sha256Hash::hash(container).to_byte_array())
    }

    /// `tagged_hash("Cashu_TransactionInput", transaction_digest || input_id)`
    /// of input `index`: the message its witness signs
    pub fn input_digest(&self, index: usize) -> Result<Digest, Error> {
        let input_id = self.input_id(index)?;
        Ok(tagged_hash_parts(
            TRANSACTION_INPUT_TAG,
            &[&self.digest, &input_id],
        ))
    }
}

fn container(container_type: u8, fields: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    write_record(&mut out, container_type, fields)?;
    Ok(out)
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::util::hex;

    const KEYSET: &str = "022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d701";

    fn proof(amount: u64, secret: &str, c: &str) -> Proof {
        serde_json::from_value(serde_json::json!({
            "amount": amount, "id": KEYSET, "secret": secret, "C": c,
        }))
        .unwrap()
    }

    fn output(amount: u64, b: &str) -> TransactionOutput {
        let message: BlindedMessage = serde_json::from_value(serde_json::json!({
            "amount": amount, "id": KEYSET, "B_": b,
        }))
        .unwrap();
        TransactionOutput::from_blinded_message(&message)
    }

    fn swap_input() -> TransactionInput {
        TransactionInput::from_proof(&proof(
            8,
            "0240c186f3e19f6f8cd00db81c5266d5c74ddca48ff848efd0e3e125b2846f30d9",
            "a33dd4de26ea950060ae5c45bdc408d72aaf51ba9f4ccdeee4a040df067ca87c799a406fec88c2a328b75b290e27f7b6",
        ))
        .unwrap()
    }

    /// tests/00-tests.md "Swap"
    #[test]
    fn swap_vector() {
        let tx = Transaction::new(
            vec![swap_input()],
            vec![
                output(4, "b42a0bcc39598db1dca617aeea6bc367f2566636826dc961a54faae15b3b8d10afc1cb0206e70ab3b0e12c2b9478cd55"),
                output(4, "a14669f181e074cc466e2a165c47ab7db5b3939e42b349f1201874033943ef279f36476e627ab1c937e8ea6a76448fc2"),
            ],
        )
        .unwrap();
        assert_eq!(
            hex::encode(tx.transcript()),
            "11008e01000108020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d70103003090f53b3088936b80ecfcc065a0746c5c1843ffd4527dbe3b9aba5a6109aed338ddc0f109511e9dfd37342aa99dd816d8040030a33dd4de26ea950060ae5c45bdc408d72aaf51ba9f4ccdeee4a040df067ca87c799a406fec88c2a328b75b290e27f7b621004b01000104020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d701030020567b4ba980dd0c51c80919633b384c0764b30e5250c4a8b8e3957c9f4208cdee21004b01000104020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d7010300200081a8744c8f3309419a09b08f3ae7e8cdaaaaad64309b78f6fd91f61dfaf502"
        );
        assert_eq!(
            hex::encode(tx.digest()),
            "e88d16feadb0abc0cf4a08a7f5929a3a7c5e595f8d4e659d9ba66086aead402e"
        );
        assert_eq!(
            hex::encode(tx.input_id(0).unwrap()),
            "194bd20817e5806bbe198206e63938c8f0ad38a0d89af2179bb517e24caf1db2"
        );
        assert_eq!(
            hex::encode(tx.input_digest(0).unwrap()),
            "88a664e4f25fbd76baf5846221aa8f9d95beb0341f67d5458332e4f6809abcde"
        );
        assert!(matches!(tx.input_digest(1), Err(Error::InputIndex(1))));
    }

    #[test]
    fn rejects_empty_sections_and_duplicates() {
        let out = output(4, "b42a0bcc39598db1dca617aeea6bc367f2566636826dc961a54faae15b3b8d10afc1cb0206e70ab3b0e12c2b9478cd55");
        assert!(matches!(
            Transaction::new(vec![], vec![out.clone()]),
            Err(Error::NoInputs)
        ));
        assert!(matches!(
            Transaction::new(vec![swap_input()], vec![]),
            Err(Error::NoOutputs)
        ));
        assert!(matches!(
            Transaction::new(vec![swap_input(), swap_input()], vec![out.clone()]),
            Err(Error::DuplicateProof)
        ));
        let quote = TransactionInput::MintQuote {
            amount: Amount::from(1),
            quote_id: "q".to_string(),
            lock_key: PublicKey::from_hex(
                "02f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
            )
            .unwrap(),
            issued: Amount::ZERO,
        };
        assert!(matches!(
            Transaction::new(vec![quote.clone(), quote], vec![out.clone()]),
            Err(Error::DuplicateQuote)
        ));
        let melt = TransactionOutput::MeltQuote {
            amount: Amount::from(1),
            quote_id: "m".to_string(),
        };
        assert!(matches!(
            Transaction::new(vec![swap_input()], vec![melt.clone(), melt]),
            Err(Error::DuplicateQuote)
        ));
    }

    #[test]
    fn containers_group_by_type_in_request_order() {
        let lock_key = PublicKey::from_hex(
            "02f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
        )
        .unwrap();
        let quote = |id: &str| TransactionInput::MintQuote {
            amount: Amount::from(1),
            quote_id: id.to_string(),
            lock_key,
            issued: Amount::ZERO,
        };
        let melt = TransactionOutput::MeltQuote {
            amount: Amount::from(2),
            quote_id: "m".to_string(),
        };
        let blank = output(0, "b42a0bcc39598db1dca617aeea6bc367f2566636826dc961a54faae15b3b8d10afc1cb0206e70ab3b0e12c2b9478cd55");
        // Request order: quote b, proof, quote a / melt, blank
        let tx = Transaction::new(
            vec![quote("b"), swap_input(), quote("a")],
            vec![melt, blank],
        )
        .unwrap();
        let types: Vec<u8> = {
            let mut types = Vec::new();
            let mut rest = tx.transcript();
            while let [t, hi, lo, tail @ ..] = rest {
                types.push(*t);
                let len = usize::from(u16::from_be_bytes([*hi, *lo]));
                rest = &tail[len..];
            }
            types
        };
        assert_eq!(types, vec![0x11, 0x12, 0x12, 0x21, 0x22]);
        // quote "b" precedes quote "a"
        let b_at = tx
            .transcript()
            .windows(4)
            .position(|w| w == [0x02, 0x00, 0x01, b'b'])
            .unwrap();
        let a_at = tx
            .transcript()
            .windows(4)
            .position(|w| w == [0x02, 0x00, 0x01, b'a'])
            .unwrap();
        assert!(b_at < a_at);
        // the input id still indexes request order
        assert_ne!(tx.input_id(0).unwrap(), tx.input_id(2).unwrap());
    }

    const B1: &str = "b42a0bcc39598db1dca617aeea6bc367f2566636826dc961a54faae15b3b8d10afc1cb0206e70ab3b0e12c2b9478cd55";
    const B2: &str = "a14669f181e074cc466e2a165c47ab7db5b3939e42b349f1201874033943ef279f36476e627ab1c937e8ea6a76448fc2";
    const LOCK0: &str = "0292905560a6a511a13e383ee27e220aeffc85b7a7bc293e6935b1d3678d209812";
    const LOCK1: &str = "02b357c1ec7bdd73e4ede25d68619ee607c632409e89bcc518a82370b3f499615d";

    /// Check a BIP-340 signature by `point`'s x-coordinate over `digest`
    fn assert_keypath_sig(point: &str, digest: Digest, signature: &str) {
        use bitcoin::secp256k1::{schnorr, Message};
        let point = PublicKey::from_hex(point).unwrap();
        let signature = schnorr::Signature::from_slice(&hex::decode(signature).unwrap()).unwrap();
        crate::SECP256K1
            .verify_schnorr(
                &signature,
                &Message::from_digest(digest),
                &point.x_only_public_key(),
            )
            .expect("valid key-path signature");
    }

    fn assert_no_keypath_sig(point: &str, digest: Digest, signature: &str) {
        use bitcoin::secp256k1::{schnorr, Message};
        let point = PublicKey::from_hex(point).unwrap();
        let signature = schnorr::Signature::from_slice(&hex::decode(signature).unwrap()).unwrap();
        assert!(crate::SECP256K1
            .verify_schnorr(
                &signature,
                &Message::from_digest(digest),
                &point.x_only_public_key(),
            )
            .is_err());
    }

    fn mint_quote(amount: u64, quote_id: &str, lock_key: &str, issued: u64) -> TransactionInput {
        TransactionInput::MintQuote {
            amount: Amount::from(amount),
            quote_id: quote_id.to_string(),
            lock_key: PublicKey::from_hex(lock_key).unwrap(),
            issued: Amount::from(issued),
        }
    }

    const SECRET0: &str = "0240c186f3e19f6f8cd00db81c5266d5c74ddca48ff848efd0e3e125b2846f30d9";
    const SECRET1: &str = "0385fe6eb0608c6b5241e0fd86ebbd5dd30a6a97a0014a110c0c6297001147e5be";

    #[test]
    fn swap_vector_signature() {
        let tx = Transaction::new(vec![swap_input()], vec![output(4, B1), output(4, B2)]).unwrap();
        assert_keypath_sig(
            SECRET0,
            tx.input_digest(0).unwrap(),
            "3c2ce5dc36aea01a9b9f44287ef12e67a7fb0e0113720c3a000b5942835426e6635b09c087dbd30bde77ddf03b43b87858b3d2bf29d4b3ca7895f73e4839d3c2",
        );
    }

    /// tests/00-tests.md "Multiple proof inputs"
    #[test]
    fn multi_input_vector() {
        let second = TransactionInput::from_proof(&proof(
            4,
            SECRET1,
            "84c2b06e690cf434f38bca886cfe2ca2d8739231f14b350d907bd4dd62ef901c0c99453d30651d43727cdbb2f07fd1b3",
        ))
        .unwrap();
        let tx = Transaction::new(
            vec![swap_input(), second],
            vec![output(8, B1), output(4, B2)],
        )
        .unwrap();
        assert_eq!(
            hex::encode(tx.digest()),
            "9430a89c2db9df2a8c645369ea6f8ca1100f21b3c365ac0bb153422d2fd7e483"
        );
        let expected = [
            (
                SECRET0,
                "194bd20817e5806bbe198206e63938c8f0ad38a0d89af2179bb517e24caf1db2",
                "56a8a7ea8c0e785f2c4b77b99648daca04f7e110dac4c86b2d6372a34ce9393e",
                "d390c4ad38c949c3dceb1d6a75de4bbb68291e2a72a4abef26ea3a0d373a24840572aef6d4359f8baea1ae2ed62336c8d833e91aaf3e78f32c7cf9c0581c3552",
            ),
            (
                SECRET1,
                "810d7cc016b5e6af4dff55b4c96ee7ed397c5e845058abf04ecd479edc50b69a",
                "681a5a9f1881a685e5e8fb0d33dabd7c8987bf78a1e9424bb4535d124dfe6877",
                "f608bb156579e85c5788de9e7d89e10a0aeb39ba6983bc352583406cd8eab1743cdb1eb0b7fb31f2679a2f0c4138b72cfbc92888d409af26c4109016a378a74a",
            ),
        ];
        for (index, (secret, input_id, input_digest, signature)) in expected.iter().enumerate() {
            assert_eq!(hex::encode(tx.input_id(index).unwrap()), *input_id);
            assert_eq!(hex::encode(tx.input_digest(index).unwrap()), *input_digest);
            assert_keypath_sig(secret, tx.input_digest(index).unwrap(), signature);
            // Neither signs the shared digest, nor the other input's digest.
            assert_no_keypath_sig(secret, tx.digest(), signature);
            assert_no_keypath_sig(secret, tx.input_digest(1 - index).unwrap(), signature);
        }
    }

    /// tests/00-tests.md "Mixed keysets"
    #[test]
    fn mixed_keysets_vector() {
        let legacy: Proof = serde_json::from_value(serde_json::json!({
            "amount": 2,
            "id": "00456a94ab4e1c46",
            "secret": "d341ee4871f1f889041e63cf0d3823c713eea6aff01e80f1719f08f9e5be98f6",
            "C": "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
        }))
        .unwrap();
        let legacy = TransactionInput::from_proof(&legacy).unwrap();
        let TransactionInput::Proof { y, .. } = &legacy else {
            panic!("proof input");
        };
        assert_eq!(
            y.to_hex(),
            "029ef117210f475254efd911de93a9d22d471e356f5b1e3f00df8c24bbb37bd3ae"
        );
        let tx = Transaction::new(
            vec![swap_input(), legacy],
            vec![output(8, B1), output(2, B2)],
        )
        .unwrap();
        assert_eq!(
            hex::encode(&tx.input_containers[1]),
            "1100570100010202000800456a94ab4e1c46030021029ef117210f475254efd911de93a9d22d471e356f5b1e3f00df8c24bbb37bd3ae04002102a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2"
        );
        assert_eq!(
            hex::encode(tx.input_id(1).unwrap()),
            "efbdd5d14874cb021fc962d93a9848553ca65ae101e9b57f6678aa37ffe1ebd9"
        );
        assert_eq!(
            hex::encode(tx.transcript()),
            "11008e01000108020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d70103003090f53b3088936b80ecfcc065a0746c5c1843ffd4527dbe3b9aba5a6109aed338ddc0f109511e9dfd37342aa99dd816d8040030a33dd4de26ea950060ae5c45bdc408d72aaf51ba9f4ccdeee4a040df067ca87c799a406fec88c2a328b75b290e27f7b61100570100010202000800456a94ab4e1c46030021029ef117210f475254efd911de93a9d22d471e356f5b1e3f00df8c24bbb37bd3ae04002102a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba221004b01000108020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d701030020567b4ba980dd0c51c80919633b384c0764b30e5250c4a8b8e3957c9f4208cdee21004b01000102020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d7010300200081a8744c8f3309419a09b08f3ae7e8cdaaaaad64309b78f6fd91f61dfaf502"
        );
        assert_eq!(
            hex::encode(tx.digest()),
            "c08be9ca9632ce753c31227e470ecb38ff19df071fd828d1818eb14ea4da489f"
        );
        assert_eq!(
            hex::encode(tx.input_digest(0).unwrap()),
            "94a0d329d7c66c55e0c968bbb680fbb4c7f4d01f6a29f93d6a5975dd15812776"
        );
        assert_keypath_sig(
            SECRET0,
            tx.input_digest(0).unwrap(),
            "d5e861c7ee7635fa2b8dc58aabe2ceb0997651a6e1e85cc3aed31aad42871f4d2444707ad20337a21da5f35e561ca83ec36880ecb2fd8c25eccf33856b99ca93",
        );
    }

    /// tests/00-tests.md: a pre-v3 output hashes its 33-byte `B_`
    #[test]
    fn legacy_output_container() {
        let message: BlindedMessage = serde_json::from_value(serde_json::json!({
            "amount": 2,
            "id": "00456a94ab4e1c46",
            "B_": "033b1a9737a40cc3fd9b6af4b723632b76a67a36782596304612a6c2bfb5197e6d",
        }))
        .unwrap();
        let output = TransactionOutput::from_blinded_message(&message);
        let TransactionOutput::BlindedMessage { b_hash, .. } = &output else {
            panic!("blinded output");
        };
        assert_eq!(
            hex::encode(b_hash),
            "7ae916d2f757e0a11b21f273068e83839e4e0c70f72b1a8ad312fc6da40eb169"
        );
        assert_eq!(
            hex::encode(container(output.container_type(), &output.fields().unwrap()).unwrap()),
            "2100320100010202000800456a94ab4e1c460300207ae916d2f757e0a11b21f273068e83839e4e0c70f72b1a8ad312fc6da40eb169"
        );
    }

    /// tests/00-tests.md "Mint"
    #[test]
    fn mint_vector() {
        let tx = Transaction::new(
            vec![mint_quote(8, "quote-mint-0001", LOCK0, 0)],
            vec![output(8, B1)],
        )
        .unwrap();
        assert_eq!(
            hex::encode(tx.transcript()),
            "12003d0100010802000f71756f74652d6d696e742d303030310300210292905560a6a511a13e383ee27e220aeffc85b7a7bc293e6935b1d3678d20981204000021004b01000108020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d701030020567b4ba980dd0c51c80919633b384c0764b30e5250c4a8b8e3957c9f4208cdee"
        );
        assert_eq!(
            hex::encode(tx.digest()),
            "37e64e944b7580653f44cfa06b416fac9a548d5bbf372e6545c9c982e5a3a70f"
        );
        assert_eq!(
            hex::encode(tx.input_id(0).unwrap()),
            "d294012dd54976b05eb3e36eb218b9a6d66b19d79ec122944a35b99b8ddd8839"
        );
        assert_eq!(
            hex::encode(tx.input_digest(0).unwrap()),
            "b8218d99fe1d3138c16230ed0b119a8ea8a17975131fedd9b7459f2af9aa483a"
        );
    }

    /// tests/00-tests.md "Partial mint"
    #[test]
    fn partial_mint_vector() {
        let signature = "c6709751b0d3c14fef333326f4cd9d45c7af327ea4659a7b4637e92d9a82b0f6c25a4bce9e99385f40f39ea6fe26af9ba07bfaf2c378f2f47a1eb2d474a0a7a6";
        let tx = Transaction::new(
            vec![mint_quote(4, "quote-mint-0004", LOCK0, 3)],
            vec![output(4, B1)],
        )
        .unwrap();
        assert_eq!(
            hex::encode(tx.transcript()),
            "12003e0100010402000f71756f74652d6d696e742d303030340300210292905560a6a511a13e383ee27e220aeffc85b7a7bc293e6935b1d3678d2098120400010321004b01000104020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d701030020567b4ba980dd0c51c80919633b384c0764b30e5250c4a8b8e3957c9f4208cdee"
        );
        assert_eq!(
            hex::encode(tx.digest()),
            "90d943e1f02938e5d50a7ead9f04ec1a633bc207bf7976f54569632db62d8d4d"
        );
        assert_eq!(
            hex::encode(tx.input_id(0).unwrap()),
            "c2e4168d079d7ccc6c9fc80c480e75a255282ec41e660afad42cf743c95f14e3"
        );
        assert_eq!(
            hex::encode(tx.input_digest(0).unwrap()),
            "7754960e452ae522213f1112c6eb5946b0bac278b0a307e38927d59531ed558e"
        );
        assert_keypath_sig(LOCK0, tx.input_digest(0).unwrap(), signature);

        // MUST NOT verify over a transcript committing amount 8 or issued 0.
        for (amount, issued) in [(8, 3), (4, 0)] {
            let other = Transaction::new(
                vec![mint_quote(amount, "quote-mint-0004", LOCK0, issued)],
                vec![output(4, B1)],
            )
            .unwrap();
            assert_no_keypath_sig(LOCK0, other.input_digest(0).unwrap(), signature);
        }
    }

    /// tests/00-tests.md "Serialized quote witnesses", script-only lock transcript
    #[test]
    fn script_only_quote_lock_transcript() {
        let tx = Transaction::new(
            vec![mint_quote(
                8,
                "quote-mint-0001",
                "034b15129a9c665f37fbe9f6c80a7fe6ec0c809f34edde98d298aff94a1e8af276",
                0,
            )],
            vec![output(8, B1)],
        )
        .unwrap();
        assert_eq!(
            hex::encode(tx.digest()),
            "33d8def7c0f5235c150863773e105568d44d079aa2f1d59149bb72254dc6df05"
        );
        assert_eq!(
            hex::encode(tx.input_id(0).unwrap()),
            "a639745001afaad8964370e8ddc1406bbf14d478c6d56819f188c9b10392fe45"
        );
        assert_eq!(
            hex::encode(tx.input_digest(0).unwrap()),
            "7c0b3967b3b7ba4e5daa7e8ea11607cff36062d6ff24863c8bdf31ee8346f8e4"
        );
    }

    /// tests/00-tests.md "Batched mint"
    #[test]
    fn batched_mint_vector() {
        let tx = Transaction::new(
            vec![
                mint_quote(5, "quote-mint-0002", LOCK0, 0),
                mint_quote(3, "quote-mint-0003", LOCK1, 0),
            ],
            vec![output(8, B1)],
        )
        .unwrap();
        assert_eq!(
            hex::encode(tx.transcript()),
            "12003d0100010502000f71756f74652d6d696e742d303030320300210292905560a6a511a13e383ee27e220aeffc85b7a7bc293e6935b1d3678d20981204000012003d0100010302000f71756f74652d6d696e742d3030303303002102b357c1ec7bdd73e4ede25d68619ee607c632409e89bcc518a82370b3f499615d04000021004b01000108020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d701030020567b4ba980dd0c51c80919633b384c0764b30e5250c4a8b8e3957c9f4208cdee"
        );
        assert_eq!(
            hex::encode(tx.digest()),
            "8203006b339dd16a3f6293c0f300bebb3c9ad620755477b1372c0fa479540c72"
        );
        for (index, (lock, input_id, input_digest, signature)) in [
            (
                LOCK0,
                "8c7f916779dfba1b7ea2e0d3e69d1ee7eed1895af2891406fe8bdb7fbe0f39aa",
                "5b87defd8dadbd2b0faf359f53b00a74b0642e5b91dd4046e0183738a812ed13",
                "badda23b373f0d1da963af4d424dbbbccce933ba0c81bc3e2c8ade13decdad8df57ca75efcb4eaa2e02eee9bd43066fce76fee22599f4a513354c138efa6bfb6",
            ),
            (
                LOCK1,
                "f7a60306c44e910fd5dea355541c4a4bb251220690688279bf08e7d19b3e9e45",
                "ac0d58749a343394a0fe5605771be16466ece1ecb331fa1aae0c92ac00136c13",
                "f272dca3c46470511b4ac1ab47a2135d1c7a6af4b6b49ec7c1d120f7704888cce5af8f2026c548a2d5201aebe384f2b798a2c1e439c89d9decc915bc890a4474",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(hex::encode(tx.input_id(index).unwrap()), input_id);
            assert_eq!(hex::encode(tx.input_digest(index).unwrap()), input_digest);
            assert_keypath_sig(lock, tx.input_digest(index).unwrap(), signature);
        }
    }

    /// tests/00-tests.md "Melt"
    #[test]
    fn melt_vector() {
        let tx = Transaction::new(
            vec![swap_input()],
            vec![TransactionOutput::MeltQuote {
                amount: Amount::from(8),
                quote_id: "quote-melt-0001".to_string(),
            }],
        )
        .unwrap();
        assert_eq!(
            hex::encode(tx.transcript()),
            "11008e01000108020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d70103003090f53b3088936b80ecfcc065a0746c5c1843ffd4527dbe3b9aba5a6109aed338ddc0f109511e9dfd37342aa99dd816d8040030a33dd4de26ea950060ae5c45bdc408d72aaf51ba9f4ccdeee4a040df067ca87c799a406fec88c2a328b75b290e27f7b62200160100010802000f71756f74652d6d656c742d30303031"
        );
        assert_eq!(
            hex::encode(tx.digest()),
            "f2fd9eaf27d572e47d776bd09856f760b3bb18338a9b497bee18b5ebec804048"
        );
        assert_eq!(
            hex::encode(tx.input_id(0).unwrap()),
            "194bd20817e5806bbe198206e63938c8f0ad38a0d89af2179bb517e24caf1db2"
        );
        assert_eq!(
            hex::encode(tx.input_digest(0).unwrap()),
            "e5bde063cdeb9187aafc8e521a63c3463188836ff8e1c6575e60c5a045f620b6"
        );
        // The swap witness does not verify in the melt.
        assert_no_keypath_sig(
            SECRET0,
            tx.input_digest(0).unwrap(),
            "3c2ce5dc36aea01a9b9f44287ef12e67a7fb0e0113720c3a000b5942835426e6635b09c087dbd30bde77ddf03b43b87858b3d2bf29d4b3ca7895f73e4839d3c2",
        );
    }

    /// tests/00-tests.md "Melt with change"
    #[test]
    fn melt_with_change_vector() {
        // Blank outputs listed after the melt quote in the request still
        // precede it in the transcript.
        let tx = Transaction::new(
            vec![swap_input()],
            vec![
                TransactionOutput::MeltQuote {
                    amount: Amount::from(5 + 3),
                    quote_id: "quote-melt-0002".to_string(),
                },
                output(0, B1),
                output(0, B2),
            ],
        )
        .unwrap();
        assert_eq!(
            hex::encode(tx.transcript()),
            "11008e01000108020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d70103003090f53b3088936b80ecfcc065a0746c5c1843ffd4527dbe3b9aba5a6109aed338ddc0f109511e9dfd37342aa99dd816d8040030a33dd4de26ea950060ae5c45bdc408d72aaf51ba9f4ccdeee4a040df067ca87c799a406fec88c2a328b75b290e27f7b621004a010000020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d701030020567b4ba980dd0c51c80919633b384c0764b30e5250c4a8b8e3957c9f4208cdee21004a010000020021022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d7010300200081a8744c8f3309419a09b08f3ae7e8cdaaaaad64309b78f6fd91f61dfaf5022200160100010802000f71756f74652d6d656c742d30303032"
        );
        assert_eq!(
            hex::encode(tx.digest()),
            "6649c7b016115b0d7f94fecef9af59ed616a32bb3e9f0313222427d29d087b80"
        );
        assert_eq!(
            hex::encode(tx.input_digest(0).unwrap()),
            "b30a18aa22aabb380329dbf7e2c7915662b26a6f4b901f8799b068fb4ec2fc2f"
        );
        assert_keypath_sig(
            SECRET0,
            tx.input_digest(0).unwrap(),
            "8129d244fa8d370a70c511f934788f2327f35739b8a60dac4545f190882a72774a947323ef6399d3e5e16e8687810549c19dcaab054f700caf05cbcd2e9c457b",
        );
    }

    fn blinded(amount: u64, b: &str) -> BlindedMessage {
        serde_json::from_value(serde_json::json!({"amount": amount, "id": KEYSET, "B_": b}))
            .unwrap()
    }

    fn swap_proof() -> Proof {
        proof(
            8,
            SECRET0,
            "a33dd4de26ea950060ae5c45bdc408d72aaf51ba9f4ccdeee4a040df067ca87c799a406fec88c2a328b75b290e27f7b6",
        )
    }

    /// The request builders reproduce the vectors
    #[test]
    fn request_builders() {
        use crate::nuts::nut03::SwapRequest;
        use crate::nuts::nut04::MintRequest;
        use crate::nuts::nut05::MeltRequest;
        use crate::nuts::nut29::BatchMintRequest;

        let swap = SwapRequest::new(vec![swap_proof()], vec![blinded(4, B1), blinded(4, B2)]);
        assert_eq!(
            hex::encode(swap.transaction().unwrap().digest()),
            "e88d16feadb0abc0cf4a08a7f5929a3a7c5e595f8d4e659d9ba66086aead402e"
        );

        let melt = MeltRequest::new(
            "quote-melt-0002".to_string(),
            vec![swap_proof()],
            Some(vec![blinded(0, B1), blinded(0, B2)]),
        );
        assert_eq!(
            hex::encode(melt.transaction(Amount::from(8)).unwrap().digest()),
            "6649c7b016115b0d7f94fecef9af59ed616a32bb3e9f0313222427d29d087b80"
        );
        let melt = MeltRequest::new("quote-melt-0001".to_string(), vec![swap_proof()], None);
        assert_eq!(
            hex::encode(melt.transaction(Amount::from(8)).unwrap().digest()),
            "f2fd9eaf27d572e47d776bd09856f760b3bb18338a9b497bee18b5ebec804048"
        );

        let state = |lock: &str, issued: u64| MintQuoteState {
            lock_key: PublicKey::from_hex(lock).unwrap(),
            issued: Amount::from(issued),
        };
        let mint = MintRequest {
            quote: "quote-mint-0004".to_string(),
            outputs: vec![blinded(4, B1)],
            signature: None,
        };
        assert_eq!(
            hex::encode(mint.transaction(&state(LOCK0, 3)).unwrap().digest()),
            "90d943e1f02938e5d50a7ead9f04ec1a633bc207bf7976f54569632db62d8d4d"
        );

        let mut batch = BatchMintRequest {
            quotes: vec!["quote-mint-0002".to_string(), "quote-mint-0003".to_string()],
            quote_amounts: None,
            outputs: vec![blinded(8, B1)],
            signatures: None,
        };
        let states = [state(LOCK0, 0), state(LOCK1, 0)];
        assert!(matches!(
            batch.transaction(&states),
            Err(Error::MissingQuoteAmounts)
        ));
        batch.quote_amounts = Some(vec![Amount::from(5), Amount::from(3)]);
        assert_eq!(
            hex::encode(batch.transaction(&states).unwrap().digest()),
            "8203006b339dd16a3f6293c0f300bebb3c9ad620755477b1372c0fa479540c72"
        );
        assert!(matches!(
            batch.transaction(&states[..1]),
            Err(Error::QuoteCountMismatch {
                quotes: 2,
                states: 1
            })
        ));
    }

    #[test]
    fn ys_must_match_proofs() {
        assert!(matches!(
            proof_inputs_with_ys(&[swap_proof()], &[]),
            Err(Error::YCountMismatch { proofs: 1, ys: 0 })
        ));
        let y = crate::dhke::hash_secret_to_curve(
            &swap_proof().secret,
            crate::nuts::nut02::KeySetVersion::Version02,
        )
        .unwrap()
        .into();
        assert_eq!(
            proof_inputs_with_ys(&[swap_proof()], &[y]).unwrap(),
            proof_inputs(&[swap_proof()]).unwrap()
        );
    }
}

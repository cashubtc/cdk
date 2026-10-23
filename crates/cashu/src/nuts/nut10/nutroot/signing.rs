use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use bitcoin::base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bitcoin::base64::Engine;
use bitcoin::secp256k1::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

use super::{receiver_key, Error, Leaf, Quote, Transaction, Witness};
use crate::nuts::{BlindedMessage, Proof};
use crate::util::hex;

/// One script opening awaiting signatures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningSpend {
    /// Index of the input's container in the package transcript.
    pub input: usize,
    /// The point the input spends: a v3 proof's secret, or a mint quote's lock key.
    pub secret: String,
    /// Script opening and collected signatures.
    #[serde(flatten)]
    pub witness: Witness,
    /// Sender's ephemeral key for receiver-key derivation.
    #[serde(rename = "E", default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_key: Option<PublicKey>,
    /// Optional slot hints; a mismatch falls back to scanning all script slots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slots: Option<Vec<u8>>,
}

/// A script-path signing exchange. Always inspect the transaction before signing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningPackage {
    version: String,
    /// The transaction's TLV transcript, hex.
    transcript: String,
    spends: Vec<SigningSpend>,
}

// Results are scoped to one operation, never retained across package mutations.
struct ValidatedSpend {
    index: usize,
    digest: [u8; 32],
    leaf: Leaf,
    signers: Vec<usize>,
}

impl SigningPackage {
    /// Build a package for a transaction whose quote amounts come from trusted quote state.
    pub fn new(transaction: &Transaction, spends: Vec<SigningSpend>) -> Result<Self, Error> {
        let package = Self {
            version: "nutspA".to_owned(),
            transcript: hex::encode(transaction.as_bytes()),
            spends,
        };
        package.validate()?;
        Ok(package)
    }

    /// Script openings and collected signatures.
    pub fn spends(&self) -> &[SigningSpend] {
        &self.spends
    }

    /// Decode the transaction for review; its digest is always recomputed from the transcript.
    pub fn transaction(&self) -> Result<Transaction, Error> {
        if self.version != "nutspA" {
            return Err(Error::InvalidTransaction);
        }
        let bytes = hex::decode(&self.transcript).map_err(|_| Error::InvalidTransaction)?;
        Transaction::from_bytes(&bytes)
    }

    /// Validate all openings and every collected signature, allowing incomplete thresholds.
    pub fn validate(&self) -> Result<(), Error> {
        self.validated_spends().map(|_| ())
    }

    fn validated_spends(&self) -> Result<(Transaction, Vec<ValidatedSpend>), Error> {
        let transaction = self.transaction()?;
        if self.spends.is_empty() {
            return Err(Error::InvalidWitness);
        }
        let mut seen = HashSet::new();
        let mut validated = Vec::with_capacity(self.spends.len());
        for spend in &self.spends {
            let index = spend.input;
            // The builder keeps hashlock preimages until `apply` (XX.md).
            if spend.witness.preimage.is_some()
                || !seen.insert(index)
                || !transaction.input_names_secret(index, &spend.secret)?
            {
                return Err(Error::InvalidWitness);
            }
            let leaf = spend.witness.committed_leaf(&spend.secret)?;
            if spend
                .slots
                .as_ref()
                .is_some_and(|slots| slots.len() != leaf.keys().len())
            {
                return Err(Error::InvalidWitness);
            }
            let digest = transaction.input_digest(index)?;
            let signers = spend.witness.partial_signers_for_leaf(&leaf, digest)?;
            validated.push(ValidatedSpend {
                index,
                digest,
                leaf,
                signers,
            });
        }
        Ok((transaction, validated))
    }

    /// Append signatures from matching keys. Incorrect slot hints never select
    /// an unrelated key. Caller must approve the transaction before this call.
    pub fn sign(&mut self, keys: &[SecretKey]) -> Result<usize, Error> {
        let (_, validated) = self.validated_spends()?;
        let mut added = 0;
        for (spend, validated) in self.spends.iter_mut().zip(validated) {
            for (key_index, public) in validated.leaf.keys().iter().enumerate() {
                if validated.signers.contains(&key_index) {
                    continue;
                }
                let mut found = keys
                    .iter()
                    .find(|key| PublicKey::from_secret_key(&crate::SECP256K1, key) == *public)
                    .copied();
                if found.is_none() {
                    if let Some(ephemeral) = spend.ephemeral_key {
                        'search: for key in keys {
                            let hint = spend
                                .slots
                                .as_ref()
                                .map(|slots| slots[key_index])
                                .filter(|slot| *slot != 0);
                            for slot in hint.into_iter().chain(1..=255) {
                                let derived = receiver_key(key, &ephemeral, slot)?;
                                if PublicKey::from_secret_key(&crate::SECP256K1, &derived)
                                    == *public
                                {
                                    found = Some(derived);
                                    break 'search;
                                }
                            }
                        }
                    }
                }
                if let Some(key) = found {
                    spend
                        .witness
                        .signatures
                        .extend(Witness::key_path(&key, validated.digest).signatures);
                    added += 1;
                }
            }
        }
        Ok(added)
    }

    /// Merge signatures only when transaction and script-opening metadata match.
    /// Keeps one valid signature per distinct leaf key. Changes are atomic.
    pub fn merge(&mut self, other: &Self) -> Result<(), Error> {
        let (_, validated) = self.validated_spends()?;
        let (_, other_validated) = other.validated_spends()?;
        let mut left = self.clone();
        let mut right = other.clone();
        for spend in &mut left.spends {
            spend.witness.signatures.clear();
        }
        for spend in &mut right.spends {
            spend.witness.signatures.clear();
        }
        if left != right {
            return Err(Error::InvalidTransaction);
        }
        let mut merged = self.clone();
        for ((target, source), (validated, other_validated)) in merged
            .spends
            .iter_mut()
            .zip(&other.spends)
            .zip(validated.into_iter().zip(other_validated))
        {
            let mut signers = validated.signers;
            let source_signers = other_validated.signers;
            for (signature, signer) in source.witness.signatures.iter().zip(source_signers) {
                if !signers.contains(&signer) {
                    target.witness.signatures.push(signature.clone());
                    signers.push(signer);
                }
            }
        }
        merged.validate()?;
        *self = merged;
        Ok(())
    }

    /// Apply complete witnesses to the original transaction's proof inputs. No input
    /// is changed unless every supplied script opening satisfies its policy.
    ///
    /// `preimages` maps a hashlock spend's `secret` to its preimage hex.
    pub fn apply(
        &self,
        inputs: &mut [Proof],
        outputs: &[BlindedMessage],
        quote: Option<Quote>,
        preimages: &HashMap<String, String>,
        now: u64,
    ) -> Result<(), Error> {
        let (expected, validated) = self.validated_spends()?;
        let quotes: Vec<_> = quote.into_iter().collect();
        let transaction = Transaction::new(inputs, &[], outputs, &quotes)?;
        if transaction != expected {
            return Err(Error::InvalidTransaction);
        }
        let mut witnesses = vec![];
        for (spend, validated) in self.spends.iter().zip(validated) {
            let preimage = preimages.get(&spend.secret).map(String::as_str);
            let raw = complete_witness(spend, validated.digest, preimage, now)?;
            witnesses.push((validated.index, crate::nuts::Witness::NutrootWitness(raw)));
        }
        for (index, witness) in witnesses {
            inputs[index].witness = Some(witness);
        }
        Ok(())
    }

    /// The complete witness JSON for the spend naming `input`, eg a quote input's mint request `signature`.
    ///
    /// Fails unless `transaction` is the package's and the opening satisfies its policy.
    pub fn witness(
        &self,
        transaction: &Transaction,
        input: usize,
        preimage: Option<&str>,
        now: u64,
    ) -> Result<String, Error> {
        let (expected, validated) = self.validated_spends()?;
        if *transaction != expected {
            return Err(Error::InvalidTransaction);
        }
        let (spend, validated) = self
            .spends
            .iter()
            .zip(validated)
            .find(|(spend, _)| spend.input == input)
            .ok_or(Error::InvalidWitness)?;
        complete_witness(spend, validated.digest, preimage, now)
    }

    /// Encode a validated signing package as `nutspA` followed by base64url JSON.
    pub fn encode(&self) -> Result<String, Error> {
        self.validate()?;
        encode_transport("nutspA", self)
    }
}

fn complete_witness(
    spend: &SigningSpend,
    digest: [u8; 32],
    preimage: Option<&str>,
    now: u64,
) -> Result<String, Error> {
    let mut witness = spend.witness.clone();
    witness.preimage = preimage.map(str::to_owned);
    witness.verify(&spend.secret, digest, now)?;
    serde_json::to_string(&witness).map_err(|_| Error::InvalidWitness)
}

impl FromStr for SigningPackage {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let package: Self = decode_transport("nutspA", value)?;
        package.validate()?;
        Ok(package)
    }
}

fn encode_transport<T: Serialize>(prefix: &str, value: &T) -> Result<String, Error> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::InvalidTransaction)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(Error::InvalidTransaction);
    }
    Ok(format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn decode_transport<T: serde::de::DeserializeOwned>(prefix: &str, value: &str) -> Result<T, Error> {
    if value.len() > 24 * 1024 * 1024 {
        return Err(Error::InvalidTransaction);
    }
    let data = value
        .strip_prefix(prefix)
        .ok_or(Error::InvalidTransaction)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(data)
        .map_err(|_| Error::InvalidTransaction)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::InvalidTransaction)
}

use std::collections::HashSet;
use std::str::FromStr;

use bitcoin::secp256k1::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

use super::receipt::{decode_transport, encode_transport};
use super::{receiver_key, Error, Leaf, Quote, Transaction, Witness};
use crate::nuts::{BlindedMessage, Id, KeySetVersion, Proof};
use crate::secret::Secret;
use crate::Amount;

/// A transaction input without private transfer information or unrelated witnesses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningInput {
    /// Committed amount.
    pub amount: Amount,
    /// Full keyset identifier.
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// Proof secret.
    pub secret: Secret,
    /// Mint signature.
    #[serde(rename = "C")]
    pub c: crate::nuts::PublicKey,
}

impl From<&Proof> for SigningInput {
    fn from(proof: &Proof) -> Self {
        Self {
            amount: proof.amount,
            keyset_id: proof.keyset_id,
            secret: proof.secret.clone(),
            c: proof.c,
        }
    }
}

impl SigningInput {
    fn proof(&self) -> Proof {
        Proof {
            amount: self.amount,
            keyset_id: self.keyset_id,
            secret: self.secret.clone(),
            c: self.c,
            witness: None,
            dleq: None,
            p2pk_e: None,
            spend_info: None,
        }
    }
}

/// One script opening awaiting signatures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningSpend {
    /// Secret of the input to sign.
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

/// Transaction type carried by a signing package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SigningKind {
    /// Swap proof inputs for blinded outputs.
    Swap,
    /// Spend proof inputs against a melt quote and optional change outputs.
    Melt,
}

/// A script-path signing exchange. Always inspect the transaction before signing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningPackage {
    version: String,
    #[serde(rename = "type")]
    kind: SigningKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quote: Option<String>,
    #[serde(
        rename = "quoteAmount",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    quote_amount: Option<Amount>,
    inputs: Vec<SigningInput>,
    outputs: Vec<BlindedMessage>,
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
    /// Build a package. A quote selects melt semantics; its amount includes the
    /// selected fee reserve and must be checked against trusted quote state.
    pub fn new(
        inputs: &[Proof],
        outputs: &[BlindedMessage],
        quote: Option<Quote>,
        spends: Vec<SigningSpend>,
    ) -> Result<Self, Error> {
        let (kind, quote, quote_amount) = match quote {
            Some(quote) => (SigningKind::Melt, Some(quote.id), Some(quote.amount)),
            None => (SigningKind::Swap, None, None),
        };
        let package = Self {
            version: "nutspA".to_owned(),
            kind,
            quote,
            quote_amount,
            inputs: inputs.iter().map(SigningInput::from).collect(),
            outputs: outputs
                .iter()
                .map(|o| BlindedMessage::new(o.amount, o.keyset_id, o.blinded_secret))
                .collect(),
            spends,
        };
        package.validate()?;
        Ok(package)
    }

    /// Inputs in committed request order.
    pub fn inputs(&self) -> &[SigningInput] {
        &self.inputs
    }
    /// Outputs in committed request order.
    pub fn outputs(&self) -> &[BlindedMessage] {
        &self.outputs
    }
    /// Script openings and collected signatures.
    pub fn spends(&self) -> &[SigningSpend] {
        &self.spends
    }
    /// Transaction type.
    pub fn kind(&self) -> SigningKind {
        self.kind
    }
    /// Melt quote identity and amount including selected fee reserve.
    pub fn quote(&self) -> Option<Quote> {
        self.quote
            .as_ref()
            .zip(self.quote_amount)
            .map(|(id, amount)| Quote {
                id: id.clone(),
                amount,
            })
    }
    /// Rebuild the transaction, never trusting a supplied digest.
    pub fn transaction(&self) -> Result<Transaction, Error> {
        if self.version != "nutspA" || self.outputs.iter().any(|o| o.witness.is_some()) {
            return Err(Error::InvalidTransaction);
        }
        let quotes = match (self.kind, &self.quote, self.quote_amount) {
            (SigningKind::Swap, None, None) => vec![],
            (SigningKind::Melt, Some(id), Some(amount)) => vec![Quote {
                id: id.clone(),
                amount,
            }],
            _ => return Err(Error::InvalidTransaction),
        };
        Transaction::new(
            &self
                .inputs
                .iter()
                .map(SigningInput::proof)
                .collect::<Vec<_>>(),
            &[],
            &self.outputs,
            &quotes,
        )
    }

    /// Validate all openings and every collected signature, allowing incomplete thresholds.
    pub fn validate(&self) -> Result<(), Error> {
        self.validated_spends().map(|_| ())
    }

    fn validated_spends(&self) -> Result<(Transaction, Vec<ValidatedSpend>), Error> {
        let transaction = self.transaction()?;
        if self.spends.is_empty() || self.spends.len() > self.inputs.len() {
            return Err(Error::InvalidWitness);
        }
        let mut seen = HashSet::new();
        let mut validated = Vec::with_capacity(self.spends.len());
        for spend in &self.spends {
            if !seen.insert(&spend.secret) {
                return Err(Error::InvalidWitness);
            }
            let index = self.input_index(&spend.secret)?;
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

    fn input_index(&self, secret: &str) -> Result<usize, Error> {
        let mut matches = self
            .inputs
            .iter()
            .enumerate()
            .filter(|(_, input)| input.secret.to_string() == secret);
        let (index, input) = matches.next().ok_or(Error::InvalidWitness)?;
        if matches.next().is_some() || input.keyset_id.get_version() != KeySetVersion::Version02 {
            return Err(Error::InvalidWitness);
        }
        Ok(index)
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

    /// Apply complete witnesses to the original transaction's inputs. No input
    /// is changed unless every supplied script opening satisfies its policy.
    pub fn apply(
        &self,
        inputs: &mut [Proof],
        outputs: &[BlindedMessage],
        quote: Option<Quote>,
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
            let index = validated.index;
            spend.witness.verify(&spend.secret, validated.digest, now)?;
            let raw = serde_json::to_string(&spend.witness).map_err(|_| Error::InvalidWitness)?;
            witnesses.push((index, crate::nuts::Witness::NutrootWitness(raw)));
        }
        for (index, witness) in witnesses {
            inputs[index].witness = Some(witness);
        }
        Ok(())
    }

    /// Encode a validated signing package as `nutspA` followed by base64url JSON.
    pub fn encode(&self) -> Result<String, Error> {
        self.validate()?;
        encode_transport("nutspA", self)
    }
}

impl FromStr for SigningPackage {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let package: Self = decode_transport("nutspA", value)?;
        package.validate()?;
        Ok(package)
    }
}

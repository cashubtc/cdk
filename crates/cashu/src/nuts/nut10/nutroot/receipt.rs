use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use bitcoin::base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bitcoin::base64::Engine;
use serde::{Deserialize, Serialize};

use super::{spend_commitment, Error, SpendRecord, Transaction, Witness};
use crate::mint_url::MintUrl;
use crate::nuts::{CurrencyUnit, Id, KeySetVersion, Proof, ProofState, PublicKey, State, Token};
use crate::util::hex;
use crate::KeySetInfo;

/// Opening of one mint spend commitment. Contains private transaction information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptOpening {
    /// Hash-to-curve point identifying the spent proof.
    #[serde(rename = "Y")]
    pub y: PublicKey,
    /// Full keyset identifier.
    pub keyset_id: Id,
    /// Claimed per-input digest; verification recomputes it from the transcript.
    pub input_digest: String,
    /// Exact witness bytes accepted by the mint.
    pub witness: String,
    /// Claimed commitment; verification compares it to trusted mint state.
    pub commitment: String,
    /// Canonical TLV transaction transcript in hex.
    pub transcript: String,
}

/// Payer-controlled evidence of a spend, encoded with the `nutrcA` prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendReceipt {
    /// Spent proofs, without transfer keys or witnesses.
    pub token: String,
    /// One opening for each version-02 proof in the token.
    pub receipts: Vec<ReceiptOpening>,
}

impl SpendReceipt {
    /// Build evidence before sending a signed transaction. It becomes a receipt
    /// only after verification against the mint's spent commitments succeeds.
    pub fn new(
        mint: MintUrl,
        unit: CurrencyUnit,
        proofs: &[Proof],
        transaction: &Transaction,
        now: u64,
    ) -> Result<Self, Error> {
        let mut receipts = vec![];
        for (index, proof) in proofs.iter().enumerate() {
            if proof.keyset_id.get_version() != KeySetVersion::Version02 {
                continue;
            }
            let record = SpendRecord::new(proof, transaction, index, now)?;
            let y = proof.y().map_err(|_| Error::InvalidTransaction)?;
            receipts.push(ReceiptOpening {
                y,
                keyset_id: proof.keyset_id,
                input_digest: hex::encode(record.input_digest),
                commitment: hex::encode(spend_commitment(&y, record.input_digest, &record.witness)),
                witness: record.witness,
                transcript: hex::encode(transaction.as_bytes()),
            });
        }
        if receipts.is_empty() {
            return Err(Error::InvalidTransaction);
        }
        let mut proofs = proofs.to_vec();
        for proof in &mut proofs {
            proof.witness = None;
            proof.spend_info = None;
            proof.p2pk_e = None;
        }
        Ok(Self {
            token: Token::new(mint, proofs, None, unit).to_string(),
            receipts,
        })
    }

    /// Encode the transport without exposing private transfer keys.
    pub fn encode(&self) -> Result<String, Error> {
        encode_transport("nutrcA", self)
    }

    /// Verify every opening against states obtained independently from this
    /// token's mint. Callers must authenticate the mint and its keyset mapping.
    /// This checks spend evidence, not the underlying mint signatures.
    pub fn verify(
        &self,
        keysets: &[KeySetInfo],
        states: &[ProofState],
        now: u64,
    ) -> Result<(), Error> {
        let token = Token::from_str(&self.token).map_err(|_| Error::InvalidTransaction)?;
        let proofs = token
            .proofs(keysets)
            .map_err(|_| Error::InvalidTransaction)?;
        let proofs: Vec<_> = proofs
            .iter()
            .filter(|p| p.keyset_id.get_version() == KeySetVersion::Version02)
            .collect();
        if proofs.is_empty() || proofs.len() != self.receipts.len() {
            return Err(Error::InvalidTransaction);
        }
        let mut receipts = HashMap::with_capacity(self.receipts.len());
        for receipt in &self.receipts {
            if receipts.insert(receipt.y, receipt).is_some() {
                return Err(Error::InvalidTransaction);
            }
        }
        // Duplicate states for a requested proof are ambiguous. Unrelated states
        // remain irrelevant, including duplicates, as in the original lookup.
        let mut states_by_y = HashMap::with_capacity(states.len());
        for state in states {
            states_by_y
                .entry(state.y)
                .and_modify(|entry| *entry = None)
                .or_insert(Some(state));
        }
        let mut transactions = HashMap::new();
        let mut seen = HashSet::new();
        for proof in proofs {
            let y = proof.y().map_err(|_| Error::InvalidTransaction)?;
            if !seen.insert(y) {
                return Err(Error::InvalidTransaction);
            }
            let receipt = receipts.get(&y).ok_or(Error::InvalidTransaction)?;
            if receipt.keyset_id != proof.keyset_id || receipt.witness.len() > 4096 {
                return Err(Error::InvalidTransaction);
            }
            if receipt.transcript.len() > 8 * 1024 * 1024 {
                return Err(Error::InvalidTransaction);
            }
            let transaction = match transactions.entry(receipt.transcript.as_str()) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    entry.insert(Transaction::from_bytes(&hex::decode(&receipt.transcript)?)?)
                }
            };
            let digest = transaction.proof_digest(proof)?;
            let commitment = hex::encode(spend_commitment(&y, digest, &receipt.witness));
            if receipt.input_digest != hex::encode(digest) || receipt.commitment != commitment {
                return Err(Error::InvalidTransaction);
            }
            let witness: Witness =
                serde_json::from_str(&receipt.witness).map_err(|_| Error::InvalidWitness)?;
            witness.verify(&proof.secret.to_string(), digest, now)?;
            let state = states_by_y
                .get(&y)
                .copied()
                .flatten()
                .ok_or(Error::InvalidTransaction)?;
            if state.state != State::Spent || state.commitment.as_ref() != Some(&commitment) {
                return Err(Error::InvalidTransaction);
            }
        }
        Ok(())
    }
}

impl FromStr for SpendReceipt {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        decode_transport("nutrcA", value)
    }
}

pub(super) fn encode_transport<T: Serialize>(prefix: &str, value: &T) -> Result<String, Error> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::InvalidTransaction)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(Error::InvalidTransaction);
    }
    Ok(format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

pub(super) fn decode_transport<T: serde::de::DeserializeOwned>(
    prefix: &str,
    value: &str,
) -> Result<T, Error> {
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

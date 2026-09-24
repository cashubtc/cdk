use std::collections::HashSet;

use sha2::{Digest, Sha256};

use super::leaf::integer;
use super::{parse_secret, tagged_hash, Error};
use crate::nuts::{BlindedMessage, KeySetVersion, Proof};
use crate::Amount;

/// Quote identity and amount bound by a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    /// Mint quote face amount, or melt amount including selected fee reserve.
    pub amount: Amount,
    /// Quote identifier exactly as supplied by the mint.
    pub id: String,
}

/// Canonical transaction transcript and its input records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    transcript: Vec<u8>,
    inputs: Vec<Vec<u8>>,
}

impl Transaction {
    /// Construct a transcript, preserving request order within container types.
    /// Quote amounts must be resolved from trusted quote state by the caller.
    pub fn new(
        proofs: &[Proof],
        mint_quotes: &[Quote],
        outputs: &[BlindedMessage],
        melt_quotes: &[Quote],
    ) -> Result<Self, Error> {
        if (proofs.is_empty() && mint_quotes.is_empty())
            || (outputs.is_empty() && melt_quotes.is_empty())
        {
            return Err(Error::InvalidTransaction);
        }
        let mut result = Self {
            transcript: vec![],
            inputs: vec![],
        };
        let mut ys = HashSet::new();
        for proof in proofs {
            let y = match proof.keyset_id.get_version() {
                KeySetVersion::Version02 => {
                    let secret = parse_secret(&proof.secret.to_string())?;
                    crate::nuts::nut01::BlsG1PublicKey::hash_to_curve(&secret.serialize())
                        .to_bytes()
                        .to_vec()
                }
                _ => proof.y().map_err(|_| Error::InvalidTransaction)?.to_bytes(),
            };
            if !ys.insert(y.clone()) {
                return Err(Error::InvalidTransaction);
            }
            let fields = [
                integer(u64::from(proof.amount)),
                proof.keyset_id.to_bytes(),
                y,
                proof.c.to_bytes(),
            ];
            let record = container(1, &fields)?;
            result.transcript.extend_from_slice(&record);
            result.inputs.push(record);
        }
        let mut ids = HashSet::new();
        for quote in mint_quotes {
            if !ids.insert(&quote.id) {
                return Err(Error::InvalidTransaction);
            }
            let record = container(
                2,
                &[
                    integer(u64::from(quote.amount)),
                    quote.id.as_bytes().to_vec(),
                ],
            )?;
            result.transcript.extend_from_slice(&record);
            result.inputs.push(record);
        }
        for output in outputs {
            result.transcript.extend(container(
                3,
                &[
                    integer(u64::from(output.amount)),
                    output.keyset_id.to_bytes(),
                    output.blinded_secret.to_bytes(),
                ],
            )?);
        }
        for quote in melt_quotes {
            result.transcript.extend(container(
                4,
                &[
                    integer(u64::from(quote.amount)),
                    quote.id.as_bytes().to_vec(),
                ],
            )?);
        }
        Ok(result)
    }

    /// Canonical TLV transcript.
    pub fn as_bytes(&self) -> &[u8] {
        &self.transcript
    }
    /// Shared transaction digest, which is not itself a signing message.
    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(&self.transcript).into()
    }
    /// Input identity, hashing its complete outer container record.
    pub fn input_id(&self, index: usize) -> Result<[u8; 32], Error> {
        Ok(Sha256::digest(self.inputs.get(index).ok_or(Error::InvalidTransaction)?).into())
    }
    /// Signing message for a proof input, followed by quote inputs.
    pub fn input_digest(&self, index: usize) -> Result<[u8; 32], Error> {
        let mut message = [0; 64];
        message[..32].copy_from_slice(&self.digest());
        message[32..].copy_from_slice(&self.input_id(index)?);
        Ok(tagged_hash("Cashu_TransactionInput", &message))
    }
}

fn container(tag: u8, fields: &[Vec<u8>]) -> Result<Vec<u8>, Error> {
    let mut body = vec![];
    for (index, value) in fields.iter().enumerate() {
        record(&mut body, (index + 1) as u8, value)?;
    }
    let mut output = vec![];
    record(&mut output, tag, &body)?;
    Ok(output)
}

fn record(output: &mut Vec<u8>, tag: u8, value: &[u8]) -> Result<(), Error> {
    let length = u16::try_from(value.len()).map_err(|_| Error::InvalidTransaction)?;
    output.push(tag);
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
}

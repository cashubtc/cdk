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
    digest: [u8; 32],
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
        let mut transcript = vec![];
        let mut inputs = vec![];
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
            transcript.extend_from_slice(&record);
            inputs.push(record);
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
            transcript.extend_from_slice(&record);
            inputs.push(record);
        }
        for output in outputs {
            transcript.extend(container(
                3,
                &[
                    integer(u64::from(output.amount)),
                    output.keyset_id.to_bytes(),
                    output.blinded_secret.to_bytes(),
                ],
            )?);
        }
        for quote in melt_quotes {
            transcript.extend(container(
                4,
                &[
                    integer(u64::from(quote.amount)),
                    quote.id.as_bytes().to_vec(),
                ],
            )?);
        }
        Ok(Self::from_parts(transcript, inputs))
    }

    /// Parse a canonical transaction transcript for receipt verification.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(Error::InvalidTransaction);
        }
        let records = parse_records(bytes)?;
        let mut last = 0;
        let mut inputs = vec![];
        let mut ys = HashSet::new();
        let mut quotes = HashSet::new();
        let mut output = false;
        for (tag, value, full) in records {
            if tag < last || !(1..=4).contains(&tag) {
                return Err(Error::InvalidTransaction);
            }
            last = tag;
            let fields = parse_records(value)?;
            let count = match tag {
                1 => 4,
                2 | 4 => 2,
                _ => 3,
            };
            if fields.len() != count
                || fields
                    .iter()
                    .enumerate()
                    .any(|(i, (tag, _, _))| usize::from(*tag) != i + 1)
            {
                return Err(Error::InvalidTransaction);
            }
            let amount = fields[0].1;
            if amount.len() > 8 || amount.first() == Some(&0) {
                return Err(Error::InvalidTransaction);
            }
            match tag {
                1 | 3 => {
                    let id = crate::nuts::Id::from_bytes(fields[1].1)
                        .map_err(|_| Error::InvalidTransaction)?;
                    for (_, point, _) in &fields[2..] {
                        let point = crate::nuts::PublicKey::from_slice(point)
                            .map_err(|_| Error::InvalidTransaction)?;
                        let valid = match id.get_version() {
                            KeySetVersion::Version02 => {
                                matches!(point, crate::nuts::PublicKey::BlsG1(_))
                            }
                            _ => matches!(point, crate::nuts::PublicKey::Secp256k1(_)),
                        };
                        if !valid {
                            return Err(Error::InvalidTransaction);
                        }
                    }
                    if tag == 1 && !ys.insert(fields[2].1) {
                        return Err(Error::InvalidTransaction);
                    }
                }
                2 | 4 => {
                    std::str::from_utf8(fields[1].1).map_err(|_| Error::InvalidTransaction)?;
                    if tag == 2 && !quotes.insert(fields[1].1) {
                        return Err(Error::InvalidTransaction);
                    }
                }
                _ => return Err(Error::InvalidTransaction),
            }
            if tag <= 2 {
                inputs.push(full.to_vec());
            } else {
                output = true;
            }
        }
        if inputs.is_empty() || !output {
            return Err(Error::InvalidTransaction);
        }
        Ok(Self::from_parts(bytes.to_vec(), inputs))
    }

    fn from_parts(transcript: Vec<u8>, inputs: Vec<Vec<u8>>) -> Self {
        let digest = Sha256::digest(&transcript).into();
        Self {
            transcript,
            inputs,
            digest,
        }
    }

    /// Find the digest of a held proof's exact input record in this transcript.
    pub fn proof_digest(&self, proof: &Proof) -> Result<[u8; 32], Error> {
        let record = container(
            1,
            &[
                integer(u64::from(proof.amount)),
                proof.keyset_id.to_bytes(),
                proof.y().map_err(|_| Error::InvalidTransaction)?.to_bytes(),
                proof.c.to_bytes(),
            ],
        )?;
        let index = self
            .inputs
            .iter()
            .position(|input| *input == record)
            .ok_or(Error::InvalidTransaction)?;
        self.input_digest(index)
    }

    /// Canonical TLV transcript.
    pub fn as_bytes(&self) -> &[u8] {
        &self.transcript
    }
    /// Shared transaction digest, which is not itself a signing message.
    pub fn digest(&self) -> [u8; 32] {
        self.digest
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

type Record<'a> = (u8, &'a [u8], &'a [u8]);
fn parse_records(mut bytes: &[u8]) -> Result<Vec<Record<'_>>, Error> {
    let mut records = vec![];
    while !bytes.is_empty() {
        if bytes.len() < 3 {
            return Err(Error::InvalidTransaction);
        }
        let len = usize::from(u16::from_be_bytes([bytes[1], bytes[2]]));
        if bytes.len() < 3 + len {
            return Err(Error::InvalidTransaction);
        }
        records.push((bytes[0], &bytes[3..3 + len], &bytes[..3 + len]));
        bytes = &bytes[3 + len..];
    }
    Ok(records)
}

/// NUT-22 message binding a BAT to the exact method, origin-form target and body.
pub fn authorized_request_digest(
    method: &str,
    target: &str,
    body: &[u8],
) -> Result<[u8; 32], Error> {
    if method.is_empty()
        || !method.bytes().all(|b| b.is_ascii_uppercase())
        || !target.starts_with('/')
        || target.contains('#')
    {
        return Err(Error::InvalidTransaction);
    }
    let bytes = container(
        5,
        &[
            method.as_bytes().to_vec(),
            target.as_bytes().to_vec(),
            Sha256::digest(body).to_vec(),
        ],
    )?;
    Ok(tagged_hash(
        "Cashu_AuthorizedRequest",
        &Sha256::digest(&bytes),
    ))
}

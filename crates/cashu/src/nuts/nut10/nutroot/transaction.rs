use std::collections::HashSet;

use sha2::{Digest, Sha256};

use super::leaf::integer;
use super::{parse_secret, tagged_hash, Error};
use crate::nuts::{BlindedMessage, KeySetVersion, Proof, PublicKey};
use crate::util::hex;
use crate::Amount;

// Container types: the high nibble is the section, 0x1n inputs, 0x2n outputs, 0xFn never in a
// transaction, so ascending order keeps inputs ahead of outputs (NUT-10).
const PROOF_INPUT: u8 = 0x11;
const MINT_QUOTE_INPUT: u8 = 0x12;
const BLINDED_OUTPUT: u8 = 0x21;
const MELT_QUOTE_OUTPUT: u8 = 0x22;
const CHANGE_QUOTE_OUTPUT: u8 = 0x23;
const AUTHORIZED_REQUEST: u8 = 0xf1;

/// Quote identity and amount bound by a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    /// Amount a mint quote issues in this transaction, or melt amount including
    /// the selected fee reserve.
    pub amount: Amount,
    /// Quote identifier exactly as supplied by the mint.
    pub id: String,
}

/// A mint quote input: the amount issued, the quote id and the key the quote is locked to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintQuoteInput {
    /// Amount this transaction issues against the quote.
    pub amount: Amount,
    /// Quote identifier exactly as supplied by the mint.
    pub id: String,
    /// The quote's lock key (NUT-04 `pubkey`), committed so a signer can tell which key the input needs.
    pub pubkey: PublicKey,
}

/// Change quote output bound by a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeOutput {
    /// Fixed amount, or `None` for the remainder quote that takes the balance.
    pub amount: Option<Amount>,
    /// Lock key of the change quote.
    pub pubkey: bitcoin::secp256k1::PublicKey,
}

impl ChangeOutput {
    /// The id the mint gives this change quote: derived from its lock key, so one key names one quote.
    pub fn quote_id(&self) -> String {
        hex::encode(tagged_hash("Cashu_QuoteId", &self.pubkey.serialize()))
    }
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
        mint_quotes: &[MintQuoteInput],
        outputs: &[BlindedMessage],
        melt_quotes: &[Quote],
    ) -> Result<Self, Error> {
        Self::with_change(proofs, mint_quotes, outputs, melt_quotes, &[])
    }

    /// [`Transaction::new`] with change quote outputs, at most one of them the remainder.
    pub fn with_change(
        proofs: &[Proof],
        mint_quotes: &[MintQuoteInput],
        outputs: &[BlindedMessage],
        melt_quotes: &[Quote],
        change: &[ChangeOutput],
    ) -> Result<Self, Error> {
        if (proofs.is_empty() && mint_quotes.is_empty())
            || (outputs.is_empty() && melt_quotes.is_empty() && change.is_empty())
            || change.iter().filter(|c| c.amount.is_none()).count() > 1
            || change.iter().any(|c| c.amount == Some(Amount::ZERO))
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
            let record = container(PROOF_INPUT, &fields)?;
            transcript.extend_from_slice(&record);
            inputs.push(record);
        }
        let mut ids = HashSet::new();
        for quote in mint_quotes {
            if !ids.insert(&quote.id) || quote.pubkey.as_secp256k1().is_err() {
                return Err(Error::InvalidTransaction);
            }
            let record = container(
                MINT_QUOTE_INPUT,
                &[
                    integer(u64::from(quote.amount)),
                    quote.id.as_bytes().to_vec(),
                    quote.pubkey.to_bytes(),
                ],
            )?;
            transcript.extend_from_slice(&record);
            inputs.push(record);
        }
        for output in outputs {
            transcript.extend(container(
                BLINDED_OUTPUT,
                &[
                    integer(u64::from(output.amount)),
                    output.keyset_id.to_bytes(),
                    output.blinded_secret.to_bytes(),
                ],
            )?);
        }
        for quote in melt_quotes {
            transcript.extend(container(
                MELT_QUOTE_OUTPUT,
                &[
                    integer(u64::from(quote.amount)),
                    quote.id.as_bytes().to_vec(),
                ],
            )?);
        }
        for output in change {
            // The amount record is omitted entirely on the remainder quote.
            let mut body = vec![];
            if let Some(amount) = output.amount {
                record(&mut body, 1, &integer(u64::from(amount)))?;
            }
            record(&mut body, 2, &output.pubkey.serialize())?;
            record(&mut transcript, CHANGE_QUOTE_OUTPUT, &body)?;
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
        let mut remainder = false;
        for (tag, value, full) in records {
            if tag < last || !matches!(tag >> 4, 1 | 2) {
                return Err(Error::InvalidTransaction);
            }
            last = tag;
            let fields = parse_records(value)?;
            if tag == CHANGE_QUOTE_OUTPUT {
                // `01 amount, 02 lock key`, or `02 lock key` alone on the remainder quote.
                let key = match fields.as_slice() {
                    [(2, key, _)] if !remainder => {
                        remainder = true;
                        key
                    }
                    [(1, amount, _), (2, key, _)]
                        if !amount.is_empty() && amount.len() <= 8 && amount[0] != 0 =>
                    {
                        key
                    }
                    _ => return Err(Error::InvalidTransaction),
                };
                let key = crate::nuts::PublicKey::from_slice(key)
                    .map_err(|_| Error::InvalidTransaction)?;
                if !matches!(key, crate::nuts::PublicKey::Secp256k1(_)) {
                    return Err(Error::InvalidTransaction);
                }
                output = true;
                continue;
            }
            let count = match tag {
                PROOF_INPUT => 4,
                MELT_QUOTE_OUTPUT => 2,
                MINT_QUOTE_INPUT | BLINDED_OUTPUT => 3,
                _ => return Err(Error::InvalidTransaction),
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
                PROOF_INPUT | BLINDED_OUTPUT => {
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
                    if tag == PROOF_INPUT && !ys.insert(fields[2].1) {
                        return Err(Error::InvalidTransaction);
                    }
                }
                MINT_QUOTE_INPUT | MELT_QUOTE_OUTPUT => {
                    std::str::from_utf8(fields[1].1).map_err(|_| Error::InvalidTransaction)?;
                    if tag == MINT_QUOTE_INPUT
                        && (!quotes.insert(fields[1].1)
                            || PublicKey::from_slice(fields[2].1)
                                .map_err(|_| Error::InvalidTransaction)?
                                .as_secp256k1()
                                .is_err())
                    {
                        return Err(Error::InvalidTransaction);
                    }
                }
                _ => return Err(Error::InvalidTransaction),
            }
            if tag >> 4 == 1 {
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
            PROOF_INPUT,
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
    /// The output section: every `0x2n` container, the bytes a template leaf hashes.
    pub fn outputs(&self) -> &[u8] {
        let inputs: usize = self.inputs.iter().map(Vec::len).sum();
        &self.transcript[inputs..]
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
        AUTHORIZED_REQUEST,
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

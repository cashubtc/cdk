use std::collections::HashSet;

use sha2::{Digest, Sha256};

use super::leaf::integer;
use super::{parse_secret, tagged_hash, Error};
use crate::nuts::{BlindedMessage, KeySetVersion, Proof, PublicKey};
use crate::Amount;

// Container types: the high nibble is the section, 0x1n inputs, 0x2n outputs, 0xFn never in a
// transaction, so ascending order keeps inputs ahead of outputs (NUT-10).
const PROOF_INPUT: u8 = 0x11;
const MINT_QUOTE_INPUT: u8 = 0x12;
const BLINDED_OUTPUT: u8 = 0x21;
const MELT_QUOTE_OUTPUT: u8 = 0x22;
const MINT_QUOTE_OUTPUT: u8 = 0x23;
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

/// A mint quote input: the amount issued, the quote id, the key the quote is locked to, and the
/// quote's `amount_issued` before this transaction, so a witness authorizes one issuance (NUT-00).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintQuoteInput {
    /// Amount this transaction issues against the quote.
    pub amount: Amount,
    /// Quote identifier exactly as supplied by the mint.
    pub id: String,
    /// The quote's lock key (NUT-04 `pubkey`), committed so a signer can tell which key the input needs.
    pub pubkey: PublicKey,
    /// The quote's settled `amount_issued` before this transaction.
    pub issued: Amount,
}

/// Mint quote output bound by a transaction: a fixed txout quote (NUT-XX).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintQuoteOutput {
    /// Fixed amount.
    pub amount: Amount,
    /// Lock key of the txout quote.
    pub pubkey: bitcoin::secp256k1::PublicKey,
    /// The txout quote's id: [`super::derived_quote_id`] or [`super::input_bound_quote_id`].
    pub id: String,
}

/// One element of a transaction, decoded from its transcript for review before signing (NUT-00).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionElement {
    /// A proof input; the transcript names it by `Y`, never by its secret.
    ProofInput {
        /// Proof amount.
        amount: Amount,
        /// Keyset the proof was signed under.
        keyset_id: crate::nuts::Id,
        /// The proof's hash-to-curve point.
        y: PublicKey,
    },
    /// A mint quote input.
    MintQuoteInput(MintQuoteInput),
    /// A blinded message output, committed by `SHA256(B_)`.
    BlindedOutput {
        /// Output amount.
        amount: Amount,
        /// Keyset the output asks to be signed under.
        keyset_id: crate::nuts::Id,
        /// `SHA256` of the blinded message's compressed point.
        b_hash: [u8; 32],
    },
    /// A melt quote output: the melt amount including the selected fee reserve.
    MeltQuoteOutput(Quote),
    /// A fixed txout quote output (NUT-XX).
    MintQuoteOutput(MintQuoteOutput),
}

/// Canonical transaction transcript and its input records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    transcript: Vec<u8>,
    inputs: Vec<Vec<u8>>,
    digest: [u8; 32],
    txout_keys: Option<Vec<bitcoin::secp256k1::PublicKey>>,
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
        Self::with_quote_outputs(proofs, mint_quotes, outputs, melt_quotes, &[])
    }

    /// [`Transaction::new`] with fixed txout quote outputs (NUT-XX).
    pub fn with_quote_outputs(
        proofs: &[Proof],
        mint_quotes: &[MintQuoteInput],
        outputs: &[BlindedMessage],
        melt_quotes: &[Quote],
        quote_outputs: &[MintQuoteOutput],
    ) -> Result<Self, Error> {
        if (proofs.is_empty() && mint_quotes.is_empty())
            || (outputs.is_empty() && melt_quotes.is_empty() && quote_outputs.is_empty())
            || quote_outputs.iter().any(|c| c.amount == Amount::ZERO)
        {
            return Err(Error::InvalidTransaction);
        }
        let inputs = input_records(proofs, mint_quotes)?;
        let mut transcript = inputs.concat();
        for output in outputs {
            transcript.extend(container(
                BLINDED_OUTPUT,
                &[
                    integer(u64::from(output.amount)),
                    output.keyset_id.to_bytes(),
                    Sha256::digest(output.blinded_secret.to_bytes()).to_vec(),
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
        for output in quote_outputs {
            transcript.extend(container(
                MINT_QUOTE_OUTPUT,
                &[
                    integer(u64::from(output.amount)),
                    output.pubkey.serialize().to_vec(),
                    output.id.as_bytes().to_vec(),
                ],
            )?);
        }
        let txout_keys = (outputs.is_empty() && melt_quotes.is_empty())
            .then(|| quote_outputs.iter().map(|output| output.pubkey).collect());
        Ok(Self::from_parts(transcript, inputs, txout_keys))
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
        let mut quote_output_ids = HashSet::new();
        let mut txout_keys = Some(vec![]);
        for (tag, value, full) in records {
            if tag < last || !matches!(tag >> 4, 1 | 2) {
                return Err(Error::InvalidTransaction);
            }
            last = tag;
            let fields = parse_records(value)?;
            if tag == MINT_QUOTE_OUTPUT {
                // `01 amount, 02 lock key, 03 quote id`.
                let [(1, amount, _), (2, key, _), (3, id, _)] = fields.as_slice() else {
                    return Err(Error::InvalidTransaction);
                };
                if amount.is_empty() || amount.len() > 8 || amount[0] == 0 {
                    return Err(Error::InvalidTransaction);
                }
                let key = crate::nuts::PublicKey::from_slice(key)
                    .map_err(|_| Error::InvalidTransaction)?;
                // A derived id is 64 lowercase hex characters (NUT-04).
                let derived =
                    id.len() == 64 && id.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
                let crate::nuts::PublicKey::Secp256k1(key) = key else {
                    return Err(Error::InvalidTransaction);
                };
                if !derived || !quote_output_ids.insert(*id) {
                    return Err(Error::InvalidTransaction);
                }
                if let Some(keys) = txout_keys.as_mut() {
                    keys.push(key);
                }
                output = true;
                continue;
            }
            let count = match tag {
                PROOF_INPUT | MINT_QUOTE_INPUT => 4,
                MELT_QUOTE_OUTPUT => 2,
                BLINDED_OUTPUT => 3,
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
                BLINDED_OUTPUT => {
                    crate::nuts::Id::from_bytes(fields[1].1)
                        .map_err(|_| Error::InvalidTransaction)?;
                    if fields[2].1.len() != 32 {
                        return Err(Error::InvalidTransaction);
                    }
                }
                PROOF_INPUT => {
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
                    if !ys.insert(fields[2].1) {
                        return Err(Error::InvalidTransaction);
                    }
                }
                MINT_QUOTE_INPUT | MELT_QUOTE_OUTPUT => {
                    std::str::from_utf8(fields[1].1).map_err(|_| Error::InvalidTransaction)?;
                    if tag == MINT_QUOTE_INPUT
                        && (!quotes.insert(fields[1].1)
                            || fields[3].1.len() > 8
                            || fields[3].1.first() == Some(&0)
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
                txout_keys = None;
            }
        }
        if inputs.is_empty() || !output {
            return Err(Error::InvalidTransaction);
        }
        Ok(Self::from_parts(bytes.to_vec(), inputs, txout_keys))
    }

    fn from_parts(
        transcript: Vec<u8>,
        inputs: Vec<Vec<u8>>,
        txout_keys: Option<Vec<bitcoin::secp256k1::PublicKey>>,
    ) -> Self {
        let digest = Sha256::digest(&transcript).into();
        Self {
            transcript,
            inputs,
            digest,
            txout_keys,
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

    /// Whether `secret` is the point an input names: a v3 proof's secret, or a mint quote's lock key.
    pub(super) fn input_names_secret(&self, index: usize, secret: &str) -> Result<bool, Error> {
        let container = self.inputs.get(index).ok_or(Error::InvalidTransaction)?;
        let fields = parse_records(container.get(3..).ok_or(Error::InvalidTransaction)?)?;
        let field = fields.get(2).ok_or(Error::InvalidTransaction)?.1;
        Ok(match container[0] {
            PROOF_INPUT => {
                crate::dhke::hash_to_curve_for_version(secret.as_bytes(), KeySetVersion::Version02)
                    .is_ok_and(|y| y.to_bytes() == field)
            }
            MINT_QUOTE_INPUT => parse_secret(secret).is_ok_and(|key| key.serialize() == field),
            _ => false,
        })
    }

    /// Canonical TLV transcript.
    pub fn as_bytes(&self) -> &[u8] {
        &self.transcript
    }
    /// Lock keys of the fixed txout quote outputs, or `None` when any output is another type.
    pub fn txout_keys(&self) -> Option<&[bitcoin::secp256k1::PublicKey]> {
        self.txout_keys.as_deref()
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

    /// The transaction's inputs and outputs in transcript order, so a signer can review what it signs.
    pub fn elements(&self) -> Result<Vec<TransactionElement>, Error> {
        let amount = |bytes: &[u8]| -> Result<Amount, Error> {
            if bytes.len() > 8 {
                return Err(Error::InvalidTransaction);
            }
            let mut be = [0u8; 8];
            be[8 - bytes.len()..].copy_from_slice(bytes);
            Ok(Amount::from(u64::from_be_bytes(be)))
        };
        let id = |bytes: &[u8]| {
            crate::nuts::Id::from_bytes(bytes).map_err(|_| Error::InvalidTransaction)
        };
        let text = |bytes: &[u8]| {
            std::str::from_utf8(bytes)
                .map(str::to_owned)
                .map_err(|_| Error::InvalidTransaction)
        };
        parse_records(&self.transcript)?
            .into_iter()
            .map(|(tag, value, _)| {
                let fields: Vec<_> = parse_records(value)?
                    .into_iter()
                    .map(|(_, field, _)| field)
                    .collect();
                let field = |i: usize| fields.get(i).copied().ok_or(Error::InvalidTransaction);
                Ok(match tag {
                    PROOF_INPUT => TransactionElement::ProofInput {
                        amount: amount(field(0)?)?,
                        keyset_id: id(field(1)?)?,
                        y: PublicKey::from_slice(field(2)?)
                            .map_err(|_| Error::InvalidTransaction)?,
                    },
                    MINT_QUOTE_INPUT => TransactionElement::MintQuoteInput(MintQuoteInput {
                        amount: amount(field(0)?)?,
                        id: text(field(1)?)?,
                        pubkey: PublicKey::from_slice(field(2)?)
                            .map_err(|_| Error::InvalidTransaction)?,
                        issued: amount(field(3)?)?,
                    }),
                    BLINDED_OUTPUT => TransactionElement::BlindedOutput {
                        amount: amount(field(0)?)?,
                        keyset_id: id(field(1)?)?,
                        b_hash: field(2)?
                            .try_into()
                            .map_err(|_| Error::InvalidTransaction)?,
                    },
                    MELT_QUOTE_OUTPUT => TransactionElement::MeltQuoteOutput(Quote {
                        amount: amount(field(0)?)?,
                        id: text(field(1)?)?,
                    }),
                    MINT_QUOTE_OUTPUT => TransactionElement::MintQuoteOutput(MintQuoteOutput {
                        amount: amount(field(0)?)?,
                        pubkey: bitcoin::secp256k1::PublicKey::from_slice(field(1)?)
                            .map_err(|_| Error::InvalidTransaction)?,
                        id: text(field(2)?)?,
                    }),
                    _ => return Err(Error::InvalidTransaction),
                })
            })
            .collect()
    }
}

/// The transcript's input containers (`0x1n`), the bytes an input-bound quote id hashes.
pub fn input_section(proofs: &[Proof], mint_quotes: &[MintQuoteInput]) -> Result<Vec<u8>, Error> {
    Ok(input_records(proofs, mint_quotes)?.concat())
}

fn input_records(proofs: &[Proof], mint_quotes: &[MintQuoteInput]) -> Result<Vec<Vec<u8>>, Error> {
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
        inputs.push(container(PROOF_INPUT, &fields)?);
    }
    let mut ids = HashSet::new();
    for quote in mint_quotes {
        if !ids.insert(&quote.id) || quote.pubkey.as_secp256k1().is_err() {
            return Err(Error::InvalidTransaction);
        }
        inputs.push(container(
            MINT_QUOTE_INPUT,
            &[
                integer(u64::from(quote.amount)),
                quote.id.as_bytes().to_vec(),
                quote.pubkey.to_bytes(),
                integer(u64::from(quote.issued)),
            ],
        )?);
    }
    Ok(inputs)
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
    let bytes = authorized_request_transcript(method, target, body)?;
    Ok(tagged_hash(
        "Cashu_AuthorizedRequest",
        &Sha256::digest(&bytes),
    ))
}

/// NUT-22 authorized-request transcript, before hashing.
pub(crate) fn authorized_request_transcript(
    method: &str,
    target: &str,
    body: &[u8],
) -> Result<Vec<u8>, Error> {
    if method.is_empty()
        || !method.bytes().all(|b| b.is_ascii_uppercase())
        || !target.starts_with('/')
        || target.contains('#')
    {
        return Err(Error::InvalidTransaction);
    }
    container(
        AUTHORIZED_REQUEST,
        &[
            method.as_bytes().to_vec(),
            target.as_bytes().to_vec(),
            Sha256::digest(body).to_vec(),
        ],
    )
}

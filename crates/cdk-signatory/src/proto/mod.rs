use cdk_common::secret::Secret;
use cdk_common::{HTLCWitness, P2PKWitness};
use tonic::Status;

tonic::include_proto!("cdk_signatory");

pub mod client;
pub mod server;

impl From<cdk_common::ProofDleq> for ProofDleq {
    fn from(value: cdk_common::ProofDleq) -> Self {
        ProofDleq {
            e: value.e.as_secret_bytes().to_vec(),
            s: value.s.as_secret_bytes().to_vec(),
            r: value.r.as_secret_bytes().to_vec(),
        }
    }
}

impl TryInto<cdk_common::ProofDleq> for ProofDleq {
    type Error = Status;

    fn try_into(self) -> Result<cdk_common::ProofDleq, Self::Error> {
        Ok(cdk_common::ProofDleq {
            e: cdk_common::SecretKey::from_slice(&self.e)
                .map_err(|e| Status::from_error(Box::new(e)))?,
            s: cdk_common::SecretKey::from_slice(&self.s)
                .map_err(|e| Status::from_error(Box::new(e)))?,
            r: cdk_common::SecretKey::from_slice(&self.r)
                .map_err(|e| Status::from_error(Box::new(e)))?,
        })
    }
}

impl From<cdk_common::Proof> for Proof {
    fn from(value: cdk_common::Proof) -> Self {
        Proof {
            amount: value.amount.into(),
            keyset_id: value.keyset_id.to_string(),
            secret: value.secret.to_bytes(),
            c: value.c.to_bytes().to_vec(),
            witness: value.witness.map(|w| w.into()),
            dleq: value.dleq.map(|dleq| dleq.into()),
        }
    }
}

impl TryInto<cdk_common::Proof> for Proof {
    type Error = Status;
    fn try_into(self) -> Result<cdk_common::Proof, Self::Error> {
        Ok(cdk_common::Proof {
            amount: self.amount.into(),
            keyset_id: self
                .keyset_id
                .parse()
                .map_err(|e| Status::from_error(Box::new(e)))?,
            secret: Secret::from_bytes(self.secret),
            c: cdk_common::PublicKey::from_slice(&self.c)
                .map_err(|e| Status::from_error(Box::new(e)))?,
            witness: self.witness.map(|w| w.try_into()).transpose()?,
            dleq: self.dleq.map(|x| x.try_into()).transpose()?,
        })
    }
}

impl From<cdk_common::BlindedMessage> for BlindedMessage {
    fn from(value: cdk_common::BlindedMessage) -> Self {
        BlindedMessage {
            amount: value.amount.into(),
            keyset_id: value.keyset_id.to_string(),
            blinded_secret: value.blinded_secret.to_bytes().to_vec(),
            witness: value.witness.map(|x| x.into()),
        }
    }
}

impl TryInto<cdk_common::BlindedMessage> for BlindedMessage {
    type Error = Status;
    fn try_into(self) -> Result<cdk_common::BlindedMessage, Self::Error> {
        Ok(cdk_common::BlindedMessage {
            amount: self.amount.into(),
            keyset_id: self
                .keyset_id
                .parse()
                .map_err(|e| Status::from_error(Box::new(e)))?,
            blinded_secret: cdk_common::PublicKey::from_slice(&self.blinded_secret)
                .map_err(|e| Status::from_error(Box::new(e)))?,
            witness: self.witness.map(|x| x.try_into()).transpose()?,
        })
    }
}

impl From<cdk_common::BlindSignatureDleq> for BlindSignatureDleq {
    fn from(value: cdk_common::BlindSignatureDleq) -> Self {
        BlindSignatureDleq {
            e: value.e.as_secret_bytes().to_vec(),
            s: value.s.as_secret_bytes().to_vec(),
        }
    }
}

impl TryInto<cdk_common::BlindSignatureDleq> for BlindSignatureDleq {
    type Error = cdk_common::error::Error;
    fn try_into(self) -> Result<cdk_common::BlindSignatureDleq, Self::Error> {
        Ok(cdk_common::BlindSignatureDleq {
            e: cdk_common::SecretKey::from_slice(&self.e)?,
            s: cdk_common::SecretKey::from_slice(&self.s)?,
        })
    }
}

impl From<cdk_common::BlindSignature> for BlindSignature {
    fn from(value: cdk_common::BlindSignature) -> Self {
        BlindSignature {
            amount: value.amount.into(),
            blinded_secret: value.c.to_bytes().to_vec(),
            keyset_id: value.keyset_id.to_string(),
            dleq: value.dleq.map(|x| x.into()),
        }
    }
}

impl TryInto<cdk_common::BlindSignature> for BlindSignature {
    type Error = cdk_common::error::Error;

    fn try_into(self) -> Result<cdk_common::BlindSignature, Self::Error> {
        Ok(cdk_common::BlindSignature {
            amount: self.amount.into(),
            c: cdk_common::PublicKey::from_slice(&self.blinded_secret)?,
            keyset_id: self.keyset_id.parse().expect("Invalid keyset id"),
            dleq: self.dleq.map(|dleq| dleq.try_into()).transpose()?,
        })
    }
}

impl From<cdk_common::Witness> for Witness {
    fn from(value: cdk_common::Witness) -> Self {
        match value {
            cdk_common::Witness::P2PKWitness(P2PKWitness { signatures }) => Witness {
                witness_type: Some(witness::WitnessType::P2pkWitness(P2pkWitness {
                    signatures,
                })),
            },
            cdk_common::Witness::HTLCWitness(HTLCWitness {
                preimage,
                signatures,
            }) => Witness {
                witness_type: Some(witness::WitnessType::HtlcWitness(HtlcWitness {
                    preimage,
                    signatures: signatures.unwrap_or_default(),
                })),
            },
        }
    }
}

impl TryInto<cdk_common::Witness> for Witness {
    type Error = Status;
    fn try_into(self) -> Result<cdk_common::Witness, Self::Error> {
        match self.witness_type {
            Some(witness::WitnessType::P2pkWitness(P2pkWitness { signatures })) => {
                Ok(P2PKWitness { signatures }.into())
            }
            Some(witness::WitnessType::HtlcWitness(hltc_witness)) => Ok(HTLCWitness {
                preimage: hltc_witness.preimage,
                signatures: if hltc_witness.signatures.is_empty() {
                    None
                } else {
                    Some(hltc_witness.signatures)
                },
            }
            .into()),
            None => Err(Status::invalid_argument("Witness type not set")),
        }
    }
}

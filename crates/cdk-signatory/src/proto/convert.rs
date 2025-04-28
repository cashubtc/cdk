//! Type conversions between Rust types and the generated protobuf types.
use std::collections::BTreeMap;

use cashu::secret::Secret;
use cashu::util::hex;
use cashu::{Amount, PublicKey};
use cdk_common::{HTLCWitness, P2PKWitness};
use tonic::Status;

use super::*;

const INTERNAL_ERROR: &str = "Missing property";

impl From<crate::signatory::SignatoryKeysets> for SignatoryKeysets {
    fn from(keyset: crate::signatory::SignatoryKeysets) -> Self {
        Self {
            pubkey: keyset.pubkey.to_bytes().to_vec(),
            keysets: keyset
                .keysets
                .into_iter()
                .map(|keyset| keyset.into())
                .collect(),
        }
    }
}

impl TryInto<crate::signatory::SignatoryKeysets> for SignatoryKeysets {
    type Error = cdk_common::Error;

    fn try_into(self) -> Result<crate::signatory::SignatoryKeysets, Self::Error> {
        Ok(crate::signatory::SignatoryKeysets {
            pubkey: PublicKey::from_slice(&self.pubkey)?,
            keysets: self
                .keysets
                .into_iter()
                .map(|keyset| keyset.try_into())
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl TryInto<crate::signatory::SignatoryKeySet> for KeySet {
    type Error = cdk_common::Error;

    fn try_into(self) -> Result<crate::signatory::SignatoryKeySet, Self::Error> {
        Ok(crate::signatory::SignatoryKeySet {
            id: self.id.parse()?,
            unit: self
                .unit
                .ok_or(cdk_common::Error::Custom(INTERNAL_ERROR.to_owned()))?
                .try_into()
                .map_err(|_| cdk_common::Error::Custom("Invalid currency unit".to_owned()))?,
            active: self.active,
            input_fee_ppk: self.input_fee_ppk,
            keys: cdk_common::Keys::new(
                self.keys
                    .ok_or(cdk_common::Error::Custom(INTERNAL_ERROR.to_owned()))?
                    .keys
                    .into_iter()
                    .map(|(amount, pk)| PublicKey::from_slice(&pk).map(|pk| (amount.into(), pk)))
                    .collect::<Result<BTreeMap<Amount, _>, _>>()?,
            ),
        })
    }
}

impl From<crate::signatory::SignatoryKeySet> for KeySet {
    fn from(keyset: crate::signatory::SignatoryKeySet) -> Self {
        Self {
            id: keyset.id.to_string(),
            unit: Some(keyset.unit.into()),
            active: keyset.active,
            input_fee_ppk: keyset.input_fee_ppk,
            keys: Some(Keys {
                keys: keyset
                    .keys
                    .iter()
                    .map(|(key, value)| ((*key).into(), value.to_bytes().to_vec()))
                    .collect(),
            }),
        }
    }
}

impl From<cdk_common::Error> for Error {
    fn from(err: cdk_common::Error) -> Self {
        let code = match err {
            cdk_common::Error::AmountError(_) => ErrorCode::AmountOutsideLimit,
            cdk_common::Error::DuplicateInputs => ErrorCode::DuplicateInputsProvided,
            cdk_common::Error::DuplicateOutputs => ErrorCode::DuplicateInputsProvided,
            cdk_common::Error::UnknownKeySet => ErrorCode::KeysetNotKnown,
            cdk_common::Error::InactiveKeyset => ErrorCode::KeysetInactive,
            _ => ErrorCode::Unknown,
        };

        Error {
            code: code.into(),
            detail: err.to_string(),
        }
    }
}

impl From<Error> for cdk_common::Error {
    fn from(val: Error) -> Self {
        match val.code.try_into().expect("valid code") {
            ErrorCode::DuplicateInputsProvided => cdk_common::Error::DuplicateInputs,
            ErrorCode::Unknown => cdk_common::Error::Custom(val.detail),
            _ => todo!(),
        }
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

impl From<Vec<cdk_common::Proof>> for Proofs {
    fn from(value: Vec<cdk_common::Proof>) -> Self {
        Proofs {
            proof: value.into_iter().map(|x| x.into()).collect(),
        }
    }
}

impl From<cdk_common::Proof> for Proof {
    fn from(value: cdk_common::Proof) -> Self {
        Proof {
            amount: value.amount.into(),
            keyset_id: value.keyset_id.to_string(),
            secret: value.secret.to_bytes(),
            c: value.c.to_bytes().to_vec(),
        }
    }
}

impl TryInto<cdk_common::Proof> for Proof {
    type Error = Status;
    fn try_into(self) -> Result<cdk_common::Proof, Self::Error> {
        let secret = if let Ok(str) = String::from_utf8(self.secret.clone()) {
            str
        } else {
            hex::encode(&self.secret)
        };

        Ok(cdk_common::Proof {
            amount: self.amount.into(),
            keyset_id: self
                .keyset_id
                .parse()
                .map_err(|e| Status::from_error(Box::new(e)))?,
            secret: Secret::new(secret),
            c: cdk_common::PublicKey::from_slice(&self.c)
                .map_err(|e| Status::from_error(Box::new(e)))?,
            witness: None,
            dleq: None,
        })
    }
}

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

impl From<cdk_common::BlindedMessage> for BlindedMessage {
    fn from(value: cdk_common::BlindedMessage) -> Self {
        BlindedMessage {
            amount: value.amount.into(),
            keyset_id: value.keyset_id.to_string(),
            blinded_secret: value.blinded_secret.to_bytes().to_vec(),
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
            witness: None,
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

impl From<()> for EmptyRequest {
    fn from(_: ()) -> Self {
        EmptyRequest {}
    }
}

impl TryInto<()> for EmptyRequest {
    type Error = cdk_common::error::Error;

    fn try_into(self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl From<cashu::CurrencyUnit> for CurrencyUnit {
    fn from(value: cashu::CurrencyUnit) -> Self {
        match value {
            cashu::CurrencyUnit::Sat => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::Unit(
                    CurrencyUnitType::Sat.into(),
                )),
            },
            cashu::CurrencyUnit::Msat => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::Unit(
                    CurrencyUnitType::Msat.into(),
                )),
            },
            cashu::CurrencyUnit::Usd => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::Unit(
                    CurrencyUnitType::Usd.into(),
                )),
            },
            cashu::CurrencyUnit::Eur => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::Unit(
                    CurrencyUnitType::Eur.into(),
                )),
            },
            cashu::CurrencyUnit::Custom(name) => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::CustomUnit(name)),
            },
            _ => unreachable!(),
        }
    }
}

impl TryInto<cashu::CurrencyUnit> for CurrencyUnit {
    type Error = Status;

    fn try_into(self) -> Result<cashu::CurrencyUnit, Self::Error> {
        match self.currency_unit {
            Some(currency_unit::CurrencyUnit::Unit(u)) => match u
                .try_into()
                .map_err(|_| Status::invalid_argument("Invalid currency unit"))?
            {
                CurrencyUnitType::Sat => Ok(cashu::CurrencyUnit::Sat),
                CurrencyUnitType::Msat => Ok(cashu::CurrencyUnit::Msat),
                CurrencyUnitType::Usd => Ok(cashu::CurrencyUnit::Usd),
                CurrencyUnitType::Eur => Ok(cashu::CurrencyUnit::Eur),
                CurrencyUnitType::Auth => Ok(cashu::CurrencyUnit::Auth),
            },
            Some(currency_unit::CurrencyUnit::CustomUnit(name)) => {
                Ok(cashu::CurrencyUnit::Custom(name))
            }
            None => Err(Status::invalid_argument("Currency unit not set")),
        }
    }
}

impl TryInto<cashu::KeySet> for KeySet {
    type Error = cdk_common::error::Error;
    fn try_into(self) -> Result<cashu::KeySet, Self::Error> {
        Ok(cashu::KeySet {
            id: self
                .id
                .parse()
                .map_err(|_| cdk_common::error::Error::Custom("Invalid ID".to_owned()))?,
            unit: self
                .unit
                .ok_or(cdk_common::error::Error::Custom(INTERNAL_ERROR.to_owned()))?
                .try_into()
                .map_err(|_| cdk_common::Error::Custom("Invalid unit encoding".to_owned()))?,
            keys: cashu::Keys::new(
                self.keys
                    .ok_or(cdk_common::error::Error::Custom(INTERNAL_ERROR.to_owned()))?
                    .keys
                    .into_iter()
                    .map(|(k, v)| cdk_common::PublicKey::from_slice(&v).map(|pk| (k.into(), pk)))
                    .collect::<Result<BTreeMap<cashu::Amount, cdk_common::PublicKey>, _>>()?,
            ),
        })
    }
}

impl From<crate::signatory::RotateKeyArguments> for RotationRequest {
    fn from(value: crate::signatory::RotateKeyArguments) -> Self {
        Self {
            unit: Some(value.unit.into()),
            max_order: value.max_order.into(),
            input_fee_ppk: value.input_fee_ppk,
        }
    }
}

impl TryInto<crate::signatory::RotateKeyArguments> for RotationRequest {
    type Error = Status;

    fn try_into(self) -> Result<crate::signatory::RotateKeyArguments, Self::Error> {
        Ok(crate::signatory::RotateKeyArguments {
            unit: self
                .unit
                .ok_or(Status::invalid_argument("unit not set"))?
                .try_into()?,
            max_order: self
                .max_order
                .try_into()
                .map_err(|_| Status::invalid_argument("Invalid max_order"))?,
            input_fee_ppk: self.input_fee_ppk,
        })
    }
}

impl From<cdk_common::KeySetInfo> for KeySet {
    fn from(value: cdk_common::KeySetInfo) -> Self {
        Self {
            id: value.id.into(),
            unit: Some(value.unit.into()),
            active: value.active,
            input_fee_ppk: value.input_fee_ppk,
            keys: Default::default(),
        }
    }
}

impl TryInto<cdk_common::KeySetInfo> for KeySet {
    type Error = cdk_common::Error;

    fn try_into(self) -> Result<cdk_common::KeySetInfo, Self::Error> {
        Ok(cdk_common::KeySetInfo {
            id: self.id.try_into()?,
            unit: self
                .unit
                .ok_or(cdk_common::Error::Custom(INTERNAL_ERROR.to_owned()))?
                .try_into()
                .map_err(|_| cdk_common::Error::Custom("Invalid unit encoding".to_owned()))?,
            active: self.active,
            input_fee_ppk: self.input_fee_ppk,
        })
    }
}

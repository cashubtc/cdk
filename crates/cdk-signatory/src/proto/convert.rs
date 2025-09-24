//! Type conversions between Rust types and the generated protobuf types.
use std::collections::BTreeMap;

use cdk_common::secret::Secret;
use cdk_common::util::hex;
use cdk_common::{Amount, Id, PublicKey};
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
    /// TODO: Make sure that all type Error here are cdk_common::Error
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
            id: Id::from_bytes(&self.id)?,
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
            amounts: self.amounts,
            final_expiry: self.final_expiry,
        })
    }
}

impl From<crate::signatory::SignatoryKeySet> for KeySet {
    fn from(keyset: crate::signatory::SignatoryKeySet) -> Self {
        Self {
            id: keyset.id.to_bytes(),
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
            final_expiry: keyset.final_expiry,
            amounts: keyset.amounts,
            version: Default::default(),
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
            _ => ErrorCode::Unspecified,
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
            ErrorCode::AmountOutsideLimit => {
                cdk_common::Error::AmountError(cdk_common::amount::Error::AmountOverflow)
            }
            ErrorCode::DuplicateInputsProvided => cdk_common::Error::DuplicateInputs,
            ErrorCode::KeysetNotKnown => cdk_common::Error::UnknownKeySet,
            ErrorCode::KeysetInactive => cdk_common::Error::InactiveKeyset,
            ErrorCode::Unspecified => cdk_common::Error::Custom(val.detail),
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
            keyset_id: value.keyset_id.to_bytes(),
            dleq: value.dleq.map(|x| x.into()),
        }
    }
}

impl From<Vec<cdk_common::Proof>> for Proofs {
    fn from(value: Vec<cdk_common::Proof>) -> Self {
        Proofs {
            proof: value.into_iter().map(|x| x.into()).collect(),
            operation: Operation::Unspecified.into(),
            correlation_id: "".to_owned(),
        }
    }
}

impl From<cdk_common::Proof> for Proof {
    fn from(value: cdk_common::Proof) -> Self {
        Proof {
            amount: value.amount.into(),
            keyset_id: value.keyset_id.to_bytes(),
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
            keyset_id: Id::from_bytes(&self.keyset_id)
                .map_err(|e| Status::from_error(Box::new(e)))?,
            secret: Secret::new(secret),
            c: cdk_common::PublicKey::from_slice(&self.c)
                .map_err(|e| Status::from_error(Box::new(e)))?,
            witness: None,
            dleq: None,
        })
    }
}

impl TryInto<cdk_common::BlindSignature> for BlindSignature {
    type Error = cdk_common::error::Error;

    fn try_into(self) -> Result<cdk_common::BlindSignature, Self::Error> {
        Ok(cdk_common::BlindSignature {
            amount: self.amount.into(),
            c: cdk_common::PublicKey::from_slice(&self.blinded_secret)?,
            keyset_id: Id::from_bytes(&self.keyset_id)?,
            dleq: self.dleq.map(|dleq| dleq.try_into()).transpose()?,
        })
    }
}

impl From<cdk_common::BlindedMessage> for BlindedMessage {
    fn from(value: cdk_common::BlindedMessage) -> Self {
        BlindedMessage {
            amount: value.amount.into(),
            keyset_id: value.keyset_id.to_bytes(),
            blinded_secret: value.blinded_secret.to_bytes().to_vec(),
        }
    }
}

impl TryInto<cdk_common::BlindedMessage> for BlindedMessage {
    type Error = Status;
    fn try_into(self) -> Result<cdk_common::BlindedMessage, Self::Error> {
        Ok(cdk_common::BlindedMessage {
            amount: self.amount.into(),
            keyset_id: Id::from_bytes(&self.keyset_id)
                .map_err(|e| Status::from_error(Box::new(e)))?,
            blinded_secret: cdk_common::PublicKey::from_slice(&self.blinded_secret)
                .map_err(|e| Status::from_error(Box::new(e)))?,
            witness: None,
        })
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

impl From<cdk_common::CurrencyUnit> for CurrencyUnit {
    fn from(value: cdk_common::CurrencyUnit) -> Self {
        match value {
            cdk_common::CurrencyUnit::Sat => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::Unit(
                    CurrencyUnitType::Sat.into(),
                )),
            },
            cdk_common::CurrencyUnit::Msat => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::Unit(
                    CurrencyUnitType::Msat.into(),
                )),
            },
            cdk_common::CurrencyUnit::Usd => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::Unit(
                    CurrencyUnitType::Usd.into(),
                )),
            },
            cdk_common::CurrencyUnit::Eur => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::Unit(
                    CurrencyUnitType::Eur.into(),
                )),
            },
            cdk_common::CurrencyUnit::Auth => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::Unit(
                    CurrencyUnitType::Auth.into(),
                )),
            },
            cdk_common::CurrencyUnit::Custom(name) => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::CustomUnit(name)),
            },
            _ => unreachable!(),
        }
    }
}

impl TryInto<cdk_common::CurrencyUnit> for CurrencyUnit {
    type Error = Status;

    fn try_into(self) -> Result<cdk_common::CurrencyUnit, Self::Error> {
        match self.currency_unit {
            Some(currency_unit::CurrencyUnit::Unit(u)) => match u
                .try_into()
                .map_err(|_| Status::invalid_argument("Invalid currency unit"))?
            {
                CurrencyUnitType::Sat => Ok(cdk_common::CurrencyUnit::Sat),
                CurrencyUnitType::Msat => Ok(cdk_common::CurrencyUnit::Msat),
                CurrencyUnitType::Usd => Ok(cdk_common::CurrencyUnit::Usd),
                CurrencyUnitType::Eur => Ok(cdk_common::CurrencyUnit::Eur),
                CurrencyUnitType::Auth => Ok(cdk_common::CurrencyUnit::Auth),
                CurrencyUnitType::Unspecified => {
                    Err(Status::invalid_argument("Current unit is not specified"))
                }
            },
            Some(currency_unit::CurrencyUnit::CustomUnit(name)) => {
                Ok(cdk_common::CurrencyUnit::Custom(name))
            }
            None => Err(Status::invalid_argument("Currency unit not set")),
        }
    }
}

impl TryInto<cdk_common::KeySet> for KeySet {
    type Error = cdk_common::error::Error;
    fn try_into(self) -> Result<cdk_common::KeySet, Self::Error> {
        Ok(cdk_common::KeySet {
            id: Id::from_bytes(&self.id)?,
            unit: self
                .unit
                .ok_or(cdk_common::error::Error::Custom(INTERNAL_ERROR.to_owned()))?
                .try_into()
                .map_err(|_| cdk_common::Error::Custom("Invalid unit encoding".to_owned()))?,
            keys: cdk_common::Keys::new(
                self.keys
                    .ok_or(cdk_common::error::Error::Custom(INTERNAL_ERROR.to_owned()))?
                    .keys
                    .into_iter()
                    .map(|(k, v)| cdk_common::PublicKey::from_slice(&v).map(|pk| (k.into(), pk)))
                    .collect::<Result<BTreeMap<cdk_common::Amount, cdk_common::PublicKey>, _>>()?,
            ),
            final_expiry: self.final_expiry,
        })
    }
}

impl From<crate::signatory::RotateKeyArguments> for RotationRequest {
    fn from(value: crate::signatory::RotateKeyArguments) -> Self {
        Self {
            unit: Some(value.unit.into()),
            amounts: value.amounts,
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
            amounts: self.amounts,
            input_fee_ppk: self.input_fee_ppk,
        })
    }
}

impl From<cdk_common::KeySetInfo> for KeySet {
    fn from(value: cdk_common::KeySetInfo) -> Self {
        Self {
            id: value.id.to_bytes(),
            unit: Some(value.unit.into()),
            active: value.active,
            input_fee_ppk: value.input_fee_ppk,
            keys: Default::default(),
            final_expiry: value.final_expiry,
            amounts: vec![],
            version: Default::default(),
        }
    }
}

impl TryInto<cdk_common::KeySetInfo> for KeySet {
    type Error = cdk_common::Error;

    fn try_into(self) -> Result<cdk_common::KeySetInfo, Self::Error> {
        Ok(cdk_common::KeySetInfo {
            id: Id::from_bytes(&self.id)?,
            unit: self
                .unit
                .ok_or(cdk_common::Error::Custom(INTERNAL_ERROR.to_owned()))?
                .try_into()
                .map_err(|_| cdk_common::Error::Custom("Invalid unit encoding".to_owned()))?,
            active: self.active,
            input_fee_ppk: self.input_fee_ppk,
            final_expiry: self.final_expiry,
        })
    }
}

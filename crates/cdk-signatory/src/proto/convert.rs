//! Type conversions between Rust types and the generated protobuf types.
use std::collections::BTreeMap;
use std::str::FromStr;

use cashu::secret::Secret;
use cdk_common::{HTLCWitness, P2PKWitness};
use tonic::Status;

use super::*;

impl From<cashu::Id> for Id {
    fn from(value: cashu::Id) -> Self {
        Id {
            inner: value.to_bytes().to_vec(),
        }
    }
}

impl TryInto<cashu::Id> for Id {
    type Error = cdk_common::error::Error;

    fn try_into(self) -> Result<cashu::Id, Self::Error> {
        Ok(cashu::Id::from_bytes(&self.inner)?)
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

impl From<crate::signatory::SignatoryKeySet> for SignatoryKeySet {
    fn from(value: crate::signatory::SignatoryKeySet) -> Self {
        SignatoryKeySet {
            key: Some(value.key.into()),
            info: Some(value.info.into()),
        }
    }
}

impl TryInto<crate::signatory::SignatoryKeySet> for SignatoryKeySet {
    type Error = cdk_common::error::Error;

    fn try_into(self) -> Result<crate::signatory::SignatoryKeySet, Self::Error> {
        Ok(crate::signatory::SignatoryKeySet {
            key: self
                .key
                .ok_or(cdk_common::Error::RecvError(
                    "Missing property key".to_owned(),
                ))?
                .try_into()?,
            info: self
                .info
                .ok_or(cdk_common::Error::RecvError(
                    "Missing property info".to_owned(),
                ))?
                .try_into()?,
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

impl From<cdk_common::Proof> for Proof {
    fn from(value: cdk_common::Proof) -> Self {
        Proof {
            amount: value.amount.into(),
            keyset_id: value.keyset_id.to_string(),
            secret: value.secret.to_string(),
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
            secret: Secret::from_str(&self.secret).map_err(|e| Status::from_error(Box::new(e)))?,
            c: cdk_common::PublicKey::from_slice(&self.c)
                .map_err(|e| Status::from_error(Box::new(e)))?,
            witness: self.witness.map(|w| w.try_into()).transpose()?,
            dleq: self.dleq.map(|x| x.try_into()).transpose()?,
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

impl From<()> for Empty {
    fn from(_: ()) -> Self {
        Empty {}
    }
}

impl TryInto<()> for Empty {
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
                    CurrencyUnitType::CurrencyUnitSat.into(),
                )),
            },
            cashu::CurrencyUnit::Msat => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::Unit(
                    CurrencyUnitType::CurrencyUnitMsat.into(),
                )),
            },
            cashu::CurrencyUnit::Usd => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::Unit(
                    CurrencyUnitType::CurrencyUnitUsd.into(),
                )),
            },
            cashu::CurrencyUnit::Eur => CurrencyUnit {
                currency_unit: Some(currency_unit::CurrencyUnit::Unit(
                    CurrencyUnitType::CurrencyUnitEur.into(),
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
                CurrencyUnitType::CurrencyUnitSat => Ok(cashu::CurrencyUnit::Sat),
                CurrencyUnitType::CurrencyUnitMsat => Ok(cashu::CurrencyUnit::Msat),
                CurrencyUnitType::CurrencyUnitUsd => Ok(cashu::CurrencyUnit::Usd),
                CurrencyUnitType::CurrencyUnitEur => Ok(cashu::CurrencyUnit::Eur),
            },
            Some(currency_unit::CurrencyUnit::CustomUnit(name)) => {
                Ok(cashu::CurrencyUnit::Custom(name))
            }
            None => Err(Status::invalid_argument("Currency unit not set")),
        }
    }
}

impl From<&bitcoin::bip32::ChildNumber> for derivation_path::ChildNumber {
    fn from(value: &bitcoin::bip32::ChildNumber) -> Self {
        match value {
            bitcoin::bip32::ChildNumber::Normal { index } => {
                derivation_path::ChildNumber::Normal(*index)
            }
            bitcoin::bip32::ChildNumber::Hardened { index } => {
                derivation_path::ChildNumber::Hardened(*index)
            }
        }
    }
}

impl TryInto<bitcoin::bip32::ChildNumber> for derivation_path::ChildNumber {
    type Error = cdk_common::error::Error;

    fn try_into(self) -> Result<bitcoin::bip32::ChildNumber, Self::Error> {
        Ok(match self {
            derivation_path::ChildNumber::Normal(index) => {
                bitcoin::bip32::ChildNumber::Normal { index }
            }
            derivation_path::ChildNumber::Hardened(index) => {
                bitcoin::bip32::ChildNumber::Hardened { index }
            }
        })
    }
}

impl From<cdk_common::mint::MintKeySetInfo> for MintKeySetInfo {
    fn from(value: cdk_common::mint::MintKeySetInfo) -> Self {
        Self {
            id: Some(value.id.into()),
            unit: Some(value.unit.into()),
            active: value.active,
            valid_from: value.valid_from,
            valid_to: value.valid_to,
            derivation_path: value
                .derivation_path
                .into_iter()
                .map(|x| DerivationPath {
                    child_number: Some(x.into()),
                })
                .collect(),
            derivation_path_index: value.derivation_path_index,
            max_order: value.max_order.into(),
            input_fee_ppk: value.input_fee_ppk,
        }
    }
}

impl TryInto<cdk_common::mint::MintKeySetInfo> for MintKeySetInfo {
    type Error = cdk_common::error::Error;

    fn try_into(self) -> Result<cdk_common::mint::MintKeySetInfo, Self::Error> {
        Ok(cdk_common::mint::MintKeySetInfo {
            id: self
                .id
                .ok_or(cdk_common::error::Error::Custom("id not set".to_owned()))?
                .try_into()?,
            unit: self
                .unit
                .ok_or(cdk_common::error::Error::Custom("unit not set".to_owned()))?
                .try_into()
                .map_err(|_| cdk_common::Error::Custom("Invalid unit encoding".to_owned()))?,
            active: self.active,
            valid_from: self.valid_from,
            valid_to: self.valid_to,
            max_order: self
                .max_order
                .try_into()
                .map_err(|_| cdk_common::Error::Custom("Invalid max_order".to_owned()))?,
            input_fee_ppk: self.input_fee_ppk,
            derivation_path: self
                .derivation_path
                .into_iter()
                .map(|derivation_path| {
                    derivation_path
                        .child_number
                        .ok_or(cdk_common::error::Error::Custom(
                            "child_number not set".to_owned(),
                        ))?
                        .try_into()
                })
                .collect::<Result<Vec<bitcoin::bip32::ChildNumber>, _>>()?
                .into(),
            derivation_path_index: self.derivation_path_index,
        })
    }
}

impl From<cashu::KeySet> for KeySet {
    fn from(value: cashu::KeySet) -> Self {
        Self {
            id: Some(value.id.into()),
            unit: Some(value.unit.into()),
            keys: Some(Keys {
                keys: value
                    .keys
                    .iter()
                    .map(|(amount, pk)| (*(amount.as_ref()), pk.to_bytes().to_vec()))
                    .collect(),
            }),
        }
    }
}

impl TryInto<cashu::KeySet> for KeySet {
    type Error = cdk_common::error::Error;
    fn try_into(self) -> Result<cashu::KeySet, Self::Error> {
        Ok(cashu::KeySet {
            id: self
                .id
                .ok_or(cdk_common::error::Error::Custom("id not set".to_owned()))?
                .try_into()?,
            unit: self
                .unit
                .ok_or(cdk_common::error::Error::Custom("unit not set".to_owned()))?
                .try_into()
                .map_err(|_| cdk_common::Error::Custom("Invalid unit encoding".to_owned()))?,
            keys: cashu::Keys::new(
                self.keys
                    .ok_or(cdk_common::error::Error::Custom("keys not set".to_owned()))?
                    .keys
                    .into_iter()
                    .map(|(k, v)| cdk_common::PublicKey::from_slice(&v).map(|pk| (k.into(), pk)))
                    .collect::<Result<BTreeMap<cashu::Amount, cdk_common::PublicKey>, _>>()?,
            ),
        })
    }
}

impl From<cashu::KeysResponse> for KeysResponse {
    fn from(value: cashu::KeysResponse) -> Self {
        Self {
            keysets: value.keysets.into_iter().map(|x| x.into()).collect(),
        }
    }
}

impl TryInto<cashu::KeysResponse> for KeysResponse {
    type Error = cdk_common::error::Error;

    fn try_into(self) -> Result<cashu::KeysResponse, Self::Error> {
        Ok(cashu::KeysResponse {
            keysets: self
                .keysets
                .into_iter()
                .map(|x| x.try_into())
                .collect::<Result<Vec<cashu::KeySet>, _>>()?,
        })
    }
}

impl From<crate::signatory::RotateKeyArguments> for RotateKeyArguments {
    fn from(value: crate::signatory::RotateKeyArguments) -> Self {
        Self {
            unit: Some(value.unit.into()),
            derivation_path_index: value.derivation_path_index,
            max_order: value.max_order.into(),
            input_fee_ppk: value.input_fee_ppk,
        }
    }
}

impl TryInto<crate::signatory::RotateKeyArguments> for RotateKeyArguments {
    type Error = Status;

    fn try_into(self) -> Result<crate::signatory::RotateKeyArguments, Self::Error> {
        Ok(crate::signatory::RotateKeyArguments {
            unit: self
                .unit
                .ok_or(Status::invalid_argument("unit not set"))?
                .try_into()?,
            derivation_path_index: self.derivation_path_index,
            max_order: self
                .max_order
                .try_into()
                .map_err(|_| Status::invalid_argument("Invalid max_order"))?,
            input_fee_ppk: self.input_fee_ppk,
        })
    }
}

impl From<cdk_common::KeySetInfo> for KeySetInfo {
    fn from(value: cdk_common::KeySetInfo) -> Self {
        Self {
            id: Some(value.id.into()),
            unit: Some(value.unit.into()),
            active: value.active,
            input_fee_ppk: value.input_fee_ppk,
        }
    }
}

impl TryInto<cdk_common::KeySetInfo> for KeySetInfo {
    type Error = cdk_common::Error;

    fn try_into(self) -> Result<cdk_common::KeySetInfo, Self::Error> {
        Ok(cdk_common::KeySetInfo {
            id: self
                .id
                .ok_or(cdk_common::Error::Custom("id not set".to_owned()))?
                .try_into()?,
            unit: self
                .unit
                .ok_or(cdk_common::Error::Custom("unit not set".to_owned()))?
                .try_into()
                .map_err(|_| cdk_common::Error::Custom("Invalid unit encoding".to_owned()))?,
            active: self.active,
            input_fee_ppk: self.input_fee_ppk,
        })
    }
}

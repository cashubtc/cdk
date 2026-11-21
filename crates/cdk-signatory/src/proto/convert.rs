//! Type conversions between Rust types and the generated protobuf types.
use std::collections::BTreeMap;
use std::str::FromStr;

use cdk_common::common::IssuerVersion;
use cdk_common::nut02::KeySetVersion;
use cdk_common::secret::Secret;
use cdk_common::util::hex;
use cdk_common::{Amount, DhkePoint, Id, MintPublicKey, PublicKey};
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
        let keys = self
            .keys
            .ok_or(cdk_common::Error::Custom(INTERNAL_ERROR.to_owned()))?
            .keys
            .into_iter()
            .map(|(amount, pk)| MintPublicKey::from_slice(&pk).map(|pk| (amount.into(), pk)))
            .collect::<Result<BTreeMap<Amount, _>, _>>()?;

        Ok(crate::signatory::SignatoryKeySet {
            id: Id::from_bytes(&self.id)?,
            unit: self
                .unit
                .ok_or(cdk_common::Error::Custom(INTERNAL_ERROR.to_owned()))?
                .try_into()
                .map_err(|_| cdk_common::Error::Custom("Invalid currency unit".to_owned()))?,
            active: self.active,
            input_fee_ppk: self.input_fee_ppk,
            amounts: keys.keys().map(|x| x.to_u64()).collect::<Vec<_>>(),
            keys: cdk_common::Keys::new(keys),
            final_expiry: self.final_expiry,
            version: self.version,
            issuer_version: self
                .issuer_version
                .map(|v| IssuerVersion::from_str(&v))
                .transpose()
                .map_err(|e| cdk_common::Error::Custom(e.to_string()))?,
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
            version: keyset.version,
            issuer_version: keyset.issuer_version.map(|v| v.to_string()),
        }
    }
}

impl From<cdk_common::Error> for super::Error {
    fn from(err: cdk_common::Error) -> Self {
        let code = match err {
            cdk_common::Error::AmountError(_) => ErrorCode::AmountOutsideLimit,
            cdk_common::Error::DuplicateInputs => ErrorCode::DuplicateInputsProvided,
            cdk_common::Error::DuplicateOutputs => ErrorCode::DuplicateInputsProvided,
            cdk_common::Error::UnknownKeySet => ErrorCode::KeysetNotKnown,
            cdk_common::Error::InactiveKeyset => ErrorCode::KeysetInactive,
            cdk_common::Error::SignatureMissingOrInvalid => ErrorCode::InvalidBlindMessage,
            _ => ErrorCode::Unspecified,
        };

        super::Error {
            code: code.into(),
            detail: err.to_string(),
        }
    }
}

impl From<super::Error> for cdk_common::Error {
    fn from(val: super::Error) -> Self {
        match val.code.try_into().expect("valid code") {
            ErrorCode::AmountOutsideLimit => {
                cdk_common::Error::AmountError(cdk_common::amount::Error::AmountOverflow)
            }
            ErrorCode::DuplicateInputsProvided => cdk_common::Error::DuplicateInputs,
            ErrorCode::KeysetNotKnown => cdk_common::Error::UnknownKeySet,
            ErrorCode::KeysetInactive => cdk_common::Error::InactiveKeyset,
            ErrorCode::InvalidBlindMessage => cdk_common::Error::SignatureMissingOrInvalid,
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

        let keyset_id =
            Id::from_bytes(&self.keyset_id).map_err(|e| Status::from_error(Box::new(e)))?;
        Ok(cdk_common::Proof {
            amount: self.amount.into(),
            keyset_id,
            secret: Secret::new(secret),
            c: DhkePoint::from_slice_for_version(&self.c, keyset_id.get_version())
                .map_err(|e| Status::from_error(Box::new(e)))?,
            witness: None,
            dleq: None,
            p2pk_e: None,
            spend_info: None,
        })
    }
}

impl TryInto<cdk_common::BlindSignature> for BlindSignature {
    type Error = cdk_common::error::Error;

    fn try_into(self) -> Result<cdk_common::BlindSignature, Self::Error> {
        let keyset_id = Id::from_bytes(&self.keyset_id)?;
        Ok(cdk_common::BlindSignature {
            amount: self.amount.into(),
            c: DhkePoint::from_slice_for_version(&self.blinded_secret, keyset_id.get_version())?,
            keyset_id,
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
        let keyset_id =
            Id::from_bytes(&self.keyset_id).map_err(|e| Status::from_error(Box::new(e)))?;
        Ok(cdk_common::BlindedMessage {
            amount: self.amount.into(),
            keyset_id,
            blinded_secret: DhkePoint::from_slice_for_version(
                &self.blinded_secret,
                keyset_id.get_version(),
            )
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
                currency_unit: Some(currency_unit::CurrencyUnit::CustomUnit(
                    cdk_common::CurrencyUnit::custom(name).to_string(),
                )),
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
                Ok(cdk_common::CurrencyUnit::custom(name))
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
            active: Some(self.active),
            keys: cdk_common::Keys::new(
                self.keys
                    .ok_or(cdk_common::error::Error::Custom(INTERNAL_ERROR.to_owned()))?
                    .keys
                    .into_iter()
                    .map(|(k, v)| MintPublicKey::from_slice(&v).map(|pk| (k.into(), pk)))
                    .collect::<Result<BTreeMap<cdk_common::Amount, MintPublicKey>, _>>()?,
            ),
            input_fee_ppk: self.input_fee_ppk,
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
            keyset_id_type: value.keyset_id_type.to_proto_i32(),
            final_expiry: value.final_expiry,
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
            final_expiry: self.final_expiry,
            keyset_id_type: KeySetVersion::from_proto_i32(self.keyset_id_type)
                .map_err(|err| Status::invalid_argument(err.to_string()))?,
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
            version: Default::default(),
            issuer_version: None,
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

impl From<crate::signatory::ReconstructDleqArguments> for ReconstructDleqEntry {
    fn from(value: crate::signatory::ReconstructDleqArguments) -> Self {
        Self {
            blind_signature: Some(value.blind_signature.into()),
            blind_secret: value.blind_secret.to_bytes().to_vec(),
        }
    }
}

impl TryInto<crate::signatory::ReconstructDleqArguments> for ReconstructDleqEntry {
    type Error = Status;

    fn try_into(self) -> Result<crate::signatory::ReconstructDleqArguments, Self::Error> {
        let blind_signature: cdk_common::BlindSignature = self
            .blind_signature
            .ok_or(Status::invalid_argument("blind_signature not set"))?
            .try_into()
            .map_err(|err: cdk_common::error::Error| Status::invalid_argument(err.to_string()))?;
        let blind_secret = DhkePoint::from_slice_for_version(
            &self.blind_secret,
            blind_signature.keyset_id.get_version(),
        )
        .map_err(|err| Status::invalid_argument(err.to_string()))?;
        Ok(crate::signatory::ReconstructDleqArguments {
            blind_signature,
            blind_secret,
        })
    }
}

#[cfg(test)]
mod tests {
    use prost::Message;

    use super::*;

    #[test]
    fn points_are_parsed_by_keyset_version() {
        use cdk_common::nuts::nut01::BlsG1PublicKey;

        let secp = cdk_common::SecretKey::generate()
            .public_key()
            .to_bytes()
            .to_vec();
        let bls = BlsG1PublicKey::hash_to_curve(b"point").to_bytes().to_vec();
        let legacy_id = Id::from_str("009a1f293253e41e").unwrap().to_bytes();
        let mut v3_id = vec![1u8; 33];
        v3_id[0] = KeySetVersion::Version02.to_byte();

        let message = |keyset_id: &[u8], point: &[u8]| BlindedMessage {
            amount: 1,
            keyset_id: keyset_id.to_vec(),
            blinded_secret: point.to_vec(),
        };
        let parse = |message: BlindedMessage| -> Result<cdk_common::BlindedMessage, Status> {
            message.try_into()
        };
        assert!(parse(message(&legacy_id, &secp)).is_ok());
        assert!(parse(message(&v3_id, &bls)).is_ok());
        assert!(parse(message(&legacy_id, &bls)).is_err());
        assert!(parse(message(&v3_id, &secp)).is_err());

        let proof = |keyset_id: &[u8], c: &[u8]| -> Result<cdk_common::Proof, Status> {
            Proof {
                amount: 1,
                keyset_id: keyset_id.to_vec(),
                secret: b"secret".to_vec(),
                c: c.to_vec(),
            }
            .try_into()
        };
        assert!(proof(&v3_id, &bls).is_ok());
        assert!(proof(&v3_id, &secp).is_err());
        assert!(proof(&legacy_id, &bls).is_err());
    }

    #[test]
    fn rotation_versions_roundtrip_through_protobuf() {
        for (version, wire_version) in [
            (KeySetVersion::Version00, KeysetVersion::V1),
            (KeySetVersion::Version01, KeysetVersion::V2),
            (KeySetVersion::Version02, KeysetVersion::V3),
        ] {
            let args = crate::signatory::RotateKeyArguments {
                unit: cdk_common::CurrencyUnit::Sat,
                amounts: vec![1, 2, 4],
                input_fee_ppk: 100,
                keyset_id_type: version,
                final_expiry: None,
            };
            let request: RotationRequest = args.into();
            let decoded = RotationRequest::decode(request.encode_to_vec().as_slice()).unwrap();
            assert_eq!(decoded.keyset_id_type(), wire_version);
            let restored: crate::signatory::RotateKeyArguments = decoded.try_into().unwrap();
            assert_eq!(restored.keyset_id_type, version);
            assert_eq!(restored.amounts, vec![1, 2, 4]);
            assert_eq!(restored.input_fee_ppk, 100);
        }
    }

    #[test]
    fn signatory_schema_rejects_pre_bls_peers() {
        use cdk_common::grpc::{
            create_version_check_interceptor, VersionInterceptor, VERSION_SIGNATORY_HEADER,
        };
        use tonic::service::Interceptor;

        let current = (Constants::SchemaVersion as i32).to_string();
        let expected = Box::leak(current.clone().into_boxed_str());
        let check = create_version_check_interceptor(VERSION_SIGNATORY_HEADER, expected);
        for version in ["1", "2"] {
            let request = VersionInterceptor::new(VERSION_SIGNATORY_HEADER, version)
                .call(tonic::Request::new(()))
                .unwrap();
            assert_eq!(
                check(request).unwrap_err().code(),
                tonic::Code::FailedPrecondition
            );
        }
        let request = VersionInterceptor::new(VERSION_SIGNATORY_HEADER, current)
            .call(tonic::Request::new(()))
            .unwrap();
        assert!(check(request).is_ok());
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn v3_rotation_through_rpc_handler_returns_bls_keyset() {
        use std::sync::Arc;

        use super::super::signatory_server::Signatory;
        use crate::db_signatory::DbSignatory;
        use crate::proto::server::CdkSignatoryServer;

        let store = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let signatory = Arc::new(
            DbSignatory::new(
                store,
                b"v3-rpc-rotation-test",
                Default::default(),
                Default::default(),
            )
            .await
            .unwrap(),
        );
        let server = CdkSignatoryServer::new(signatory);
        let request = RotationRequest {
            unit: Some(cdk_common::CurrencyUnit::Sat.into()),
            amounts: vec![1, 2, 4],
            input_fee_ppk: 100,
            keyset_id_type: KeysetVersion::V3 as i32,
            final_expiry: None,
        };
        let decoded = RotationRequest::decode(request.encode_to_vec().as_slice()).unwrap();
        let response = server
            .rotate_keyset(tonic::Request::new(decoded))
            .await
            .unwrap()
            .into_inner();
        let decoded = KeyRotationResponse::decode(response.encode_to_vec().as_slice()).unwrap();
        assert!(decoded.error.is_none());
        let keyset: crate::signatory::SignatoryKeySet = decoded.keyset.unwrap().try_into().unwrap();
        assert_eq!(keyset.id.get_version(), KeySetVersion::Version02);
        assert!(keyset
            .keys
            .values()
            .all(|key| matches!(key, MintPublicKey::Bls(_))));
        let public_keyset: cdk_common::KeySet = keyset.into();
        public_keyset.verify_id().unwrap();
    }

    #[test]
    fn custom_currency_unit_is_lowercase_across_proto_boundary() {
        let proto: CurrencyUnit = cdk_common::CurrencyUnit::Custom("BADCOIN".into()).into();

        assert_eq!(
            proto.currency_unit,
            Some(currency_unit::CurrencyUnit::CustomUnit(
                "badcoin".to_string()
            ))
        );

        let common: cdk_common::CurrencyUnit = CurrencyUnit {
            currency_unit: Some(currency_unit::CurrencyUnit::CustomUnit(
                "BADCOIN".to_string(),
            )),
        }
        .try_into()
        .unwrap();

        assert_eq!(common, cdk_common::CurrencyUnit::Custom("badcoin".into()));
    }
}

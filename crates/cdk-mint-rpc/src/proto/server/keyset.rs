//! Keyset administration service.

use std::str::FromStr;

use cdk::mint::MintKeySetInfo;
use cdk::nuts::nut02::KeySetVersion;
use cdk::nuts::CurrencyUnit;
use tonic::{Request, Response, Status};

use super::MintRPCServer;
use crate::keyset::keyset_service_server::KeysetService;
use crate::keyset::KeysetVersion;

impl MintRPCServer {
    /// Rotates to the next keyset for the given unit
    async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        amounts: Vec<u64>,
        input_fee_ppk: Option<u64>,
        keyset_version: KeySetVersion,
        final_expiry: Option<u64>,
    ) -> Result<MintKeySetInfo, Status> {
        self.ensure_mutation_allowed().await?;
        self.mint
            .rotate_keyset_by_version(
                unit,
                amounts,
                input_fee_ppk.unwrap_or(0),
                keyset_version,
                None,
                None,
                final_expiry,
            )
            .await
            .map_err(|_| Status::invalid_argument("Could not rotate keyset".to_string()))
    }
}

#[tonic::async_trait]
impl KeysetService for MintRPCServer {
    /// Rotates to the next keyset for the specified currency unit
    async fn rotate_next_keyset(
        &self,
        request: Request<crate::keyset::RotateNextKeysetRequest>,
    ) -> Result<Response<crate::keyset::RotateNextKeysetResponse>, Status> {
        let request = request.into_inner();

        let unit = CurrencyUnit::from_str(&request.unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let requested = request
            .keyset_version
            .map(|raw| match KeysetVersion::try_from(raw) {
                Ok(KeysetVersion::V1) => Ok(KeySetVersion::Version00),
                Ok(KeysetVersion::V2) => Ok(KeySetVersion::Version01),
                Ok(KeysetVersion::Unspecified) | Err(_) => Err(Status::invalid_argument(format!(
                    "Invalid keyset version: {raw}"
                ))),
            })
            .transpose()?;
        if let (Some(version), Some(use_v2)) = (requested, request.use_keyset_v2) {
            if (version == KeySetVersion::Version01) != use_v2 {
                return Err(Status::invalid_argument(
                    "keyset_version and use_keyset_v2 must select the same version",
                ));
            }
        }
        let keyset_version = requested.unwrap_or(match request.use_keyset_v2 {
            Some(false) => KeySetVersion::Version00,
            Some(true) | None => KeySetVersion::Version01,
        });

        let keyset_info = self
            .rotate_keyset(
                unit,
                request.amounts,
                request.input_fee_ppk,
                keyset_version,
                request.final_expiry,
            )
            .await?;

        Ok(Response::new(crate::keyset::RotateNextKeysetResponse {
            id: keyset_info.id.to_string(),
            unit: keyset_info.unit.to_string(),
            amounts: keyset_info.amounts,
            input_fee_ppk: keyset_info.input_fee_ppk,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::create_test_rpc_server;
    use super::*;

    #[tokio::test]
    async fn test_keyset_service_rotate_next_keyset() {
        let server = create_test_rpc_server().await;

        let response = KeysetService::rotate_next_keyset(
            &server,
            Request::new(crate::keyset::RotateNextKeysetRequest {
                unit: "sat".to_string(),
                amounts: vec![1, 2, 4, 8],
                input_fee_ppk: Some(1),
                use_keyset_v2: Some(true),
                keyset_version: None,
                final_expiry: None,
            }),
        )
        .await
        .unwrap();

        let response = response.into_inner();
        assert!(!response.id.is_empty());
        assert_eq!(response.unit, "sat");
        assert_eq!(response.amounts, vec![1, 2, 4, 8]);
        assert_eq!(response.input_fee_ppk, 1);
    }
    #[tokio::test]
    async fn test_keyset_version_selection_and_legacy_defaults() {
        let server = create_test_rpc_server().await;
        for (explicit, legacy, expected) in [
            (None, None, KeySetVersion::Version01),
            (None, Some(false), KeySetVersion::Version00),
            (None, Some(true), KeySetVersion::Version01),
            (Some(KeysetVersion::V1), None, KeySetVersion::Version00),
            (Some(KeysetVersion::V2), None, KeySetVersion::Version01),
            (
                Some(KeysetVersion::V1),
                Some(false),
                KeySetVersion::Version00,
            ),
            (
                Some(KeysetVersion::V2),
                Some(true),
                KeySetVersion::Version01,
            ),
        ] {
            let response = KeysetService::rotate_next_keyset(
                &server,
                Request::new(crate::keyset::RotateNextKeysetRequest {
                    unit: "sat".to_owned(),
                    amounts: vec![1, 2, 4, 8],
                    input_fee_ppk: Some(1),
                    use_keyset_v2: legacy,
                    keyset_version: explicit.map(i32::from),
                    final_expiry: None,
                }),
            )
            .await
            .unwrap()
            .into_inner();
            let id = cdk::nuts::Id::from_str(&response.id).unwrap();
            assert_eq!(id.get_version(), expected);
            assert!(server.mint.keyset(&id).is_some());
        }
    }

    #[tokio::test]
    async fn test_conflicting_keyset_versions_do_not_rotate() {
        let server = create_test_rpc_server().await;
        let before = server.mint.pubkeys();
        for (version, legacy) in [(KeysetVersion::V1, true), (KeysetVersion::V2, false)] {
            let error = KeysetService::rotate_next_keyset(
                &server,
                Request::new(crate::keyset::RotateNextKeysetRequest {
                    unit: "sat".to_owned(),
                    amounts: vec![1, 2, 4, 8],
                    input_fee_ppk: Some(0),
                    use_keyset_v2: Some(legacy),
                    keyset_version: Some(version.into()),
                    final_expiry: None,
                }),
            )
            .await
            .unwrap_err();
            assert_eq!(error.code(), tonic::Code::InvalidArgument);
            assert_eq!(
                error.message(),
                "keyset_version and use_keyset_v2 must select the same version"
            );
            assert_eq!(server.mint.pubkeys(), before);
        }
    }

    #[tokio::test]
    async fn test_invalid_keyset_version_does_not_rotate() {
        let server = create_test_rpc_server().await;
        let before = server.mint.pubkeys();
        for version in [KeysetVersion::Unspecified as i32, -1, 3, i32::MAX] {
            let error = KeysetService::rotate_next_keyset(
                &server,
                Request::new(crate::keyset::RotateNextKeysetRequest {
                    unit: "sat".to_owned(),
                    amounts: vec![1, 2, 4, 8],
                    input_fee_ppk: Some(0),
                    use_keyset_v2: Some(true),
                    keyset_version: Some(version),
                    final_expiry: None,
                }),
            )
            .await
            .unwrap_err();
            assert_eq!(error.code(), tonic::Code::InvalidArgument);
        }
        assert_eq!(server.mint.pubkeys(), before);
    }

    #[tokio::test]
    async fn test_legacy_mint_rotation_api_preserves_versions() {
        let server = create_test_rpc_server().await;
        for (legacy, expected) in [
            (false, KeySetVersion::Version00),
            (true, KeySetVersion::Version01),
        ] {
            let keyset = server
                .mint
                .rotate_keyset(
                    CurrencyUnit::Sat,
                    vec![1, 2, 4],
                    0,
                    legacy,
                    None,
                    None,
                    None,
                )
                .await
                .unwrap();
            assert_eq!(keyset.id.get_version(), expected);
            assert!(server.mint.keyset(&keyset.id).is_some());
        }
    }
}

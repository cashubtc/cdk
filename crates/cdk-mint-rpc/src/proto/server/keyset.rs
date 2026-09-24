//! Keyset administration service.

use std::str::FromStr;

use cdk::nuts::{CurrencyUnit, KeySetVersion};
use tonic::{Request, Response, Status};

use super::MintRPCServer;
use crate::keyset::keyset_service_server::KeysetService;

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

        self.ensure_mutation_allowed().await?;
        let amounts = request.amounts;
        let keyset_version = match request.keyset_version.as_deref() {
            Some("v1") | Some("00") => KeySetVersion::Version00,
            Some("v2") | Some("01") => KeySetVersion::Version01,
            Some("v3") | Some("02") => KeySetVersion::from_proto_i32(3)
                .map_err(|_| Status::invalid_argument("BLS keysets are not enabled"))?,
            Some(version) => {
                return Err(Status::invalid_argument(format!(
                    "Invalid keyset version: {version}"
                )));
            }
            None => match request.use_keyset_v2 {
                Some(true) => KeySetVersion::Version01,
                Some(false) => KeySetVersion::Version00,
                None => KeySetVersion::from_proto_i32(3).unwrap_or(KeySetVersion::Version01),
            },
        };

        let keyset_info = self
            .mint
            .rotate_keyset_by_version(
                unit,
                amounts,
                request.input_fee_ppk.unwrap_or(0),
                keyset_version,
                request.final_expiry,
            )
            .await
            .map_err(|_| Status::invalid_argument("Could not rotate keyset".to_string()))?;

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
                keyset_version: None,
                unit: "sat".to_string(),
                amounts: vec![1, 2, 4, 8],
                input_fee_ppk: Some(1),
                use_keyset_v2: Some(true),
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
}

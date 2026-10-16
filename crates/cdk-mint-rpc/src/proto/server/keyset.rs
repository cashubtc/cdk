//! Keyset administration service.

use std::str::FromStr;

use cdk::mint::MintKeySetInfo;
use cdk::nuts::CurrencyUnit;
use tonic::{Request, Response, Status};

use super::MintRPCServer;
use crate::keyset::keyset_service_server::KeysetService;

impl MintRPCServer {
    /// Rotates to the next keyset for the given unit
    async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        amounts: Vec<u64>,
        input_fee_ppk: Option<u64>,
        use_keyset_v2: Option<bool>,
        final_expiry: Option<u64>,
    ) -> Result<MintKeySetInfo, Status> {
        self.ensure_mutation_allowed().await?;
        self.mint
            .rotate_keyset(
                unit,
                amounts,
                input_fee_ppk.unwrap_or(0),
                use_keyset_v2.unwrap_or(true),
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

        let keyset_info = self
            .rotate_keyset(
                unit,
                request.amounts,
                request.input_fee_ppk,
                request.use_keyset_v2,
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

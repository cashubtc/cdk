//! Keyset administration service.

use std::str::FromStr;

use cdk::nuts::CurrencyUnit;
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
        let keyset_info = self
            .mint
            .rotate_keyset(
                unit,
                request.amounts,
                request.input_fee_ppk.unwrap_or(0),
                request.use_keyset_v2.unwrap_or(true),
                request.active_from,
                request.active_until,
                request.final_expiry,
            )
            .await
            .map_err(|err| Status::invalid_argument(format!("Could not rotate keyset: {err}")))?;

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
                active_from: None,
                active_until: Some(4_000_000_000),
            }),
        )
        .await
        .unwrap();

        let response = response.into_inner();
        assert!(!response.id.is_empty());
        assert_eq!(response.unit, "sat");
        assert_eq!(response.amounts, vec![1, 2, 4, 8]);
        assert_eq!(response.input_fee_ppk, 1);

        let keyset = server
            .mint
            .keysets()
            .keysets
            .into_iter()
            .find(|k| k.id.to_string() == response.id)
            .expect("rotated keyset is listed");
        assert_eq!(keyset.active_until, Some(4_000_000_000));
    }
}

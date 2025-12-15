use cdk::Amount;
use tonic::{Request, Response, Status};

use crate::cdk_mint_reporting_server::CdkMintReporting;
use crate::{
    ContactInfo, GetInfoRequest, GetInfoResponse, GetKeysetsRequest, GetKeysetsResponse, Keyset,
};

use super::server::MintRPCServer;

#[tonic::async_trait]
impl CdkMintReporting for MintRPCServer {
    /// Returns information about the mint
    async fn get_info(
        &self,
        _request: Request<GetInfoRequest>,
    ) -> Result<Response<GetInfoResponse>, Status> {
        let info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let total_issued = self
            .mint()
            .total_issued()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let total_issued: Amount = Amount::try_sum(total_issued.values().cloned())
            .map_err(|_| Status::internal("Overflow".to_string()))?;

        let total_redeemed = self
            .mint()
            .total_redeemed()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let total_redeemed: Amount = Amount::try_sum(total_redeemed.values().cloned())
            .map_err(|_| Status::internal("Overflow".to_string()))?;

        let contact = info
            .contact
            .unwrap_or_default()
            .into_iter()
            .map(|c| ContactInfo {
                method: c.method,
                info: c.info,
            })
            .collect();

        Ok(Response::new(GetInfoResponse {
            name: info.name,
            description: info.description,
            long_description: info.description_long,
            version: info.version.map(|v| v.to_string()),
            contact,
            motd: info.motd,
            icon_url: info.icon_url,
            urls: info.urls.unwrap_or_default(),
            tos_url: info.tos_url,
            total_issued: total_issued.into(),
            total_redeemed: total_redeemed.into(),
        }))
    }

    /// Returns keysets from the mint
    async fn get_keysets(
        &self,
        request: Request<GetKeysetsRequest>,
    ) -> Result<Response<GetKeysetsResponse>, Status> {
        let request = request.into_inner();

        let keyset_response = self.mint().keysets();

        let keysets: Vec<Keyset> = keyset_response
            .keysets
            .into_iter()
            .filter(|ks| {
                // Filter by units if specified
                if !request.units.is_empty() && !request.units.contains(&ks.unit.to_string()) {
                    return false;
                }
                // Filter by active if specified
                if let Some(active_only) = request.active_only {
                    if active_only && !ks.active {
                        return false;
                    }
                }
                true
            })
            .map(|ks| Keyset {
                id: ks.id.to_string(),
                unit: ks.unit.to_string(),
                active: ks.active,
            })
            .collect();

        Ok(Response::new(GetKeysetsResponse { keysets }))
    }
}

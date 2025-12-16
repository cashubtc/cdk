use std::collections::HashSet;
use std::str::FromStr;

use cdk::nuts::CurrencyUnit;
use cdk::Amount;
use tonic::{Request, Response, Status};

use crate::cdk_mint_reporting_server::CdkMintReporting;
use crate::{
    Balance, ContactInfo, GetBalancesRequest, GetBalancesResponse, GetInfoRequest, GetInfoResponse,
    GetKeysetsRequest, GetKeysetsResponse, Keyset,
};

use super::helpers::get_balances_by_unit;
use super::server::MintRPCServer;

#[tonic::async_trait]
impl CdkMintReporting for MintRPCServer {
    /// Returns the net balance (issued - redeemed) per unit
    async fn get_balances(
        &self,
        request: Request<GetBalancesRequest>,
    ) -> Result<Response<GetBalancesResponse>, Status> {
        let unit_filter = request
            .into_inner()
            .unit
            .map(|u| CurrencyUnit::from_str(&u))
            .transpose()
            .map_err(|_| Status::invalid_argument("Invalid unit"))?;

        let balances_data = get_balances_by_unit(&self.mint()).await?;

        // Collect all units
        let all_units: HashSet<_> = balances_data
            .issued
            .keys()
            .chain(balances_data.redeemed.keys())
            .chain(balances_data.fees.keys())
            .cloned()
            .collect();

        let balances = all_units
            .into_iter()
            .filter(|unit| unit_filter.as_ref().is_none_or(|f| f == unit))
            .map(|unit| {
                let issued = balances_data
                    .issued
                    .get(&unit)
                    .copied()
                    .unwrap_or(Amount::ZERO);
                let redeemed = balances_data
                    .redeemed
                    .get(&unit)
                    .copied()
                    .unwrap_or(Amount::ZERO);
                let fees = balances_data
                    .fees
                    .get(&unit)
                    .copied()
                    .unwrap_or(Amount::ZERO);

                Balance {
                    unit: unit.to_string(),
                    total_balance: issued.checked_sub(redeemed).unwrap_or(Amount::ZERO).into(),
                    total_issued: issued.into(),
                    total_redeemed: redeemed.into(),
                    total_fees_collected: fees.into(),
                }
            })
            .collect();

        Ok(Response::new(GetBalancesResponse { balances }))
    }

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
        }))
    }

    /// Returns keysets from the mint
    async fn get_keysets(
        &self,
        request: Request<GetKeysetsRequest>,
    ) -> Result<Response<GetKeysetsResponse>, Status> {
        let request = request.into_inner();
        let mint = self.mint();

        // Get all keyset infos from in-memory cache
        let all_keyset_infos = mint.keyset_infos();

        // Only fetch balance data if requested (avoids unnecessary DB calls)
        let balances = if request.include_balances.unwrap_or(false) {
            Some(super::helpers::MintBalances::fetch(&mint).await?)
        } else {
            None
        };

        // Filter and map keysets to proto response
        let keysets: Vec<Keyset> = all_keyset_infos
            .into_iter()
            .filter(|info| {
                // Filter auth keysets based on include_auth flag
                if info.unit == cdk::nuts::CurrencyUnit::Auth {
                    return request.include_auth.unwrap_or(false);
                }
                // Filter by units if specified
                if !request.units.is_empty() && !request.units.contains(&info.unit.to_string()) {
                    return false;
                }
                // Filter inactive if exclude_inactive is true
                if request.exclude_inactive.unwrap_or(false) && !info.active {
                    return false;
                }
                true
            })
            .map(|info| {
                // Get stats for this keyset only if balances were requested
                let (total_balance, total_issued, total_redeemed, total_fees_collected) =
                    if let Some(ref b) = balances {
                        let stats = b.get_keyset_stats(&info.id);
                        (
                            Some(stats.total_balance().into()),
                            Some(stats.total_issued.into()),
                            Some(stats.total_redeemed.into()),
                            Some(stats.total_fees_collected.into()),
                        )
                    } else {
                        (None, None, None, None)
                    };

                Keyset {
                    id: info.id.to_string(),
                    unit: info.unit.to_string(),
                    active: info.active,
                    valid_from: info.valid_from,
                    valid_to: info.final_expiry.unwrap_or(0),
                    input_fee_ppk: info.input_fee_ppk,
                    derivation_path_index: info
                        .derivation_path_index
                        .map(|idx| idx.to_string())
                        .unwrap_or_default(),
                    amounts: info.amounts,
                    total_balance,
                    total_issued,
                    total_redeemed,
                    total_fees_collected,
                }
            })
            .collect();

        Ok(Response::new(GetKeysetsResponse { keysets }))
    }
}

use std::collections::HashSet;
use std::str::FromStr;

use cdk::nuts::CurrencyUnit;
use cdk::Amount;
use tonic::{Request, Response, Status};

use crate::cdk_mint_reporting_server::CdkMintReporting;
use crate::{
    Balance, ContactInfo, GetBalancesRequest, GetBalancesResponse, GetInfoRequest, GetInfoResponse,
    GetKeysetsRequest, GetKeysetsResponse, Keyset, ListMintQuotesRequest, ListMintQuotesResponse,
    LookupMintQuoteRequest, LookupMintQuoteResponse,
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

    /// Lists mint quotes with optional filtering and pagination
    async fn list_mint_quotes(
        &self,
        request: Request<ListMintQuotesRequest>,
    ) -> Result<Response<ListMintQuotesResponse>, Status> {
        let request = request.into_inner();
        let mint = self.mint();

        // Get all mint quotes from database
        let all_quotes = mint
            .mint_quotes()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Apply filters
        let mut filtered_quotes: Vec<_> = all_quotes
            .into_iter()
            .filter(|quote| {
                // Filter by creation date range
                if let Some(start) = request.creation_date_start {
                    if (quote.created_time as i64) < start {
                        return false;
                    }
                }
                if let Some(end) = request.creation_date_end {
                    if (quote.created_time as i64) > end {
                        return false;
                    }
                }
                // Filter by states
                if !request.states.is_empty() {
                    let quote_state = quote.state().to_string();
                    if !request.states.contains(&quote_state) {
                        return false;
                    }
                }
                // Filter by units
                if !request.units.is_empty() && !request.units.contains(&quote.unit.to_string()) {
                    return false;
                }
                true
            })
            .collect();

        // Sort by created_time (for pagination)
        if request.reversed {
            filtered_quotes.sort_by(|a, b| b.created_time.cmp(&a.created_time));
        } else {
            filtered_quotes.sort_by(|a, b| a.created_time.cmp(&b.created_time));
        }

        // Apply pagination
        let total_count = filtered_quotes.len();
        let start_index = request.index_offset.max(0) as usize;
        let max_quotes = if request.num_max_quotes > 0 {
            request.num_max_quotes as usize
        } else {
            usize::MAX
        };

        let paginated_quotes: Vec<_> = filtered_quotes
            .into_iter()
            .skip(start_index)
            .take(max_quotes)
            .collect();

        // Calculate response offsets
        let first_index_offset = start_index as i64;
        let last_index_offset = (start_index + paginated_quotes.len()) as i64;

        // Convert to proto
        let quotes = paginated_quotes
            .iter()
            .map(super::helpers::mint_quote_to_proto)
            .collect();

        Ok(Response::new(ListMintQuotesResponse {
            quotes,
            first_index_offset,
            last_index_offset,
        }))
    }

    /// Looks up a specific mint quote by ID
    async fn lookup_mint_quote(
        &self,
        request: Request<LookupMintQuoteRequest>,
    ) -> Result<Response<LookupMintQuoteResponse>, Status> {
        let quote_id = request.into_inner().quote_id;
        let mint = self.mint();

        // Parse the quote ID
        let quote_id = quote_id
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid quote ID format"))?;

        // Get the quote from database
        let quote = mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found("Quote not found"))?;

        // Convert to proto
        let proto_quote = super::helpers::mint_quote_to_proto(&quote);

        Ok(Response::new(LookupMintQuoteResponse {
            quote: Some(proto_quote),
        }))
    }
}

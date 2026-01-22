use std::str::FromStr;

use cdk::cdk_database::{
    BlindSignatureFilter, MeltQuoteFilter, MintQuoteFilter, OperationFilter, ProofFilter,
};
use cdk::nuts::CurrencyUnit;
use tonic::{Request, Response, Status};

use crate::cdk_mint_reporting_server::CdkMintReporting;
use crate::{
    BlindSignature, ContactInfo, GetBalancesRequest, GetBalancesResponse, GetInfoRequest,
    GetInfoResponse, GetKeysetsRequest, GetKeysetsResponse, Keyset, ListBlindSignaturesRequest,
    ListBlindSignaturesResponse, ListMeltQuotesResponse, ListMintQuotesResponse,
    ListOperationsRequest, ListOperationsResponse, ListProofsRequest, ListProofsResponse,
    ListQuotesRequest, LookupMeltQuoteResponse, LookupMintQuoteResponse, LookupQuoteRequest,
    MintQuoteDetail, MintQuoteIssuance, MintQuotePayment, MintQuoteSummary, Operations, Proof,
};

use super::server::MintRPCServer;
use super::utils::{
    effective_limit, melt_quote_to_proto, parse_keyset_ids, parse_melt_quote_states,
    parse_mint_quote_states, parse_proof_states, validate_operations, validate_pagination,
    validate_units_against_mint, MintBalances, ValidateUnitsResult,
};

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

        let balances = MintBalances::fetch(&self.mint())
            .await?
            .aggregate_by_unit()
            .ok_or_else(|| Status::internal("Overflow during balance aggregation"))?
            .to_balances(unit_filter.as_ref());

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
    ///
    /// Only fetch balance data if requested (avoids unnecessary DB calls)
    /// Filter and map keysets to proto response
    async fn get_keysets(
        &self,
        request: Request<GetKeysetsRequest>,
    ) -> Result<Response<GetKeysetsResponse>, Status> {
        let request = request.into_inner();
        let mint = self.mint();
        let all_keyset_infos = mint.keyset_infos();

        let balances = if request.include_balances.unwrap_or(false) {
            Some(MintBalances::fetch(&mint).await?)
        } else {
            None
        };

        let keysets: Vec<Keyset> = all_keyset_infos
            .into_iter()
            .filter(|info| {
                if info.unit == cdk::nuts::CurrencyUnit::Auth {
                    return request.include_auth.unwrap_or(false);
                }
                if !request.units.is_empty() && !request.units.contains(&info.unit.to_string()) {
                    return false;
                }
                if request.exclude_inactive.unwrap_or(false) && !info.active {
                    return false;
                }
                true
            })
            .map(|info| {
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
                    valid_from: match info.valid_from {
                        0 => None,
                        v => Some(v),
                    },
                    valid_to: info.final_expiry,
                    input_fee_ppk: info.input_fee_ppk,
                    derivation_path_index: info.derivation_path_index.map(|idx| idx.to_string()),
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
    ///
    /// Filtering is performed at the SQL level for efficiency.
    async fn list_mint_quotes(
        &self,
        request: Request<ListQuotesRequest>,
    ) -> Result<Response<ListMintQuotesResponse>, Status> {
        let request = request.into_inner();
        let mint = self.mint();
        let (states, invalid_states) = parse_mint_quote_states(&request.states);
        if !invalid_states.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid mint quote state(s): {}. Valid states: unpaid, paid, issued, pending",
                invalid_states.join(", ")
            )));
        }
        let ValidateUnitsResult {
            parsed: units,
            invalid: invalid_units,
            valid_units,
        } = validate_units_against_mint(&request.units, &mint);
        if !invalid_units.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid unit(s): {}. Valid units for this mint: {}",
                invalid_units.join(", "),
                valid_units.join(", ")
            )));
        }

        if !validate_pagination(request.index_offset, request.num_max_quotes) {
            return Err(Status::invalid_argument(
                "num_max_quotes is required when index_offset is provided",
            ));
        }

        let start_index = request.index_offset.max(0) as u64;
        let filter = MintQuoteFilter {
            creation_date_start: request.creation_date_start.map(|t| t as u64),
            creation_date_end: request.creation_date_end.map(|t| t as u64),
            states,
            units,
            limit: Some(effective_limit(request.num_max_quotes)),
            offset: start_index,
            reversed: request.reversed,
        };

        let result = mint
            .list_mint_quotes_filtered(filter)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let quotes = result
            .quotes
            .iter()
            .map(|quote| MintQuoteSummary {
                id: quote.id.to_string(),
                amount: quote.amount.as_ref().map(|a| a.value()),
                unit: quote.unit.to_string(),
                request: quote.request.clone(),
                state: quote.state().to_string(),
                request_lookup_id: Some(quote.request_lookup_id.to_string()),
                request_lookup_id_kind: quote.request_lookup_id.kind().to_string(),
                pubkey: quote.pubkey.as_ref().map(|pk| pk.to_string()),
                created_time: quote.created_time,
                amount_paid: quote.amount_paid().value(),
                amount_issued: quote.amount_issued().value(),
                payment_method: quote.payment_method.to_string(),
            })
            .collect();

        Ok(Response::new(ListMintQuotesResponse {
            quotes,
            has_more: result.has_more,
        }))
    }

    /// Looks up a specific mint quote by ID
    ///
    /// Returns the detailed version with paid_time and issued_time.
    async fn lookup_mint_quote(
        &self,
        request: Request<LookupQuoteRequest>,
    ) -> Result<Response<LookupMintQuoteResponse>, Status> {
        let quote_id = request.into_inner().quote_id;
        let mint = self.mint();

        let quote_id = quote_id
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid quote ID format"))?;

        let quote = mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found("Quote not found"))?;

        let payments: Vec<MintQuotePayment> = quote
            .payments
            .iter()
            .map(|p| MintQuotePayment {
                amount: p.amount.value(),
                time: p.time,
                payment_id: p.payment_id.clone(),
            })
            .collect();

        let issuances: Vec<MintQuoteIssuance> = quote
            .issuance
            .iter()
            .map(|i| MintQuoteIssuance {
                amount: i.amount.value(),
                time: i.time,
            })
            .collect();

        let proto_quote = MintQuoteDetail {
            id: quote.id.to_string(),
            amount: quote.amount.as_ref().map(|a| a.value()),
            unit: quote.unit.to_string(),
            request: quote.request.clone(),
            state: quote.state().to_string(),
            request_lookup_id: Some(quote.request_lookup_id.to_string()),
            request_lookup_id_kind: quote.request_lookup_id.kind().to_string(),
            pubkey: quote.pubkey.as_ref().map(|pk| pk.to_string()),
            created_time: quote.created_time,
            payments,
            issuances,
            amount_paid: quote.amount_paid().value(),
            amount_issued: quote.amount_issued().value(),
            payment_method: quote.payment_method.to_string(),
        };

        Ok(Response::new(LookupMintQuoteResponse {
            quote: Some(proto_quote),
        }))
    }

    /// Lists melt quotes with optional filtering and pagination
    ///
    /// Filtering is performed at the SQL level for efficiency.
    async fn list_melt_quotes(
        &self,
        request: Request<ListQuotesRequest>,
    ) -> Result<Response<ListMeltQuotesResponse>, Status> {
        let request = request.into_inner();
        let mint = self.mint();
        let (states, invalid_states) = parse_melt_quote_states(&request.states);
        if !invalid_states.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid melt quote state(s): {}. Valid states: unpaid, pending, paid, unknown",
                invalid_states.join(", ")
            )));
        }
        let ValidateUnitsResult {
            parsed: units,
            invalid: invalid_units,
            valid_units,
        } = validate_units_against_mint(&request.units, &mint);
        if !invalid_units.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid unit(s): {}. Valid units for this mint: {}",
                invalid_units.join(", "),
                valid_units.join(", ")
            )));
        }

        if !validate_pagination(request.index_offset, request.num_max_quotes) {
            return Err(Status::invalid_argument(
                "num_max_quotes is required when index_offset is provided",
            ));
        }

        let start_index = request.index_offset.max(0) as u64;
        let filter = MeltQuoteFilter {
            creation_date_start: request.creation_date_start.map(|t| t as u64),
            creation_date_end: request.creation_date_end.map(|t| t as u64),
            states,
            units,
            limit: Some(effective_limit(request.num_max_quotes)),
            offset: start_index,
            reversed: request.reversed,
        };

        let result = mint
            .list_melt_quotes_filtered(filter)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let quotes = result.quotes.iter().map(melt_quote_to_proto).collect();

        Ok(Response::new(ListMeltQuotesResponse {
            quotes,
            has_more: result.has_more,
        }))
    }

    /// Looks up a specific melt quote by ID
    async fn lookup_melt_quote(
        &self,
        request: Request<LookupQuoteRequest>,
    ) -> Result<Response<LookupMeltQuoteResponse>, Status> {
        let quote_id = request.into_inner().quote_id;
        let mint = self.mint();

        let quote_id = quote_id
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid quote ID format"))?;

        let quote = mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found("Quote not found"))?;

        let proto_quote = melt_quote_to_proto(&quote);

        Ok(Response::new(LookupMeltQuoteResponse {
            quote: Some(proto_quote),
        }))
    }

    /// Lists proofs with optional filtering and pagination
    ///
    /// Filtering is performed at the SQL level for efficiency.
    async fn list_proofs(
        &self,
        request: Request<ListProofsRequest>,
    ) -> Result<Response<ListProofsResponse>, Status> {
        let request = request.into_inner();
        let mint = self.mint();
        let (states, invalid_states) = parse_proof_states(&request.states);
        if !invalid_states.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid proof state(s): {}. Valid states: unspent, spent, pending, reserved",
                invalid_states.join(", ")
            )));
        }
        let ValidateUnitsResult {
            parsed: units,
            invalid: invalid_units,
            valid_units,
        } = validate_units_against_mint(&request.units, &mint);
        if !invalid_units.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid unit(s): {}. Valid units for this mint: {}",
                invalid_units.join(", "),
                valid_units.join(", ")
            )));
        }
        let (keyset_ids, invalid_keysets) = parse_keyset_ids(&request.keyset_ids);
        if !invalid_keysets.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid keyset ID(s): {}",
                invalid_keysets.join(", ")
            )));
        }
        let (operations, invalid_ops) = validate_operations(&request.operations);
        if !invalid_ops.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid operation(s): {}. Valid operations: mint, melt, swap",
                invalid_ops.join(", ")
            )));
        }

        if !validate_pagination(request.index_offset, request.num_max_proofs) {
            return Err(Status::invalid_argument(
                "num_max_proofs is required when index_offset is provided",
            ));
        }

        let start_index = request.index_offset.max(0) as u64;
        let filter = ProofFilter {
            creation_date_start: request.creation_date_start.map(|t| t as u64),
            creation_date_end: request.creation_date_end.map(|t| t as u64),
            states,
            units,
            keyset_ids,
            operations,
            limit: Some(effective_limit(request.num_max_proofs)),
            offset: start_index,
            reversed: request.reversed,
        };

        let result = mint
            .localstore()
            .list_proofs_filtered(filter)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let proofs = result
            .proofs
            .iter()
            .map(|proof| Proof {
                amount: proof.amount.into(),
                keyset_id: proof.keyset_id.to_string(),
                state: proof.state.to_string(),
                quote_id: proof.quote_id.clone(),
                created_time: proof.created_time,
                operation_kind: proof.operation_kind.clone().unwrap_or_default(),
                operation_id: proof.operation_id.clone().unwrap_or_default(),
            })
            .collect();

        Ok(Response::new(ListProofsResponse {
            proofs,
            has_more: result.has_more,
        }))
    }

    /// Lists blind signatures with optional filtering and pagination
    ///
    /// Filtering is performed at the SQL level for efficiency.
    async fn list_blind_signatures(
        &self,
        request: Request<ListBlindSignaturesRequest>,
    ) -> Result<Response<ListBlindSignaturesResponse>, Status> {
        let request = request.into_inner();
        let mint = self.mint();
        let ValidateUnitsResult {
            parsed: units,
            invalid: invalid_units,
            valid_units,
        } = validate_units_against_mint(&request.units, &mint);
        if !invalid_units.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid unit(s): {}. Valid units for this mint: {}",
                invalid_units.join(", "),
                valid_units.join(", ")
            )));
        }
        let (keyset_ids, invalid_keysets) = parse_keyset_ids(&request.keyset_ids);
        if !invalid_keysets.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid keyset ID(s): {}",
                invalid_keysets.join(", ")
            )));
        }
        let (operations, invalid_ops) = validate_operations(&request.operations);
        if !invalid_ops.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid operation(s): {}. Valid operations: mint, melt, swap",
                invalid_ops.join(", ")
            )));
        }

        if !validate_pagination(request.index_offset, request.num_max_signatures) {
            return Err(Status::invalid_argument(
                "num_max_signatures is required when index_offset is provided",
            ));
        }

        let start_index = request.index_offset.max(0) as u64;
        let filter = BlindSignatureFilter {
            creation_date_start: request.creation_date_start.map(|t| t as u64),
            creation_date_end: request.creation_date_end.map(|t| t as u64),
            units,
            keyset_ids,
            operations,
            limit: Some(effective_limit(request.num_max_signatures)),
            offset: start_index,
            reversed: request.reversed,
        };

        let result = mint
            .localstore()
            .list_blind_signatures_filtered(filter)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let signatures = result
            .signatures
            .iter()
            .map(|sig| BlindSignature {
                amount: sig.amount.into(),
                keyset_id: sig.keyset_id.to_string(),
                quote_id: sig.quote_id.clone(),
                created_time: sig.created_time,
                signed_time: sig.signed_time,
                operation_kind: sig.operation_kind.clone().unwrap_or_default(),
                operation_id: sig.operation_id.clone().unwrap_or_default(),
            })
            .collect();

        Ok(Response::new(ListBlindSignaturesResponse {
            signatures,
            has_more: result.has_more,
        }))
    }

    /// Lists completed operations with optional filtering and pagination
    ///
    /// Unit is derived via JOIN through proof → keyset tables.
    async fn list_operations(
        &self,
        request: Request<ListOperationsRequest>,
    ) -> Result<Response<ListOperationsResponse>, Status> {
        let request = request.into_inner();
        let mint = self.mint();
        let ValidateUnitsResult {
            parsed: units,
            invalid: invalid_units,
            valid_units,
        } = validate_units_against_mint(&request.units, &mint);
        if !invalid_units.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid unit(s): {}. Valid units for this mint: {}",
                invalid_units.join(", "),
                valid_units.join(", ")
            )));
        }
        let (operations, invalid_ops) = validate_operations(&request.operations);
        if !invalid_ops.is_empty() {
            return Err(Status::invalid_argument(format!(
                "Invalid operation(s): {}. Valid operations: mint, melt, swap",
                invalid_ops.join(", ")
            )));
        }

        if !validate_pagination(request.index_offset, request.num_max_operations) {
            return Err(Status::invalid_argument(
                "num_max_operations is required when index_offset is provided",
            ));
        }

        let start_index = request.index_offset.max(0) as u64;
        let filter = OperationFilter {
            creation_date_start: request.creation_date_start.map(|t| t as u64),
            creation_date_end: request.creation_date_end.map(|t| t as u64),
            units,
            operations,
            limit: Some(effective_limit(request.num_max_operations)),
            offset: start_index,
            reversed: request.reversed,
        };

        let result = mint
            .localstore()
            .list_operations_filtered(filter)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let operations = result
            .operations
            .iter()
            .map(|op| Operations {
                operation_id: op.operation_id.clone(),
                operation_kind: op.operation_kind.clone(),
                completed_time: op.completed_time,
                total_issued: op.total_issued.into(),
                total_redeemed: op.total_redeemed.into(),
                fee_collected: op.fee_collected.into(),
                payment_amount: op.payment_amount.map(|a| a.into()),
                payment_fee: op.payment_fee.map(|a| a.into()),
                payment_method: op.payment_method.clone(),
                unit: op.unit.clone().unwrap_or_default(),
            })
            .collect();

        Ok(Response::new(ListOperationsResponse {
            operations,
            has_more: result.has_more,
        }))
    }
}

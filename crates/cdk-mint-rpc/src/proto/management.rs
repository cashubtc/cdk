use std::str::FromStr;

use cdk::mint::MintQuote;
use cdk::nuts::nut04::MintMethodSettings;
use cdk::nuts::nut05::MeltMethodSettings;
use cdk::nuts::{CurrencyUnit, MintQuoteState, PaymentMethod};
use cdk::types::QuoteTTL;
use cdk::Amount;
use cdk_common::payment::WaitPaymentResponse;
use tonic::{Request, Response, Status};

use crate::cdk_mint_management_server::CdkMintManagement;
use crate::{
    GetQuoteTtlRequest, GetQuoteTtlResponse, RotateNextKeysetRequest, RotateNextKeysetResponse,
    UpdateContactRequest, UpdateDescriptionRequest, UpdateIconUrlRequest, UpdateMotdRequest,
    UpdateNameRequest, UpdateNut04QuoteRequest, UpdateNut04Request, UpdateNut05Request,
    UpdateQuoteTtlRequest, UpdateResponse, UpdateTosUrlRequest, UpdateUrlRequest,
};

use super::server::MintRPCServer;

#[tonic::async_trait]
impl CdkMintManagement for MintRPCServer {
    /// Updates the mint's message of the day
    async fn update_motd(
        &self,
        request: Request<UpdateMotdRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let motd = request.into_inner().motd;
        let mut info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        info.motd = Some(motd);

        self.mint()
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's short description
    async fn update_short_description(
        &self,
        request: Request<UpdateDescriptionRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let description = request.into_inner().description;
        let mut info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        info.description = Some(description);

        self.mint()
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's long description
    async fn update_long_description(
        &self,
        request: Request<UpdateDescriptionRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let description = request.into_inner().description;
        let mut info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        info.description_long = Some(description);

        self.mint()
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's name
    async fn update_name(
        &self,
        request: Request<UpdateNameRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let name = request.into_inner().name;
        let mut info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        info.name = Some(name);

        self.mint()
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's icon URL
    async fn update_icon_url(
        &self,
        request: Request<UpdateIconUrlRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let icon_url = request.into_inner().icon_url;

        let mut info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        info.icon_url = Some(icon_url);

        self.mint()
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's terms of service URL
    async fn update_tos_url(
        &self,
        request: Request<UpdateTosUrlRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let tos_url = request.into_inner().tos_url;

        let mut info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        info.tos_url = Some(tos_url);

        self.mint()
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Adds a URL to the mint's list of URLs
    async fn add_url(
        &self,
        request: Request<UpdateUrlRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let url = request.into_inner().url;
        let mut info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        let mut urls = info.urls.unwrap_or_default();
        urls.push(url);

        info.urls = Some(urls.clone());

        self.mint()
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Removes a URL from the mint's list of URLs
    async fn remove_url(
        &self,
        request: Request<UpdateUrlRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let url = request.into_inner().url;
        let mut info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        let urls = info.urls;
        let mut urls = urls.clone().unwrap_or_default();

        urls.retain(|u| u != &url);

        let urls = if urls.is_empty() { None } else { Some(urls) };

        info.urls = urls;

        self.mint()
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }

    /// Adds a contact method to the mint's contact information
    async fn add_contact(
        &self,
        request: Request<UpdateContactRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let request_inner = request.into_inner();
        let mut info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        info.contact
            .get_or_insert_with(Vec::new)
            .push(cdk::nuts::ContactInfo::new(
                request_inner.method,
                request_inner.info,
            ));

        self.mint()
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        Ok(Response::new(UpdateResponse {}))
    }
    /// Removes a contact method from the mint's contact information
    async fn remove_contact(
        &self,
        request: Request<UpdateContactRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let request_inner = request.into_inner();
        let mut info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        if let Some(contact) = info.contact.as_mut() {
            let contact_info =
                cdk::nuts::ContactInfo::new(request_inner.method, request_inner.info);
            contact.retain(|x| x != &contact_info);

            self.mint()
                .set_mint_info(info)
                .await
                .map_err(|err| Status::internal(err.to_string()))?;
        }
        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's NUT-04 (mint) settings
    async fn update_nut04(
        &self,
        request: Request<UpdateNut04Request>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let mut info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let mut nut04_settings = info.nuts.nut04.clone();

        let request_inner = request.into_inner();

        let unit = CurrencyUnit::from_str(&request_inner.unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let payment_method = PaymentMethod::from_str(&request_inner.method)
            .map_err(|_| Status::invalid_argument("Invalid method".to_string()))?;

        self.mint()
            .get_payment_processor(unit.clone(), payment_method.clone())
            .map_err(|_| Status::invalid_argument("Unit payment method pair is not supported"))?;

        let current_nut04_settings = nut04_settings.remove_settings(&unit, &payment_method);

        let mut methods = nut04_settings.methods.clone();

        // Create options from the request
        let options = if let Some(options) = request_inner.options {
            Some(cdk::nuts::nut04::MintMethodOptions::Bolt11 {
                description: options.description,
            })
        } else if let Some(current_settings) = current_nut04_settings.as_ref() {
            current_settings.options.clone()
        } else {
            None
        };

        let updated_method_settings = MintMethodSettings {
            method: payment_method,
            unit,
            min_amount: request_inner
                .min_amount
                .map(Amount::from)
                .or_else(|| current_nut04_settings.as_ref().and_then(|s| s.min_amount)),
            max_amount: request_inner
                .max_amount
                .map(Amount::from)
                .or_else(|| current_nut04_settings.as_ref().and_then(|s| s.max_amount)),
            options,
        };

        methods.push(updated_method_settings);

        nut04_settings.methods = methods;

        if let Some(disabled) = request_inner.disabled {
            nut04_settings.disabled = disabled;
        }

        info.nuts.nut04 = nut04_settings;

        self.mint()
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's NUT-05 (melt) settings
    async fn update_nut05(
        &self,
        request: Request<UpdateNut05Request>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let mut info = self
            .mint()
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        let mut nut05_settings = info.nuts.nut05.clone();

        let request_inner = request.into_inner();

        let unit = CurrencyUnit::from_str(&request_inner.unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let payment_method = PaymentMethod::from_str(&request_inner.method)
            .map_err(|_| Status::invalid_argument("Invalid method".to_string()))?;

        self.mint()
            .get_payment_processor(unit.clone(), payment_method.clone())
            .map_err(|_| Status::invalid_argument("Unit payment method pair is not supported"))?;

        let current_nut05_settings = nut05_settings.remove_settings(&unit, &payment_method);

        let mut methods = nut05_settings.methods;

        // Create options from the request
        let options = if let Some(options) = request_inner.options {
            Some(cdk::nuts::nut05::MeltMethodOptions::Bolt11 {
                amountless: options.amountless,
            })
        } else if let Some(current_settings) = current_nut05_settings.as_ref() {
            current_settings.options.clone()
        } else {
            None
        };

        let updated_method_settings = MeltMethodSettings {
            method: payment_method,
            unit,
            min_amount: request_inner
                .min_amount
                .map(Amount::from)
                .or_else(|| current_nut05_settings.as_ref().and_then(|s| s.min_amount)),
            max_amount: request_inner
                .max_amount
                .map(Amount::from)
                .or_else(|| current_nut05_settings.as_ref().and_then(|s| s.max_amount)),
            options,
        };

        methods.push(updated_method_settings);
        nut05_settings.methods = methods;

        if let Some(disabled) = request_inner.disabled {
            nut05_settings.disabled = disabled;
        }

        info.nuts.nut05 = nut05_settings;

        self.mint()
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(UpdateResponse {}))
    }

    /// Updates the mint's quote time-to-live settings
    async fn update_quote_ttl(
        &self,
        request: Request<UpdateQuoteTtlRequest>,
    ) -> Result<Response<UpdateResponse>, Status> {
        let current_ttl = self
            .mint()
            .quote_ttl()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let request = request.into_inner();

        let quote_ttl = QuoteTTL {
            mint_ttl: request.mint_ttl.unwrap_or(current_ttl.mint_ttl),
            melt_ttl: request.melt_ttl.unwrap_or(current_ttl.melt_ttl),
        };

        self.mint()
            .set_quote_ttl(quote_ttl)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(UpdateResponse {}))
    }

    /// Gets the mint's quote time-to-live settings
    async fn get_quote_ttl(
        &self,
        _request: Request<GetQuoteTtlRequest>,
    ) -> Result<Response<GetQuoteTtlResponse>, Status> {
        let ttl = self
            .mint()
            .quote_ttl()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(GetQuoteTtlResponse {
            mint_ttl: ttl.mint_ttl,
            melt_ttl: ttl.melt_ttl,
        }))
    }

    /// Updates a specific NUT-04 quote's state
    async fn update_nut04_quote(
        &self,
        request: Request<UpdateNut04QuoteRequest>,
    ) -> Result<Response<UpdateNut04QuoteRequest>, Status> {
        let request = request.into_inner();
        let quote_id = request
            .quote_id
            .parse()
            .map_err(|_| Status::invalid_argument("Invalid quote id".to_string()))?;

        let state = MintQuoteState::from_str(&request.state)
            .map_err(|_| Status::invalid_argument("Invalid quote state".to_string()))?;

        let mint_quote = self
            .mint()
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .map_err(|_| Status::invalid_argument("Could not find quote".to_string()))?
            .ok_or(Status::invalid_argument("Could not find quote".to_string()))?;

        match state {
            MintQuoteState::Paid => {
                // Create a dummy payment response
                let response = WaitPaymentResponse {
                    payment_id: String::new(),
                    payment_amount: mint_quote.amount_paid(),
                    unit: mint_quote.unit.clone(),
                    payment_identifier: mint_quote.request_lookup_id.clone(),
                };

                let localstore = self.mint().localstore();
                let mut tx = localstore
                    .begin_transaction()
                    .await
                    .map_err(|_| Status::internal("Could not start db transaction".to_string()))?;

                self.mint()
                    .pay_mint_quote(&mut tx, &mint_quote, response)
                    .await
                    .map_err(|_| Status::internal("Could not process payment".to_string()))?;

                tx.commit()
                    .await
                    .map_err(|_| Status::internal("Could not commit db transaction".to_string()))?;
            }
            _ => {
                // Create a new quote with the same values
                let quote = MintQuote::new(
                    Some(mint_quote.id.clone()),          // id
                    mint_quote.request.clone(),           // request
                    mint_quote.unit.clone(),              // unit
                    mint_quote.amount,                    // amount
                    mint_quote.expiry,                    // expiry
                    mint_quote.request_lookup_id.clone(), // request_lookup_id
                    mint_quote.pubkey,                    // pubkey
                    mint_quote.amount_issued(),           // amount_issued
                    mint_quote.amount_paid(),             // amount_paid
                    mint_quote.payment_method.clone(),    // method
                    0,                                    // created_at
                    vec![],                               // blinded_messages
                    vec![],                               // payment_ids
                );

                let mint_store = self.mint().localstore();
                let mut tx = mint_store
                    .begin_transaction()
                    .await
                    .map_err(|_| Status::internal("Could not update quote".to_string()))?;
                tx.add_mint_quote(quote.clone())
                    .await
                    .map_err(|_| Status::internal("Could not update quote".to_string()))?;
                tx.commit()
                    .await
                    .map_err(|_| Status::internal("Could not update quote".to_string()))?;
            }
        }

        let mint_quote = self
            .mint()
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .map_err(|_| Status::invalid_argument("Could not find quote".to_string()))?
            .ok_or(Status::invalid_argument("Could not find quote".to_string()))?;

        Ok(Response::new(UpdateNut04QuoteRequest {
            state: mint_quote.state().to_string(),
            quote_id: mint_quote.id.to_string(),
        }))
    }

    /// Rotates to the next keyset for the specified currency unit
    async fn rotate_next_keyset(
        &self,
        request: Request<RotateNextKeysetRequest>,
    ) -> Result<Response<RotateNextKeysetResponse>, Status> {
        let request = request.into_inner();

        let unit = CurrencyUnit::from_str(&request.unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let amounts = if request.amounts.is_empty() {
            return Err(Status::invalid_argument("amounts cannot be empty"));
        } else {
            request.amounts
        };

        let keyset_info = self
            .mint()
            .rotate_keyset(unit, amounts, request.input_fee_ppk.unwrap_or(0))
            .await
            .map_err(|_| Status::invalid_argument("Could not rotate keyset".to_string()))?;

        Ok(Response::new(RotateNextKeysetResponse {
            id: keyset_info.id.to_string(),
            unit: keyset_info.unit.to_string(),
            amounts: keyset_info.amounts,
            input_fee_ppk: keyset_info.input_fee_ppk,
        }))
    }
}

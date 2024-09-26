use anyhow::{bail, Result};
use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cdk::amount::{to_unit, Amount};
use cdk::error::{Error, ErrorResponse};
use cdk::mint::MeltQuote;
use cdk::nuts::nut05::MeltBolt11Response;
use cdk::nuts::{
    CheckStateRequest, CheckStateResponse, CurrencyUnit, Id, KeysResponse, KeysetResponse,
    MeltBolt11Request, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltQuoteState,
    MintBolt11Request, MintBolt11Response, MintInfo, MintQuoteBolt11Request,
    MintQuoteBolt11Response, PaymentMethod, RestoreRequest, RestoreResponse, SwapRequest,
    SwapResponse,
};
use cdk::util::unix_time;

use crate::{LnKey, MintState};

pub async fn get_keys(State(state): State<MintState>) -> Result<Json<KeysResponse>, Response> {
    let pubkeys = state.mint.pubkeys().await.map_err(|err| {
        tracing::error!("Could not get keys: {}", err);
        into_response(err)
    })?;

    Ok(Json(pubkeys))
}

pub async fn get_keyset_pubkeys(
    State(state): State<MintState>,
    Path(keyset_id): Path<Id>,
) -> Result<Json<KeysResponse>, Response> {
    let pubkeys = state.mint.keyset_pubkeys(&keyset_id).await.map_err(|err| {
        tracing::error!("Could not get keyset pubkeys: {}", err);
        into_response(err)
    })?;

    Ok(Json(pubkeys))
}

pub async fn get_keysets(State(state): State<MintState>) -> Result<Json<KeysetResponse>, Response> {
    let mint = state.mint.keysets().await.map_err(|err| {
        tracing::error!("Could not get keyset: {}", err);
        into_response(err)
    })?;

    Ok(Json(mint))
}

pub async fn get_mint_bolt11_quote(
    State(state): State<MintState>,
    Json(payload): Json<MintQuoteBolt11Request>,
) -> Result<Json<MintQuoteBolt11Response>, Response> {
    let ln = state
        .ln
        .get(&LnKey::new(payload.unit, PaymentMethod::Bolt11))
        .ok_or_else(|| {
            tracing::info!("Bolt11 mint request for unsupported unit");

            into_response(Error::UnitUnsupported)
        })?;

    let quote_expiry = unix_time() + state.quote_ttl;

    if payload.description.is_some() && !ln.get_settings().invoice_description {
        tracing::error!("Backend does not support invoice description");
        return Err(into_response(Error::InvoiceDescriptionUnsupported));
    }

    let create_invoice_response = ln
        .create_invoice(
            payload.amount,
            &payload.unit,
            payload.description.unwrap_or("".to_string()),
            quote_expiry,
        )
        .await
        .map_err(|err| {
            tracing::error!("Could not create invoice: {}", err);
            into_response(Error::InvalidPaymentRequest)
        })?;

    let quote = state
        .mint
        .new_mint_quote(
            state.mint_url,
            create_invoice_response.request.to_string(),
            payload.unit,
            payload.amount,
            create_invoice_response.expiry.unwrap_or(0),
            create_invoice_response.request_lookup_id,
        )
        .await
        .map_err(|err| {
            tracing::error!("Could not create new mint quote: {}", err);
            into_response(err)
        })?;

    Ok(Json(quote.into()))
}

pub async fn get_check_mint_bolt11_quote(
    State(state): State<MintState>,
    Path(quote_id): Path<String>,
) -> Result<Json<MintQuoteBolt11Response>, Response> {
    let quote = state
        .mint
        .check_mint_quote(&quote_id)
        .await
        .map_err(|err| {
            tracing::error!("Could not check mint quote {}: {}", quote_id, err);
            into_response(err)
        })?;

    Ok(Json(quote))
}

pub async fn post_mint_bolt11(
    State(state): State<MintState>,
    Json(payload): Json<MintBolt11Request>,
) -> Result<Json<MintBolt11Response>, Response> {
    let res = state
        .mint
        .process_mint_request(payload)
        .await
        .map_err(|err| {
            tracing::error!("Could not process mint: {}", err);
            into_response(err)
        })?;

    Ok(Json(res))
}

pub async fn get_melt_bolt11_quote(
    State(state): State<MintState>,
    Json(payload): Json<MeltQuoteBolt11Request>,
) -> Result<Json<MeltQuoteBolt11Response>, Response> {
    let ln = state
        .ln
        .get(&LnKey::new(payload.unit, PaymentMethod::Bolt11))
        .ok_or_else(|| {
            tracing::info!("Could not get ln backend for {}, bolt11 ", payload.unit);

            into_response(Error::UnitUnsupported)
        })?;

    let payment_quote = ln.get_payment_quote(&payload).await.map_err(|err| {
        tracing::error!(
            "Could not get payment quote for mint quote, {} bolt11, {}",
            payload.unit,
            err
        );

        into_response(Error::UnitUnsupported)
    })?;

    let quote = state
        .mint
        .new_melt_quote(
            payload.request.to_string(),
            payload.unit,
            payment_quote.amount,
            payment_quote.fee,
            unix_time() + state.quote_ttl,
            payment_quote.request_lookup_id,
        )
        .await
        .map_err(|err| {
            tracing::error!("Could not create melt quote: {}", err);
            into_response(err)
        })?;

    Ok(Json(quote.into()))
}

pub async fn get_check_melt_bolt11_quote(
    State(state): State<MintState>,
    Path(quote_id): Path<String>,
) -> Result<Json<MeltQuoteBolt11Response>, Response> {
    let quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .map_err(|err| {
            tracing::error!("Could not check melt quote: {}", err);
            into_response(err)
        })?;

    Ok(Json(quote))
}

pub async fn post_melt_bolt11(
    State(state): State<MintState>,
    Json(payload): Json<MeltBolt11Request>,
) -> Result<Json<MeltBolt11Response>, Response> {
    use std::sync::Arc;
    async fn check_payment_state(
        ln: Arc<dyn MintLightning<Err = cdk::cdk_lightning::Error> + Send + Sync>,
        melt_quote: &MeltQuote,
    ) -> Result<PayInvoiceResponse> {
        match ln
            .check_outgoing_payment(&melt_quote.request_lookup_id)
            .await
        {
            Ok(response) => Ok(response),
            Err(check_err) => {
                // If we cannot check the status of the payment we keep the proofs stuck as pending.
                tracing::error!(
                    "Could not check the status of payment for {},. Proofs stuck as pending",
                    melt_quote.id
                );
                tracing::error!("Checking payment error: {}", check_err);
                bail!("Could not check payment status")
            }
        }
    }

    let quote = match state.mint.verify_melt_request(&payload).await {
        Ok(quote) => quote,
        Err(err) => {
            tracing::debug!("Error attempting to verify melt quote: {}", err);

            if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                tracing::error!(
                    "Could not reset melt quote {} state: {}",
                    payload.quote,
                    err
                );
            }
            return Err(into_response(err));
        }
    };

    let settled_internally_amount =
        match state.mint.handle_internal_melt_mint(&quote, &payload).await {
            Ok(amount) => amount,
            Err(err) => {
                tracing::error!("Attempting to settle internally failed");
                if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                    tracing::error!(
                        "Could not reset melt quote {} state: {}",
                        payload.quote,
                        err
                    );
                }
                return Err(into_response(err));
            }
        };

    let (preimage, amount_spent_quote_unit) = match settled_internally_amount {
        Some(amount_spent) => (None, amount_spent),
        None => {
            // If the quote unit is SAT or MSAT we can check that the expected fees are
            // provided. We also check if the quote is less then the invoice
            // amount in the case that it is a mmp However, if the quote is not
            // of a bitcoin unit we cannot do these checks as the mint
            // is unaware of a conversion rate. In this case it is assumed that the quote is
            // correct and the mint should pay the full invoice amount if inputs
            // > `then quote.amount` are included. This is checked in the
            // `verify_melt` method.
            let partial_amount = match quote.unit {
                CurrencyUnit::Sat | CurrencyUnit::Msat => {
                    match state
                        .mint
                        .check_melt_expected_ln_fees(&quote, &payload)
                        .await
                    {
                        Ok(amount) => amount,
                        Err(err) => {
                            tracing::error!("Fee is not expected: {}", err);
                            if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                                tracing::error!("Could not reset melt quote state: {}", err);
                            }
                            return Err(into_response(Error::Internal));
                        }
                    }
                }
                _ => None,
            };

            let ln = match state.ln.get(&LnKey::new(quote.unit, PaymentMethod::Bolt11)) {
                Some(ln) => ln,
                None => {
                    tracing::info!("Could not get ln backend for {}, bolt11 ", quote.unit);
                    if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                        tracing::error!("Could not reset melt quote state: {}", err);
                    }

                    return Err(into_response(Error::UnitUnsupported));
                }
            };

            let pre = match ln
                .pay_invoice(quote.clone(), partial_amount, Some(quote.fee_reserve))
                .await
            {
                Ok(pay)
                    if pay.status == MeltQuoteState::Unknown
                        || pay.status == MeltQuoteState::Failed =>
                {
                    let check_response = check_payment_state(Arc::clone(ln), &quote)
                        .await
                        .map_err(|_| into_response(Error::Internal))?;

                    if check_response.status == MeltQuoteState::Paid {
                        tracing::warn!("Pay invoice returned {} but check returned {}. Proofs stuck as pending", pay.status.to_string(), check_response.status.to_string());

                        return Err(into_response(Error::Internal));
                    }

                    check_response
                }
                Ok(pay) => pay,
                Err(err) => {
                    // If the error is that the invoice was already paid we do not want to hold
                    // hold the proofs as pending to we reset them  and return an error.
                    if matches!(err, cdk::cdk_lightning::Error::InvoiceAlreadyPaid) {
                        tracing::debug!("Invoice already paid, resetting melt quote");
                        if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                            tracing::error!("Could not reset melt quote state: {}", err);
                        }
                        return Err(into_response(Error::RequestAlreadyPaid));
                    }

                    tracing::error!("Error returned attempting to pay: {} {}", quote.id, err);

                    let check_response = check_payment_state(Arc::clone(ln), &quote)
                        .await
                        .map_err(|_| into_response(Error::Internal))?;
                    // If there error is something else we want to check the status of the payment ensure it is not pending or has been made.
                    if check_response.status == MeltQuoteState::Paid {
                        tracing::warn!("Pay invoice returned an error but check returned {}. Proofs stuck as pending", check_response.status.to_string());

                        return Err(into_response(Error::Internal));
                    }
                    check_response
                }
            };

            match pre.status {
                MeltQuoteState::Paid => (),
                MeltQuoteState::Unpaid | MeltQuoteState::Unknown | MeltQuoteState::Failed => {
                    tracing::info!("Lightning payment for quote {} failed.", payload.quote);
                    if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                        tracing::error!("Could not reset melt quote state: {}", err);
                    }
                    return Err(into_response(Error::PaymentFailed));
                }
                MeltQuoteState::Pending => {
                    tracing::warn!(
                        "LN payment pending, proofs are stuck as pending for quote: {}",
                        payload.quote
                    );
                    return Err(into_response(Error::PendingQuote));
                }
            }

            // Convert from unit of backend to quote unit
            // Note: this should never fail since these conversions happen earlier and would fail there.
            // Since it will not fail and even if it does the ln payment has already been paid, proofs should still be burned
            let amount_spent = to_unit(pre.total_spent, &pre.unit, &quote.unit).unwrap_or_default();

            let payment_lookup_id = pre.payment_lookup_id;

            if payment_lookup_id != quote.request_lookup_id {
                tracing::info!(
                    "Payment lookup id changed post payment from {} to {}",
                    quote.request_lookup_id,
                    payment_lookup_id
                );

                let mut melt_quote = quote;
                melt_quote.request_lookup_id = payment_lookup_id;

                if let Err(err) = state.mint.localstore.add_melt_quote(melt_quote).await {
                    tracing::warn!("Could not update payment lookup id: {}", err);
                }
            }

            (pre.payment_preimage, amount_spent)
        }
    };

    // If we made it here the payment has been made.
    // We process the melt burning the inputs and returning change
    let res = state
        .mint
        .process_melt_request(&payload, preimage, amount_spent_quote_unit)
        .await
        .map_err(|err| {
            tracing::error!("Could not process melt request: {}", err);
            into_response(err)
        })?;

    Ok(Json(res.into()))
}

pub async fn post_check(
    State(state): State<MintState>,
    Json(payload): Json<CheckStateRequest>,
) -> Result<Json<CheckStateResponse>, Response> {
    let state = state.mint.check_state(&payload).await.map_err(|err| {
        tracing::error!("Could not check state of proofs");
        into_response(err)
    })?;

    Ok(Json(state))
}

pub async fn get_mint_info(State(state): State<MintState>) -> Result<Json<MintInfo>, Response> {
    Ok(Json(state.mint.mint_info().clone().time(unix_time())))
}

pub async fn post_swap(
    State(state): State<MintState>,
    Json(payload): Json<SwapRequest>,
) -> Result<Json<SwapResponse>, Response> {
    let swap_response = state
        .mint
        .process_swap_request(payload)
        .await
        .map_err(|err| {
            tracing::error!("Could not process swap request: {}", err);
            into_response(err)
        })?;
    Ok(Json(swap_response))
}

pub async fn post_restore(
    State(state): State<MintState>,
    Json(payload): Json<RestoreRequest>,
) -> Result<Json<RestoreResponse>, Response> {
    let restore_response = state.mint.restore(payload).await.map_err(|err| {
        tracing::error!("Could not process restore: {}", err);
        into_response(err)
    })?;

    Ok(Json(restore_response))
}

pub fn into_response<T>(error: T) -> Response
where
    T: Into<ErrorResponse>,
{
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json::<ErrorResponse>(error.into()),
    )
        .into_response()
}

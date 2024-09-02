use std::str::FromStr;

use anyhow::Result;
use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cdk::amount::Amount;
use cdk::cdk_lightning::to_unit;
use cdk::error::{Error, ErrorResponse};
use cdk::nuts::nut05::MeltBolt11Response;
use cdk::nuts::{
    CheckStateRequest, CheckStateResponse, CurrencyUnit, Id, KeysResponse, KeysetResponse,
    MeltBolt11Request, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MintBolt11Request,
    MintBolt11Response, MintInfo, MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteState,
    PaymentMethod, RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use cdk::util::unix_time;
use cdk::Bolt11Invoice;

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

            into_response(Error::UnsupportedUnit)
        })?;

    let amount =
        to_unit(payload.amount, &payload.unit, &ln.get_settings().unit).map_err(|err| {
            tracing::error!("Backed does not support unit: {}", err);
            into_response(Error::UnsupportedUnit)
        })?;

    let quote_expiry = unix_time() + state.quote_ttl;

    let create_invoice_response = ln
        .create_invoice(amount, &payload.unit, "".to_string(), quote_expiry)
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

            into_response(Error::UnsupportedUnit)
        })?;

    let payment_quote = ln.get_payment_quote(&payload).await.map_err(|err| {
        tracing::error!(
            "Could not get payment quote for mint quote, {} bolt11, {}",
            payload.unit,
            err
        );

        into_response(Error::UnsupportedUnit)
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
    let quote = match state.mint.verify_melt_request(&payload).await {
        Ok(quote) => quote,
        Err(err) => {
            tracing::debug!("Error attempting to verify melt quote: {}", err);

            if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                tracing::error!("Could not reset melt quote state: {}", err);
            }
            return Err(into_response(Error::MeltRequestInvalid));
        }
    };

    // Check to see if there is a corresponding mint quote for a melt.
    // In this case the mint can settle the payment internally and no ln payment is
    // needed
    let mint_quote = match state
        .mint
        .localstore
        .get_mint_quote_by_request(&quote.request)
        .await
    {
        Ok(mint_quote) => mint_quote,
        Err(err) => {
            tracing::debug!("Error attempting to get mint quote: {}", err);

            if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                tracing::error!("Could not reset melt quote state: {}", err);
            }
            return Err(into_response(Error::DatabaseError));
        }
    };

    let inputs_amount_quote_unit = payload.proofs_amount();

    let (preimage, amount_spent_quote_unit) = match mint_quote {
        Some(mint_quote) => {
            if mint_quote.state == MintQuoteState::Issued
                || mint_quote.state == MintQuoteState::Paid
            {
                return Err(into_response(Error::RequestAlreadyPaid));
            }

            let mut mint_quote = mint_quote;

            if mint_quote.amount > inputs_amount_quote_unit {
                tracing::debug!(
                    "Not enough inuts provided: {} needed {}",
                    inputs_amount_quote_unit,
                    mint_quote.amount
                );
                if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                    tracing::error!("Could not reset melt quote state: {}", err);
                }
                return Err(into_response(Error::InsufficientInputProofs));
            }

            mint_quote.state = MintQuoteState::Paid;

            let amount = quote.amount;

            if let Err(_err) = state.mint.update_mint_quote(mint_quote).await {
                if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                    tracing::error!("Could not reset melt quote state: {}", err);
                }
                return Err(into_response(Error::DatabaseError));
            }

            (None, amount)
        }
        None => {
            let invoice = match Bolt11Invoice::from_str(&quote.request) {
                Ok(bolt11) => bolt11,
                Err(_) => {
                    tracing::error!("Melt quote has invalid payment request");
                    if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                        tracing::error!("Could not reset melt quote state: {}", err);
                    }
                    return Err(into_response(Error::InvalidPaymentRequest));
                }
            };

            let mut partial_amount = None;

            // If the quote unit is SAT or MSAT we can check that the expected fees are
            // provided. We also check if the quote is less then the invoice
            // amount in the case that it is a mmp However, if the quote id not
            // of a bitcoin unit we cannot do these checks as the mint
            // is unaware of a conversion rate. In this case it is assumed that the quote is
            // correct and the mint should pay the full invoice amount if inputs
            // > then quote.amount are included. This is checked in the
            // verify_melt method.
            if quote.unit == CurrencyUnit::Msat || quote.unit == CurrencyUnit::Sat {
                let quote_msats = to_unit(quote.amount, &quote.unit, &CurrencyUnit::Msat)
                    .expect("Quote unit is checked above that it can convert to msat");

                let invoice_amount_msats: Amount = match invoice.amount_milli_satoshis() {
                    Some(amount) => amount.into(),
                    None => {
                        if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                            tracing::error!("Could not reset melt quote state: {}", err);
                        }
                        return Err(into_response(Error::InvoiceAmountUndefined));
                    }
                };

                partial_amount = match invoice_amount_msats > quote_msats {
                    true => {
                        let partial_msats = invoice_amount_msats - quote_msats;

                        Some(
                            to_unit(partial_msats, &CurrencyUnit::Msat, &quote.unit)
                                .map_err(|_| into_response(Error::UnsupportedUnit))?,
                        )
                    }
                    false => None,
                };

                let amount_to_pay = match partial_amount {
                    Some(amount_to_pay) => amount_to_pay,
                    None => to_unit(invoice_amount_msats, &CurrencyUnit::Msat, &quote.unit)
                        .map_err(|_| into_response(Error::UnsupportedUnit))?,
                };

                if amount_to_pay + quote.fee_reserve > inputs_amount_quote_unit {
                    tracing::debug!(
                        "Not enough inuts provided: {} msats needed {} msats",
                        inputs_amount_quote_unit,
                        amount_to_pay
                    );

                    if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                        tracing::error!("Could not reset melt quote state: {}", err);
                    }
                    return Err(into_response(Error::InsufficientInputProofs));
                }
            }

            let ln = match state.ln.get(&LnKey::new(quote.unit, PaymentMethod::Bolt11)) {
                Some(ln) => ln,
                None => {
                    tracing::info!("Could not get ln backend for {}, bolt11 ", quote.unit);
                    if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                        tracing::error!("Could not reset melt quote state: {}", err);
                    }

                    return Err(into_response(Error::UnsupportedUnit));
                }
            };

            let pre = match ln
                .pay_invoice(quote.clone(), partial_amount, Some(quote.fee_reserve))
                .await
            {
                Ok(pay) => pay,
                Err(err) => {
                    tracing::error!("Could not pay invoice: {}", err);
                    if let Err(err) = state.mint.process_unpaid_melt(&payload).await {
                        tracing::error!("Could not reset melt quote state: {}", err);
                    }

                    let err = match err {
                        cdk::cdk_lightning::Error::InvoiceAlreadyPaid => Error::RequestAlreadyPaid,
                        _ => Error::PaymentFailed,
                    };

                    return Err(into_response(err));
                }
            };

            let amount_spent = to_unit(pre.total_spent, &ln.get_settings().unit, &quote.unit)
                .map_err(|_| into_response(Error::UnsupportedUnit))?;

            (pre.payment_preimage, amount_spent)
        }
    };

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

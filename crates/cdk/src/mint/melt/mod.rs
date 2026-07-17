#[cfg(feature = "federation")]
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::database::DynMintDatabase;
#[cfg(feature = "federation")]
use cdk_common::federation::{FederationMeltQuoteMetadata, FederationOperationId};
use cdk_common::melt::MeltQuoteRequest;
use cdk_common::mint::MeltPaymentRequest;
#[cfg(feature = "federation")]
use cdk_common::mint::Saga;
use cdk_common::nut00::KnownMethod;
use cdk_common::nut05::MeltMethodOptions;
use cdk_common::nuts::nut17::{Kind, NotificationPayload};
use cdk_common::payment::{
    Bolt11OutgoingPaymentOptions, Bolt12OutgoingPaymentOptions, CustomOutgoingPaymentOptions,
    OutgoingPaymentOptions, PaymentIdentifier,
};
use cdk_common::quote_id::QuoteId;
use cdk_common::subscription::Params;
#[cfg(feature = "federation")]
use cdk_common::ProofsMethods;
use cdk_common::{
    MeltOptions, MeltQuoteBolt12Request, MeltQuoteCreateResponse, MeltQuoteCustomRequest,
    MeltQuoteCustomResponse, MeltQuoteOnchainRequest, MeltQuoteOnchainResponse, MeltQuoteResponse,
};
use lightning::offers::offer::Offer;
use tracing::instrument;

use super::{
    CurrencyUnit, MeltQuote, MeltQuoteBolt11Request, MeltQuoteBolt11Response,
    MeltQuoteBolt12Response, MeltRequest, Mint, PaymentMethod,
};
use crate::mint::verification::{validate_custom_payment_method, MAX_REQUEST_FIELD_LEN};
use crate::nuts::MeltQuoteState;
use crate::types::PaymentProcessorKey;
use crate::util::unix_time;
use crate::{ensure_cdk, Amount, Error};

pub(crate) mod melt_saga;
pub(crate) mod shared;

#[cfg(test)]
mod tests;

#[cfg(feature = "federation")]
use melt_saga::FederationInternalSettlementOutcome;
use melt_saga::{MeltSaga, PaymentOutcome};

fn pending_melt_wait_timeout() -> Duration {
    if cfg!(test) {
        // Bumped from 100ms to 250ms to reduce flake on loaded CI while
        // keeping the timeout test fast.
        Duration::from_millis(250)
    } else {
        Duration::from_secs(60)
    }
}

fn pending_melt_notification_wait_interval() -> Duration {
    if cfg!(test) {
        Duration::from_millis(10)
    } else {
        Duration::from_secs(1)
    }
}

fn pending_melt_status_recheck_interval() -> Duration {
    if cfg!(test) {
        Duration::from_millis(20)
    } else {
        Duration::from_secs(5)
    }
}

#[cfg(feature = "federation")]
pub(super) fn ensure_melt_quote_matches_request(
    existing: &MeltQuote,
    request: &MeltQuoteRequest,
) -> Result<(), Error> {
    let matches = match request {
        MeltQuoteRequest::Bolt11(request) => {
            existing.unit == request.unit
                && existing.options == request.options
                && existing.payment_method == PaymentMethod::Known(KnownMethod::Bolt11)
                && existing.request
                    == cdk_common::mint::MeltPaymentRequest::Bolt11 {
                        bolt11: request.request.clone(),
                    }
        }
        MeltQuoteRequest::Bolt12(request) => {
            existing.unit == request.unit
                && existing.options == request.options
                && existing.payment_method == PaymentMethod::Known(KnownMethod::Bolt12)
                && existing.request.to_string() == request.request
        }
        MeltQuoteRequest::Onchain(request) => {
            existing.unit == request.unit
                && existing.amount() == request.amount.with_unit(request.unit.clone())
                && existing.payment_method == PaymentMethod::Known(KnownMethod::Onchain)
                && existing.request
                    == cdk_common::mint::MeltPaymentRequest::Onchain {
                        address: request.request.clone(),
                    }
        }
        MeltQuoteRequest::Custom(request) => {
            existing.unit == request.unit
                && existing.payment_method == request.method.as_str()
                && existing.request
                    == cdk_common::mint::MeltPaymentRequest::Custom {
                        method: request.method.clone(),
                        request: request.request.clone(),
                    }
        }
    };

    if matches {
        Ok(())
    } else {
        Err(Error::Custom(
            "federation melt quote replay found mismatched local quote".to_string(),
        ))
    }
}

fn melt_quote_create_response(quote: MeltQuote) -> MeltQuoteCreateResponse<QuoteId> {
    match quote.into_response(None) {
        MeltQuoteResponse::Bolt11(response) => MeltQuoteCreateResponse::Bolt11(response),
        MeltQuoteResponse::Bolt12(response) => MeltQuoteCreateResponse::Bolt12(response),
        MeltQuoteResponse::Onchain(response) => MeltQuoteCreateResponse::Onchain(response),
        MeltQuoteResponse::Custom(response) => MeltQuoteCreateResponse::Custom(response),
    }
}

#[cfg(feature = "federation")]
fn federation_melt_quote_preparation(
    quote: &MeltQuote,
) -> (
    MeltQuoteCreateResponse<QuoteId>,
    FederationMeltQuoteMetadata,
) {
    let mut canonical = quote.clone();
    canonical.state = MeltQuoteState::Unpaid;
    canonical.payment_proof = None;
    canonical.paid_time = None;
    canonical.selected_fee_index = None;

    let mut metadata = FederationMeltQuoteMetadata::from_melt_quote(quote);
    if quote.payment_method == PaymentMethod::Known(KnownMethod::Onchain) {
        metadata.estimated_blocks = quote
            .fee_options()
            .iter()
            .min_by_key(|option| u64::from(option.fee_reserve))
            .map(|option| option.estimated_blocks);
    }

    (melt_quote_create_response(canonical), metadata)
}

/// A pending mint melt that can optionally be awaited.
#[derive(Debug)]
pub struct PendingMelt {
    response: MeltQuoteResponse<QuoteId>,
    completion: tokio::task::JoinHandle<Result<MeltQuoteResponse<QuoteId>, Error>>,
}

impl PendingMelt {
    /// Return the immediate pending response (NUT-05 style) without consuming self.
    pub fn pending_response(&self) -> &MeltQuoteResponse<QuoteId> {
        &self.response
    }

    /// Return the immediate pending response (NUT-05 style).
    pub fn into_pending_response(self) -> MeltQuoteResponse<QuoteId> {
        self.response
    }

    async fn wait(self) -> Result<MeltQuoteResponse<QuoteId>, Error> {
        match self.completion.await {
            Ok(result) => result,
            Err(err) => {
                tracing::error!("Background melt task failed to join: {}", err);
                Err(Error::Internal)
            }
        }
    }
}

impl std::future::IntoFuture for PendingMelt {
    type Output = Result<MeltQuoteResponse<QuoteId>, Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.wait())
    }
}

fn validate_custom_quote_fields(request: &str, extra: &serde_json::Value) -> Result<(), Error> {
    if request.len() > MAX_REQUEST_FIELD_LEN {
        return Err(Error::RequestFieldTooLarge {
            field: "request".to_string(),
            actual: request.len(),
            max: MAX_REQUEST_FIELD_LEN,
        });
    }

    if !extra.is_null() {
        let extra_str = extra.to_string();
        if extra_str.len() > MAX_REQUEST_FIELD_LEN {
            return Err(Error::RequestFieldTooLarge {
                field: "extra".to_string(),
                actual: extra_str.len(),
                max: MAX_REQUEST_FIELD_LEN,
            });
        }
    }

    Ok(())
}

impl Mint {
    #[cfg(feature = "federation")]
    pub(crate) async fn attempt_federation_external_melt_payment(
        &self,
        quote: MeltQuote,
        saga: Saga,
    ) -> Result<Option<cdk_common::payment::MakePaymentResponse>, Error> {
        MeltSaga::resume_federation_external_payment(
            Arc::new(self.clone()),
            self.localstore.clone(),
            Arc::clone(&self.pubsub_manager),
            quote,
            saga,
        )?
        .attempt_external_payment_for_federation_observation()
        .await
    }

    /// Loads a settled melt response from the database.
    ///
    /// Returns `Ok(None)` while the quote is still `Pending` or in an `Unknown`
    /// state (backend could not determine status); the caller should keep
    /// waiting. Returns `Ok(Some(response))` once the quote has reached a
    /// final state (`Paid`, `Failed`, or post-attempt `Unpaid`), with any
    /// change blind signatures attached.
    ///
    /// `Unknown` is treated as "keep waiting" by design: a backend may recover
    /// and report a definitive status through its event stream or an explicit
    /// quote check. A permanently `Unknown` quote eventually returns
    /// `Error::PendingMeltTimeout` to this waiter without changing durable
    /// state.
    async fn load_settled_melt_response(
        localstore: &DynMintDatabase,
        quote_id: &QuoteId,
    ) -> Result<Option<MeltQuoteResponse<QuoteId>>, Error> {
        let quote = localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        if matches!(
            quote.state,
            MeltQuoteState::Pending | MeltQuoteState::Unknown
        ) {
            return Ok(None);
        }

        let change = localstore.get_blind_signatures_for_quote(quote_id).await?;
        let change_opt = if change.is_empty() {
            None
        } else {
            Some(change)
        };

        Ok(Some(quote.into_response(change_opt)))
    }

    #[instrument(skip_all)]
    async fn check_melt_request_acceptable(
        &self,
        amount: Amount<CurrencyUnit>,
        method: PaymentMethod,
        request: String,
        options: Option<MeltOptions>,
    ) -> Result<(), Error> {
        let unit = amount.unit().clone();
        let mint_info = self.mint_info().await?;
        let nut05 = mint_info.nuts.nut05;

        ensure_cdk!(!nut05.disabled, Error::MeltingDisabled);

        let settings = nut05
            .get_settings(&unit, &method)
            .ok_or(Error::UnsupportedUnit)?;

        match options {
            Some(MeltOptions::Mpp { mpp: _ }) => {
                let nut15 = mint_info.nuts.nut15;
                // Verify there is no corresponding mint quote.
                // Otherwise a wallet is trying to pay someone internally, but
                // with a multi-part quote. And that's just not possible.
                if (self.localstore.get_mint_quote_by_request(&request).await?).is_some() {
                    return Err(Error::InternalMultiPartMeltQuote);
                }
                // Verify MPP is enabled for unit and method
                if !nut15
                    .methods
                    .into_iter()
                    .any(|m| m.method == method && m.unit == unit)
                {
                    return Err(Error::MppUnitMethodNotSupported(unit, method));
                }
            }
            Some(MeltOptions::Amountless { amountless: _ })
                if method.is_bolt11()
                    && !matches!(
                        settings.options,
                        Some(MeltMethodOptions::Bolt11 { amountless: true })
                    ) =>
            {
                return Err(Error::AmountlessInvoiceNotSupported(unit, method));
            }
            _ => {}
        };

        // Compare using raw values since settings use Amount without unit
        let amount_value = amount.value();
        let is_above_max = matches!(settings.max_amount, Some(max) if amount_value > max.into());
        let is_below_min = matches!(settings.min_amount, Some(min) if amount_value < min.into());
        match is_above_max || is_below_min {
            true => {
                tracing::error!(
                    "Melt amount out of range: {} is not within {} and {}",
                    amount,
                    settings.min_amount.unwrap_or_default(),
                    settings.max_amount.unwrap_or_default(),
                );
                Err(Error::AmountOutofLimitRange(
                    settings.min_amount.unwrap_or_default(),
                    settings.max_amount.unwrap_or_default(),
                    amount.into(),
                ))
            }
            false => Ok(()),
        }
    }

    /// Get melt quote for BOLT11, BOLT12, or Custom payment methods
    ///
    /// This function accepts a `MeltQuoteRequest` enum and delegates to the
    /// appropriate handler based on the request type.
    #[instrument(skip_all)]
    pub async fn get_melt_quote(
        &self,
        melt_quote_request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<QuoteId>, Error> {
        let quote = match melt_quote_request {
            MeltQuoteRequest::Bolt11(request) => {
                self.get_melt_bolt11_quote_impl(&request, true).await?
            }
            MeltQuoteRequest::Bolt12(request) => {
                self.get_melt_bolt12_quote_impl(&request, true).await?
            }
            MeltQuoteRequest::Onchain(request) => {
                self.get_melt_onchain_quote_impl(&request, true).await?
            }
            MeltQuoteRequest::Custom(request) => {
                self.get_melt_custom_quote_impl(&request, true).await?
            }
        };
        Ok(melt_quote_create_response(quote))
    }

    /// Prepare a canonical melt quote without persisting proposer-local state.
    #[cfg(feature = "federation")]
    #[instrument(skip_all)]
    pub async fn prepare_melt_quote(
        &self,
        melt_quote_request: MeltQuoteRequest,
    ) -> Result<
        (
            MeltQuoteCreateResponse<QuoteId>,
            FederationMeltQuoteMetadata,
        ),
        Error,
    > {
        let quote = match melt_quote_request {
            MeltQuoteRequest::Bolt11(request) => {
                self.get_melt_bolt11_quote_impl(&request, false).await?
            }
            MeltQuoteRequest::Bolt12(request) => {
                self.get_melt_bolt12_quote_impl(&request, false).await?
            }
            MeltQuoteRequest::Onchain(request) => {
                self.get_melt_onchain_quote_impl(&request, false).await?
            }
            MeltQuoteRequest::Custom(request) => {
                self.get_melt_custom_quote_impl(&request, false).await?
            }
        };
        Ok(federation_melt_quote_preparation(&quote))
    }

    /// Implementation of get_melt_bolt11_quote
    #[instrument(skip_all)]
    async fn get_melt_bolt11_quote_impl(
        &self,
        melt_request: &MeltQuoteBolt11Request,
        persist_quote: bool,
    ) -> Result<MeltQuote, Error> {
        #[cfg(feature = "prometheus")]
        let metrics = super::MintMetricGuard::new("get_melt_bolt11_quote");

        let result = async {
            let MeltQuoteBolt11Request {
                request,
                unit,
                options,
                ..
            } = melt_request;

            let payment_backend = self
                .payment_processors
                .get(&PaymentProcessorKey::new(
                    unit.clone(),
                    PaymentMethod::Known(KnownMethod::Bolt11),
                ))
                .ok_or_else(|| {
                    tracing::info!("Could not get payment backend for {}, bolt11 ", unit);

                    Error::UnsupportedUnit
                })?;

            // Pre-generate the quote id so we can pass it to the backend in both
            // `get_payment_quote` and the eventual `make_payment`, and use the same
            // id when we persist the quote below.
            let quote_id = QuoteId::new();

            let bolt11 = Bolt11OutgoingPaymentOptions {
                bolt11: melt_request.request.clone(),
                max_fee_amount: None,
                timeout_secs: None,
                melt_options: melt_request.options,
                quote_id: quote_id.clone(),
            };

            let payment_quote = payment_backend
                .get_payment_quote(
                    &melt_request.unit,
                    OutgoingPaymentOptions::Bolt11(Box::new(bolt11)),
                )
                .await
                .map_err(|err| {
                    tracing::error!(
                        "Could not get payment quote for mint quote, {} bolt11, {}",
                        unit,
                        err
                    );

                    err
                })?;

            if payment_quote.unit() != unit {
                return Err(Error::UnitMismatch);
            }

            // Validate using processor quote amount for currency conversion
            self.check_melt_request_acceptable(
                payment_quote.amount.clone(),
                PaymentMethod::Known(KnownMethod::Bolt11),
                request.to_string(),
                *options,
            )
            .await?;

            // Extract values for quote creation
            let quote_amount = payment_quote.amount;
            let quote_fee = payment_quote.fee;

            let melt_ttl = self.quote_ttl().await?.melt_ttl;

            let quote = MeltQuote::new(
                Some(quote_id),
                MeltPaymentRequest::Bolt11 {
                    bolt11: request.clone(),
                },
                unit.clone(),
                quote_amount.clone(),
                quote_fee,
                unix_time() + melt_ttl,
                payment_quote.request_lookup_id.clone(),
                *options,
                PaymentMethod::Known(KnownMethod::Bolt11),
                payment_quote.extra_json,
                payment_quote.estimated_blocks,
            );

            tracing::debug!(
                "New {} melt quote {} for {} {} with request id {:?}",
                quote.payment_method,
                quote.id,
                quote_amount,
                unit,
                payment_quote.request_lookup_id
            );

            if persist_quote {
                let mut tx = self.localstore.begin_transaction().await?;
                tx.add_melt_quote(quote.clone()).await?;
                tx.commit().await?;
            }
            Ok(quote)
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        result
    }

    /// Implementation of get_melt_bolt12_quote
    #[instrument(skip_all)]
    async fn get_melt_bolt12_quote_impl(
        &self,
        melt_request: &MeltQuoteBolt12Request,
        persist_quote: bool,
    ) -> Result<MeltQuote, Error> {
        #[cfg(feature = "prometheus")]
        let metrics = super::MintMetricGuard::new("get_melt_bolt12_quote");

        let result = async {
            let MeltQuoteBolt12Request {
                request,
                unit,
                options,
            } = melt_request;

            let payment_backend = self
                .payment_processors
                .get(&PaymentProcessorKey::new(
                    unit.clone(),
                    PaymentMethod::Known(KnownMethod::Bolt12),
                ))
                .ok_or_else(|| {
                    tracing::info!("Could not get payment backend for {}, bolt12 ", unit);

                    Error::UnsupportedUnit
                })?;

            let offer = Offer::from_str(&melt_request.request).map_err(|_| Error::Bolt12parse)?;

            let quote_id = QuoteId::new();

            let outgoing_payment_options = Bolt12OutgoingPaymentOptions {
                offer: offer.clone(),
                max_fee_amount: None,
                timeout_secs: None,
                melt_options: *options,
                quote_id: quote_id.clone(),
            };

            let payment_quote = payment_backend
                .get_payment_quote(
                    &melt_request.unit,
                    OutgoingPaymentOptions::Bolt12(Box::new(outgoing_payment_options)),
                )
                .await
                .map_err(|err| {
                    tracing::error!(
                        "Could not get payment quote for mint quote, {} bolt12, {}",
                        unit,
                        err
                    );

                    err
                })?;

            if payment_quote.unit() != unit {
                return Err(Error::UnitMismatch);
            }

            // Validate using processor quote amount for currency conversion
            self.check_melt_request_acceptable(
                payment_quote.amount.clone(),
                PaymentMethod::Known(KnownMethod::Bolt12),
                request.clone(),
                *options,
            )
            .await?;

            // Extract values for quote creation
            let quote_amount = payment_quote.amount;
            let quote_fee = payment_quote.fee;

            let payment_request = MeltPaymentRequest::Bolt12 {
                offer: Box::new(offer),
            };

            let quote = MeltQuote::new(
                Some(quote_id),
                payment_request,
                unit.clone(),
                quote_amount.clone(),
                quote_fee,
                unix_time() + self.quote_ttl().await?.melt_ttl,
                payment_quote.request_lookup_id.clone(),
                *options,
                PaymentMethod::Known(KnownMethod::Bolt12),
                payment_quote.extra_json,
                payment_quote.estimated_blocks,
            );

            tracing::debug!(
                "New {} melt quote {} for {} {} with request id {:?}",
                quote.payment_method,
                quote.id,
                quote_amount,
                unit,
                payment_quote.request_lookup_id
            );

            if persist_quote {
                let mut tx = self.localstore.begin_transaction().await?;
                tx.add_melt_quote(quote.clone()).await?;
                tx.commit().await?;
            }
            Ok(quote)
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        result
    }

    /// Implementation of get_melt_onchain_quote
    #[instrument(skip_all)]
    async fn get_melt_onchain_quote_impl(
        &self,
        melt_request: &MeltQuoteOnchainRequest,
        persist_quote: bool,
    ) -> Result<MeltQuote, Error> {
        #[cfg(feature = "prometheus")]
        let metrics = super::MintMetricGuard::new("get_melt_onchain_quote");

        let result = async {
            let unit = &melt_request.unit;

            let payment_backend = self
                .payment_processors
                .get(&PaymentProcessorKey::new(
                    unit.clone(),
                    PaymentMethod::Known(KnownMethod::Onchain),
                ))
                .ok_or_else(|| {
                    tracing::info!("Could not get payment backend for {}, onchain ", unit);
                    Error::UnsupportedUnit
                })?;

            // Generate the authoritative `QuoteId` on the mint side *before*
            // calling the backend. The onchain contract
            // ([`OnchainOutgoingPaymentOptions.quote_id`]) requires backends to
            // echo this value verbatim in
            // [`PaymentQuoteResponse.request_lookup_id`] as
            // `PaymentIdentifier::QuoteId(..)`; we validate that echo below and
            // use our locally-generated id as the `MeltQuote.id` so the flow is
            // no longer self-referential via the backend response.
            let quote_id = QuoteId::new();

            let outgoing_payment_options = cdk_common::payment::OnchainOutgoingPaymentOptions {
                address: melt_request.request.clone(),
                amount: melt_request.amount.with_unit(unit.clone()),
                max_fee_amount: None,
                quote_id: quote_id.clone(),
                // No selection at quote creation: the backend enumerates all
                // available `fee_options` and the wallet picks one (echoed back
                // as `fee_index`) when executing the melt.
                fee_index: None,
                metadata: None,
            };

            let payment_quote = payment_backend
                .get_payment_quote(
                    unit,
                    OutgoingPaymentOptions::Onchain(Box::new(outgoing_payment_options)),
                )
                .await
                .map_err(|err| {
                    tracing::error!(
                        "Could not get payment quote for mint quote, {} onchain, {}",
                        unit,
                        err
                    );
                    err
                })?;

            if payment_quote.unit() != unit {
                return Err(Error::UnitMismatch);
            }

            // Enforce the onchain quote-id echo contract: the backend MUST return
            // `Some(PaymentIdentifier::QuoteId(quote_id))` matching the value we
            // supplied. Anything else is a contract violation; reject loudly
            // instead of silently accepting whatever the backend decided to
            // synthesize.
            match &payment_quote.request_lookup_id {
                Some(PaymentIdentifier::QuoteId(id)) if id == &quote_id => {}
                got => {
                    return Err(Error::OnchainQuoteLookupIdMismatch {
                        expected: quote_id,
                        got: got.clone(),
                    });
                }
            }

            // Validate using processor quote amount for currency conversion
            self.check_melt_request_acceptable(
                payment_quote.amount.clone(),
                PaymentMethod::Known(KnownMethod::Onchain),
                melt_request.request.clone(),
                None,
            )
            .await?;

            let melt_ttl = self.quote_ttl().await?.melt_ttl;

            // Store `request_lookup_id` deterministically from the mint-generated
            // `quote_id` rather than cloning the backend response, so the
            // persisted lookup id no longer depends on backend behavior beyond
            // the validated echo.
            let request_lookup_id = Some(PaymentIdentifier::QuoteId(quote_id.clone()));

            // Onchain backends must return explicit `fee_options`; the mint
            // validates them before persisting the quote.
            let Some(fee_options) = payment_quote.fee_options.clone() else {
                return Err(Error::OnchainFeeOptionsEmpty);
            };

            // `MeltQuote::new_onchain` applies the NUT validation. Failures are
            // returned before the quote is persisted, so a backend that violates
            // the contract never leaves state behind in the mint.
            let quote = MeltQuote::new_onchain(
                Some(quote_id),
                MeltPaymentRequest::Onchain {
                    address: melt_request.request.clone(),
                },
                unit.clone(),
                payment_quote.amount,
                unix_time() + melt_ttl,
                request_lookup_id,
                payment_quote.extra_json,
                fee_options,
            )?;

            if persist_quote {
                let mut tx = self.localstore.begin_transaction().await?;
                tx.add_melt_quote(quote.clone()).await?;
                tx.commit().await?;
            }
            Ok(quote)
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        result
    }

    /// Implementation of get_melt_custom_quote
    #[instrument(skip_all)]
    async fn get_melt_custom_quote_impl(
        &self,
        melt_request: &MeltQuoteCustomRequest,
        persist_quote: bool,
    ) -> Result<MeltQuote, Error> {
        #[cfg(feature = "prometheus")]
        let metrics = super::MintMetricGuard::new("get_melt_custom_quote");

        let result = async {
            let MeltQuoteCustomRequest {
                request,
                unit,
                method,
                amount,
                extra,
            } = melt_request;

            let payment_method = PaymentMethod::from(method.as_str());
            validate_custom_payment_method(&payment_method)?;
            validate_custom_quote_fields(request, extra)?;

            let payment_backend = self
                .payment_processors
                .get(&PaymentProcessorKey::new(
                    unit.clone(),
                    payment_method.clone(),
                ))
                .ok_or_else(|| {
                    tracing::info!("Could not get payment backend for {}, {} ", unit, method);
                    Error::UnsupportedUnit
                })?;

            // Convert extra serde_json::Value to JSON string if not null
            let extra_json = if extra.is_null() {
                None
            } else {
                Some(extra.to_string())
            };

            let quote_id = QuoteId::new();

            let custom_options =
                OutgoingPaymentOptions::Custom(Box::new(CustomOutgoingPaymentOptions {
                    method: method.to_string(),
                    request: request.clone(),
                    amount: (*amount).map(|a| a.with_unit(unit.clone())),
                    max_fee_amount: None,
                    timeout_secs: None,
                    melt_options: None,
                    extra_json,
                    quote_id: quote_id.clone(),
                }));

            let payment_quote = payment_backend
                .get_payment_quote(&melt_request.unit, custom_options)
                .await
                .map_err(|err| {
                    tracing::error!(
                        "Could not get payment quote for melt quote, {} {}, {}",
                        unit,
                        method,
                        err
                    );
                    Error::UnsupportedUnit
                })?;

            if payment_quote.unit() != unit {
                return Err(Error::UnitMismatch);
            }

            // For custom methods, we don't validate amount limits upfront since
            // the payment processor handles method-specific validation
            self.check_melt_request_acceptable(
                payment_quote.amount.clone(),
                payment_method.clone(),
                request.clone(),
                None, // Custom methods don't use options
            )
            .await?;

            let melt_ttl = self.quote_ttl().await?.melt_ttl;

            // Extract values for quote creation
            let quote_amount = payment_quote.amount;
            let quote_fee = payment_quote.fee;

            let quote = MeltQuote::new(
                Some(quote_id),
                MeltPaymentRequest::Custom {
                    method: method.to_string(),
                    request: request.clone(),
                },
                unit.clone(),
                quote_amount.clone(),
                quote_fee,
                unix_time() + melt_ttl,
                payment_quote.request_lookup_id.clone(),
                None, // Custom methods don't use options
                payment_method,
                payment_quote.extra_json,
                payment_quote.estimated_blocks,
            );

            tracing::debug!(
                "New {} melt quote {} for {} {} with request id {:?}",
                method,
                quote.id,
                quote_amount,
                unit,
                payment_quote.request_lookup_id
            );

            if persist_quote {
                let mut tx = self.localstore.begin_transaction().await?;
                tx.add_melt_quote(quote.clone()).await?;
                tx.commit().await?;
            }
            Ok(quote)
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        result
    }

    /// Check melt quote status
    #[instrument(skip(self))]
    pub async fn check_melt_quote(
        &self,
        quote_id: &QuoteId,
    ) -> Result<MeltQuoteResponse<QuoteId>, Error> {
        #[cfg(feature = "prometheus")]
        let metrics = super::MintMetricGuard::new("check_melt_quote");

        let result = async {
            let mut quote = self
                .localstore
                .get_melt_quote(quote_id)
                .await?
                .ok_or(Error::UnknownQuote)?;

            #[cfg(feature = "federation")]
            let reconcile_backend = self.federation_config().is_none();
            #[cfg(not(feature = "federation"))]
            let reconcile_backend = true;
            if reconcile_backend {
                self.handle_pending_melt_quote(&mut quote).await?;
            }

            let blind_signatures = self
                .localstore
                .get_blind_signatures_for_quote(quote_id)
                .await?;

            let change = (!blind_signatures.is_empty()).then_some(blind_signatures);

            let response = match quote.payment_method {
                PaymentMethod::Known(KnownMethod::Bolt11) => {
                    MeltQuoteResponse::Bolt11(MeltQuoteBolt11Response {
                        quote: quote.id.clone(),
                        state: quote.state,
                        expiry: quote.expiry,
                        amount: quote.amount().into(),
                        fee_reserve: quote.fee_reserve().into(),
                        payment_preimage: quote.payment_proof.clone(),
                        change: change.clone(),
                        request: Some(quote.request.to_string()),
                        unit: Some(quote.unit.clone()),
                        method: PaymentMethod::Known(KnownMethod::Bolt11),
                    })
                }
                PaymentMethod::Known(KnownMethod::Bolt12) => {
                    MeltQuoteResponse::Bolt12(MeltQuoteBolt12Response {
                        quote: quote.id.clone(),
                        state: quote.state,
                        expiry: quote.expiry,
                        amount: quote.amount().into(),
                        fee_reserve: quote.fee_reserve().into(),
                        payment_preimage: quote.payment_proof.clone(),
                        change: change.clone(),
                        request: Some(quote.request.to_string()),
                        unit: Some(quote.unit.clone()),
                        method: PaymentMethod::Known(KnownMethod::Bolt12),
                    })
                }
                PaymentMethod::Known(KnownMethod::Onchain) => {
                    let mut response: MeltQuoteOnchainResponse<QuoteId> = quote.clone().into();
                    response.change = change.clone();
                    MeltQuoteResponse::Onchain(response)
                }
                _ => {
                    let custom_response: MeltQuoteCustomResponse<QuoteId> = quote.clone().into();
                    MeltQuoteResponse::Custom((quote.payment_method.clone(), custom_response))
                }
            };

            Ok(response)
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        result
    }

    /// Get the configured payment method for an existing melt quote.
    #[instrument(skip(self))]
    pub async fn get_melt_quote_method(&self, quote_id: &QuoteId) -> Result<PaymentMethod, Error> {
        let quote = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        Ok(quote.payment_method)
    }

    /// Get melt quotes
    #[instrument(skip_all)]
    pub async fn melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let quotes = self.localstore.get_melt_quotes().await?;
        Ok(quotes)
    }

    /// Melt
    ///
    /// Uses MeltSaga typestate pattern for atomic transaction handling with automatic rollback on failure.
    #[instrument(skip_all)]
    pub async fn melt(&self, melt_request: &MeltRequest<QuoteId>) -> Result<PendingMelt, Error> {
        #[cfg(feature = "federation")]
        self.reject_standalone_federation_path("melt")?;

        // Check max outputs limit (if change outputs are provided)
        if let Some(outputs) = melt_request.outputs() {
            let outputs_count = outputs.len();
            if outputs_count > self.max_outputs {
                tracing::warn!(
                    "Melt request exceeds max outputs limit: {} > {}",
                    outputs_count,
                    self.max_outputs
                );
                return Err(Error::MaxOutputsExceeded {
                    actual: outputs_count,
                    max: self.max_outputs,
                });
            }
        }

        let verification = self.verify_inputs(melt_request.inputs()).await?;

        // Fetch the quote to get payment_method for operation tracking
        let quote_id = melt_request.quote().clone();
        let quote = self
            .localstore
            .get_melt_quote(&quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        // Keep status checks and payment events behind the live dispatch for
        // this quote. Without this process-local guard, a check could observe
        // Unpaid before `make_payment` dispatches, compensate the saga, and
        // then allow the payment task to continue.
        let quote_lock = self.melt_quote_lock(&quote_id).await;
        let active_melt_guard = quote_lock.lock_owned().await;

        let init_saga = MeltSaga::new(
            std::sync::Arc::new(self.clone()),
            self.localstore.clone(),
            std::sync::Arc::clone(&self.pubsub_manager),
        );

        // Step 1: Setup (TX1 - reserves inputs and outputs)
        let setup_saga = init_saga
            .setup_melt(melt_request, verification, quote.payment_method.clone())
            .await?;

        let melt_request_owned = melt_request.clone();
        let quote_id_for_log = quote_id.clone();
        let localstore = self.localstore();
        let pubsub = self.pubsub_manager();

        let quote_for_spawn = quote.clone();
        let completion = tokio::spawn(async move {
            tracing::debug!(
                "Starting background melt completion for quote: {}",
                quote_id_for_log
            );

            // Step 2: Attempt internal settlement (returns saga + SettlementDecision)
            // Note: Compensation is handled internally if this fails
            let result = match setup_saga
                .attempt_internal_settlement(&melt_request_owned)
                .await
            {
                Ok((setup_saga, settlement)) => {
                    // Step 3: Make payment (internal or external)
                    let payment_outcome = setup_saga.make_payment(settlement).await;

                    // Once the backend has returned a pending/unknown outcome,
                    // dispatch is complete and explicit quote checks may safely
                    // reconcile it. Confirmed payments keep the guard through
                    // finalization.
                    if matches!(&payment_outcome, Ok(PaymentOutcome::Pending { .. })) {
                        drop(active_melt_guard);
                    }

                    match payment_outcome {
                        Ok(PaymentOutcome::Confirmed(payment_saga)) => {
                            // Step 4: Finalize (TX2 - marks spent, issues change)
                            payment_saga.finalize().await
                        }
                        Ok(PaymentOutcome::Pending {
                            #[cfg(feature = "prometheus")]
                            metrics,
                        }) => {
                            let result = async {
                                // Wait for the background task to complete the payment
                                let kind = match quote_for_spawn.payment_method {
                                    PaymentMethod::Known(KnownMethod::Bolt11) => {
                                        Kind::Bolt11MeltQuote
                                    }
                                    PaymentMethod::Known(KnownMethod::Bolt12) => {
                                        Kind::Bolt12MeltQuote
                                    }
                                    PaymentMethod::Known(KnownMethod::Onchain) => {
                                        Kind::OnchainMeltQuote
                                    }
                                    PaymentMethod::Custom(ref method) => {
                                        Kind::Custom(format!("{}_melt_quote", method))
                                    }
                                };

                                let sub_id = crate::subscription::SubId::from(format!(
                                    "melt_sync_wait_{}",
                                    quote_id_for_log
                                ));
                                let params = Params {
                                    id: Arc::new(sub_id),
                                    kind,
                                    filters: vec![quote_id_for_log.to_string()],
                                };

                                let mut sub = match pubsub.subscribe(params) {
                                    Ok(sub) => sub,
                                    Err(_err) => return Err(Error::Internal),
                                };

                                let start = tokio::time::Instant::now();

                                loop {
                                    if let Some(response) = Self::load_settled_melt_response(
                                        &localstore,
                                        &quote_id_for_log,
                                    )
                                    .await?
                                    {
                                        return Ok(response);
                                    }

                                    if start.elapsed() >= pending_melt_wait_timeout() {
                                        return Err(Error::PendingMeltTimeout {
                                            last_backend_error: None,
                                        });
                                    }

                                    let Ok(maybe_event) = tokio::time::timeout(
                                        pending_melt_notification_wait_interval(),
                                        sub.recv(),
                                    )
                                    .await
                                    else {
                                        continue;
                                    };

                                    if let Some(event) = maybe_event {
                                        let (event_quote_id, state) = match event.into_inner() {
                                            NotificationPayload::MeltQuoteBolt11Response(r) => {
                                                (r.quote, r.state)
                                            }
                                            NotificationPayload::MeltQuoteBolt12Response(r) => {
                                                (r.quote, r.state)
                                            }
                                            NotificationPayload::CustomMeltQuoteResponse(_, r) => {
                                                (r.quote, r.state)
                                            }
                                            // Onchain is the one method where the subscription
                                            // fast-path is most valuable, because confirmation is
                                            // inherently asynchronous. We subscribe to
                                            // `OnchainMeltQuote` events above (see `Kind` mapping)
                                            // and must handle them here; otherwise onchain waiters
                                            // would rely solely on the
                                            // explicit quote-check path.
                                            NotificationPayload::MeltQuoteOnchainResponse(r) => {
                                                (r.quote, r.state)
                                            }
                                            _ => continue,
                                        };

                                        if event_quote_id == quote_id_for_log
                                            && state != MeltQuoteState::Pending
                                        {
                                            if let Some(response) =
                                                Self::load_settled_melt_response(
                                                    &localstore,
                                                    &quote_id_for_log,
                                                )
                                                .await?
                                            {
                                                return Ok(response);
                                            }
                                        }
                                    }
                                }
                            }
                            .await;

                            #[cfg(feature = "prometheus")]
                            if let Some(metrics) = metrics {
                                metrics.record(result.is_ok());
                            }

                            result
                        }
                        Err(err) => Err(err),
                    }
                }
                Err(err) => Err(err),
            };

            match &result {
                Ok(_) => {
                    tracing::info!(
                        "Background melt completed successfully for quote: {}",
                        quote_id_for_log
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "Background melt completion failed for quote {}: {}",
                        quote_id_for_log,
                        e
                    );
                }
            }

            result
        });

        // Return immediately with the quote in PENDING state and an awaitable completion future.
        let mut quote_clone = quote.clone();
        quote_clone.state = MeltQuoteState::Pending;
        let response = quote_clone.into_response(None);

        Ok(PendingMelt {
            response,
            completion,
        })
    }

    /// Process a federation melt after the exact request has been accepted by consensus.
    #[cfg(feature = "federation")]
    #[instrument(skip_all)]
    pub async fn melt_federation(
        &self,
        melt_request: &MeltRequest<QuoteId>,
        federation_operation_id: FederationOperationId,
    ) -> Result<MeltQuoteResponse<QuoteId>, Error> {
        self.melt_federation_with_accepted_time(melt_request, federation_operation_id, 0)
            .await
    }

    #[cfg(feature = "federation")]
    async fn ensure_terminal_federation_melt_matches_request(
        localstore: &DynMintDatabase,
        quote_id: &QuoteId,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<(), Error> {
        let stored_input_ys = localstore.get_proof_ys_by_quote_id(quote_id).await?;
        let request_input_ys = melt_request.inputs().ys()?;
        let stored_input_set = stored_input_ys.iter().copied().collect::<HashSet<_>>();
        let request_input_set = request_input_ys.iter().copied().collect::<HashSet<_>>();
        if stored_input_ys.len() != request_input_ys.len() || stored_input_set != request_input_set
        {
            return Err(Error::RequestAlreadyPaid);
        }

        let stored_change_blinded_secrets =
            localstore.get_blinded_secrets_for_quote(quote_id).await?;
        let request_change_outputs = melt_request.outputs().as_deref().unwrap_or(&[]);
        if request_change_outputs.len() < stored_change_blinded_secrets.len() {
            return Err(Error::RequestAlreadyPaid);
        }
        let request_change_blinded_secrets = request_change_outputs
            .iter()
            .take(stored_change_blinded_secrets.len())
            .map(|output| output.blinded_secret)
            .collect::<Vec<_>>();
        if stored_change_blinded_secrets != request_change_blinded_secrets {
            return Err(Error::RequestAlreadyPaid);
        }

        Ok(())
    }

    #[cfg(feature = "federation")]
    pub(crate) async fn resume_paid_federation_melt_finalization(
        &self,
        quote: &mut MeltQuote,
        saga: &cdk_common::mint::Saga,
        federation_operation_id: FederationOperationId,
    ) -> Result<(), Error> {
        let operation_id = federation_operation_id.to_string();
        if saga.federation_operation_id.as_deref() != Some(operation_id.as_str()) {
            return Err(Error::Internal);
        }
        if saga.operation_id != melt_saga::federation_operation_uuid(federation_operation_id) {
            return Err(Error::Internal);
        }
        if saga.state
            != cdk_common::mint::SagaStateEnum::Melt(cdk_common::mint::MeltSagaState::Finalizing)
        {
            return Err(Error::Internal);
        }
        let finalization_data = saga.finalization_data.clone().ok_or(Error::Internal)?;
        let payment_response = cdk_common::payment::MakePaymentResponse {
            payment_lookup_id: finalization_data.payment_lookup_id,
            payment_proof: finalization_data.payment_proof,
            status: MeltQuoteState::Paid,
            total_spent: finalization_data.total_spent,
        };

        crate::mint::saga_recovery::process_melt_saga_outcome(
            saga,
            quote,
            &payment_response,
            &self.localstore,
            &self.pubsub_manager,
            self,
        )
        .await
    }

    /// Process a federation melt after the exact request has been accepted by consensus.
    #[cfg(feature = "federation")]
    #[instrument(skip_all)]
    pub(crate) async fn melt_federation_with_accepted_time(
        &self,
        melt_request: &MeltRequest<QuoteId>,
        federation_operation_id: FederationOperationId,
        accepted_time: u64,
    ) -> Result<MeltQuoteResponse<QuoteId>, Error> {
        let federation_config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        federation_config.validate_melt_change_outputs(melt_request)?;
        if let Some(outputs) = melt_request.outputs() {
            let outputs_count = outputs.len();
            if outputs_count > self.max_outputs {
                tracing::warn!(
                    "Federation melt request exceeds max outputs limit: {} > {}",
                    outputs_count,
                    self.max_outputs
                );
                return Err(Error::MaxOutputsExceeded {
                    actual: outputs_count,
                    max: self.max_outputs,
                });
            }
        }

        let verification = self
            .verify_federation_inputs_with_context(
                melt_request.inputs(),
                super::federation::FederationProofVerificationContext::MeltApply,
            )
            .await?;
        let quote_id = melt_request.quote().clone();
        let quote = self
            .localstore
            .get_melt_quote(&quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        match quote.state {
            MeltQuoteState::Unpaid | MeltQuoteState::Failed => {}
            MeltQuoteState::Pending => {
                let saga = self.localstore.get_melt_saga_by_quote_id(&quote_id).await?;
                let operation_id = federation_operation_id.to_string();
                if saga
                    .as_ref()
                    .and_then(|saga| saga.federation_operation_id.as_deref())
                    != Some(operation_id.as_str())
                {
                    return Err(Error::PendingQuote);
                }
            }
            MeltQuoteState::Paid => {
                Self::ensure_terminal_federation_melt_matches_request(
                    &self.localstore,
                    &quote_id,
                    melt_request,
                )
                .await?;
            }
            MeltQuoteState::Unknown => {
                // Defensive only: backend uncertainty must remain a transient
                // payment observation, not a durable federated melt quote
                // state. Base SQL schemas reject UNKNOWN for melt quotes.
                tracing::warn!(
                    "Federated melt quote {} has unexpected durable Unknown state",
                    quote_id
                );
                return Err(Error::UnknownPaymentState);
            }
        }

        if quote.state == MeltQuoteState::Paid {
            if let Some(saga) = self.localstore.get_melt_saga_by_quote_id(&quote_id).await? {
                let mut paid_quote = quote.clone();
                self.resume_paid_federation_melt_finalization(
                    &mut paid_quote,
                    &saga,
                    federation_operation_id,
                )
                .await?;
            }

            let completed_operation_id =
                melt_saga::federation_operation_uuid(federation_operation_id);
            let completed_operation = self
                .localstore
                .get_completed_operation(&completed_operation_id)
                .await?
                .ok_or(Error::Internal)?;
            if completed_operation.kind() != cdk_common::mint::OperationKind::Melt {
                return Err(Error::Internal);
            }

            let quote = self
                .localstore
                .get_melt_quote(&quote_id)
                .await?
                .ok_or(Error::UnknownQuote)?;
            let change = self
                .localstore
                .get_blind_signatures_for_quote(&quote_id)
                .await?;
            let change_opt = if change.is_empty() {
                None
            } else {
                Some(change)
            };

            return Ok(quote.into_response(change_opt));
        }

        if quote.state == MeltQuoteState::Pending {
            let change = self
                .localstore
                .get_blind_signatures_for_quote(&quote_id)
                .await?;
            let change_opt = if change.is_empty() {
                None
            } else {
                Some(change)
            };

            return Ok(quote.into_response(change_opt));
        }

        let init_saga = MeltSaga::new(
            Arc::new(self.clone()),
            self.localstore.clone(),
            Arc::clone(&self.pubsub_manager),
        )
        .with_federation_operation_id(federation_operation_id);
        let setup_saga = init_saga
            .setup_melt(melt_request, verification, quote.payment_method.clone())
            .await?;
        match setup_saga
            .finalize_internal_settlement_federation(federation_operation_id, accepted_time)
            .await?
        {
            FederationInternalSettlementOutcome::Finalized(response) => return Ok(response),
            FederationInternalSettlementOutcome::External(_setup_saga) => {
                let mut quote = quote;
                quote.state = MeltQuoteState::Pending;
                Ok(quote.into_response(None))
            }
        }
    }
}

//! Fixed msat/sat [`MintPayment`](cdk_common::payment::MintPayment) decorator.

use std::pin::Pin;

use async_trait::async_trait;
use cdk_common::amount::MSAT_IN_SAT;
use cdk_common::nuts::CurrencyUnit;
use cdk_common::payment::{
    CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse, MintPayment,
    OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse, SettingsResponse,
    WaitPaymentResponse,
};
use cdk_common::Amount;
use futures::{Stream, StreamExt};

/// Decorates a sat-denominated payment backend as an msat-denominated processor.
#[derive(Debug, Clone)]
pub struct MsatSatConverter<T> {
    inner: T,
}

impl<T> MsatSatConverter<T> {
    /// Create a new fixed-ratio msat/sat converter.
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<T> MintPayment for MsatSatConverter<T>
where
    T: MintPayment<Err = cdk_common::payment::Error> + Send + Sync,
{
    type Err = cdk_common::payment::Error;

    #[tracing::instrument(skip_all)]
    async fn start(&self) -> Result<(), Self::Err> {
        self.inner.start().await
    }

    #[tracing::instrument(skip_all)]
    async fn stop(&self) -> Result<(), Self::Err> {
        self.inner.stop().await
    }

    #[tracing::instrument(skip_all)]
    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        let inner = self.inner.get_settings().await?;
        if !inner.unit.eq_ignore_ascii_case("sat") {
            tracing::error!(
                inner_unit = %inner.unit,
                "MsatSatConverter inner backend is not sat-denominated; conversions will be wrong"
            );
        }
        Ok(SettingsResponse {
            unit: CurrencyUnit::Msat.to_string(),
            bolt11: inner.bolt11,
            bolt12: inner.bolt12,
            custom: inner.custom,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        self.inner
            .create_incoming_payment_request(convert_incoming_options_to_sat(options)?)
            .await
    }

    #[tracing::instrument(skip_all)]
    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        ensure_msat_unit(unit)?;
        let quote = self
            .inner
            .get_payment_quote(
                &CurrencyUnit::Sat,
                convert_outgoing_options_to_sat(options)?,
            )
            .await?;
        Ok(PaymentQuoteResponse {
            request_lookup_id: quote.request_lookup_id,
            amount: sats_to_msats(quote.amount)?,
            fee: sats_to_msats(quote.fee)?,
            state: quote.state,
            extra_json: quote.extra_json,
        })
    }

    #[tracing::instrument(skip_all)]
    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        ensure_msat_unit(unit)?;
        let response = self
            .inner
            .make_payment(
                &CurrencyUnit::Sat,
                convert_outgoing_options_to_sat(options)?,
            )
            .await?;
        convert_make_payment_response_to_msat(response)
    }

    #[tracing::instrument(skip_all)]
    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        let stream = self.inner.wait_payment_event().await?;
        Ok(Box::pin(stream.filter_map(|event| async move {
            match convert_event_to_msat(event) {
                Ok(msat_event) => Some(msat_event),
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "failed to convert payment event to msat; dropping event"
                    );
                    None
                }
            }
        })))
    }

    fn is_payment_event_stream_active(&self) -> bool {
        self.inner.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        self.inner.cancel_payment_event_stream();
    }

    #[tracing::instrument(skip_all)]
    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        self.inner
            .check_incoming_payment_status(payment_identifier)
            .await?
            .into_iter()
            .map(convert_wait_payment_response_to_msat)
            .collect()
    }

    #[tracing::instrument(skip_all)]
    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        convert_make_payment_response_to_msat(
            self.inner
                .check_outgoing_payment(payment_identifier)
                .await?,
        )
    }
}

fn ensure_msat_unit(unit: &CurrencyUnit) -> Result<(), cdk_common::payment::Error> {
    if unit == &CurrencyUnit::Msat {
        Ok(())
    } else {
        Err(cdk_common::payment::Error::UnsupportedUnit)
    }
}

fn msats_to_sats(
    amount: Amount<CurrencyUnit>,
) -> Result<Amount<CurrencyUnit>, cdk_common::payment::Error> {
    if amount.unit() != &CurrencyUnit::Msat {
        return Err(cdk_common::payment::Error::UnsupportedUnit);
    }
    Ok(Amount::new(
        div_ceil(amount.value(), MSAT_IN_SAT),
        CurrencyUnit::Sat,
    ))
}

fn sats_to_msats(
    amount: Amount<CurrencyUnit>,
) -> Result<Amount<CurrencyUnit>, cdk_common::payment::Error> {
    if amount.unit() != &CurrencyUnit::Sat {
        return Err(cdk_common::payment::Error::UnsupportedUnit);
    }
    Ok(Amount::new(
        amount.value().checked_mul(MSAT_IN_SAT).ok_or_else(|| {
            cdk_common::payment::Error::Custom("msat amount overflow".to_string())
        })?,
        CurrencyUnit::Msat,
    ))
}

fn convert_incoming_options_to_sat(
    options: IncomingPaymentOptions,
) -> Result<IncomingPaymentOptions, cdk_common::payment::Error> {
    match options {
        IncomingPaymentOptions::Bolt11(mut options) => {
            options.amount = msats_to_sats(options.amount)?;
            Ok(IncomingPaymentOptions::Bolt11(options))
        }
        IncomingPaymentOptions::Bolt12(mut options) => {
            if let Some(amount) = options.amount {
                options.amount = Some(msats_to_sats(amount)?);
            }
            Ok(IncomingPaymentOptions::Bolt12(options))
        }
        IncomingPaymentOptions::Custom(mut options) => {
            options.amount = msats_to_sats(options.amount)?;
            Ok(IncomingPaymentOptions::Custom(options))
        }
    }
}

fn convert_outgoing_options_to_sat(
    options: OutgoingPaymentOptions,
) -> Result<OutgoingPaymentOptions, cdk_common::payment::Error> {
    match options {
        OutgoingPaymentOptions::Bolt11(mut options) => {
            if let Some(amount) = options.max_fee_amount {
                options.max_fee_amount = Some(msats_to_sats(amount)?);
            }
            Ok(OutgoingPaymentOptions::Bolt11(options))
        }
        OutgoingPaymentOptions::Bolt12(mut options) => {
            if let Some(amount) = options.max_fee_amount {
                options.max_fee_amount = Some(msats_to_sats(amount)?);
            }
            Ok(OutgoingPaymentOptions::Bolt12(options))
        }
        OutgoingPaymentOptions::Custom(mut options) => {
            if let Some(amount) = options.max_fee_amount {
                options.max_fee_amount = Some(msats_to_sats(amount)?);
            }
            Ok(OutgoingPaymentOptions::Custom(options))
        }
    }
}

fn convert_event_to_msat(event: Event) -> Result<Event, cdk_common::payment::Error> {
    match event {
        Event::PaymentReceived(payment) => Ok(Event::PaymentReceived(
            convert_wait_payment_response_to_msat(payment)?,
        )),
        Event::PaymentSuccessful { quote_id, details } => Ok(Event::PaymentSuccessful {
            quote_id,
            details: convert_make_payment_response_to_msat(details)?,
        }),
        Event::PaymentFailed { quote_id, reason } => Ok(Event::PaymentFailed { quote_id, reason }),
    }
}

fn convert_wait_payment_response_to_msat(
    payment: WaitPaymentResponse,
) -> Result<WaitPaymentResponse, cdk_common::payment::Error> {
    Ok(WaitPaymentResponse {
        payment_identifier: payment.payment_identifier,
        payment_amount: sats_to_msats(payment.payment_amount)?,
        payment_id: payment.payment_id,
    })
}

fn convert_make_payment_response_to_msat(
    response: MakePaymentResponse,
) -> Result<MakePaymentResponse, cdk_common::payment::Error> {
    Ok(MakePaymentResponse {
        payment_lookup_id: response.payment_lookup_id,
        payment_proof: response.payment_proof,
        status: response.status,
        total_spent: sats_to_msats(response.total_spent)?,
    })
}

fn div_ceil(numerator: u64, denominator: u64) -> u64 {
    numerator / denominator + u64::from(numerator % denominator != 0)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use cdk_common::nuts::MeltQuoteState;
    use cdk_common::payment::{Bolt11IncomingPaymentOptions, CustomOutgoingPaymentOptions};
    use cdk_common::QuoteId;
    use futures::stream;

    use super::*;

    #[derive(Debug, Clone, Default)]
    struct MockSatPayment {
        incoming_amounts: Arc<Mutex<Vec<Amount<CurrencyUnit>>>>,
        quote: Arc<Mutex<Option<PaymentQuoteResponse>>>,
        incoming_status: Arc<Mutex<Vec<WaitPaymentResponse>>>,
        make_response: Arc<Mutex<Option<MakePaymentResponse>>>,
        check_response: Arc<Mutex<Option<MakePaymentResponse>>>,
        events: Arc<Mutex<Vec<Event>>>,
    }

    #[async_trait]
    impl MintPayment for MockSatPayment {
        type Err = cdk_common::payment::Error;

        async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
            Ok(SettingsResponse {
                unit: CurrencyUnit::Sat.to_string(),
                bolt11: None,
                bolt12: None,
                custom: Default::default(),
            })
        }

        async fn create_incoming_payment_request(
            &self,
            options: IncomingPaymentOptions,
        ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
            let IncomingPaymentOptions::Bolt11(options) = options else {
                return Err(cdk_common::payment::Error::UnsupportedPaymentOption);
            };
            self.incoming_amounts
                .lock()
                .expect("incoming amounts mutex should not be poisoned")
                .push(options.amount);
            Ok(CreateIncomingPaymentResponse {
                request_lookup_id: PaymentIdentifier::CustomId("quote".to_string()),
                request: "invoice".to_string(),
                expiry: None,
                extra_json: None,
            })
        }

        async fn get_payment_quote(
            &self,
            unit: &CurrencyUnit,
            _options: OutgoingPaymentOptions,
        ) -> Result<PaymentQuoteResponse, Self::Err> {
            if unit != &CurrencyUnit::Sat {
                return Err(cdk_common::payment::Error::UnsupportedUnit);
            }
            self.quote
                .lock()
                .expect("quote mutex should not be poisoned")
                .clone()
                .ok_or_else(|| cdk_common::payment::Error::Custom("missing quote".to_string()))
        }

        async fn make_payment(
            &self,
            _unit: &CurrencyUnit,
            _options: OutgoingPaymentOptions,
        ) -> Result<MakePaymentResponse, Self::Err> {
            self.make_response
                .lock()
                .expect("make response mutex should not be poisoned")
                .clone()
                .ok_or_else(|| cdk_common::payment::Error::Custom("missing response".to_string()))
        }

        async fn wait_payment_event(
            &self,
        ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
            Ok(Box::pin(stream::iter(
                self.events
                    .lock()
                    .expect("events mutex should not be poisoned")
                    .clone(),
            )))
        }

        fn is_payment_event_stream_active(&self) -> bool {
            false
        }

        fn cancel_payment_event_stream(&self) {}

        async fn check_incoming_payment_status(
            &self,
            _payment_identifier: &PaymentIdentifier,
        ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
            Ok(self
                .incoming_status
                .lock()
                .expect("incoming status mutex should not be poisoned")
                .clone())
        }

        async fn check_outgoing_payment(
            &self,
            _payment_identifier: &PaymentIdentifier,
        ) -> Result<MakePaymentResponse, Self::Err> {
            self.check_response
                .lock()
                .expect("check response mutex should not be poisoned")
                .clone()
                .ok_or_else(|| cdk_common::payment::Error::Custom("missing response".to_string()))
        }
    }

    fn custom_outgoing_options() -> OutgoingPaymentOptions {
        OutgoingPaymentOptions::Custom(Box::new(CustomOutgoingPaymentOptions {
            method: "test".to_string(),
            request: "request".to_string(),
            max_fee_amount: None,
            timeout_secs: None,
            melt_options: None,
            extra_json: None,
        }))
    }

    #[tokio::test]
    async fn incoming_msat_amounts_round_up_to_sats() {
        let backend = MockSatPayment::default();
        let converter = MsatSatConverter::new(backend.clone());

        converter
            .create_incoming_payment_request(IncomingPaymentOptions::Bolt11(
                Bolt11IncomingPaymentOptions {
                    amount: Amount::new(1_001, CurrencyUnit::Msat),
                    ..Default::default()
                },
            ))
            .await
            .expect("1001 msat quote should be converted");
        converter
            .create_incoming_payment_request(IncomingPaymentOptions::Bolt11(
                Bolt11IncomingPaymentOptions {
                    amount: Amount::new(1_000, CurrencyUnit::Msat),
                    ..Default::default()
                },
            ))
            .await
            .expect("1000 msat quote should be converted");

        let amounts = backend
            .incoming_amounts
            .lock()
            .expect("incoming amounts mutex should not be poisoned");
        assert_eq!(amounts[0], Amount::new(2, CurrencyUnit::Sat));
        assert_eq!(amounts[1], Amount::new(1, CurrencyUnit::Sat));
    }

    #[tokio::test]
    async fn incoming_zero_msat_converts_to_zero_sats() {
        let backend = MockSatPayment::default();
        let converter = MsatSatConverter::new(backend.clone());

        converter
            .create_incoming_payment_request(IncomingPaymentOptions::Bolt11(
                Bolt11IncomingPaymentOptions {
                    amount: Amount::new(0, CurrencyUnit::Msat),
                    ..Default::default()
                },
            ))
            .await
            .expect("zero msat quote should be converted without forcing a minimum");

        let amounts = backend
            .incoming_amounts
            .lock()
            .expect("incoming amounts mutex should not be poisoned");
        assert_eq!(amounts[0], Amount::new(0, CurrencyUnit::Sat));
    }

    #[tokio::test]
    async fn incoming_sat_status_converts_exactly_to_msats() {
        let backend = MockSatPayment::default();
        backend
            .incoming_status
            .lock()
            .expect("incoming status mutex should not be poisoned")
            .push(WaitPaymentResponse {
                payment_identifier: PaymentIdentifier::CustomId("paid".to_string()),
                payment_amount: Amount::new(1, CurrencyUnit::Sat),
                payment_id: "payment-id".to_string(),
            });
        let converter = MsatSatConverter::new(backend);

        let payments = converter
            .check_incoming_payment_status(&PaymentIdentifier::CustomId("paid".to_string()))
            .await
            .expect("sat status should convert to msat");

        assert_eq!(
            payments[0].payment_amount,
            Amount::new(1_000, CurrencyUnit::Msat)
        );
    }

    #[tokio::test]
    async fn quote_fee_converts_from_sats_to_msats() {
        let backend = MockSatPayment::default();
        *backend
            .quote
            .lock()
            .expect("quote mutex should not be poisoned") = Some(PaymentQuoteResponse {
            request_lookup_id: Some(PaymentIdentifier::CustomId("melt".to_string())),
            amount: Amount::new(2, CurrencyUnit::Sat),
            fee: Amount::new(1, CurrencyUnit::Sat),
            state: MeltQuoteState::Unpaid,
            extra_json: None,
        });
        let converter = MsatSatConverter::new(backend);

        let quote = converter
            .get_payment_quote(
                &CurrencyUnit::Msat,
                OutgoingPaymentOptions::Custom(Box::new(CustomOutgoingPaymentOptions {
                    method: "test".to_string(),
                    request: "request".to_string(),
                    max_fee_amount: None,
                    timeout_secs: None,
                    melt_options: None,
                    extra_json: None,
                })),
            )
            .await
            .expect("sat quote should convert to msat");

        assert_eq!(quote.amount, Amount::new(2_000, CurrencyUnit::Msat));
        assert_eq!(quote.fee, Amount::new(1_000, CurrencyUnit::Msat));
    }

    #[test]
    fn payment_successful_event_converts_sat_amount_to_msat() {
        let quote_id = QuoteId::new_uuid();
        let event = Event::PaymentSuccessful {
            quote_id: quote_id.clone(),
            details: MakePaymentResponse {
                payment_lookup_id: PaymentIdentifier::CustomId("payment".to_string()),
                payment_proof: Some("proof".to_string()),
                status: MeltQuoteState::Paid,
                total_spent: Amount::new(3, CurrencyUnit::Sat),
            },
        };

        let converted = convert_event_to_msat(event).expect("event should convert");

        match converted {
            Event::PaymentSuccessful {
                quote_id: id,
                details,
            } => {
                assert_eq!(id, quote_id);
                assert_eq!(details.total_spent, Amount::new(3_000, CurrencyUnit::Msat));
            }
            _ => panic!("expected payment successful event"),
        }
    }

    #[test]
    fn payment_failed_event_passes_through_without_amount() {
        let quote_id = QuoteId::new_uuid();
        let event = Event::PaymentFailed {
            quote_id: quote_id.clone(),
            reason: "failed".to_string(),
        };

        let converted = convert_event_to_msat(event).expect("event should pass through");

        match converted {
            Event::PaymentFailed {
                quote_id: id,
                reason,
            } => {
                assert_eq!(id, quote_id);
                assert_eq!(reason, "failed");
            }
            _ => panic!("expected payment failed event"),
        }
    }

    #[test]
    fn sats_to_msats_overflow_fails_gracefully() {
        let result = sats_to_msats(Amount::new(u64::MAX / 2, CurrencyUnit::Sat));

        assert!(result.is_err());
    }

    #[test]
    fn ensure_msat_unit_rejects_non_msat_amounts() {
        assert!(ensure_msat_unit(&CurrencyUnit::Sat).is_err());
        assert!(ensure_msat_unit(&CurrencyUnit::Msat).is_ok());
    }

    #[tokio::test]
    async fn make_payment_response_converts_total_spent_to_msat() {
        let backend = MockSatPayment::default();
        *backend
            .make_response
            .lock()
            .expect("make response mutex should not be poisoned") = Some(MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("payment".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Paid,
            total_spent: Amount::new(4, CurrencyUnit::Sat),
        });
        let converter = MsatSatConverter::new(backend);

        let response = converter
            .make_payment(&CurrencyUnit::Msat, custom_outgoing_options())
            .await
            .expect("sat response should convert to msat");

        assert_eq!(response.total_spent, Amount::new(4_000, CurrencyUnit::Msat));
    }

    #[tokio::test]
    async fn check_outgoing_payment_response_converts_total_spent_to_msat() {
        let backend = MockSatPayment::default();
        *backend
            .check_response
            .lock()
            .expect("check response mutex should not be poisoned") = Some(MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("payment".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Paid,
            total_spent: Amount::new(5, CurrencyUnit::Sat),
        });
        let converter = MsatSatConverter::new(backend);

        let response = converter
            .check_outgoing_payment(&PaymentIdentifier::CustomId("payment".to_string()))
            .await
            .expect("sat response should convert to msat");

        assert_eq!(response.total_spent, Amount::new(5_000, CurrencyUnit::Msat));
    }
}

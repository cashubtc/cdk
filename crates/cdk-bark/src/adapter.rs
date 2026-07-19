//! Non-mutating `MintPayment` adapter for federated Bark.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::payment::{
    Bolt11Settings, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions,
    MakePaymentResponse, MintPayment, OutgoingPaymentOptions, PaymentIdentifier,
    PaymentQuoteResponse, SettingsResponse, WaitPaymentResponse,
};
use cdk_common::CurrencyUnit;
use futures::{stream, Stream};

use crate::{Error, FederatedBarkLimits, FederatedBarkRuntime};

/// Payment-backend facade that leaves all mutations to accepted Bark workers.
#[derive(Clone)]
pub struct FederatedBarkPaymentAdapter {
    runtime: Arc<FederatedBarkRuntime>,
    limits: FederatedBarkLimits,
}

impl std::fmt::Debug for FederatedBarkPaymentAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FederatedBarkPaymentAdapter")
            .field("scope", &self.runtime.scope())
            .field("limits", &self.limits)
            .finish()
    }
}

impl FederatedBarkPaymentAdapter {
    /// Construct the adapter over an already validated runtime.
    #[must_use]
    pub fn new(runtime: Arc<FederatedBarkRuntime>) -> Self {
        let limits = runtime.limits();
        Self { runtime, limits }
    }

    /// Accepted satoshi payment limits.
    #[must_use]
    pub const fn limits(&self) -> FederatedBarkLimits {
        self.limits
    }

    fn consensus_only<T>() -> Result<T, Error> {
        Err(Error::ConsensusOnly)
    }
}

#[async_trait]
impl MintPayment for FederatedBarkPaymentAdapter {
    type Err = Error;

    async fn start(&self) -> Result<(), Self::Err> {
        self.runtime.ensure_ready(0).await
    }

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        Ok(SettingsResponse {
            unit: CurrencyUnit::Sat.to_string(),
            bolt11: Some(Bolt11Settings {
                mpp: false,
                amountless: false,
                invoice_description: true,
            }),
            bolt12: None,
            onchain: None,
            custom: std::collections::HashMap::new(),
        })
    }

    async fn create_incoming_payment_request(
        &self,
        _options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        Self::consensus_only()
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        _options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(Error::UnsupportedUnit);
        }
        Self::consensus_only()
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        _options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(Error::UnsupportedUnit);
        }
        Self::consensus_only()
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        Ok(Box::pin(stream::pending()))
    }

    fn is_payment_event_stream_active(&self) -> bool {
        false
    }

    fn cancel_payment_event_stream(&self) {}

    async fn check_incoming_payment_status(
        &self,
        _payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        Self::consensus_only()
    }

    async fn check_outgoing_payment(
        &self,
        _payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        Self::consensus_only()
    }
}

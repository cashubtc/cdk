//! Non-mutating `MintPayment` adapter for federated Bark.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::database::DynMintDatabase;
use cdk_common::federation::{FederationBarkPolicyConfig, FederationBarkVtxoState};
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
    runtime: Option<Arc<FederatedBarkRuntime>>,
    limits: FederatedBarkLimits,
    network: bitcoin::Network,
    policy: FederationBarkPolicyConfig,
    database: DynMintDatabase,
}

impl std::fmt::Debug for FederatedBarkPaymentAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FederatedBarkPaymentAdapter")
            .field(
                "scope",
                &self.runtime.as_ref().map(|runtime| runtime.scope()),
            )
            .field("limits", &self.limits)
            .field("network", &self.network)
            .field("policy", &self.policy)
            .finish()
    }
}

impl FederatedBarkPaymentAdapter {
    /// Construct the adapter over an already validated runtime.
    #[must_use]
    pub fn new(runtime: Arc<FederatedBarkRuntime>, database: DynMintDatabase) -> Self {
        let limits = runtime.limits();
        let network = runtime.network();
        let policy = runtime.policy().clone();
        Self {
            runtime: Some(runtime),
            limits,
            network,
            policy,
            database,
        }
    }

    /// Construct the registry adapter before the Axum runtime opens Bark.
    ///
    /// Quote calculation reads only the accepted public projection. Every
    /// mutation remains consensus-only; startup validation and external I/O
    /// are owned by the router's member-local runtime.
    #[must_use]
    pub fn from_accepted_config(
        limits: FederatedBarkLimits,
        network: bitcoin::Network,
        policy: FederationBarkPolicyConfig,
        database: DynMintDatabase,
    ) -> Self {
        Self {
            runtime: None,
            limits,
            network,
            policy,
            database,
        }
    }

    /// Accepted satoshi payment limits.
    #[must_use]
    pub const fn limits(&self) -> FederatedBarkLimits {
        self.limits
    }

    fn consensus_only<T>() -> Result<T, cdk_common::payment::Error> {
        Err(Error::ConsensusOnly.into())
    }
}

#[async_trait]
impl MintPayment for FederatedBarkPaymentAdapter {
    type Err = cdk_common::payment::Error;

    async fn start(&self) -> Result<(), Self::Err> {
        match &self.runtime {
            Some(runtime) => runtime.ensure_ready(0).await.map_err(Into::into),
            None => Ok(()),
        }
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
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(cdk_common::payment::Error::UnsupportedUnit);
        }
        let OutgoingPaymentOptions::Bolt11(options) = options else {
            return Err(cdk_common::payment::Error::UnsupportedPaymentOption);
        };
        if options.melt_options.is_some() {
            return Err(cdk_common::payment::Error::UnsupportedPaymentOption);
        }
        options
            .bolt11
            .check_signature()
            .map_err(|_| cdk_common::payment::Error::InvalidHash)?;
        if options.bolt11.network() != self.network {
            return Err(cdk_common::payment::Error::UnsupportedPaymentOption);
        }
        let amount_msat = options
            .bolt11
            .amount_milli_satoshis()
            .ok_or(cdk_common::payment::Error::AmountMismatch)?;
        if amount_msat == 0 || amount_msat % 1_000 != 0 {
            return Err(cdk_common::payment::Error::AmountMismatch);
        }
        let amount_sat = amount_msat / 1_000;
        if amount_sat < self.limits.minimum_sat() || amount_sat > self.limits.maximum_sat() {
            return Err(cdk_common::payment::Error::AmountMismatch);
        }
        let fee = self
            .policy
            .maximum_outgoing_fee(cdk_common::Amount::from(amount_sat))
            .map_err(|_| cdk_common::payment::Error::AmountMismatch)?;
        let required = cdk_common::Amount::from(amount_sat)
            .checked_add(fee)
            .ok_or(cdk_common::payment::Error::AmountMismatch)?;
        let projection = self
            .database
            .get_federation_bark_projection_snapshot()
            .await
            .map_err(|_| cdk_common::payment::Error::UnknownPaymentState)?;
        let available = projection
            .vtxos
            .iter()
            .filter(|vtxo| vtxo.state == FederationBarkVtxoState::Spendable)
            .try_fold(cdk_common::Amount::ZERO, |sum, vtxo| {
                sum.checked_add(vtxo.amount)
            })
            .ok_or(cdk_common::payment::Error::AmountMismatch)?;
        if available < required {
            return Err(cdk_common::payment::Error::AmountMismatch);
        }
        Ok(PaymentQuoteResponse {
            request_lookup_id: Some(PaymentIdentifier::PaymentHash(
                *options.bolt11.payment_hash().as_ref(),
            )),
            amount: cdk_common::amount::Amount::new(amount_sat, CurrencyUnit::Sat),
            fee: cdk_common::amount::Amount::new(u64::from(fee), CurrencyUnit::Sat),
            state: cdk_common::MeltQuoteState::Unpaid,
            extra_json: None,
            estimated_blocks: None,
            fee_options: None,
        })
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        _options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(cdk_common::payment::Error::UnsupportedUnit);
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

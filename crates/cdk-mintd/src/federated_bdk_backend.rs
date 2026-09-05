//! Payment-interface adapter for the consensus-owned federated BDK wallet.
//!
//! The adapter exposes deterministic quote settings to existing mint plumbing.
//! Address allocation, payment observation, spending, signing, broadcast, and
//! settlement remain exclusively in the federation projections and workers.

use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bitcoin::{Address, Network, TxOut};
use cdk::mint::FederationOnchainTransactionPolicy;
use cdk_common::payment::{
    CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse, MintPayment,
    OnchainSettings, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse,
    SettingsResponse, WaitPaymentResponse,
};
use cdk_common::{Amount, CurrencyUnit, MeltQuoteState};
use futures::{stream, Stream};

const QUOTE_INPUT_COUNT: u64 = 24;
const QUOTE_FIXED_SAFETY_SAT: u64 = 500;

/// Watch-only payment-interface boundary used only by federation-aware mint routes.
#[derive(Debug, Clone)]
pub(crate) struct FederatedBdkPaymentAdapter {
    network: Network,
    confirmations: u32,
    minimum_receive_sat: u64,
    minimum_send_sat: u64,
    policy: FederationOnchainTransactionPolicy,
    chain_source: cdk_bdk::ChainSource,
    event_stream_active: Arc<AtomicBool>,
}

impl FederatedBdkPaymentAdapter {
    pub(crate) fn new(
        network: Network,
        confirmations: u32,
        minimum_receive_sat: u64,
        minimum_send_sat: u64,
        policy: FederationOnchainTransactionPolicy,
        chain_source: cdk_bdk::ChainSource,
    ) -> Result<Self, cdk_common::payment::Error> {
        chain_source
            .validate()
            .map_err(|error| cdk_common::payment::Error::Custom(error.to_string()))?;
        if confirmations == 0 || minimum_receive_sat == 0 || minimum_send_sat == 0 {
            return Err(cdk_common::payment::Error::Custom(
                "federated BDK adapter settings must be non-zero".to_string(),
            ));
        }
        Ok(Self {
            network,
            confirmations,
            minimum_receive_sat,
            minimum_send_sat,
            policy,
            chain_source,
            event_stream_active: Arc::new(AtomicBool::new(false)),
        })
    }

    async fn quoted_fee_rate(&self, target_blocks: u32) -> u64 {
        let target = u16::try_from(target_blocks).unwrap_or(u16::MAX);
        let estimate = tokio::time::timeout(
            Duration::from_millis(self.policy.fee_estimation_timeout_millis),
            self.chain_source.fetch_fee_rate(target),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .map(|rate| rate.ceil() as u64)
        .unwrap_or(self.policy.fallback_fee_rate_sat_per_vb);
        let minimum = self.policy.minimum_fee_rate_sat_per_kwu.saturating_add(249) / 250;
        let maximum = self.policy.maximum_fee_rate_sat_per_kwu / 250;
        estimate.clamp(minimum.max(1), maximum.max(1))
    }

    fn validate_destination(
        &self,
        address: &str,
        amount_sat: u64,
    ) -> Result<(), cdk_common::payment::Error> {
        let address = Address::from_str(address)
            .map_err(|_| cdk_common::payment::Error::UnsupportedPaymentOption)?
            .require_network(self.network)
            .map_err(|_| cdk_common::payment::Error::UnsupportedPaymentOption)?;
        let dust = TxOut::minimal_non_dust(address.script_pubkey())
            .value
            .to_sat();
        if amount_sat < self.minimum_send_sat.max(dust) {
            return Err(cdk_common::payment::Error::AmountMismatch);
        }
        Ok(())
    }

    fn quote_fee_sat(&self, rate_sat_per_vb: u64) -> u64 {
        // Conservative key-path Taproot estimate: version/locktime/varints,
        // 24 inputs including witnesses, one recipient, and one change output.
        let estimated_vbytes = 11_u64
            .saturating_add(QUOTE_INPUT_COUNT.saturating_mul(58))
            .saturating_add(2_u64.saturating_mul(43));
        estimated_vbytes
            .saturating_mul(rate_sat_per_vb)
            .saturating_add(QUOTE_FIXED_SAFETY_SAT)
            .min(self.policy.maximum_absolute_fee_sat)
    }

    fn consensus_only_error() -> cdk_common::payment::Error {
        cdk_common::payment::Error::Custom(
            "federated BDK operations require the accepted consensus wallet path".to_string(),
        )
    }
}

#[async_trait]
impl MintPayment for FederatedBdkPaymentAdapter {
    type Err = cdk_common::payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        Ok(SettingsResponse {
            unit: "sat".to_string(),
            bolt11: None,
            bolt12: None,
            onchain: Some(OnchainSettings {
                confirmations: self.confirmations,
                min_receive_amount_sat: self.minimum_receive_sat,
                min_send_amount_sat: self.minimum_send_sat,
            }),
            custom: std::collections::HashMap::new(),
        })
    }

    async fn create_incoming_payment_request(
        &self,
        _options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        Err(Self::consensus_only_error())
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(cdk_common::payment::Error::UnsupportedUnit);
        }
        let OutgoingPaymentOptions::Onchain(options) = options else {
            return Err(cdk_common::payment::Error::UnsupportedPaymentOption);
        };
        let amount_sat = options.amount.clone().to_u64();
        self.validate_destination(&options.address, amount_sat)?;

        let mut fee_options = Vec::with_capacity(self.policy.allowed_fee_target_blocks.len());
        for (index, target_blocks) in self
            .policy
            .allowed_fee_target_blocks
            .iter()
            .copied()
            .enumerate()
        {
            let rate = self.quoted_fee_rate(target_blocks).await;
            fee_options.push(cdk_common::nuts::nut30::MeltQuoteOnchainFeeOption {
                fee_index: u32::try_from(index).unwrap_or(u32::MAX),
                fee_reserve: Amount::from(self.quote_fee_sat(rate)),
                estimated_blocks: target_blocks,
            });
        }
        let cheapest = fee_options
            .iter()
            .min_by_key(|option| u64::from(option.fee_reserve))
            .copied()
            .ok_or_else(Self::consensus_only_error)?;
        Ok(PaymentQuoteResponse {
            request_lookup_id: Some(PaymentIdentifier::QuoteId(options.quote_id.clone())),
            amount: options.amount,
            fee: Amount::new(u64::from(cheapest.fee_reserve), CurrencyUnit::Sat),
            state: MeltQuoteState::Unpaid,
            extra_json: None,
            estimated_blocks: Some(cheapest.estimated_blocks),
            fee_options: Some(fee_options),
        })
    }

    async fn make_payment(
        &self,
        _unit: &CurrencyUnit,
        _options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        Err(Self::consensus_only_error())
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        self.event_stream_active.store(true, Ordering::SeqCst);
        Ok(Box::pin(stream::pending()))
    }

    fn is_payment_event_stream_active(&self) -> bool {
        self.event_stream_active.load(Ordering::SeqCst)
    }

    fn cancel_payment_event_stream(&self) {
        self.event_stream_active.store(false, Ordering::SeqCst);
    }

    async fn check_incoming_payment_status(
        &self,
        _payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        Err(Self::consensus_only_error())
    }

    async fn check_outgoing_payment(
        &self,
        _payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        Err(Self::consensus_only_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_contains_no_endpoint_or_credentials() {
        let adapter = FederatedBdkPaymentAdapter::new(
            Network::Regtest,
            2,
            1_000,
            546,
            FederationOnchainTransactionPolicy::for_version(1).expect("policy"),
            cdk_bdk::ChainSource::BitcoinRpc(cdk_bdk::BitcoinRpcConfig {
                wallet_rescan_from_height: Some(0),
                host: "private-node.internal".to_string(),
                port: 18_443,
                user: "operator".to_string(),
                password: "secret".to_string(),
                request_timeout_secs: 15,
            }),
        )
        .expect("adapter");
        let debug = format!("{adapter:?}");
        assert!(!debug.contains("private-node"));
        assert!(!debug.contains("operator"));
        assert!(!debug.contains("secret"));
    }
}

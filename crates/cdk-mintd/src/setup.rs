#[cfg(feature = "fakewallet")]
use std::collections::HashMap;
#[cfg(feature = "fakewallet")]
use std::collections::HashSet;
#[cfg(feature = "lnbits")]
use std::sync::Arc;

#[cfg(feature = "cln")]
use anyhow::anyhow;
use async_trait::async_trait;
use axum::Router;
#[cfg(feature = "fakewallet")]
use bip39::rand::{thread_rng, Rng};
use cdk::cdk_payment::MintPayment;
#[cfg(feature = "lnbits")]
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
#[cfg(any(
    feature = "lnbits",
    feature = "cln",
    feature = "lnd",
    feature = "fakewallet"
))]
use cdk::types::FeeReserve;
#[cfg(feature = "lnbits")]
use tokio::sync::Mutex;

use crate::config::{self, Settings};
#[cfg(feature = "cln")]
use crate::expand_path;

#[async_trait]
pub trait LnBackendSetup {
    async fn setup(
        &self,
        routers: &mut Vec<Router>,
        settings: &Settings,
        unit: CurrencyUnit,
    ) -> anyhow::Result<impl MintPayment>;
}

#[cfg(feature = "cln")]
#[async_trait]
impl LnBackendSetup for config::Cln {
    async fn setup(
        &self,
        _routers: &mut Vec<Router>,
        _settings: &Settings,
        _unit: CurrencyUnit,
    ) -> anyhow::Result<cdk_cln::Cln> {
        let cln_socket = expand_path(
            self.rpc_path
                .to_str()
                .ok_or(anyhow!("cln socket not defined"))?,
        )
        .ok_or(anyhow!("cln socket not defined"))?;

        let fee_reserve = FeeReserve {
            min_fee_reserve: self.reserve_fee_min,
            percent_fee_reserve: self.fee_percent,
        };

        let cln = cdk_cln::Cln::new(cln_socket, fee_reserve).await?;

        Ok(cln)
    }
}

#[cfg(feature = "lnbits")]
#[async_trait]
impl LnBackendSetup for config::LNbits {
    async fn setup(
        &self,
        routers: &mut Vec<Router>,
        settings: &Settings,
        _unit: CurrencyUnit,
    ) -> anyhow::Result<cdk_lnbits::LNbits> {
        let admin_api_key = &self.admin_api_key;
        let invoice_api_key = &self.invoice_api_key;

        // Channel used for lnbits web hook
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let webhook_endpoint = "/webhook/lnbits/sat/invoice";

        let mint_url: MintUrl = settings.info.url.parse()?;
        let webhook_url = mint_url.join(webhook_endpoint)?;

        let fee_reserve = FeeReserve {
            min_fee_reserve: self.reserve_fee_min,
            percent_fee_reserve: self.fee_percent,
        };

        let lnbits = cdk_lnbits::LNbits::new(
            admin_api_key.clone(),
            invoice_api_key.clone(),
            self.lnbits_api.clone(),
            fee_reserve,
            Arc::new(Mutex::new(Some(receiver))),
            webhook_url.to_string(),
        )
        .await?;

        let router = lnbits
            .create_invoice_webhook_router(webhook_endpoint, sender)
            .await?;

        routers.push(router);

        Ok(lnbits)
    }
}

#[cfg(feature = "lnd")]
#[async_trait]
impl LnBackendSetup for config::Lnd {
    async fn setup(
        &self,
        _routers: &mut Vec<Router>,
        _settings: &Settings,
        _unit: CurrencyUnit,
    ) -> anyhow::Result<cdk_lnd::Lnd> {
        let address = &self.address;
        let cert_file = &self.cert_file;
        let macaroon_file = &self.macaroon_file;

        let fee_reserve = FeeReserve {
            min_fee_reserve: self.reserve_fee_min,
            percent_fee_reserve: self.fee_percent,
        };

        let lnd = cdk_lnd::Lnd::new(
            address.to_string(),
            cert_file.clone(),
            macaroon_file.clone(),
            fee_reserve,
        )
        .await?;

        Ok(lnd)
    }
}

#[cfg(feature = "fakewallet")]
#[async_trait]
impl LnBackendSetup for config::FakeWallet {
    async fn setup(
        &self,
        _router: &mut Vec<Router>,
        _settings: &Settings,
        _unit: CurrencyUnit,
    ) -> anyhow::Result<cdk_fake_wallet::FakeWallet> {
        let fee_reserve = FeeReserve {
            min_fee_reserve: self.reserve_fee_min,
            percent_fee_reserve: self.fee_percent,
        };

        // calculate random delay time
        let mut rng = thread_rng();
        let delay_time = rng.gen_range(self.min_delay_time..=self.max_delay_time);

        let fake_wallet = cdk_fake_wallet::FakeWallet::new(
            fee_reserve,
            HashMap::default(),
            HashSet::default(),
            delay_time,
        );

        Ok(fake_wallet)
    }
}

#[cfg(feature = "grpc-processor")]
#[async_trait]
impl LnBackendSetup for config::GrpcProcessor {
    async fn setup(
        &self,
        _routers: &mut Vec<Router>,
        _settings: &Settings,
        _unit: CurrencyUnit,
    ) -> anyhow::Result<cdk_payment_processor::PaymentProcessorClient> {
        let payment_processor = cdk_payment_processor::PaymentProcessorClient::new(
            &self.addr,
            self.port,
            self.tls_dir.clone(),
        )
        .await?;

        Ok(payment_processor)
    }
}

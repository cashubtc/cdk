use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{anyhow, bail};
use axum::{async_trait, Router};

use cdk::{cdk_lightning::MintLightning, mint::FeeReserve, mint_url::MintUrl, nuts::CurrencyUnit};
use tokio::sync::Mutex;
use url::Url;

use crate::{
    config::{self, Settings},
    expand_path,
};

#[async_trait]
pub trait LnBackendSetup {
    async fn setup(
        &self,
        routers: &mut Vec<Router>,
        settings: &Settings,
        unit: CurrencyUnit,
    ) -> anyhow::Result<impl MintLightning>;
}

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

        let cln = cdk_cln::Cln::new(cln_socket, fee_reserve, true, true).await?;

        Ok(cln)
    }
}

#[async_trait]
impl LnBackendSetup for config::Strike {
    async fn setup(
        &self,
        routers: &mut Vec<Router>,
        settings: &Settings,
        unit: CurrencyUnit,
    ) -> anyhow::Result<cdk_strike::Strike> {
        let api_key = &self.api_key;

        // Channel used for strike web hook
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let webhook_endpoint = format!("/webhook/{}/invoice", unit);

        let mint_url: MintUrl = settings.info.url.parse()?;
        let webhook_url = mint_url.join(&webhook_endpoint)?;

        let strike = cdk_strike::Strike::new(
            api_key.clone(),
            unit,
            Arc::new(Mutex::new(Some(receiver))),
            webhook_url.to_string(),
        )
        .await?;

        let router = strike
            .create_invoice_webhook(&webhook_endpoint, sender)
            .await?;
        routers.push(router);

        Ok(strike)
    }
}

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

#[async_trait]
impl LnBackendSetup for config::Phoenixd {
    async fn setup(
        &self,
        routers: &mut Vec<Router>,
        settings: &Settings,
        _unit: CurrencyUnit,
    ) -> anyhow::Result<cdk_phoenixd::Phoenixd> {
        let api_password = &self.api_password;

        let api_url = &self.api_url;

        let fee_reserve = FeeReserve {
            min_fee_reserve: self.reserve_fee_min,
            percent_fee_reserve: self.fee_percent,
        };

        if fee_reserve.percent_fee_reserve < 0.04 {
            bail!("Fee reserve is too low needs to be at least 0.02");
        }

        let webhook_endpoint = "/webhook/phoenixd";

        let mint_url = Url::parse(&settings.info.url)?;

        let webhook_url = mint_url.join(webhook_endpoint)?.to_string();

        let (sender, receiver) = tokio::sync::mpsc::channel(8);

        let phoenixd = cdk_phoenixd::Phoenixd::new(
            api_password.to_string(),
            api_url.to_string(),
            fee_reserve,
            Arc::new(Mutex::new(Some(receiver))),
            webhook_url,
        )?;

        let router = phoenixd
            .create_invoice_webhook(webhook_endpoint, sender)
            .await?;

        routers.push(router);

        Ok(phoenixd)
    }
}

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

        let fake_wallet = cdk_fake_wallet::FakeWallet::new(
            fee_reserve,
            HashMap::default(),
            HashSet::default(),
            0,
        );

        Ok(fake_wallet)
    }
}

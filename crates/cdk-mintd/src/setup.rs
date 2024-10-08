use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{anyhow, bail};
use axum::{async_trait, Router};

use cdk::{
    cdk_lightning::{self, MintLightning},
    mint::FeeReserve,
    mint_url::MintUrl,
    nuts::{CurrencyUnit, PaymentMethod},
    types::LnKey,
};
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
        ln_backends: &mut HashMap<
            LnKey,
            Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
        >,
        supported_units: &mut HashMap<CurrencyUnit, (u64, u8)>,
        routers: &mut Vec<Router>,
        settings: &Settings,
    ) -> anyhow::Result<()>;
}

#[async_trait]
impl LnBackendSetup for config::Cln {
    async fn setup(
        &self,
        ln_backends: &mut HashMap<
            LnKey,
            Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
        >,
        supported_units: &mut HashMap<CurrencyUnit, (u64, u8)>,
        _routers: &mut Vec<Router>,
        settings: &Settings,
    ) -> anyhow::Result<()> {
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

        let cln = Arc::new(cdk_cln::Cln::new(cln_socket, fee_reserve, true, true).await?);

        ln_backends.insert(
            LnKey::new(CurrencyUnit::Sat, PaymentMethod::Bolt11),
            cln.clone(),
        );

        if self.bolt12 {
            ln_backends.insert(LnKey::new(CurrencyUnit::Sat, PaymentMethod::Bolt12), cln);
        }

        let input_fee_ppk = settings.info.input_fee_ppk.unwrap_or(0);
        supported_units.insert(CurrencyUnit::Sat, (input_fee_ppk, 64));

        Ok(())
    }
}

#[async_trait]
impl LnBackendSetup for config::Strike {
    async fn setup(
        &self,
        ln_backends: &mut HashMap<
            LnKey,
            Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
        >,
        supported_units: &mut HashMap<CurrencyUnit, (u64, u8)>,
        routers: &mut Vec<Router>,
        settings: &Settings,
    ) -> anyhow::Result<()> {
        let api_key = &self.api_key;

        let units = self
            .supported_units
            .clone()
            .unwrap_or(vec![CurrencyUnit::Sat]);

        for unit in units {
            // Channel used for strike web hook
            let (sender, receiver) = tokio::sync::mpsc::channel(8);
            let webhook_endpoint = format!("/webhook/{}/invoice", unit);

            let mint_url: MintUrl = settings.info.url.parse()?;
            let webhook_url = mint_url.join(&webhook_endpoint)?;

            let strike = cdk_strike::Strike::new(
                api_key.clone(),
                unit.clone(),
                Arc::new(Mutex::new(Some(receiver))),
                webhook_url.to_string(),
            )
            .await?;

            let router = strike
                .create_invoice_webhook(&webhook_endpoint, sender)
                .await?;
            routers.push(router);

            let ln_key = LnKey::new(unit.clone(), PaymentMethod::Bolt11);

            ln_backends.insert(ln_key, Arc::new(strike));

            let input_fee_ppk = settings.info.input_fee_ppk.unwrap_or(0);
            supported_units.insert(unit.clone(), (input_fee_ppk, 64));
        }

        Ok(())
    }
}

#[async_trait]
impl LnBackendSetup for config::LNbits {
    async fn setup(
        &self,
        ln_backends: &mut HashMap<
            LnKey,
            Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
        >,
        supported_units: &mut HashMap<CurrencyUnit, (u64, u8)>,
        routers: &mut Vec<Router>,
        settings: &Settings,
    ) -> anyhow::Result<()> {
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

        let unit = CurrencyUnit::Sat;

        let ln_key = LnKey::new(unit, PaymentMethod::Bolt11);

        ln_backends.insert(ln_key, Arc::new(lnbits));

        let input_fee_ppk = settings.info.input_fee_ppk.unwrap_or(0);
        supported_units.insert(unit, (input_fee_ppk, 64));
        routers.push(router);

        Ok(())
    }
}

#[async_trait]
impl LnBackendSetup for config::Phoenixd {
    async fn setup(
        &self,
        ln_backends: &mut HashMap<
            LnKey,
            Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
        >,
        supported_units: &mut HashMap<CurrencyUnit, (u64, u8)>,
        routers: &mut Vec<Router>,
        settings: &Settings,
    ) -> anyhow::Result<()> {
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

        let input_fee_ppk = settings.info.input_fee_ppk.unwrap_or(0);
        supported_units.insert(CurrencyUnit::Sat, (input_fee_ppk, 64));

        let phd = Arc::new(phoenixd);
        ln_backends.insert(
            LnKey {
                unit: CurrencyUnit::Sat,
                method: PaymentMethod::Bolt11,
            },
            phd.clone(),
        );

        if self.bolt12 {
            ln_backends.insert(
                LnKey {
                    unit: CurrencyUnit::Sat,
                    method: PaymentMethod::Bolt12,
                },
                phd,
            );
        }

        routers.push(router);

        Ok(())
    }
}

#[async_trait]
impl LnBackendSetup for config::Lnd {
    async fn setup(
        &self,
        ln_backends: &mut HashMap<
            LnKey,
            Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
        >,
        supported_units: &mut HashMap<CurrencyUnit, (u64, u8)>,
        _routers: &mut Vec<Router>,
        settings: &Settings,
    ) -> anyhow::Result<()> {
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

        let input_fee_ppk = settings.info.input_fee_ppk.unwrap_or(0);
        supported_units.insert(CurrencyUnit::Sat, (input_fee_ppk, 64));
        ln_backends.insert(
            LnKey {
                unit: CurrencyUnit::Sat,
                method: PaymentMethod::Bolt11,
            },
            Arc::new(lnd),
        );

        Ok(())
    }
}

#[async_trait]
impl LnBackendSetup for config::FakeWallet {
    async fn setup(
        &self,
        ln_backends: &mut HashMap<
            LnKey,
            Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
        >,
        supported_units: &mut HashMap<CurrencyUnit, (u64, u8)>,
        _router: &mut Vec<Router>,
        settings: &Settings,
    ) -> anyhow::Result<()> {
        let units = settings
            .clone()
            .fake_wallet
            .unwrap_or_default()
            .supported_units;

        let input_fee_ppk = settings.info.input_fee_ppk.unwrap_or(0);
        let fee_reserve = FeeReserve {
            min_fee_reserve: self.reserve_fee_min,
            percent_fee_reserve: self.fee_percent,
        };

        for unit in units {
            let ln_key = LnKey::new(unit, PaymentMethod::Bolt11);

            let wallet = Arc::new(cdk_fake_wallet::FakeWallet::new(
                fee_reserve,
                HashMap::default(),
                HashSet::default(),
                0,
            ));

            ln_backends.insert(ln_key, wallet);

            supported_units.insert(unit, (input_fee_ppk, 64));
        }

        let ln_key = LnKey::new(CurrencyUnit::Sat, PaymentMethod::Bolt12);

        let wallet = Arc::new(cdk_fake_wallet::FakeWallet::new(
            fee_reserve.clone(),
            HashMap::default(),
            HashSet::default(),
            0,
        ));

        ln_backends.insert(ln_key, wallet);

        supported_units.insert(CurrencyUnit::Sat, (input_fee_ppk, 64));

        Ok(())
    }
}

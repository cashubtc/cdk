#![allow(missing_docs)]
//! Environment variables module
//!
//! This module contains all environment variable definitions and parsing logic
//! organized by component.

mod common;
mod database;
mod info;
mod limits;
mod ln;
mod mint_info;
mod onchain;

mod auth;
#[cfg(feature = "bdk")]
mod bdk;
#[cfg(feature = "cln")]
mod cln;
#[cfg(feature = "fakewallet")]
mod fake_wallet;
#[cfg(feature = "grpc-processor")]
mod grpc_processor;
#[cfg(feature = "ldk-node")]
mod ldk_node;
#[cfg(feature = "ldk-server")]
mod ldk_server;
#[cfg(feature = "lnbits")]
mod lnbits;
#[cfg(feature = "lnd")]
mod lnd;
#[cfg(feature = "management-rpc")]
mod management_rpc;
#[cfg(feature = "prometheus")]
mod prometheus;

use std::env;
use std::str::FromStr;

use anyhow::{anyhow, bail, Result};
pub use auth::*;
#[cfg(feature = "bdk")]
pub use bdk::*;
#[cfg(feature = "cln")]
pub use cln::*;
pub use common::*;
pub use database::*;
#[cfg(feature = "fakewallet")]
pub use fake_wallet::*;
#[cfg(feature = "grpc-processor")]
pub use grpc_processor::*;
#[cfg(feature = "ldk-node")]
pub use ldk_node::*;
#[cfg(feature = "ldk-server")]
pub use ldk_server::*;
pub use limits::*;
pub use ln::*;
#[cfg(feature = "lnbits")]
pub use lnbits::*;
#[cfg(feature = "lnd")]
pub use lnd::*;
#[cfg(feature = "management-rpc")]
pub use management_rpc::*;
pub use mint_info::*;
pub use onchain::*;
#[cfg(feature = "prometheus")]
pub use prometheus::*;

use crate::config::{DatabaseEngine, Ln, LnBackend, OnchainBackend, Settings};

impl Settings {
    pub fn from_env(&mut self) -> Result<Self> {
        if let Ok(database) = env::var(DATABASE_ENV_VAR) {
            let engine = DatabaseEngine::from_str(&database).map_err(|err| anyhow!(err))?;
            self.database.engine = engine;
        }

        // Parse PostgreSQL-specific configuration from environment variables
        if self.database.engine == DatabaseEngine::Postgres {
            self.database.postgres = Some(
                self.database
                    .postgres
                    .clone()
                    .unwrap_or_default()
                    .from_env(),
            );
        }

        // Parse auth database configuration from environment variables
        self.auth_database = Some(crate::config::AuthDatabase {
            postgres: Some(
                self.auth_database
                    .clone()
                    .unwrap_or_default()
                    .postgres
                    .unwrap_or_default()
                    .from_env(),
            ),
        });

        self.info = self.info.clone().from_env();
        self.mint_info = self.mint_info.clone().from_env();
        // CDK_MINTD_LN_* env vars only apply when there is exactly one
        // configured Lightning entry. Multi-backend setups must choose units
        // and backends in the config file so env overrides do not collapse them.
        match self.ln.len() {
            0 => {
                let ln = Ln::default().from_env();
                if ln.ln_backend != LnBackend::None {
                    self.ln.push(ln);
                }
            }
            1 => {
                self.ln[0] = self.ln[0].clone().from_env();
            }
            _ => {
                tracing::warn!(
                    "CDK_MINTD_LN_* environment variables ignored: multiple [[ln]] entries configured"
                );
            }
        }
        self.onchain = Some(self.onchain.clone().unwrap_or_default().from_env());
        self.limits = self.limits.clone().from_env();

        {
            // Check env vars for auth config even if None
            let auth = self.auth.clone().unwrap_or_default().from_env();

            // Only set auth if auth_enabled flag is true
            if auth.auth_enabled {
                self.auth = Some(auth);
            } else {
                self.auth = None;
            }
        }

        #[cfg(feature = "management-rpc")]
        {
            self.mint_management_rpc = Some(
                self.mint_management_rpc
                    .clone()
                    .unwrap_or_default()
                    .from_env(),
            );
        }

        #[cfg(feature = "prometheus")]
        {
            self.prometheus = Some(self.prometheus.clone().unwrap_or_default().from_env());
        }

        #[cfg(feature = "cln")]
        {
            let cln = self.cln.clone().unwrap_or_default().from_env();
            if cln.rpc_path.as_os_str().is_empty() {
                self.cln = None;
            } else {
                self.cln = Some(cln);
            }
        }

        #[cfg(feature = "lnbits")]
        {
            let lnbits = self.lnbits.clone().unwrap_or_default().from_env();
            if lnbits.admin_api_key.is_empty() {
                self.lnbits = None;
            } else {
                self.lnbits = Some(lnbits);
            }
        }

        #[cfg(feature = "fakewallet")]
        {
            // Fake wallet has defaults so it is always Some if feature enabled
            let fake_wallet_supported_units_from_env =
                env::var(ENV_FAKE_WALLET_SUPPORTED_UNITS).is_ok();
            let fake_wallet = self.fake_wallet.clone().unwrap_or_default().from_env();
            let supported_units_configured =
                fake_wallet.supported_units != vec![cdk::nuts::CurrencyUnit::Sat];

            if fake_wallet_supported_units_from_env || supported_units_configured {
                self.expand_single_fake_wallet_ln_entry(&fake_wallet);
            }

            self.fake_wallet = Some(fake_wallet);
        }

        #[cfg(feature = "lnd")]
        {
            let lnd = self.lnd.clone().unwrap_or_default().from_env();
            if lnd.address.is_empty() {
                self.lnd = None;
            } else {
                self.lnd = Some(lnd);
            }
        }

        #[cfg(feature = "ldk-node")]
        {
            let ldk_node = self.ldk_node.clone().unwrap_or_default().from_env();
            if ldk_node.bitcoin_network.is_none() && ldk_node.esplora_url.is_none() {
                self.ldk_node = None;
            } else {
                self.ldk_node = Some(ldk_node);
            }
        }

        #[cfg(feature = "ldk-server")]
        {
            let ldk_server = self.ldk_server.clone().unwrap_or_default().from_env();
            if ldk_server.address.is_empty()
                && ldk_server.api_key.is_empty()
                && ldk_server.cert_path.as_os_str().is_empty()
            {
                self.ldk_server = None;
            } else {
                self.ldk_server = Some(ldk_server);
            }
        }

        #[cfg(feature = "grpc-processor")]
        {
            let grpc_processor = self.grpc_processor.clone().unwrap_or_default().from_env();
            let grpc_processor_configured = self
                .ln
                .iter()
                .any(|ln| ln.ln_backend == LnBackend::GrpcProcessor);
            if grpc_processor.supported_units.is_empty() && !grpc_processor_configured {
                self.grpc_processor = None;
            } else {
                self.grpc_processor = Some(grpc_processor);
            }
        }

        #[cfg(feature = "bdk")]
        {
            let bdk = self.bdk.clone().unwrap_or_default().from_env();
            if bdk.network.is_none() && bdk.mnemonic.is_none() {
                self.bdk = None;
            } else {
                self.bdk = Some(bdk);
            }
        }

        for ln in &self.ln {
            match ln.ln_backend {
                #[cfg(feature = "cln")]
                LnBackend::Cln => {}
                #[cfg(feature = "lnbits")]
                LnBackend::LNbits => {}
                #[cfg(feature = "fakewallet")]
                LnBackend::FakeWallet => {}
                #[cfg(feature = "lnd")]
                LnBackend::Lnd => {}
                #[cfg(feature = "ldk-node")]
                LnBackend::LdkNode => {}
                #[cfg(feature = "ldk-server")]
                LnBackend::LdkServer => {}
                #[cfg(feature = "grpc-processor")]
                LnBackend::GrpcProcessor => {}
                LnBackend::None => {}
                #[allow(unreachable_patterns)]
                _ => bail!("Selected Ln backend is not enabled in this build"),
            }
        }

        let has_lightning_backend = self.ln.iter().any(|ln| ln.ln_backend != LnBackend::None);
        let has_onchain_backend = self
            .onchain
            .as_ref()
            .map(|onchain| onchain.onchain_backend != OnchainBackend::None)
            .unwrap_or(false);
        if !has_lightning_backend && !has_onchain_backend {
            bail!("At least one payment backend (Lightning or On-chain) must be set");
        }

        self.validate_backend_pairing()
            .map_err(|err| anyhow!(err))?;

        Ok(self.clone())
    }

    #[cfg(feature = "fakewallet")]
    fn expand_single_fake_wallet_ln_entry(&mut self, fake_wallet: &crate::config::FakeWallet) {
        let fake_wallet_ln_index = self
            .ln
            .iter()
            .enumerate()
            .filter_map(|(index, ln)| (ln.ln_backend == LnBackend::FakeWallet).then_some(index))
            .collect::<Vec<_>>();

        if fake_wallet_ln_index.len() != 1 {
            return;
        }

        let mut units = Vec::new();
        for unit in &fake_wallet.supported_units {
            if !units.contains(unit) {
                units.push(unit.clone());
            }
        }

        if units.is_empty() {
            return;
        }

        let index = fake_wallet_ln_index[0];
        let base_ln = self.ln[index].clone();
        let expanded_ln = units.into_iter().map(|unit| Ln {
            unit,
            ..base_ln.clone()
        });

        self.ln.splice(index..=index, expanded_ln);
    }
}

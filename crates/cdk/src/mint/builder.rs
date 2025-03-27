//! Mint Builder

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::anyhow;
use bitcoin::bip32::DerivationPath;
use cdk_common::database::{self, MintDatabase};
use cdk_common::error::Error;
use cdk_common::payment::Bolt11Settings;
use cdk_common::{nut21, nut22};

use super::nut17::SupportedMethods;
use super::nut19::{self, CachedEndpoint};
#[cfg(feature = "auth")]
use super::MintAuthDatabase;
use super::Nuts;
use crate::amount::Amount;
#[cfg(feature = "auth")]
use crate::cdk_database;
use crate::cdk_payment::{self, MintPayment};
use crate::mint::Mint;
use crate::nuts::{
    ContactInfo, CurrencyUnit, MeltMethodSettings, MintInfo, MintMethodSettings, MintVersion,
    MppMethodSettings, PaymentMethod,
};
use crate::types::PaymentProcessorKey;

/// Cashu Mint
#[derive(Default)]
pub struct MintBuilder {
    /// Mint Info
    pub mint_info: MintInfo,
    /// Mint Storage backend
    localstore: Option<Arc<dyn MintDatabase<database::Error> + Send + Sync>>,
    /// Mint Storage backend
    #[cfg(feature = "auth")]
    auth_localstore: Option<Arc<dyn MintAuthDatabase<Err = cdk_database::Error> + Send + Sync>>,
    /// Ln backends for mint
    ln: Option<
        HashMap<PaymentProcessorKey, Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>>,
    >,
    seed: Option<Vec<u8>>,
    supported_units: HashMap<CurrencyUnit, (u64, u8)>,
    custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    // protected_endpoints: HashMap<ProtectedEndpoint, AuthRequired>,
    openid_discovery: Option<String>,
}

impl MintBuilder {
    /// New mint builder
    pub fn new() -> MintBuilder {
        let mut builder = MintBuilder::default();

        let nuts = Nuts::new()
            .nut07(true)
            .nut08(true)
            .nut09(true)
            .nut10(true)
            .nut11(true)
            .nut12(true)
            .nut14(true)
            .nut20(true);

        builder.mint_info.nuts = nuts;

        builder
    }

    /// Set localstore
    pub fn with_localstore(
        mut self,
        localstore: Arc<dyn MintDatabase<database::Error> + Send + Sync>,
    ) -> MintBuilder {
        self.localstore = Some(localstore);
        self
    }

    /// Set auth localstore
    #[cfg(feature = "auth")]
    pub fn with_auth_localstore(
        mut self,
        localstore: Arc<dyn MintAuthDatabase<Err = cdk_database::Error> + Send + Sync>,
    ) -> MintBuilder {
        self.auth_localstore = Some(localstore);
        self
    }

    /// Set Openid discovery url    
    pub fn with_openid_discovery(mut self, openid_discovery: String) -> Self {
        self.openid_discovery = Some(openid_discovery);
        self
    }

    /// Set seed
    pub fn with_seed(mut self, seed: Vec<u8>) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Set name
    pub fn with_name(mut self, name: String) -> Self {
        self.mint_info.name = Some(name);
        self
    }

    /// Set initial mint URLs
    pub fn with_urls(mut self, urls: Vec<String>) -> Self {
        self.mint_info.urls = Some(urls);
        self
    }

    /// Set icon url
    pub fn with_icon_url(mut self, icon_url: String) -> Self {
        self.mint_info.icon_url = Some(icon_url);
        self
    }

    /// Set icon url
    pub fn with_motd(mut self, motd: String) -> Self {
        self.mint_info.motd = Some(motd);
        self
    }

    /// Set terms of service URL
    pub fn with_tos_url(mut self, tos_url: String) -> Self {
        self.mint_info.tos_url = Some(tos_url);
        self
    }

    /// Set description
    pub fn with_description(mut self, description: String) -> Self {
        self.mint_info.description = Some(description);
        self
    }

    /// Set long description
    pub fn with_long_description(mut self, description: String) -> Self {
        self.mint_info.description_long = Some(description);
        self
    }

    /// Set version
    pub fn with_version(mut self, version: MintVersion) -> Self {
        self.mint_info.version = Some(version);
        self
    }

    /// Set contact info
    pub fn add_contact_info(mut self, contact_info: ContactInfo) -> Self {
        let mut contacts = self.mint_info.contact.clone().unwrap_or_default();
        contacts.push(contact_info);
        self.mint_info.contact = Some(contacts);
        self
    }

    /// Add ln backend
    pub async fn add_ln_backend(
        mut self,
        unit: CurrencyUnit,
        method: PaymentMethod,
        limits: MintMeltLimits,
        ln_backend: Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>,
    ) -> Result<Self, Error> {
        let ln_key = PaymentProcessorKey {
            unit: unit.clone(),
            method: method.clone(),
        };

        tracing::debug!("Adding ln backed for {}, {}", unit, method);
        tracing::debug!("with limits {:?}", limits);

        let mut ln = self.ln.unwrap_or_default();

        let settings = ln_backend.get_settings().await?;

        let settings: Bolt11Settings = settings.try_into()?;

        if settings.mpp {
            let mpp_settings = MppMethodSettings {
                method: method.clone(),
                unit: unit.clone(),
            };

            let mut mpp = self.mint_info.nuts.nut15.clone();

            mpp.methods.push(mpp_settings);

            self.mint_info.nuts.nut15 = mpp;
        }

        if method == PaymentMethod::Bolt11 {
            let mint_method_settings = MintMethodSettings {
                method: method.clone(),
                unit: unit.clone(),
                min_amount: Some(limits.mint_min),
                max_amount: Some(limits.mint_max),
                description: settings.invoice_description,
            };

            self.mint_info.nuts.nut04.methods.push(mint_method_settings);
            self.mint_info.nuts.nut04.disabled = false;

            let melt_method_settings = MeltMethodSettings {
                method: method.clone(),
                unit,
                min_amount: Some(limits.melt_min),
                max_amount: Some(limits.melt_max),
            };
            self.mint_info.nuts.nut05.methods.push(melt_method_settings);
            self.mint_info.nuts.nut05.disabled = false;
        }

        ln.insert(ln_key.clone(), ln_backend);

        let mut supported_units = self.supported_units.clone();

        supported_units.insert(ln_key.unit, (0, 32));
        self.supported_units = supported_units;

        self.ln = Some(ln);

        Ok(self)
    }

    /// Set pubkey
    pub fn with_pubkey(mut self, pubkey: crate::nuts::PublicKey) -> Self {
        self.mint_info.pubkey = Some(pubkey);

        self
    }

    /// Support websockets
    pub fn add_supported_websockets(mut self, supported_method: SupportedMethods) -> Self {
        let mut supported_settings = self.mint_info.nuts.nut17.supported.clone();

        if !supported_settings.contains(&supported_method) {
            supported_settings.push(supported_method);

            self.mint_info.nuts = self.mint_info.nuts.nut17(supported_settings);
        }

        self
    }

    /// Add support for NUT19
    pub fn add_cache(mut self, ttl: Option<u64>, cached_endpoints: Vec<CachedEndpoint>) -> Self {
        let nut19_settings = nut19::Settings {
            ttl,
            cached_endpoints,
        };

        self.mint_info.nuts.nut19 = nut19_settings;

        self
    }

    /// Set custom derivation paths for mint units
    pub fn add_custom_derivation_paths(
        mut self,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Self {
        self.custom_paths = custom_paths;
        self
    }

    /// Set clear auth settings
    pub fn set_clear_auth_settings(mut self, openid_discovery: String, client_id: String) -> Self {
        let mut nuts = self.mint_info.nuts;

        nuts.nut21 = Some(nut21::Settings::new(
            openid_discovery.clone(),
            client_id,
            vec![],
        ));

        self.openid_discovery = Some(openid_discovery);

        self.mint_info.nuts = nuts;

        self
    }

    /// Set blind auth settings
    pub fn set_blind_auth_settings(mut self, bat_max_mint: u64) -> Self {
        let mut nuts = self.mint_info.nuts;

        nuts.nut22 = Some(nut22::Settings::new(bat_max_mint, vec![]));

        self.mint_info.nuts = nuts;

        self
    }

    /// Build mint
    pub async fn build(&self) -> anyhow::Result<Mint> {
        let localstore = self
            .localstore
            .clone()
            .ok_or(anyhow!("Localstore not set"))?;
        let seed = self.seed.as_ref().ok_or(anyhow!("Mint seed not set"))?;
        let ln = self.ln.clone().ok_or(anyhow!("Ln backends not set"))?;

        #[cfg(feature = "auth")]
        if let Some(openid_discovery) = &self.openid_discovery {
            let auth_localstore = self
                .auth_localstore
                .clone()
                .ok_or(anyhow!("Auth localstore not set"))?;

            return Ok(Mint::new_with_auth(
                seed,
                localstore,
                auth_localstore,
                ln,
                self.supported_units.clone(),
                self.custom_paths.clone(),
                openid_discovery.clone(),
            )
            .await?);
        }

        #[cfg(not(feature = "auth"))]
        if self.openid_discovery.is_some() {
            return Err(anyhow!(
                "OpenID discovery URL provided but auth feature is not enabled"
            ));
        }

        Ok(Mint::new(
            seed,
            localstore,
            ln,
            self.supported_units.clone(),
            self.custom_paths.clone(),
        )
        .await?)
    }
}

/// Mint and Melt Limits
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MintMeltLimits {
    /// Min mint amount
    pub mint_min: Amount,
    /// Max mint amount
    pub mint_max: Amount,
    /// Min melt amount
    pub melt_min: Amount,
    /// Max melt amount
    pub melt_max: Amount,
}

impl MintMeltLimits {
    /// Create new [`MintMeltLimits`]. The `min` and `max` limits apply to both minting and melting.
    pub fn new(min: u64, max: u64) -> Self {
        Self {
            mint_min: min.into(),
            mint_max: max.into(),
            melt_min: min.into(),
            melt_max: max.into(),
        }
    }
}

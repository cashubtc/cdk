//! Mint Builder

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::anyhow;

use super::nut17::SupportedMethods;
use super::nut19::{self, CachedEndpoint};
use super::Nuts;
use crate::amount::Amount;
use crate::cdk_database::{self, MintDatabase};
use crate::cdk_lightning::{self, MintLightning};
use crate::mint::Mint;
use crate::nuts::{
    ContactInfo, CurrencyUnit, MeltMethodSettings, MintInfo, MintMethodSettings, MintVersion,
    MppMethodSettings, PaymentMethod,
};
use crate::types::{LnKey, QuoteTTL};

/// Cashu Mint
#[derive(Default)]
pub struct MintBuilder {
    /// Mint Url
    mint_url: Option<String>,
    /// Mint Info
    mint_info: MintInfo,
    /// Mint Storage backend
    localstore: Option<Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>>,
    /// Ln backends for mint
    ln: Option<HashMap<LnKey, Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>>>,
    seed: Option<Vec<u8>>,
    quote_ttl: Option<QuoteTTL>,
    supported_units: HashMap<CurrencyUnit, (u64, u8)>,
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
            .nut14(true);

        builder.mint_info.nuts = nuts;

        builder
    }

    /// Set localstore
    pub fn with_localstore(
        mut self,
        localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>,
    ) -> MintBuilder {
        self.localstore = Some(localstore);
        self
    }

    /// Set mint url
    pub fn with_mint_url(mut self, mint_url: String) -> Self {
        self.mint_url = Some(mint_url);
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
    pub fn add_ln_backend(
        mut self,
        unit: CurrencyUnit,
        method: PaymentMethod,
        limits: MintMeltLimits,
        ln_backend: Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
    ) -> Self {
        let ln_key = LnKey {
            unit: unit.clone(),
            method,
        };

        let mut ln = self.ln.unwrap_or_default();

        let settings = ln_backend.get_settings();

        if settings.mpp {
            let mpp_settings = MppMethodSettings {
                method,
                unit: unit.clone(),
            };

            let mut mpp = self.mint_info.nuts.nut15.clone();

            mpp.methods.push(mpp_settings);

            self.mint_info.nuts.nut15 = mpp;
        }

        match method {
            PaymentMethod::Bolt11 => {
                let mint_method_settings = MintMethodSettings {
                    method,
                    unit: unit.clone(),
                    min_amount: Some(limits.mint_min),
                    max_amount: Some(limits.mint_max),
                    description: settings.invoice_description,
                };

                self.mint_info.nuts.nut04.methods.push(mint_method_settings);
                self.mint_info.nuts.nut04.disabled = false;

                let melt_method_settings = MeltMethodSettings {
                    method,
                    unit,
                    min_amount: Some(limits.melt_min),
                    max_amount: Some(limits.melt_max),
                };
                self.mint_info.nuts.nut05.methods.push(melt_method_settings);
                self.mint_info.nuts.nut05.disabled = false;
            }
        }

        ln.insert(ln_key.clone(), ln_backend);

        let mut supported_units = self.supported_units.clone();

        supported_units.insert(ln_key.unit, (0, 32));
        self.supported_units = supported_units;

        self.ln = Some(ln);

        self
    }

    /// Set quote ttl
    pub fn with_quote_ttl(mut self, mint_ttl: u64, melt_ttl: u64) -> Self {
        let quote_ttl = QuoteTTL { mint_ttl, melt_ttl };

        self.quote_ttl = Some(quote_ttl);

        self
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

    /// Build mint
    pub async fn build(&self) -> anyhow::Result<Mint> {
        Ok(Mint::new(
            self.mint_url.as_ref().ok_or(anyhow!("Mint url not set"))?,
            self.seed.as_ref().ok_or(anyhow!("Mint seed not set"))?,
            self.mint_info.clone(),
            self.quote_ttl.ok_or(anyhow!("Quote ttl not set"))?,
            self.localstore
                .clone()
                .ok_or(anyhow!("Localstore not set"))?,
            self.ln.clone().ok_or(anyhow!("Ln backends not set"))?,
            self.supported_units.clone(),
            HashMap::new(),
        )
        .await?)
    }
}

/// Mint Melt Limits
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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

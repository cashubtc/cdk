//! Mint Builder

use std::collections::HashMap;
use std::sync::Arc;

use bitcoin::bip32::DerivationPath;
use cdk_common::database::{DynMintDatabase, MintKeysDatabase};
use cdk_common::error::Error;
use cdk_common::nut04::MintMethodOptions;
use cdk_common::nut05::MeltMethodOptions;
use cdk_common::payment::{Bolt11Settings, DynMintPayment};
#[cfg(feature = "auth")]
use cdk_common::{database::DynMintAuthDatabase, nut21, nut22};
use cdk_signatory::signatory::Signatory;

use super::nut17::SupportedMethods;
use super::nut19::{self, CachedEndpoint};
use super::Nuts;
use crate::amount::Amount;
use crate::cdk_database;
use crate::mint::Mint;
#[cfg(feature = "auth")]
use crate::nuts::ProtectedEndpoint;
use crate::nuts::{
    ContactInfo, CurrencyUnit, MeltMethodSettings, MintInfo, MintMethodSettings, MintVersion,
    MppMethodSettings, PaymentMethod,
};
use crate::types::PaymentProcessorKey;

/// Cashu Mint Builder
pub struct MintBuilder {
    mint_info: MintInfo,
    localstore: DynMintDatabase,
    #[cfg(feature = "auth")]
    auth_localstore: Option<DynMintAuthDatabase>,
    payment_processors: HashMap<PaymentProcessorKey, DynMintPayment>,
    supported_units: HashMap<CurrencyUnit, (u64, u8)>,
    custom_paths: HashMap<CurrencyUnit, DerivationPath>,
}

impl MintBuilder {
    /// New [`MintBuilder`]
    pub fn new(localstore: DynMintDatabase) -> MintBuilder {
        let mint_info = MintInfo {
            nuts: Nuts::new()
                .nut07(true)
                .nut08(true)
                .nut09(true)
                .nut10(true)
                .nut11(true)
                .nut12(true)
                .nut20(true),
            ..Default::default()
        };

        MintBuilder {
            mint_info,
            localstore,
            #[cfg(feature = "auth")]
            auth_localstore: None,
            payment_processors: HashMap::new(),
            supported_units: HashMap::new(),
            custom_paths: HashMap::new(),
        }
    }

    /// Set clear auth settings
    #[cfg(feature = "auth")]
    pub fn with_auth(
        mut self,
        auth_localstore: DynMintAuthDatabase,
        openid_discovery: String,
        client_id: String,
        protected_endpoints: Vec<ProtectedEndpoint>,
    ) -> Self {
        self.auth_localstore = Some(auth_localstore);
        self.mint_info.nuts.nut21 = Some(nut21::Settings::new(
            openid_discovery,
            client_id,
            protected_endpoints,
        ));
        self
    }

    /// Initialize builder's MintInfo from the database if present.
    /// If not present or parsing fails, keeps the current MintInfo.
    pub async fn init_from_db_if_present(&mut self) -> Result<(), cdk_database::Error> {
        // Attempt to read existing mint_info from the KV store
        let bytes_opt = self
            .localstore
            .kv_read(
                super::CDK_MINT_PRIMARY_NAMESPACE,
                super::CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
                super::CDK_MINT_CONFIG_KV_KEY,
            )
            .await?;

        if let Some(bytes) = bytes_opt {
            if let Ok(info) = serde_json::from_slice::<MintInfo>(&bytes) {
                self.mint_info = info;
            } else {
                // If parsing fails, leave the current builder state untouched
                tracing::warn!("Failed to parse existing mint_info from DB; using builder state");
            }
        }

        Ok(())
    }

    /// Set blind auth settings
    #[cfg(feature = "auth")]
    pub fn with_blind_auth(
        mut self,
        bat_max_mint: u64,
        protected_endpoints: Vec<ProtectedEndpoint>,
    ) -> Self {
        let mut nuts = self.mint_info.nuts;

        nuts.nut22 = Some(nut22::Settings::new(bat_max_mint, protected_endpoints));

        self.mint_info.nuts = nuts;

        self
    }

    /// Set mint info
    pub fn with_mint_info(mut self, mint_info: MintInfo) -> Self {
        self.mint_info = mint_info;
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

    /// Get a clone of the current MintInfo configured on the builder
    /// This allows using config-derived settings to initialize persistent state
    /// before any attempt to read from the database, which avoids first-run
    /// failures when the DB is empty.
    pub fn current_mint_info(&self) -> MintInfo {
        self.mint_info.clone()
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
    pub fn with_contact_info(mut self, contact_info: ContactInfo) -> Self {
        let mut contacts = self.mint_info.contact.clone().unwrap_or_default();
        contacts.push(contact_info);
        self.mint_info.contact = Some(contacts);
        self
    }

    /// Set pubkey
    pub fn with_pubkey(mut self, pubkey: crate::nuts::PublicKey) -> Self {
        self.mint_info.pubkey = Some(pubkey);

        self
    }

    /// Support websockets
    pub fn with_supported_websockets(mut self, supported_method: SupportedMethods) -> Self {
        let mut supported_settings = self.mint_info.nuts.nut17.supported.clone();

        if !supported_settings.contains(&supported_method) {
            supported_settings.push(supported_method);

            self.mint_info.nuts = self.mint_info.nuts.nut17(supported_settings);
        }

        self
    }

    /// Add support for NUT19
    pub fn with_cache(mut self, ttl: Option<u64>, cached_endpoints: Vec<CachedEndpoint>) -> Self {
        let nut19_settings = nut19::Settings {
            ttl,
            cached_endpoints,
        };

        self.mint_info.nuts.nut19 = nut19_settings;

        self
    }

    /// Set custom derivation paths for mint units
    pub fn with_custom_derivation_paths(
        mut self,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Self {
        self.custom_paths = custom_paths;
        self
    }

    /// Add payment processor
    pub async fn add_payment_processor(
        &mut self,
        unit: CurrencyUnit,
        method: PaymentMethod,
        limits: MintMeltLimits,
        payment_processor: DynMintPayment,
    ) -> Result<(), Error> {
        let key = PaymentProcessorKey {
            unit: unit.clone(),
            method: method.clone(),
        };

        let settings = payment_processor.get_settings().await?;

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

        let mint_method_settings = MintMethodSettings {
            method: method.clone(),
            unit: unit.clone(),
            min_amount: Some(limits.mint_min),
            max_amount: Some(limits.mint_max),
            options: Some(MintMethodOptions::Bolt11 {
                description: settings.invoice_description,
            }),
        };

        self.mint_info.nuts.nut04.methods.push(mint_method_settings);
        self.mint_info.nuts.nut04.disabled = false;

        let melt_method_settings = MeltMethodSettings {
            method,
            unit,
            min_amount: Some(limits.melt_min),
            max_amount: Some(limits.melt_max),
            options: Some(MeltMethodOptions::Bolt11 {
                amountless: settings.amountless,
            }),
        };
        self.mint_info.nuts.nut05.methods.push(melt_method_settings);
        self.mint_info.nuts.nut05.disabled = false;

        let mut supported_units = self.supported_units.clone();

        supported_units.insert(key.unit.clone(), (0, 32));
        self.supported_units = supported_units;

        self.payment_processors.insert(key, payment_processor);
        Ok(())
    }
    /// Sets the input fee ppk for a given unit
    ///
    /// The unit **MUST** already have been added with a ln backend
    pub fn set_unit_fee(&mut self, unit: &CurrencyUnit, input_fee_ppk: u64) -> Result<(), Error> {
        let (input_fee, _max_order) = self
            .supported_units
            .get_mut(unit)
            .ok_or(Error::UnsupportedUnit)?;

        *input_fee = input_fee_ppk;

        Ok(())
    }

    /// Build the mint with the provided signatory
    pub async fn build_with_signatory(
        self,
        signatory: Arc<dyn Signatory + Send + Sync>,
    ) -> Result<Mint, Error> {
        #[cfg(feature = "auth")]
        if let Some(auth_localstore) = self.auth_localstore {
            return Mint::new_with_auth(
                self.mint_info,
                signatory,
                self.localstore,
                auth_localstore,
                self.payment_processors,
            )
            .await;
        }
        Mint::new(
            self.mint_info,
            signatory,
            self.localstore,
            self.payment_processors,
        )
        .await
    }

    /// Build the mint with the provided keystore and seed
    pub async fn build_with_seed(
        self,
        keystore: Arc<dyn MintKeysDatabase<Err = cdk_database::Error> + Send + Sync>,
        seed: &[u8],
    ) -> Result<Mint, Error> {
        let in_memory_signatory = cdk_signatory::db_signatory::DbSignatory::new(
            keystore,
            seed,
            self.supported_units.clone(),
            HashMap::new(),
        )
        .await?;

        let signatory = Arc::new(cdk_signatory::embedded::Service::new(Arc::new(
            in_memory_signatory,
        )));

        self.build_with_signatory(signatory).await
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

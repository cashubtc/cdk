//! Mint Builder

use std::collections::HashMap;
use std::sync::Arc;

use bitcoin::bip32::DerivationPath;
use cdk_common::database::{DynMintAuthDatabase, DynMintDatabase, MintKeysDatabase};
use cdk_common::error::Error;
use cdk_common::nut00::KnownMethod;
use cdk_common::nut04::MintMethodOptions;
use cdk_common::nut05::MeltMethodOptions;
use cdk_common::payment::DynMintPayment;
use cdk_common::{nut21, nut22};
use cdk_signatory::signatory::{RotateKeyArguments, Signatory};

use super::nut17::SupportedMethods;
use super::nut19::{self, CachedEndpoint};
use super::Nuts;
use crate::amount::Amount;
use crate::cdk_database;
use crate::mint::Mint;
use crate::nuts::{
    ContactInfo, CurrencyUnit, MeltMethodSettings, MintInfo, MintMethodSettings, MintVersion,
    MppMethodSettings, PaymentMethod, ProtectedEndpoint,
};
use crate::types::PaymentProcessorKey;

/// Cashu Mint Builder
pub struct MintBuilder {
    mint_info: MintInfo,
    localstore: DynMintDatabase,
    auth_localstore: Option<DynMintAuthDatabase>,
    payment_processors: HashMap<PaymentProcessorKey, DynMintPayment>,
    supported_units: HashMap<CurrencyUnit, (u64, u8)>,
    custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    use_keyset_v2: Option<bool>,
    max_inputs: usize,
    max_outputs: usize,
}

impl std::fmt::Debug for MintBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintBuilder")
            .field("mint_info", &self.mint_info)
            .field("supported_units", &self.supported_units)
            .finish_non_exhaustive()
    }
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
                .nut14(true)
                .nut20(true),
            ..Default::default()
        };

        MintBuilder {
            mint_info,
            localstore,
            auth_localstore: None,
            payment_processors: HashMap::new(),
            supported_units: HashMap::new(),
            custom_paths: HashMap::new(),
            use_keyset_v2: None,
            max_inputs: 1000,
            max_outputs: 1000,
        }
    }

    /// Set use keyset v2
    pub fn with_keyset_v2(mut self, use_keyset_v2: Option<bool>) -> Self {
        self.use_keyset_v2 = use_keyset_v2;
        self
    }

    /// Set clear auth settings
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

    /// Set transaction limits for DoS protection
    pub fn with_limits(mut self, max_inputs: usize, max_outputs: usize) -> Self {
        self.max_inputs = max_inputs;
        self.max_outputs = max_outputs;
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

        match method {
            // Handle bolt11 methods
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                if let Some(ref bolt11_settings) = settings.bolt11 {
                    // Add MPP support if available
                    if bolt11_settings.mpp {
                        let mpp_settings = MppMethodSettings {
                            method: method.clone(),
                            unit: unit.clone(),
                        };

                        let mut mpp = self.mint_info.nuts.nut15.clone();
                        mpp.methods.push(mpp_settings);
                        self.mint_info.nuts.nut15 = mpp;
                    }

                    // Add to NUT04 (mint)
                    let mint_method_settings = MintMethodSettings {
                        method: method.clone(),
                        unit: unit.clone(),
                        min_amount: Some(limits.mint_min),
                        max_amount: Some(limits.mint_max),
                        options: Some(MintMethodOptions::Bolt11 {
                            description: bolt11_settings.invoice_description,
                        }),
                    };
                    self.mint_info.nuts.nut04.methods.push(mint_method_settings);
                    self.mint_info.nuts.nut04.disabled = false;

                    // Add to NUT05 (melt)
                    let melt_method_settings = MeltMethodSettings {
                        method: method.clone(),
                        unit: unit.clone(),
                        min_amount: Some(limits.melt_min),
                        max_amount: Some(limits.melt_max),
                        options: Some(MeltMethodOptions::Bolt11 {
                            amountless: bolt11_settings.amountless,
                        }),
                    };
                    self.mint_info.nuts.nut05.methods.push(melt_method_settings);
                    self.mint_info.nuts.nut05.disabled = false;
                }
            }
            // Handle bolt12 methods
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                if settings.bolt12.is_some() {
                    // Add to NUT04 (mint) - bolt12 doesn't have specific options yet
                    let mint_method_settings = MintMethodSettings {
                        method: method.clone(),
                        unit: unit.clone(),
                        min_amount: Some(limits.mint_min),
                        max_amount: Some(limits.mint_max),
                        options: None, // No bolt12-specific options in NUT04 yet
                    };
                    self.mint_info.nuts.nut04.methods.push(mint_method_settings);
                    self.mint_info.nuts.nut04.disabled = false;

                    // Add to NUT05 (melt) - bolt12 doesn't have specific options in MeltMethodOptions yet
                    let melt_method_settings = MeltMethodSettings {
                        method: method.clone(),
                        unit: unit.clone(),
                        min_amount: Some(limits.melt_min),
                        max_amount: Some(limits.melt_max),
                        options: None, // No bolt12-specific options in NUT05 yet
                    };
                    self.mint_info.nuts.nut05.methods.push(melt_method_settings);
                    self.mint_info.nuts.nut05.disabled = false;
                }
            }
            // Handle custom methods
            PaymentMethod::Custom(_) => {
                // Check if this custom method is supported by the payment processor
                if settings.custom.contains_key(method.as_str()) {
                    // Add to NUT04 (mint)
                    let mint_method_settings = MintMethodSettings {
                        method: method.clone(),
                        unit: unit.clone(),
                        min_amount: Some(limits.mint_min),
                        max_amount: Some(limits.mint_max),
                        options: Some(MintMethodOptions::Custom {}),
                    };
                    self.mint_info.nuts.nut04.methods.push(mint_method_settings);
                    self.mint_info.nuts.nut04.disabled = false;

                    // Add to NUT05 (melt)
                    let melt_method_settings = MeltMethodSettings {
                        method: method.clone(),
                        unit: unit.clone(),
                        min_amount: Some(limits.melt_min),
                        max_amount: Some(limits.melt_max),
                        options: None, // No custom-specific options in NUT05 yet
                    };
                    self.mint_info.nuts.nut05.methods.push(melt_method_settings);
                    self.mint_info.nuts.nut05.disabled = false;
                }
            }
        }

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
        let (input_fee, _) = self
            .supported_units
            .get_mut(unit)
            .ok_or(Error::UnsupportedUnit)?;

        *input_fee = input_fee_ppk;

        Ok(())
    }

    /// Build the mint with the provided signatory
    pub async fn build_with_signatory(
        #[allow(unused_mut)] mut self,
        signatory: Arc<dyn Signatory + Send + Sync>,
    ) -> Result<Mint, Error> {
        // Check active keysets and rotate if necessary
        let active_keysets = signatory.keysets().await?;

        // Ensure Auth keyset is created when auth is enabled
        if self.auth_localstore.is_some() {
            self.supported_units
                .entry(CurrencyUnit::Auth)
                .or_insert((0, 1));
        }

        for (unit, (fee, max_order)) in &self.supported_units {
            // Check if we have an active keyset for this unit
            let keyset = active_keysets
                .keysets
                .iter()
                .find(|k| k.active && k.unit == *unit);

            let mut rotate = false;

            if let Some(keyset) = keyset {
                // Check if fee matches
                if keyset.input_fee_ppk != *fee {
                    tracing::info!(
                        "Rotating keyset for unit {} due to fee mismatch (current: {}, expected: {})",
                        unit,
                        keyset.input_fee_ppk,
                        fee
                    );
                    rotate = true;
                }

                // Check if version matches explicit preference
                if let Some(want_v2) = self.use_keyset_v2 {
                    let is_v2 =
                        keyset.id.get_version() == cdk_common::nut02::KeySetVersion::Version02;
                    if want_v2 && !is_v2 {
                        tracing::info!("Rotating keyset for unit {} due to explicit V2 preference (current is V1)", unit);
                        rotate = true;
                    } else if !want_v2 && is_v2 {
                        tracing::info!("Rotating keyset for unit {} due to explicit V1 preference (current is V2)", unit);
                        rotate = true;
                    }
                }
            } else {
                // No active keyset for this unit
                tracing::info!("Rotating keyset for unit {} (no active keyset found)", unit);
                rotate = true;
            }

            if rotate {
                let amounts: Vec<u64> = (0..*max_order).map(|i| 2_u64.pow(i as u32)).collect();
                signatory
                    .rotate_keyset(RotateKeyArguments {
                        unit: unit.clone(),
                        amounts,
                        input_fee_ppk: *fee,
                        keyset_id_type: if self.use_keyset_v2.unwrap_or(true) {
                            cdk_common::nut02::KeySetVersion::Version02
                        } else {
                            cdk_common::nut02::KeySetVersion::Version01
                        },
                        final_expiry: None,
                    })
                    .await?;
            }
        }

        if let Some(auth_localstore) = self.auth_localstore {
            return Mint::new_with_auth(
                self.mint_info,
                signatory,
                self.localstore,
                auth_localstore,
                self.payment_processors,
                self.max_inputs,
                self.max_outputs,
            )
            .await;
        }
        Mint::new(
            self.mint_info,
            signatory,
            self.localstore,
            self.payment_processors,
            self.max_inputs,
            self.max_outputs,
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
            self.custom_paths.clone(),
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::Arc;

    use async_trait::async_trait;
    use cdk_common::payment::{
        Bolt11Settings, Bolt12Settings, CreateIncomingPaymentResponse, Event,
        IncomingPaymentOptions, MakePaymentResponse, OutgoingPaymentOptions, PaymentIdentifier,
        PaymentQuoteResponse, SettingsResponse,
    };
    use cdk_sqlite::mint::memory;
    use futures::Stream;
    use KnownMethod;

    use super::*;

    // Mock payment processor for testing
    struct MockPaymentProcessor {
        settings: SettingsResponse,
    }

    #[async_trait]
    impl cdk_common::payment::MintPayment for MockPaymentProcessor {
        type Err = cdk_common::payment::Error;

        async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
            Ok(self.settings.clone())
        }

        async fn create_incoming_payment_request(
            &self,
            _unit: &CurrencyUnit,
            _options: IncomingPaymentOptions,
        ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
            unimplemented!()
        }

        async fn get_payment_quote(
            &self,
            _unit: &CurrencyUnit,
            _options: OutgoingPaymentOptions,
        ) -> Result<PaymentQuoteResponse, Self::Err> {
            unimplemented!()
        }

        async fn make_payment(
            &self,
            _unit: &CurrencyUnit,
            _options: OutgoingPaymentOptions,
        ) -> Result<MakePaymentResponse, Self::Err> {
            unimplemented!()
        }

        async fn wait_payment_event(
            &self,
        ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
            unimplemented!()
        }

        fn is_wait_invoice_active(&self) -> bool {
            false
        }

        fn cancel_wait_invoice(&self) {}

        async fn check_incoming_payment_status(
            &self,
            _payment_identifier: &PaymentIdentifier,
        ) -> Result<Vec<cdk_common::payment::WaitPaymentResponse>, Self::Err> {
            unimplemented!()
        }

        async fn check_outgoing_payment(
            &self,
            _payment_identifier: &PaymentIdentifier,
        ) -> Result<MakePaymentResponse, Self::Err> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn test_mint_builder_default_nuts_support() {
        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);
        let mint_info = builder.current_mint_info();

        assert!(
            mint_info.nuts.nut07.supported,
            "NUT-07 should be supported by default"
        );
        assert!(
            mint_info.nuts.nut08.supported,
            "NUT-08 should be supported by default"
        );
        assert!(
            mint_info.nuts.nut09.supported,
            "NUT-09 should be supported by default"
        );
        assert!(
            mint_info.nuts.nut10.supported,
            "NUT-10 should be supported by default"
        );
        assert!(
            mint_info.nuts.nut11.supported,
            "NUT-11 should be supported by default"
        );
        assert!(
            mint_info.nuts.nut12.supported,
            "NUT-12 should be supported by default"
        );
        assert!(
            mint_info.nuts.nut14.supported,
            "NUT-14 (HTLC) should be supported by default"
        );
        assert!(
            mint_info.nuts.nut20.supported,
            "NUT-20 should be supported by default"
        );
    }

    #[tokio::test]
    async fn test_add_payment_processor_bolt11() {
        let localstore = Arc::new(memory::empty().await.unwrap());
        let mut builder = MintBuilder::new(localstore);

        let bolt11_settings = Bolt11Settings {
            mpp: true,
            amountless: true,
            invoice_description: true,
        };

        let settings = SettingsResponse {
            unit: "sat".to_string(),
            bolt11: Some(bolt11_settings),
            bolt12: None,
            custom: HashMap::new(),
        };

        let payment_processor = Arc::new(MockPaymentProcessor { settings });
        let unit = CurrencyUnit::Sat;
        let method = PaymentMethod::Known(KnownMethod::Bolt11);
        let limits = MintMeltLimits::new(100, 10000);

        builder
            .add_payment_processor(unit.clone(), method.clone(), limits, payment_processor)
            .await
            .unwrap();

        let mint_info = builder.current_mint_info();

        // Check NUT04 (mint) settings
        assert!(!mint_info.nuts.nut04.disabled);
        assert_eq!(mint_info.nuts.nut04.methods.len(), 1);
        let mint_method = &mint_info.nuts.nut04.methods[0];
        assert_eq!(mint_method.method, method);
        assert_eq!(mint_method.unit, unit);
        assert_eq!(mint_method.min_amount, Some(limits.mint_min));
        assert_eq!(mint_method.max_amount, Some(limits.mint_max));
        assert!(matches!(
            mint_method.options,
            Some(MintMethodOptions::Bolt11 { description: true })
        ));

        // Check NUT05 (melt) settings
        assert!(!mint_info.nuts.nut05.disabled);
        assert_eq!(mint_info.nuts.nut05.methods.len(), 1);
        let melt_method = &mint_info.nuts.nut05.methods[0];
        assert_eq!(melt_method.method, method);
        assert_eq!(melt_method.unit, unit);
        assert_eq!(melt_method.min_amount, Some(limits.melt_min));
        assert_eq!(melt_method.max_amount, Some(limits.melt_max));
        assert!(matches!(
            melt_method.options,
            Some(MeltMethodOptions::Bolt11 { amountless: true })
        ));

        // Check NUT15 (MPP) settings
        assert_eq!(mint_info.nuts.nut15.methods.len(), 1);
        let mpp_method = &mint_info.nuts.nut15.methods[0];
        assert_eq!(mpp_method.method, method);
        assert_eq!(mpp_method.unit, unit);
    }

    #[tokio::test]
    async fn test_add_payment_processor_bolt11_without_mpp() {
        let localstore = Arc::new(memory::empty().await.unwrap());
        let mut builder = MintBuilder::new(localstore);

        let bolt11_settings = Bolt11Settings {
            mpp: false, // MPP disabled
            amountless: false,
            invoice_description: false,
        };

        let settings = SettingsResponse {
            unit: "sat".to_string(),
            bolt11: Some(bolt11_settings),
            bolt12: None,
            custom: HashMap::new(),
        };

        let payment_processor = Arc::new(MockPaymentProcessor { settings });
        let unit = CurrencyUnit::Sat;
        let method = PaymentMethod::Known(KnownMethod::Bolt11);
        let limits = MintMeltLimits::new(100, 10000);

        builder
            .add_payment_processor(unit, method, limits, payment_processor)
            .await
            .unwrap();

        let mint_info = builder.current_mint_info();

        // NUT15 should be empty when MPP is disabled
        assert_eq!(mint_info.nuts.nut15.methods.len(), 0);

        // But NUT04 and NUT05 should still be populated
        assert_eq!(mint_info.nuts.nut04.methods.len(), 1);
        assert_eq!(mint_info.nuts.nut05.methods.len(), 1);
    }

    #[tokio::test]
    async fn test_add_payment_processor_bolt12() {
        let localstore = Arc::new(memory::empty().await.unwrap());
        let mut builder = MintBuilder::new(localstore);

        let bolt12_settings = Bolt12Settings { amountless: true };

        let settings = SettingsResponse {
            unit: "sat".to_string(),
            bolt11: None,
            bolt12: Some(bolt12_settings),
            custom: HashMap::new(),
        };

        let payment_processor = Arc::new(MockPaymentProcessor { settings });
        let unit = CurrencyUnit::Sat;
        let method = PaymentMethod::Known(KnownMethod::Bolt12);
        let limits = MintMeltLimits::new(100, 10000);

        builder
            .add_payment_processor(unit.clone(), method.clone(), limits, payment_processor)
            .await
            .unwrap();

        let mint_info = builder.current_mint_info();

        // Check NUT04 (mint) settings
        assert!(!mint_info.nuts.nut04.disabled);
        assert_eq!(mint_info.nuts.nut04.methods.len(), 1);
        let mint_method = &mint_info.nuts.nut04.methods[0];
        assert_eq!(mint_method.method, method);
        assert_eq!(mint_method.unit, unit);
        assert_eq!(mint_method.min_amount, Some(limits.mint_min));
        assert_eq!(mint_method.max_amount, Some(limits.mint_max));
        assert!(mint_method.options.is_none());

        // Check NUT05 (melt) settings
        assert!(!mint_info.nuts.nut05.disabled);
        assert_eq!(mint_info.nuts.nut05.methods.len(), 1);
        let melt_method = &mint_info.nuts.nut05.methods[0];
        assert_eq!(melt_method.method, method);
        assert_eq!(melt_method.unit, unit);
        assert_eq!(melt_method.min_amount, Some(limits.melt_min));
        assert_eq!(melt_method.max_amount, Some(limits.melt_max));
        assert!(melt_method.options.is_none());
    }

    #[tokio::test]
    async fn test_add_payment_processor_custom() {
        let localstore = Arc::new(memory::empty().await.unwrap());
        let mut builder = MintBuilder::new(localstore);

        let mut custom_methods = HashMap::new();
        custom_methods.insert("paypal".to_string(), "{}".to_string());

        let settings = SettingsResponse {
            unit: "usd".to_string(),
            bolt11: None,
            bolt12: None,
            custom: custom_methods,
        };

        let payment_processor = Arc::new(MockPaymentProcessor { settings });
        let unit = CurrencyUnit::Usd;
        let method = PaymentMethod::Custom("paypal".to_string());
        let limits = MintMeltLimits::new(100, 10000);

        builder
            .add_payment_processor(unit.clone(), method.clone(), limits, payment_processor)
            .await
            .unwrap();

        let mint_info = builder.current_mint_info();

        // Check NUT04 (mint) settings
        assert!(!mint_info.nuts.nut04.disabled);
        assert_eq!(mint_info.nuts.nut04.methods.len(), 1);
        let mint_method = &mint_info.nuts.nut04.methods[0];
        assert_eq!(mint_method.method, method);
        assert_eq!(mint_method.unit, unit);
        assert_eq!(mint_method.min_amount, Some(limits.mint_min));
        assert_eq!(mint_method.max_amount, Some(limits.mint_max));
        assert!(matches!(
            mint_method.options,
            Some(MintMethodOptions::Custom {})
        ));

        // Check NUT05 (melt) settings
        assert!(!mint_info.nuts.nut05.disabled);
        assert_eq!(mint_info.nuts.nut05.methods.len(), 1);
        let melt_method = &mint_info.nuts.nut05.methods[0];
        assert_eq!(melt_method.method, method);
        assert_eq!(melt_method.unit, unit);
        assert_eq!(melt_method.min_amount, Some(limits.melt_min));
        assert_eq!(melt_method.max_amount, Some(limits.melt_max));
        assert!(melt_method.options.is_none());
    }

    #[tokio::test]
    async fn test_add_payment_processor_custom_not_supported() {
        let localstore = Arc::new(memory::empty().await.unwrap());
        let mut builder = MintBuilder::new(localstore);

        // Settings with no custom methods
        let settings = SettingsResponse {
            unit: "usd".to_string(),
            bolt11: None,
            bolt12: None,
            custom: HashMap::new(), // Empty - no custom methods supported
        };

        let payment_processor = Arc::new(MockPaymentProcessor { settings });
        let unit = CurrencyUnit::Usd;
        let method = PaymentMethod::Custom("paypal".to_string());
        let limits = MintMeltLimits::new(1, 1000);

        builder
            .add_payment_processor(unit, method, limits, payment_processor)
            .await
            .unwrap();

        let mint_info = builder.current_mint_info();

        // NUT04 and NUT05 should remain empty since the custom method is not in settings
        assert_eq!(mint_info.nuts.nut04.methods.len(), 0);
        assert_eq!(mint_info.nuts.nut05.methods.len(), 0);
    }

    #[tokio::test]
    async fn test_add_multiple_payment_processors() {
        let localstore = Arc::new(memory::empty().await.unwrap());
        let mut builder = MintBuilder::new(localstore);

        // Add Bolt11
        let bolt11_settings = Bolt11Settings {
            mpp: false,
            amountless: true,
            invoice_description: false,
        };
        let settings1 = SettingsResponse {
            unit: "sat".to_string(),
            bolt11: Some(bolt11_settings),
            bolt12: None,
            custom: HashMap::new(),
        };
        let processor1 = Arc::new(MockPaymentProcessor {
            settings: settings1,
        });
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(100, 10000),
                processor1,
            )
            .await
            .unwrap();

        // Add Bolt12
        let bolt12_settings = Bolt12Settings { amountless: false };
        let settings2 = SettingsResponse {
            unit: "sat".to_string(),
            bolt11: None,
            bolt12: Some(bolt12_settings),
            custom: HashMap::new(),
        };
        let processor2 = Arc::new(MockPaymentProcessor {
            settings: settings2,
        });
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt12),
                MintMeltLimits::new(200, 20000),
                processor2,
            )
            .await
            .unwrap();

        let mint_info = builder.current_mint_info();

        // Should have both methods in NUT04 and NUT05
        assert_eq!(mint_info.nuts.nut04.methods.len(), 2);
        assert_eq!(mint_info.nuts.nut05.methods.len(), 2);
    }
}

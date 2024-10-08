use std::{collections::HashMap, sync::Arc};

use cdk::{
    cdk_database::{self, MintDatabase},
    cdk_lightning::{self, MintLightning},
    nuts::{CurrencyUnit, MintInfo, PaymentMethod},
    types::LnKey,
};

/// Cashu Mint
#[derive(Default)]
pub struct MintBuilder {
    /// Mint Url
    mint_url: Option<String>,
    /// Mint Info
    mint_info: Option<MintInfo>,
    /// Mint Storage backend
    localstore: Option<Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>>,
    /// Ln backends for mint
    ln: Option<HashMap<LnKey, Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>>>,
    seed: Option<Vec<u8>>,
}

impl MintBuilder {
    pub fn new() -> MintBuilder {
        MintBuilder::default()
    }

    pub fn with_localstore(
        mut self,
        localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>,
    ) -> MintBuilder {
        self.localstore = Some(localstore);
        self
    }

    pub fn with_info(mut self, mint_info: MintInfo) -> Self {
        self.mint_info = Some(mint_info);
        self
    }

    pub fn with_mint_url(mut self, mint_url: String) -> Self {
        self.mint_url = Some(mint_url);
        self
    }

    pub fn with_seed(mut self, seed: Vec<u8>) -> Self {
        self.seed = Some(seed);
        self
    }

    pub fn add_ln_backend(
        mut self,
        unit: CurrencyUnit,
        method: PaymentMethod,
        ln_backend: Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
    ) -> Self {
        let ln_key = LnKey { unit, method };

        let mut ln = self.ln.unwrap_or_default();

        let _settings = ln_backend.get_settings();

        ln.insert(ln_key, ln_backend);

        self.ln = Some(ln);

        self
    }
}

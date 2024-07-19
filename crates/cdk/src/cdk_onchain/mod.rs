//! CDK Mint Lightning

use async_trait::async_trait;
use lightning_invoice::ParseOrSemanticError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::nuts::CurrencyUnit;
use crate::{mint, Amount};

/// CDK Onchain Error
#[derive(Debug, Error)]
pub enum Error {
    /// Lightning Error
    #[error(transparent)]
    Oncahin(Box<dyn std::error::Error + Send + Sync>),
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// AnyHow Error
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
    /// Parse Error
    #[error(transparent)]
    Parse(#[from] ParseOrSemanticError),
    /// Cannot convert units
    #[error("Cannot convert units")]
    CannotConvertUnits,
}

/// MintLighting Trait
#[async_trait]
pub trait MintOnChain {
    /// Mint Lightning Error
    type Err: Into<Error> + From<Error>;

    /// Base Unit
    fn get_settings(&self) -> Settings;

    /// New onchain address
    async fn new_address(&self) -> Result<NewAddressResponse, Self::Err>;

    /// Pay Address
    async fn pay_address(
        &self,
        melt_quote: mint::MeltQuote,
        max_fee_sat: Amount,
    ) -> Result<String, Self::Err>;

    /// Check if an address has been paid
    async fn check_address_paid(&self, address: &str) -> Result<AddressPaidResponse, Self::Err>;
}

/// New Address Response
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewAddressResponse {
    /// Address
    pub address: String,
    /// Payjoin Url
    pub payjoin_url: Option<String>,
    /// pjos for use with payjoin
    pub pjos: Option<bool>,
}

/// Address paid response
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressPaidResponse {
    /// Amount paid to address (sats)
    pub amount: Amount,
    /// Max block height
    ///
    /// If an address has received multiple payments (it shouldn't).
    /// The most recent blocktime will be used
    pub max_block_height: Option<u32>,
}

/// Ln backend settings
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Min amount to mint
    pub min_mint_amount: u64,
    /// Max amount to mint
    pub max_mint_amount: u64,
    /// Min amount to melt
    pub min_melt_amount: u64,
    /// Max amount to melt
    pub max_melt_amount: u64,
    /// Base unit of backend
    pub unit: CurrencyUnit,
    /// Minting enabled
    pub mint_enabled: bool,
    /// Melting enabled
    pub melt_enabled: bool,
    /// Payjoin supported
    pub payjoin_settings: PayjoinSettings,
}

/// Payjoin settings
#[derive(Debug, Clone, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PayjoinSettings {
    /// Enable payjoin receive support
    pub receive_enabled: bool,
    /// Enable payjoin send support
    pub send_enabled: bool,
    /// Payjoin v2 ohttp relay
    pub ohttp_relay: Option<String>,
    /// Payjoin v2 directory
    pub payjoin_directory: Option<String>,
}

const MSAT_IN_SAT: u64 = 1000;

/// Helper function to convert units
pub fn to_unit<T>(
    amount: T,
    current_unit: &CurrencyUnit,
    target_unit: &CurrencyUnit,
) -> Result<u64, Error>
where
    T: Into<u64>,
{
    let amount = amount.into();
    match (current_unit, target_unit) {
        (CurrencyUnit::Sat, CurrencyUnit::Sat) => Ok(amount),
        (CurrencyUnit::Msat, CurrencyUnit::Msat) => Ok(amount),
        (CurrencyUnit::Sat, CurrencyUnit::Msat) => Ok(amount * MSAT_IN_SAT),
        (CurrencyUnit::Msat, CurrencyUnit::Sat) => Ok(amount / MSAT_IN_SAT),
        (CurrencyUnit::Usd, CurrencyUnit::Usd) => Ok(amount),
        _ => Err(Error::CannotConvertUnits),
    }
}

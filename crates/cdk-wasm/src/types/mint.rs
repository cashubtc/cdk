//! Mint-related WASM types

use std::str::FromStr;

use cdk::nuts::nut00::{KnownMethod, PaymentMethod as NutPaymentMethod};
use serde::{Deserialize, Serialize};

use super::amount::{Amount, CurrencyUnit};
use super::quote::PaymentMethod;
use crate::error::WasmError;

/// WASM-compatible Mint URL
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MintUrl {
    pub url: String,
}

impl MintUrl {
    pub fn new(url: String) -> Result<Self, WasmError> {
        // Validate URL format
        url::Url::parse(&url).map_err(|e| WasmError::internal(format!("Invalid URL: {}", e)))?;

        Ok(Self { url })
    }
}

impl From<cdk::mint_url::MintUrl> for MintUrl {
    fn from(mint_url: cdk::mint_url::MintUrl) -> Self {
        Self {
            url: mint_url.to_string(),
        }
    }
}

impl TryFrom<MintUrl> for cdk::mint_url::MintUrl {
    type Error = WasmError;

    fn try_from(mint_url: MintUrl) -> Result<Self, Self::Error> {
        cdk::mint_url::MintUrl::from_str(&mint_url.url)
            .map_err(|e| WasmError::internal(format!("Invalid URL: {}", e)))
    }
}

/// WASM-compatible MintVersion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintVersion {
    /// Mint Software name
    pub name: String,
    /// Mint Version
    pub version: String,
}

impl From<cdk::nuts::MintVersion> for MintVersion {
    fn from(version: cdk::nuts::MintVersion) -> Self {
        Self {
            name: version.name,
            version: version.version,
        }
    }
}

impl From<MintVersion> for cdk::nuts::MintVersion {
    fn from(version: MintVersion) -> Self {
        Self {
            name: version.name,
            version: version.version,
        }
    }
}

impl MintVersion {
    /// Convert MintVersion to JSON string
    pub fn to_json(&self) -> Result<String, WasmError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode MintVersion from JSON string
pub fn decode_mint_version(json: String) -> Result<MintVersion, WasmError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode MintVersion to JSON string
pub fn encode_mint_version(version: MintVersion) -> Result<String, WasmError> {
    Ok(serde_json::to_string(&version)?)
}

/// WASM-compatible ContactInfo
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactInfo {
    /// Contact Method i.e. nostr
    pub method: String,
    /// Contact info i.e. npub...
    pub info: String,
}

impl From<cdk::nuts::ContactInfo> for ContactInfo {
    fn from(contact: cdk::nuts::ContactInfo) -> Self {
        Self {
            method: contact.method,
            info: contact.info,
        }
    }
}

impl From<ContactInfo> for cdk::nuts::ContactInfo {
    fn from(contact: ContactInfo) -> Self {
        Self {
            method: contact.method,
            info: contact.info,
        }
    }
}

impl ContactInfo {
    /// Convert ContactInfo to JSON string
    pub fn to_json(&self) -> Result<String, WasmError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode ContactInfo from JSON string
pub fn decode_contact_info(json: String) -> Result<ContactInfo, WasmError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode ContactInfo to JSON string
pub fn encode_contact_info(info: ContactInfo) -> Result<String, WasmError> {
    Ok(serde_json::to_string(&info)?)
}

/// WASM-compatible SupportedSettings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SupportedSettings {
    /// Setting supported
    pub supported: bool,
}

impl From<cdk::nuts::nut06::SupportedSettings> for SupportedSettings {
    fn from(settings: cdk::nuts::nut06::SupportedSettings) -> Self {
        Self {
            supported: settings.supported,
        }
    }
}

impl From<SupportedSettings> for cdk::nuts::nut06::SupportedSettings {
    fn from(settings: SupportedSettings) -> Self {
        Self {
            supported: settings.supported,
        }
    }
}

// -----------------------------
// NUT-04/05 WASM Types
// -----------------------------

/// WASM-compatible MintMethodSettings (NUT-04)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintMethodSettings {
    pub method: PaymentMethod,
    pub unit: CurrencyUnit,
    pub min_amount: Option<Amount>,
    pub max_amount: Option<Amount>,
    /// For bolt11, whether mint supports setting invoice description
    pub description: Option<bool>,
}

impl From<cdk::nuts::nut04::MintMethodSettings> for MintMethodSettings {
    fn from(s: cdk::nuts::nut04::MintMethodSettings) -> Self {
        let description = match s.options {
            Some(cdk::nuts::nut04::MintMethodOptions::Bolt11 { description }) => Some(description),
            _ => None,
        };
        Self {
            method: s.method.into(),
            unit: s.unit.into(),
            min_amount: s.min_amount.map(Into::into),
            max_amount: s.max_amount.map(Into::into),
            description,
        }
    }
}

impl TryFrom<MintMethodSettings> for cdk::nuts::nut04::MintMethodSettings {
    type Error = WasmError;

    fn try_from(s: MintMethodSettings) -> Result<Self, Self::Error> {
        let options = match s.method {
            PaymentMethod::Bolt11 => s
                .description
                .map(|description| cdk::nuts::nut04::MintMethodOptions::Bolt11 { description }),
            PaymentMethod::Custom { .. } => Some(cdk::nuts::nut04::MintMethodOptions::Custom {}),
            _ => None,
        };
        Ok(Self {
            method: s.method.into(),
            unit: s.unit.into(),
            min_amount: s.min_amount.map(Into::into),
            max_amount: s.max_amount.map(Into::into),
            options,
        })
    }
}

/// WASM-compatible Nut04 Settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nut04Settings {
    pub methods: Vec<MintMethodSettings>,
    pub disabled: bool,
}

impl From<cdk::nuts::nut04::Settings> for Nut04Settings {
    fn from(s: cdk::nuts::nut04::Settings) -> Self {
        Self {
            methods: s.methods.into_iter().map(Into::into).collect(),
            disabled: s.disabled,
        }
    }
}

impl TryFrom<Nut04Settings> for cdk::nuts::nut04::Settings {
    type Error = WasmError;

    fn try_from(s: Nut04Settings) -> Result<Self, Self::Error> {
        Ok(Self {
            methods: s
                .methods
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            disabled: s.disabled,
        })
    }
}

/// WASM-compatible MeltMethodSettings (NUT-05)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeltMethodSettings {
    pub method: PaymentMethod,
    pub unit: CurrencyUnit,
    pub min_amount: Option<Amount>,
    pub max_amount: Option<Amount>,
    /// For bolt11, whether mint supports amountless invoices
    pub amountless: Option<bool>,
}

impl From<cdk::nuts::nut05::MeltMethodSettings> for MeltMethodSettings {
    fn from(s: cdk::nuts::nut05::MeltMethodSettings) -> Self {
        let amountless = match s.options {
            Some(cdk::nuts::nut05::MeltMethodOptions::Bolt11 { amountless }) => Some(amountless),
            _ => None,
        };
        Self {
            method: s.method.into(),
            unit: s.unit.into(),
            min_amount: s.min_amount.map(Into::into),
            max_amount: s.max_amount.map(Into::into),
            amountless,
        }
    }
}

impl TryFrom<MeltMethodSettings> for cdk::nuts::nut05::MeltMethodSettings {
    type Error = WasmError;

    fn try_from(s: MeltMethodSettings) -> Result<Self, Self::Error> {
        let options = match s.method {
            PaymentMethod::Bolt11 => s
                .amountless
                .map(|amountless| cdk::nuts::nut05::MeltMethodOptions::Bolt11 { amountless }),
            _ => None,
        };
        Ok(Self {
            method: s.method.into(),
            unit: s.unit.into(),
            min_amount: s.min_amount.map(Into::into),
            max_amount: s.max_amount.map(Into::into),
            options,
        })
    }
}

/// WASM-compatible Nut05 Settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nut05Settings {
    pub methods: Vec<MeltMethodSettings>,
    pub disabled: bool,
}

impl From<cdk::nuts::nut05::Settings> for Nut05Settings {
    fn from(s: cdk::nuts::nut05::Settings) -> Self {
        Self {
            methods: s.methods.into_iter().map(Into::into).collect(),
            disabled: s.disabled,
        }
    }
}

impl TryFrom<Nut05Settings> for cdk::nuts::nut05::Settings {
    type Error = WasmError;

    fn try_from(s: Nut05Settings) -> Result<Self, Self::Error> {
        Ok(Self {
            methods: s
                .methods
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            disabled: s.disabled,
        })
    }
}

/// WASM-compatible ProtectedEndpoint (for auth nuts)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectedEndpoint {
    /// HTTP method (GET, POST, etc.)
    pub method: String,
    /// Endpoint path
    pub path: String,
}

/// WASM-compatible ClearAuthSettings (NUT-21)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearAuthSettings {
    /// OpenID Connect discovery URL
    pub openid_discovery: String,
    /// OAuth 2.0 client ID
    pub client_id: String,
    /// Protected endpoints requiring clear authentication
    pub protected_endpoints: Vec<ProtectedEndpoint>,
}

/// WASM-compatible BlindAuthSettings (NUT-22)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlindAuthSettings {
    /// Maximum number of blind auth tokens that can be minted per request
    pub bat_max_mint: u64,
    /// Protected endpoints requiring blind authentication
    pub protected_endpoints: Vec<ProtectedEndpoint>,
}

impl From<cdk::nuts::ClearAuthSettings> for ClearAuthSettings {
    fn from(settings: cdk::nuts::ClearAuthSettings) -> Self {
        Self {
            openid_discovery: settings.openid_discovery,
            client_id: settings.client_id,
            protected_endpoints: settings
                .protected_endpoints
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

impl TryFrom<ClearAuthSettings> for cdk::nuts::ClearAuthSettings {
    type Error = WasmError;

    fn try_from(settings: ClearAuthSettings) -> Result<Self, Self::Error> {
        Ok(Self {
            openid_discovery: settings.openid_discovery,
            client_id: settings.client_id,
            protected_endpoints: settings
                .protected_endpoints
                .into_iter()
                .map(|e| e.try_into())
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl From<cdk::nuts::BlindAuthSettings> for BlindAuthSettings {
    fn from(settings: cdk::nuts::BlindAuthSettings) -> Self {
        Self {
            bat_max_mint: settings.bat_max_mint,
            protected_endpoints: settings
                .protected_endpoints
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

impl TryFrom<BlindAuthSettings> for cdk::nuts::BlindAuthSettings {
    type Error = WasmError;

    fn try_from(settings: BlindAuthSettings) -> Result<Self, Self::Error> {
        Ok(Self {
            bat_max_mint: settings.bat_max_mint,
            protected_endpoints: settings
                .protected_endpoints
                .into_iter()
                .map(|e| e.try_into())
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl From<cdk::nuts::ProtectedEndpoint> for ProtectedEndpoint {
    fn from(endpoint: cdk::nuts::ProtectedEndpoint) -> Self {
        Self {
            method: match endpoint.method {
                cdk::nuts::Method::Get => "GET".to_string(),
                cdk::nuts::Method::Post => "POST".to_string(),
            },
            path: endpoint.path.to_string(),
        }
    }
}

impl TryFrom<ProtectedEndpoint> for cdk::nuts::ProtectedEndpoint {
    type Error = WasmError;

    fn try_from(endpoint: ProtectedEndpoint) -> Result<Self, Self::Error> {
        let method = match endpoint.method.as_str() {
            "GET" => cdk::nuts::Method::Get,
            "POST" => cdk::nuts::Method::Post,
            _ => {
                return Err(WasmError::internal(format!(
                    "Invalid HTTP method: {}. Only GET and POST are supported",
                    endpoint.method
                )))
            }
        };

        // Convert path string to RoutePath by matching against known paths
        let route_path = match endpoint.path.as_str() {
            "/v1/mint/quote/bolt11" => cdk::nuts::RoutePath::MintQuote(
                NutPaymentMethod::Known(KnownMethod::Bolt11).to_string(),
            ),
            "/v1/mint/bolt11" => {
                cdk::nuts::RoutePath::Mint(NutPaymentMethod::Known(KnownMethod::Bolt11).to_string())
            }
            "/v1/melt/quote/bolt11" => cdk::nuts::RoutePath::MeltQuote(
                NutPaymentMethod::Known(KnownMethod::Bolt11).to_string(),
            ),
            "/v1/melt/bolt11" => {
                cdk::nuts::RoutePath::Melt(NutPaymentMethod::Known(KnownMethod::Bolt11).to_string())
            }
            "/v1/swap" => cdk::nuts::RoutePath::Swap,
            "/v1/ws" => cdk::nuts::RoutePath::Ws,
            "/v1/checkstate" => cdk::nuts::RoutePath::Checkstate,
            "/v1/restore" => cdk::nuts::RoutePath::Restore,
            "/v1/auth/blind/mint" => cdk::nuts::RoutePath::MintBlindAuth,
            "/v1/mint/quote/bolt12" => cdk::nuts::RoutePath::MintQuote(
                NutPaymentMethod::Known(KnownMethod::Bolt12).to_string(),
            ),
            "/v1/mint/bolt12" => {
                cdk::nuts::RoutePath::Mint(NutPaymentMethod::Known(KnownMethod::Bolt12).to_string())
            }
            "/v1/melt/quote/bolt12" => cdk::nuts::RoutePath::MeltQuote(
                NutPaymentMethod::Known(KnownMethod::Bolt12).to_string(),
            ),
            "/v1/melt/bolt12" => {
                cdk::nuts::RoutePath::Melt(NutPaymentMethod::Known(KnownMethod::Bolt12).to_string())
            }
            _ => {
                return Err(WasmError::internal(format!(
                    "Unknown route path: {}",
                    endpoint.path
                )))
            }
        };

        Ok(cdk::nuts::ProtectedEndpoint::new(method, route_path))
    }
}

/// WASM-compatible Nuts settings (extended to include NUT-04 and NUT-05 settings)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nuts {
    /// NUT04 Settings
    pub nut04: Nut04Settings,
    /// NUT05 Settings
    pub nut05: Nut05Settings,
    /// NUT07 Settings - Token state check
    pub nut07_supported: bool,
    /// NUT08 Settings - Lightning fee return
    pub nut08_supported: bool,
    /// NUT09 Settings - Restore signature
    pub nut09_supported: bool,
    /// NUT10 Settings - Spending conditions
    pub nut10_supported: bool,
    /// NUT11 Settings - Pay to Public Key Hash
    pub nut11_supported: bool,
    /// NUT12 Settings - DLEQ proofs
    pub nut12_supported: bool,
    /// NUT14 Settings - Hashed Time Locked Contracts
    pub nut14_supported: bool,
    /// NUT20 Settings - Web sockets
    pub nut20_supported: bool,
    /// NUT21 Settings - Clear authentication
    pub nut21: Option<ClearAuthSettings>,
    /// NUT22 Settings - Blind authentication
    pub nut22: Option<BlindAuthSettings>,
    /// Supported currency units for minting
    pub mint_units: Vec<CurrencyUnit>,
    /// Supported currency units for melting
    pub melt_units: Vec<CurrencyUnit>,
}

impl From<cdk::nuts::Nuts> for Nuts {
    fn from(nuts: cdk::nuts::Nuts) -> Self {
        let mint_units = nuts
            .supported_mint_units()
            .into_iter()
            .map(|u| u.clone().into())
            .collect();
        let melt_units = nuts
            .supported_melt_units()
            .into_iter()
            .map(|u| u.clone().into())
            .collect();

        Self {
            nut04: nuts.nut04.clone().into(),
            nut05: nuts.nut05.clone().into(),
            nut07_supported: nuts.nut07.supported,
            nut08_supported: nuts.nut08.supported,
            nut09_supported: nuts.nut09.supported,
            nut10_supported: nuts.nut10.supported,
            nut11_supported: nuts.nut11.supported,
            nut12_supported: nuts.nut12.supported,
            nut14_supported: nuts.nut14.supported,
            nut20_supported: nuts.nut20.supported,
            nut21: nuts.nut21.map(Into::into),
            nut22: nuts.nut22.map(Into::into),
            mint_units,
            melt_units,
        }
    }
}

impl TryFrom<Nuts> for cdk::nuts::Nuts {
    type Error = WasmError;

    fn try_from(n: Nuts) -> Result<Self, Self::Error> {
        Ok(Self {
            nut04: n.nut04.try_into()?,
            nut05: n.nut05.try_into()?,
            nut07: cdk::nuts::nut06::SupportedSettings {
                supported: n.nut07_supported,
            },
            nut08: cdk::nuts::nut06::SupportedSettings {
                supported: n.nut08_supported,
            },
            nut09: cdk::nuts::nut06::SupportedSettings {
                supported: n.nut09_supported,
            },
            nut10: cdk::nuts::nut06::SupportedSettings {
                supported: n.nut10_supported,
            },
            nut11: cdk::nuts::nut06::SupportedSettings {
                supported: n.nut11_supported,
            },
            nut12: cdk::nuts::nut06::SupportedSettings {
                supported: n.nut12_supported,
            },
            nut14: cdk::nuts::nut06::SupportedSettings {
                supported: n.nut14_supported,
            },
            nut15: Default::default(),
            nut17: Default::default(),
            nut19: Default::default(),
            nut20: cdk::nuts::nut06::SupportedSettings {
                supported: n.nut20_supported,
            },
            nut21: n.nut21.map(|s| s.try_into()).transpose()?,
            nut22: n.nut22.map(|s| s.try_into()).transpose()?,
        })
    }
}

impl Nuts {
    /// Convert Nuts to JSON string
    pub fn to_json(&self) -> Result<String, WasmError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode Nuts from JSON string
pub fn decode_nuts(json: String) -> Result<Nuts, WasmError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode Nuts to JSON string
pub fn encode_nuts(nuts: Nuts) -> Result<String, WasmError> {
    Ok(serde_json::to_string(&nuts)?)
}

/// WASM-compatible MintInfo
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintInfo {
    /// name of the mint and should be recognizable
    pub name: Option<String>,
    /// hex pubkey of the mint
    pub pubkey: Option<String>,
    /// implementation name and the version running
    pub version: Option<MintVersion>,
    /// short description of the mint
    pub description: Option<String>,
    /// long description
    pub description_long: Option<String>,
    /// Contact info
    pub contact: Option<Vec<ContactInfo>>,
    /// shows which NUTs the mint supports
    pub nuts: Nuts,
    /// Mint's icon URL
    pub icon_url: Option<String>,
    /// Mint's endpoint URLs
    pub urls: Option<Vec<String>>,
    /// message of the day that the wallet must display to the user
    pub motd: Option<String>,
    /// server unix timestamp
    pub time: Option<u64>,
    /// terms of url service of the mint
    pub tos_url: Option<String>,
}

impl From<cdk::nuts::MintInfo> for MintInfo {
    fn from(info: cdk::nuts::MintInfo) -> Self {
        Self {
            name: info.name,
            pubkey: info.pubkey.map(|p| p.to_string()),
            version: info.version.map(Into::into),
            description: info.description,
            description_long: info.description_long,
            contact: info
                .contact
                .map(|contacts| contacts.into_iter().map(Into::into).collect()),
            nuts: info.nuts.into(),
            icon_url: info.icon_url,
            urls: info.urls,
            motd: info.motd,
            time: info.time,
            tos_url: info.tos_url,
        }
    }
}

impl From<MintInfo> for cdk::nuts::MintInfo {
    fn from(info: MintInfo) -> Self {
        // Convert WASM Nuts back to cdk::nuts::Nuts (best-effort)
        let nuts_cdk: cdk::nuts::Nuts = info.nuts.clone().try_into().unwrap_or_default();
        Self {
            name: info.name,
            pubkey: info.pubkey.and_then(|p| p.parse().ok()),
            version: info.version.map(Into::into),
            description: info.description,
            description_long: info.description_long,
            contact: info
                .contact
                .map(|contacts| contacts.into_iter().map(Into::into).collect()),
            nuts: nuts_cdk,
            icon_url: info.icon_url,
            urls: info.urls,
            motd: info.motd,
            time: info.time,
            tos_url: info.tos_url,
        }
    }
}

impl MintInfo {
    /// Convert MintInfo to JSON string
    pub fn to_json(&self) -> Result<String, WasmError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode MintInfo from JSON string
pub fn decode_mint_info(json: String) -> Result<MintInfo, WasmError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode MintInfo to JSON string
pub fn encode_mint_info(info: MintInfo) -> Result<String, WasmError> {
    Ok(serde_json::to_string(&info)?)
}

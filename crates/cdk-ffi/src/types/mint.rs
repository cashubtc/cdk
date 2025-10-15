//! Mint-related FFI types

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::amount::{Amount, CurrencyUnit};
use super::quote::PaymentMethod;
use crate::error::FfiError;

/// FFI-compatible Mint URL
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Record)]
#[serde(transparent)]
pub struct MintUrl {
    pub url: String,
}

impl MintUrl {
    pub fn new(url: String) -> Result<Self, FfiError> {
        // Validate URL format
        url::Url::parse(&url).map_err(|e| FfiError::InvalidUrl { msg: e.to_string() })?;

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
    type Error = FfiError;

    fn try_from(mint_url: MintUrl) -> Result<Self, Self::Error> {
        cdk::mint_url::MintUrl::from_str(&mint_url.url)
            .map_err(|e| FfiError::InvalidUrl { msg: e.to_string() })
    }
}

/// FFI-compatible MintVersion
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode MintVersion from JSON string
#[uniffi::export]
pub fn decode_mint_version(json: String) -> Result<MintVersion, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode MintVersion to JSON string
#[uniffi::export]
pub fn encode_mint_version(version: MintVersion) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&version)?)
}

/// FFI-compatible ContactInfo
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode ContactInfo from JSON string
#[uniffi::export]
pub fn decode_contact_info(json: String) -> Result<ContactInfo, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode ContactInfo to JSON string
#[uniffi::export]
pub fn encode_contact_info(info: ContactInfo) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&info)?)
}

/// FFI-compatible SupportedSettings
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
// NUT-04/05 FFI Types
// -----------------------------

/// FFI-compatible MintMethodSettings (NUT-04)
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
    type Error = FfiError;

    fn try_from(s: MintMethodSettings) -> Result<Self, Self::Error> {
        let options = match (s.method.clone(), s.description) {
            (PaymentMethod::Bolt11, Some(description)) => {
                Some(cdk::nuts::nut04::MintMethodOptions::Bolt11 { description })
            }
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

/// FFI-compatible Nut04 Settings
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
    type Error = FfiError;

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

/// FFI-compatible MeltMethodSettings (NUT-05)
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
    type Error = FfiError;

    fn try_from(s: MeltMethodSettings) -> Result<Self, Self::Error> {
        let options = match (s.method.clone(), s.amountless) {
            (PaymentMethod::Bolt11, Some(amountless)) => {
                Some(cdk::nuts::nut05::MeltMethodOptions::Bolt11 { amountless })
            }
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

/// FFI-compatible Nut05 Settings
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
    type Error = FfiError;

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

/// FFI-compatible ProtectedEndpoint (for auth nuts)
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct ProtectedEndpoint {
    /// HTTP method (GET, POST, etc.)
    pub method: String,
    /// Endpoint path
    pub path: String,
}

/// FFI-compatible ClearAuthSettings (NUT-21)
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct ClearAuthSettings {
    /// OpenID Connect discovery URL
    pub openid_discovery: String,
    /// OAuth 2.0 client ID
    pub client_id: String,
    /// Protected endpoints requiring clear authentication
    pub protected_endpoints: Vec<ProtectedEndpoint>,
}

/// FFI-compatible BlindAuthSettings (NUT-22)
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
    type Error = FfiError;

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
    type Error = FfiError;

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
    type Error = FfiError;

    fn try_from(endpoint: ProtectedEndpoint) -> Result<Self, Self::Error> {
        let method = match endpoint.method.as_str() {
            "GET" => cdk::nuts::Method::Get,
            "POST" => cdk::nuts::Method::Post,
            _ => {
                return Err(FfiError::Generic {
                    msg: format!(
                        "Invalid HTTP method: {}. Only GET and POST are supported",
                        endpoint.method
                    ),
                })
            }
        };

        // Convert path string to RoutePath by matching against known paths
        let route_path = match endpoint.path.as_str() {
            "/v1/mint/quote/bolt11" => cdk::nuts::RoutePath::MintQuoteBolt11,
            "/v1/mint/quote/mining_share" => cdk::nuts::RoutePath::MintQuoteMiningShare,
            "/v1/mint/bolt11" => cdk::nuts::RoutePath::MintBolt11,
            "/v1/mint/mining_share" => cdk::nuts::RoutePath::MintMiningShare,
            "/v1/melt/quote/bolt11" => cdk::nuts::RoutePath::MeltQuoteBolt11,
            "/v1/melt/bolt11" => cdk::nuts::RoutePath::MeltBolt11,
            "/v1/swap" => cdk::nuts::RoutePath::Swap,
            "/v1/ws" => cdk::nuts::RoutePath::Ws,
            "/v1/checkstate" => cdk::nuts::RoutePath::Checkstate,
            "/v1/restore" => cdk::nuts::RoutePath::Restore,
            "/v1/auth/blind/mint" => cdk::nuts::RoutePath::MintBlindAuth,
            "/v1/mint/quote/bolt12" => cdk::nuts::RoutePath::MintQuoteBolt12,
            "/v1/mint/bolt12" => cdk::nuts::RoutePath::MintBolt12,
            "/v1/melt/quote/bolt12" => cdk::nuts::RoutePath::MeltQuoteBolt12,
            "/v1/melt/bolt12" => cdk::nuts::RoutePath::MeltBolt12,
            _ => {
                return Err(FfiError::Generic {
                    msg: format!("Unknown route path: {}", endpoint.path),
                })
            }
        };

        Ok(cdk::nuts::ProtectedEndpoint::new(method, route_path))
    }
}

/// FFI-compatible Nuts settings (extended to include NUT-04 and NUT-05 settings)
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
    type Error = FfiError;

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
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode Nuts from JSON string
#[uniffi::export]
pub fn decode_nuts(json: String) -> Result<Nuts, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode Nuts to JSON string
#[uniffi::export]
pub fn encode_nuts(nuts: Nuts) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&nuts)?)
}

/// FFI-compatible MintInfo
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
        // Convert FFI Nuts back to cdk::nuts::Nuts (best-effort)
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
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode MintInfo from JSON string
#[uniffi::export]
pub fn decode_mint_info(json: String) -> Result<MintInfo, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode MintInfo to JSON string
#[uniffi::export]
pub fn encode_mint_info(info: MintInfo) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&info)?)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Helper function to create a sample cdk::nuts::Nuts for testing
    fn create_sample_cdk_nuts() -> cdk::nuts::Nuts {
        cdk::nuts::Nuts {
            nut04: cdk::nuts::nut04::Settings {
                methods: vec![cdk::nuts::nut04::MintMethodSettings {
                    method: cdk::nuts::PaymentMethod::Bolt11,
                    unit: cdk::nuts::CurrencyUnit::Sat,
                    min_amount: Some(cdk::Amount::from(1)),
                    max_amount: Some(cdk::Amount::from(100000)),
                    options: Some(cdk::nuts::nut04::MintMethodOptions::Bolt11 {
                        description: true,
                    }),
                }],
                disabled: false,
            },
            nut05: cdk::nuts::nut05::Settings {
                methods: vec![cdk::nuts::nut05::MeltMethodSettings {
                    method: cdk::nuts::PaymentMethod::Bolt11,
                    unit: cdk::nuts::CurrencyUnit::Sat,
                    min_amount: Some(cdk::Amount::from(1)),
                    max_amount: Some(cdk::Amount::from(100000)),
                    options: Some(cdk::nuts::nut05::MeltMethodOptions::Bolt11 { amountless: true }),
                }],
                disabled: false,
            },
            nut07: cdk::nuts::nut06::SupportedSettings { supported: true },
            nut08: cdk::nuts::nut06::SupportedSettings { supported: true },
            nut09: cdk::nuts::nut06::SupportedSettings { supported: false },
            nut10: cdk::nuts::nut06::SupportedSettings { supported: true },
            nut11: cdk::nuts::nut06::SupportedSettings { supported: true },
            nut12: cdk::nuts::nut06::SupportedSettings { supported: true },
            nut14: cdk::nuts::nut06::SupportedSettings { supported: false },
            nut15: Default::default(),
            nut17: Default::default(),
            nut19: Default::default(),
            nut20: cdk::nuts::nut06::SupportedSettings { supported: true },
            nut21: Some(cdk::nuts::ClearAuthSettings {
                openid_discovery: "https://example.com/.well-known/openid-configuration"
                    .to_string(),
                client_id: "test-client".to_string(),
                protected_endpoints: vec![cdk::nuts::ProtectedEndpoint::new(
                    cdk::nuts::Method::Post,
                    cdk::nuts::RoutePath::Swap,
                )],
            }),
            nut22: Some(cdk::nuts::BlindAuthSettings {
                bat_max_mint: 100,
                protected_endpoints: vec![cdk::nuts::ProtectedEndpoint::new(
                    cdk::nuts::Method::Post,
                    cdk::nuts::RoutePath::MintBolt11,
                )],
            }),
        }
    }

    #[test]
    fn test_nuts_from_cdk_to_ffi() {
        let cdk_nuts = create_sample_cdk_nuts();
        let ffi_nuts: Nuts = cdk_nuts.clone().into();

        // Verify NUT04 settings
        assert_eq!(ffi_nuts.nut04.disabled, false);
        assert_eq!(ffi_nuts.nut04.methods.len(), 1);
        assert_eq!(ffi_nuts.nut04.methods[0].description, Some(true));

        // Verify NUT05 settings
        assert_eq!(ffi_nuts.nut05.disabled, false);
        assert_eq!(ffi_nuts.nut05.methods.len(), 1);
        assert_eq!(ffi_nuts.nut05.methods[0].amountless, Some(true));

        // Verify supported flags
        assert!(ffi_nuts.nut07_supported);
        assert!(ffi_nuts.nut08_supported);
        assert!(!ffi_nuts.nut09_supported);
        assert!(ffi_nuts.nut10_supported);
        assert!(ffi_nuts.nut11_supported);
        assert!(ffi_nuts.nut12_supported);
        assert!(!ffi_nuts.nut14_supported);
        assert!(ffi_nuts.nut20_supported);

        // Verify auth settings
        assert!(ffi_nuts.nut21.is_some());
        let nut21 = ffi_nuts.nut21.as_ref().unwrap();
        assert_eq!(
            nut21.openid_discovery,
            "https://example.com/.well-known/openid-configuration"
        );
        assert_eq!(nut21.client_id, "test-client");
        assert_eq!(nut21.protected_endpoints.len(), 1);

        assert!(ffi_nuts.nut22.is_some());
        let nut22 = ffi_nuts.nut22.as_ref().unwrap();
        assert_eq!(nut22.bat_max_mint, 100);
        assert_eq!(nut22.protected_endpoints.len(), 1);

        // Verify units
        assert!(!ffi_nuts.mint_units.is_empty());
        assert!(!ffi_nuts.melt_units.is_empty());
    }

    #[test]
    fn test_nuts_round_trip_conversion() {
        let original_cdk_nuts = create_sample_cdk_nuts();

        // Convert cdk -> ffi -> cdk
        let ffi_nuts: Nuts = original_cdk_nuts.clone().into();
        let converted_back: cdk::nuts::Nuts = ffi_nuts.try_into().unwrap();

        // Verify all supported flags match
        assert_eq!(
            original_cdk_nuts.nut07.supported,
            converted_back.nut07.supported
        );
        assert_eq!(
            original_cdk_nuts.nut08.supported,
            converted_back.nut08.supported
        );
        assert_eq!(
            original_cdk_nuts.nut09.supported,
            converted_back.nut09.supported
        );
        assert_eq!(
            original_cdk_nuts.nut10.supported,
            converted_back.nut10.supported
        );
        assert_eq!(
            original_cdk_nuts.nut11.supported,
            converted_back.nut11.supported
        );
        assert_eq!(
            original_cdk_nuts.nut12.supported,
            converted_back.nut12.supported
        );
        assert_eq!(
            original_cdk_nuts.nut14.supported,
            converted_back.nut14.supported
        );
        assert_eq!(
            original_cdk_nuts.nut20.supported,
            converted_back.nut20.supported
        );

        // Verify NUT04 settings
        assert_eq!(
            original_cdk_nuts.nut04.disabled,
            converted_back.nut04.disabled
        );
        assert_eq!(
            original_cdk_nuts.nut04.methods.len(),
            converted_back.nut04.methods.len()
        );

        // Verify NUT05 settings
        assert_eq!(
            original_cdk_nuts.nut05.disabled,
            converted_back.nut05.disabled
        );
        assert_eq!(
            original_cdk_nuts.nut05.methods.len(),
            converted_back.nut05.methods.len()
        );

        // Verify auth settings presence
        assert_eq!(
            original_cdk_nuts.nut21.is_some(),
            converted_back.nut21.is_some()
        );
        assert_eq!(
            original_cdk_nuts.nut22.is_some(),
            converted_back.nut22.is_some()
        );
    }

    #[test]
    fn test_nuts_without_auth() {
        let cdk_nuts = cdk::nuts::Nuts {
            nut04: Default::default(),
            nut05: Default::default(),
            nut07: cdk::nuts::nut06::SupportedSettings { supported: true },
            nut08: cdk::nuts::nut06::SupportedSettings { supported: false },
            nut09: cdk::nuts::nut06::SupportedSettings { supported: false },
            nut10: cdk::nuts::nut06::SupportedSettings { supported: false },
            nut11: cdk::nuts::nut06::SupportedSettings { supported: false },
            nut12: cdk::nuts::nut06::SupportedSettings { supported: false },
            nut14: cdk::nuts::nut06::SupportedSettings { supported: false },
            nut15: Default::default(),
            nut17: Default::default(),
            nut19: Default::default(),
            nut20: cdk::nuts::nut06::SupportedSettings { supported: false },
            nut21: None,
            nut22: None,
        };

        let ffi_nuts: Nuts = cdk_nuts.into();

        assert!(ffi_nuts.nut21.is_none());
        assert!(ffi_nuts.nut22.is_none());
        assert!(ffi_nuts.nut07_supported);
        assert!(!ffi_nuts.nut08_supported);
    }

    #[test]
    fn test_ffi_nuts_to_cdk_with_defaults() {
        let ffi_nuts = Nuts {
            nut04: Nut04Settings {
                methods: vec![],
                disabled: true,
            },
            nut05: Nut05Settings {
                methods: vec![],
                disabled: true,
            },
            nut07_supported: false,
            nut08_supported: false,
            nut09_supported: false,
            nut10_supported: false,
            nut11_supported: false,
            nut12_supported: false,
            nut14_supported: false,
            nut20_supported: false,
            nut21: None,
            nut22: None,
            mint_units: vec![],
            melt_units: vec![],
        };

        let cdk_nuts: Result<cdk::nuts::Nuts, _> = ffi_nuts.try_into();
        assert!(cdk_nuts.is_ok());

        let cdk_nuts = cdk_nuts.unwrap();
        assert!(!cdk_nuts.nut07.supported);
        assert!(!cdk_nuts.nut08.supported);
        assert!(cdk_nuts.nut21.is_none());
        assert!(cdk_nuts.nut22.is_none());

        // Verify default values for nuts not included in FFI
        assert_eq!(cdk_nuts.nut17.supported.len(), 0);
    }

    #[test]
    fn test_nuts_serialization() {
        let cdk_nuts = create_sample_cdk_nuts();
        let ffi_nuts: Nuts = cdk_nuts.into();

        // Test JSON serialization
        let json = ffi_nuts.to_json();
        assert!(json.is_ok());

        let json_str = json.unwrap();
        assert!(json_str.contains("nut04"));
        assert!(json_str.contains("nut05"));

        // Test deserialization
        let decoded: Result<Nuts, _> = serde_json::from_str(&json_str);
        assert!(decoded.is_ok());

        let decoded_nuts = decoded.unwrap();
        assert_eq!(decoded_nuts.nut07_supported, ffi_nuts.nut07_supported);
        assert_eq!(decoded_nuts.nut08_supported, ffi_nuts.nut08_supported);
    }

    #[test]
    fn test_nuts_multiple_units() {
        let mut cdk_nuts = create_sample_cdk_nuts();

        // Add multiple payment methods to test unit collection
        cdk_nuts
            .nut04
            .methods
            .push(cdk::nuts::nut04::MintMethodSettings {
                method: cdk::nuts::PaymentMethod::Bolt11,
                unit: cdk::nuts::CurrencyUnit::Msat,
                min_amount: Some(cdk::Amount::from(1)),
                max_amount: Some(cdk::Amount::from(100000)),
                options: None,
            });

        cdk_nuts
            .nut05
            .methods
            .push(cdk::nuts::nut05::MeltMethodSettings {
                method: cdk::nuts::PaymentMethod::Bolt11,
                unit: cdk::nuts::CurrencyUnit::Usd,
                min_amount: None,
                max_amount: None,
                options: None,
            });

        let ffi_nuts: Nuts = cdk_nuts.into();

        // Should have collected multiple units
        assert!(ffi_nuts.mint_units.len() >= 1);
        assert!(ffi_nuts.melt_units.len() >= 1);
    }

    #[test]
    fn test_protected_endpoint_conversion() {
        let cdk_endpoint =
            cdk::nuts::ProtectedEndpoint::new(cdk::nuts::Method::Post, cdk::nuts::RoutePath::Swap);

        let ffi_endpoint: ProtectedEndpoint = cdk_endpoint.into();

        assert_eq!(ffi_endpoint.method, "POST");
        assert_eq!(ffi_endpoint.path, "/v1/swap");

        // Test round-trip
        let converted_back: Result<cdk::nuts::ProtectedEndpoint, _> = ffi_endpoint.try_into();
        assert!(converted_back.is_ok());
    }

    #[test]
    fn test_invalid_protected_endpoint_method() {
        let invalid_endpoint = ProtectedEndpoint {
            method: "INVALID".to_string(),
            path: "/v1/swap".to_string(),
        };

        let result: Result<cdk::nuts::ProtectedEndpoint, _> = invalid_endpoint.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_protected_endpoint_path() {
        let invalid_endpoint = ProtectedEndpoint {
            method: "POST".to_string(),
            path: "/invalid/path".to_string(),
        };

        let result: Result<cdk::nuts::ProtectedEndpoint, _> = invalid_endpoint.try_into();
        assert!(result.is_err());
    }
}

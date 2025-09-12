//! NUT-26: OHttp

use serde::{Deserialize, Serialize};

use crate::MintInfo;

/// NUT-26 OHTTP Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct OhttpSettings {
    /// Ohttp is enabled
    pub supported: bool,
    /// OHTTP gateway URL (actual destination, typically same as mint URL)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_url: Option<String>,
}

impl OhttpSettings {
    /// Create new [`OhttpSettings`]
    pub fn new(supported: bool, gateway_url: Option<String>) -> Self {
        Self {
            supported,
            gateway_url,
        }
    }

    /// Validate OHTTP settings URLs
    pub fn validate(&self) -> Result<(), String> {
        use url::Url;

        if let Some(url) = self.gateway_url.as_ref() {
            Url::parse(url).map_err(|_| format!("Invalid gateway URL: {}", url))?;
        }

        Ok(())
    }
}

impl MintInfo {
    /// Check if mint supports OHTTP (NUT-26)
    pub fn supports_ohttp(&self) -> bool {
        self.nuts
            .nut26
            .as_ref()
            .map(|s| s.supported)
            .unwrap_or_default()
    }

    /// Get OHTTP configuration if supported
    pub fn ohttp_config(&self) -> Option<&OhttpSettings> {
        self.nuts.nut26.as_ref()
    }
}

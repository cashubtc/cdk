//! NUT-15: Multipart payments
//!
//! <https://github.com/cashubtc/nuts/blob/main/15.md>

use serde::{Deserialize, Deserializer, Serialize};

use super::{CurrencyUnit, PaymentMethod};
use crate::Amount;

/// Multi-part payment
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename = "lowercase")]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct Mpp {
    /// Amount
    pub amount: Amount,
}

/// Mpp Method Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MppMethodSettings {
    /// Payment Method e.g. bolt11
    pub method: PaymentMethod,
    /// Currency Unit e.g. sat
    pub unit: CurrencyUnit,
}

/// Mpp Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema), schema(as = nut15::Settings))]
pub struct Settings {
    /// Method settings
    pub methods: Vec<MppMethodSettings>,
}

impl Settings {
    /// Check if methods is empty
    pub fn is_empty(&self) -> bool {
        self.methods.is_empty()
    }
}

// Custom deserialization to handle both array and object formats
impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum SettingsFormat {
            Array(Vec<MppMethodSettings>),
            Object { methods: Vec<MppMethodSettings> },
        }

        let format = SettingsFormat::deserialize(deserializer)?;
        match format {
            SettingsFormat::Array(methods) => Ok(Settings { methods }),
            SettingsFormat::Object { methods } => Ok(Settings { methods }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PaymentMethod;

    #[test]
    fn test_nut15_settings_deserialization() {
        // Test array format
        let array_json = r#"[{"method":"bolt11","unit":"sat"}]"#;
        let settings: Settings = serde_json::from_str(array_json).unwrap();
        assert_eq!(settings.methods.len(), 1);
        assert_eq!(settings.methods[0].method, PaymentMethod::Bolt11);
        assert_eq!(settings.methods[0].unit, CurrencyUnit::Sat);

        // Test object format
        let object_json = r#"{"methods":[{"method":"bolt11","unit":"sat"}]}"#;
        let settings: Settings = serde_json::from_str(object_json).unwrap();
        assert_eq!(settings.methods.len(), 1);
        assert_eq!(settings.methods[0].method, PaymentMethod::Bolt11);
        assert_eq!(settings.methods[0].unit, CurrencyUnit::Sat);
    }

    #[test]
    fn test_nut15_settings_serialization() {
        let settings = Settings {
            methods: vec![MppMethodSettings {
                method: PaymentMethod::Bolt11,
                unit: CurrencyUnit::Sat,
            }],
        };

        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(json, r#"{"methods":[{"method":"bolt11","unit":"sat"}]}"#);
    }

    #[test]
    fn test_nut15_settings_empty() {
        let settings = Settings { methods: vec![] };
        assert!(settings.is_empty());

        let settings_with_data = Settings {
            methods: vec![MppMethodSettings {
                method: PaymentMethod::Bolt11,
                unit: CurrencyUnit::Sat,
            }],
        };
        assert!(!settings_with_data.is_empty());
    }
}

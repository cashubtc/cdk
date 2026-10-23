//! Display-only currency unit metadata (draft NUT, cashubtc/nuts#434).
//!
//! The experimental wire setting is `nuts["XX"]`. Metadata never changes
//! transaction amounts, currency identity, or keyset verification.

use std::collections::{HashMap, HashSet};

use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use serde_with::{serde_as, DefaultOnError, Map, VecSkipError};
use thiserror::Error;

mod iso4217;

/// A mint's display metadata for one exact unit identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UnitMetadata {
    /// Unit string, exactly as published in the mint's keysets.
    pub unit: String,
    /// Number of decimal places, between zero and eighteen.
    pub precision: u8,
    /// Optional currency name. This does not establish currency identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional symbol. Display the unit identifier alongside this symbol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

/// Invalid publisher metadata.
#[derive(Debug, Error)]
pub enum Error {
    /// Metadata names a unit the mint does not publish.
    #[error("No keyset for metadata unit {unit}")]
    UnknownUnit {
        /// Exact unit identifier.
        unit: String,
    },
    /// Stored precision history is invalid.
    #[error("Invalid stored precision for {unit}")]
    InvalidStoredPrecision {
        /// Exact unit identifier.
        unit: String,
    },
    /// The application attempted to acknowledge a value that is no longer pending.
    #[error("Precision {proposed} is no longer pending; refresh display metadata")]
    PrecisionNotPending {
        /// Value the application attempted to acknowledge.
        proposed: u8,
    },
    /// The experimental extension is disabled.
    #[error("Currency unit metadata requires the experimental-unit-metadata feature")]
    Disabled,
    /// Precision is outside the protocol range.
    #[error("Invalid precision {precision} for unit {unit}; maximum is 18")]
    InvalidPrecision {
        /// Exact unit identifier.
        unit: String,
        /// Invalid precision.
        precision: u8,
    },
    /// More than one entry names the same unit.
    #[error("Duplicate unit metadata for {unit}")]
    DuplicateUnit {
        /// Repeated unit identifier.
        unit: String,
    },
    /// Published precision conflicts with a locally defined precision.
    #[error("Unit {unit} requires precision {expected}, received {actual}")]
    DefinedPrecision {
        /// Exact unit identifier.
        unit: String,
        /// Standardized precision.
        expected: u8,
        /// Published precision.
        actual: u8,
    },
    /// A previously published precision cannot be changed.
    #[error("Precision for {unit} cannot change from {previous} to {proposed}")]
    PrecisionChanged {
        /// Exact unit identifier.
        unit: String,
        /// Previously published precision.
        previous: u8,
        /// Proposed precision.
        proposed: u8,
    },
}

/// Validate locally authored metadata before publishing it.
pub fn validate(entries: &[UnitMetadata]) -> Result<(), Error> {
    if !cfg!(feature = "experimental-unit-metadata") {
        return Err(Error::Disabled);
    }
    let mut units = HashSet::new();
    for entry in entries {
        if entry.precision > 18 {
            return Err(Error::InvalidPrecision {
                unit: entry.unit.clone(),
                precision: entry.precision,
            });
        }
        if !units.insert(&entry.unit) {
            return Err(Error::DuplicateUnit {
                unit: entry.unit.clone(),
            });
        }
        if let Some(expected) = defined_precision(&entry.unit) {
            if entry.precision != expected {
                return Err(Error::DefinedPrecision {
                    unit: entry.unit.clone(),
                    expected,
                    actual: entry.precision,
                });
            }
        }
    }
    Ok(())
}

/// Precision established independently of a mint's display claims.
///
/// Peg recognition is deliberately limited to the examples in NUT-01.
pub fn defined_precision(unit: &str) -> Option<u8> {
    match unit {
        "btc" => Some(8),
        "sat" | "msat" | "auth" => Some(0),
        "usdt" | "usdc" | "eurc" => Some(2),
        "gyen" => Some(0),
        _ => iso4217::precision(unit),
    }
}

/// Display policy for an amount in one mint and unit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UnitDisplayInfo {
    /// Exact unit identifier, which must remain visible to users.
    pub unit: String,
    /// Effective decimal places; `None` means display the raw integer.
    pub precision: Option<u8>,
    /// Mint-provided name for a nonstandard unit.
    pub name: Option<String>,
    /// Mint-provided symbol for a nonstandard unit.
    pub symbol: Option<String>,
    /// Previously observed precision when a change needs acknowledgment.
    pub previous_precision: Option<u8>,
    /// New precision awaiting acknowledgment. Effective precision remains absent.
    pub pending_precision: Option<u8>,
}

impl UnitDisplayInfo {
    /// Resolve display rules using metadata from a single mint.
    pub fn resolve(unit: &str, entries: Option<&[UnitMetadata]>) -> Self {
        let mut info = Self {
            unit: unit.to_owned(),
            precision: defined_precision(unit),
            name: None,
            symbol: None,
            previous_precision: None,
            pending_precision: None,
        };
        if matches!(unit, "btc" | "sat" | "msat" | "auth") || iso4217::precision(unit).is_some() {
            return info;
        }
        let mut matches = entries
            .unwrap_or_default()
            .iter()
            .filter(|entry| entry.unit == unit);
        if let Some(entry) = matches.next() {
            if matches.next().is_none() && entry.precision <= 18 {
                info.precision = info.precision.or(Some(entry.precision));
                info.name.clone_from(&entry.name);
                info.symbol.clone_from(&entry.symbol);
            }
        }
        info
    }

    /// Format an integer amount exactly, always including its unit identifier.
    ///
    /// Names and symbols are exposed separately so applications can escape them
    /// appropriately for their output medium.
    pub fn format_amount(&self, amount: u64) -> String {
        match self.precision {
            Some(precision @ 1..=18) => {
                let scale = 10_u64.pow(u32::from(precision));
                format!(
                    "{}.{:0width$} {}",
                    amount / scale,
                    amount % scale,
                    self.unit,
                    width = usize::from(precision)
                )
            }
            _ => format!("{amount} {}", self.unit),
        }
    }
}

/// Decode malformed display data without rejecting unrelated mint info.
pub(crate) fn deserialize_settings<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<UnitMetadata>>, D::Error>
where
    D: Deserializer<'de>,
{
    if !cfg!(feature = "experimental-unit-metadata") {
        IgnoredAny::deserialize(deserializer)?;
        return Ok(None);
    }
    // Keep members as pairs until duplicate detection is complete.
    #[serde_as]
    #[derive(Deserialize)]
    struct Entry(#[serde_as(as = "Map<_, _>")] Vec<(String, Value)>);

    #[serde_as]
    #[derive(Deserialize)]
    struct Setting(#[serde_as(as = "DefaultOnError<Option<VecSkipError<_>>>")] Option<Vec<Entry>>);

    let Some(entries) = Setting::deserialize(deserializer)?.0 else {
        return Ok(None);
    };
    let mut counts = HashMap::new();
    let mut valid = Vec::new();
    for Entry(fields) in entries {
        let member_count = fields.len();
        let fields: HashMap<_, _> = fields.into_iter().collect();
        if fields.len() != member_count {
            return Ok(None);
        }
        let Some(unit) = fields.get("unit").and_then(Value::as_str) else {
            continue;
        };
        *counts.entry(unit.to_owned()).or_insert(0usize) += 1;
        if let Some(precision) = fields
            .get("precision")
            .and_then(Value::as_u64)
            .filter(|precision| *precision <= 18)
        {
            valid.push(UnitMetadata {
                unit: unit.to_owned(),
                precision: precision as u8,
                name: fields
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                symbol: fields
                    .get("symbol")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
    }
    valid.retain(|entry| counts[&entry.unit] == 1);
    Ok(Some(valid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nuts::MintInfo;

    #[test]
    fn exact_amounts_and_precedence() {
        let entries = vec![UnitMetadata {
            unit: "ora".to_owned(),
            precision: 2,
            name: None,
            symbol: None,
        }];
        assert_eq!(
            UnitDisplayInfo::resolve("ora", Some(&entries)).format_amount(500),
            "5.00 ora"
        );
        assert_eq!(
            UnitDisplayInfo::resolve("ORA", Some(&entries)).format_amount(500),
            "500 ORA"
        );
        for (unit, precision) in [
            ("btc", 8),
            ("sat", 0),
            ("msat", 0),
            ("auth", 0),
            ("jpy", 0),
            ("usd", 2),
            ("bhd", 3),
            ("clf", 4),
            ("usdc", 2),
            ("gyen", 0),
        ] {
            assert_eq!(defined_precision(unit), Some(precision));
        }
        assert_eq!(defined_precision("xau"), None);
        let mut info = UnitDisplayInfo::resolve("ora", None);
        info.precision = Some(18);
        assert_eq!(info.format_amount(u64::MAX), "18.446744073709551615 ora");
        assert_eq!(info.format_amount(0), "0.000000000000000000 ora");
    }

    #[cfg(feature = "experimental-unit-metadata")]
    #[test]
    fn publisher_validation_and_display_precedence() {
        let mut entry = UnitMetadata {
            unit: "ora".to_owned(),
            precision: 18,
            name: Some("Not a currency identity".to_owned()),
            symbol: None,
        };
        assert!(validate(&[entry.clone()]).is_ok());
        assert!(matches!(
            validate(&[entry.clone(), entry.clone()]),
            Err(Error::DuplicateUnit { .. })
        ));
        entry.precision = 19;
        assert!(matches!(
            validate(&[entry.clone()]),
            Err(Error::InvalidPrecision { .. })
        ));
        for unit in ["sat", "usd", "jpy", "btc", "usdc"] {
            entry.unit = unit.to_owned();
            entry.precision = 18;
            assert!(matches!(
                validate(&[entry.clone()]),
                Err(Error::DefinedPrecision { .. })
            ));
            let display = UnitDisplayInfo::resolve(unit, Some(&[entry.clone()]));
            assert_eq!(display.precision, defined_precision(unit));
            if unit != "usdc" {
                assert!(display.name.is_none());
            }
        }
    }

    #[cfg(feature = "experimental-unit-metadata")]
    #[test]
    fn malformed_and_duplicate_metadata_is_isolated() {
        let info: MintInfo = serde_json::from_str(
            r#"{"nuts":{"XX":[
            {"unit":"ora","precision":2},
            {"unit":"ora","precision":"bad"},
            {"unit":"ok","precision":18,"name":false},
            {"unit":"negative","precision":-1},
            {"unit":"fraction","precision":1.5},
            {"unit":"large","precision":19}, {"precision":2}, 7, null
        ],"7":{"supported":true}}}"#,
        )
        .unwrap();
        let entries = info.nuts.unit_metadata.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].unit, "ok");
        assert_eq!(entries[0].name, None);
        assert!(info.nuts.nut07.supported);

        for field in ["unit", "precision", "name", "unknown"] {
            let json = format!(
                r#"{{"nuts":{{"XX":[{{"unit":"ora","precision":2,"{field}":null,"{field}":null}}],"7":{{"supported":true}}}}}}"#
            );
            let info: MintInfo = serde_json::from_str(&json).unwrap();
            assert!(info.nuts.unit_metadata.is_none());
            assert!(info.nuts.nut07.supported);
        }
        for value in ["null", "{}", "true", "3", r#""bad""#] {
            let info: MintInfo =
                serde_json::from_str(&format!(r#"{{"nuts":{{"XX":{value}}}}}"#)).unwrap();
            assert!(info.nuts.unit_metadata.is_none());
        }
        let info: MintInfo = serde_json::from_str(r#"{"nuts":{"XX":[]}}"#).unwrap();
        assert_eq!(info.nuts.unit_metadata, Some(vec![]));
        assert!(serde_json::to_string(&info).unwrap().contains(r#""XX":[]"#));
    }

    #[cfg(not(feature = "experimental-unit-metadata"))]
    #[test]
    fn draft_wire_support_requires_opt_in() {
        let info: MintInfo =
            serde_json::from_str(r#"{"nuts":{"XX":[{"unit":"ora","precision":2}]}}"#).unwrap();
        assert!(info.nuts.unit_metadata.is_none());
        assert!(!serde_json::to_string(&info).unwrap().contains("XX"));
        assert!(matches!(validate(&[]), Err(Error::Disabled)));
    }
}

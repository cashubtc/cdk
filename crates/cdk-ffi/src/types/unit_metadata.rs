//! Currency display metadata and resolved formatting policy.

use serde::{Deserialize, Serialize};

/// FFI currency metadata.
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct UnitMetadata {
    pub unit: String,
    pub precision: u8,
    pub name: Option<String>,
    pub symbol: Option<String>,
}

impl From<cdk::nuts::UnitMetadata> for UnitMetadata {
    fn from(value: cdk::nuts::UnitMetadata) -> Self {
        Self {
            unit: value.unit,
            precision: value.precision,
            name: value.name,
            symbol: value.symbol,
        }
    }
}

impl From<UnitMetadata> for cdk::nuts::UnitMetadata {
    fn from(value: UnitMetadata) -> Self {
        Self {
            unit: value.unit,
            precision: value.precision,
            name: value.name,
            symbol: value.symbol,
        }
    }
}

/// FFI currency display policy. Pending changes use raw integer display.
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct UnitDisplayInfo {
    pub unit: String,
    pub precision: Option<u8>,
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub previous_precision: Option<u8>,
    pub pending_precision: Option<u8>,
}

impl From<cdk::nuts::UnitDisplayInfo> for UnitDisplayInfo {
    fn from(value: cdk::nuts::UnitDisplayInfo) -> Self {
        Self {
            unit: value.unit,
            precision: value.precision,
            name: value.name,
            symbol: value.symbol,
            previous_precision: value.previous_precision,
            pending_precision: value.pending_precision,
        }
    }
}

impl From<UnitDisplayInfo> for cdk::nuts::UnitDisplayInfo {
    fn from(value: UnitDisplayInfo) -> Self {
        Self {
            unit: value.unit,
            precision: value.precision,
            name: value.name,
            symbol: value.symbol,
            previous_precision: value.previous_precision,
            pending_precision: value.pending_precision,
        }
    }
}

/// Format a raw integer amount with a resolved display policy.
#[uniffi::export]
pub fn format_unit_amount(info: UnitDisplayInfo, amount: u64) -> String {
    cdk::nuts::UnitDisplayInfo::from(info).format_amount(amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_metadata_ffi_round_trip_preserves_pending_changes() {
        let mut display = cdk::nuts::UnitDisplayInfo::resolve("ora", None);
        display.previous_precision = Some(2);
        display.pending_precision = Some(3);
        let ffi = UnitDisplayInfo::from(display.clone());
        assert_eq!(format_unit_amount(ffi.clone(), 500), "500 ora");
        assert_eq!(cdk::nuts::UnitDisplayInfo::from(ffi), display);
    }

    #[test]
    fn unit_metadata_survives_ffi_mint_info_and_json_conversions() {
        let mut core = cdk::nuts::MintInfo::default();
        core.nuts.unit_metadata = Some(vec![cdk::nuts::UnitMetadata {
            unit: " ORA ".to_owned(),
            precision: 18,
            name: Some("Ora".to_owned()),
            symbol: Some("O".to_owned()),
        }]);
        let ffi = crate::types::MintInfo::from(core.clone());
        let json = crate::types::encode_mint_info(ffi).unwrap();
        let decoded = crate::types::decode_mint_info(json).unwrap();
        let round_trip = cdk::nuts::MintInfo::try_from(decoded).unwrap();
        assert_eq!(round_trip.nuts.unit_metadata, core.nuts.unit_metadata);
    }
}

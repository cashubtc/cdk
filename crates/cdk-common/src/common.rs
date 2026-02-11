//! Types

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::mint_url::MintUrl;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    CurrencyUnit, MeltQuoteState, PaymentMethod, Proof, Proofs, PublicKey, SpendingConditions,
    State,
};
use crate::Amount;

/// Melt response with proofs
#[derive(Debug, Clone, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Melted {
    /// State of quote
    pub state: MeltQuoteState,
    /// Preimage of melt payment
    pub preimage: Option<String>,
    /// Melt change
    pub change: Option<Proofs>,
    /// Melt amount
    pub amount: Amount,
    /// Fee paid
    pub fee_paid: Amount,
}

impl Melted {
    /// Create new [`Melted`]
    pub fn from_proofs(
        state: MeltQuoteState,
        preimage: Option<String>,
        quote_amount: Amount,
        proofs: Proofs,
        change_proofs: Option<Proofs>,
    ) -> Result<Self, Error> {
        let proofs_amount = proofs.total_amount()?;
        let change_amount = match &change_proofs {
            Some(change_proofs) => change_proofs.total_amount()?,
            None => Amount::ZERO,
        };

        tracing::info!(
            "Proofs amount: {} Amount: {} Change: {}",
            proofs_amount,
            quote_amount,
            change_amount
        );

        let fee_paid = proofs_amount
            .checked_sub(
                quote_amount
                    .checked_add(change_amount)
                    .ok_or(Error::AmountOverflow)?,
            )
            .ok_or(Error::AmountOverflow)?;

        Ok(Self {
            state,
            preimage,
            change: change_proofs,
            amount: quote_amount,
            fee_paid,
        })
    }

    /// Total amount melted
    ///
    /// # Panics
    ///
    /// Panics if the sum of `amount` and `fee_paid` overflows. This should not
    /// happen as the fee is validated when calculated.
    pub fn total_amount(&self) -> Amount {
        self.amount
            .checked_add(self.fee_paid)
            .expect("We check when calc fee paid")
    }
}

/// Prooinfo
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofInfo {
    /// Proof
    pub proof: Proof,
    /// y
    pub y: PublicKey,
    /// Mint Url
    pub mint_url: MintUrl,
    /// Proof State
    pub state: State,
    /// Proof Spending Conditions
    pub spending_condition: Option<SpendingConditions>,
    /// Unit
    pub unit: CurrencyUnit,
}

impl ProofInfo {
    /// Create new [`ProofInfo`]
    pub fn new(
        proof: Proof,
        mint_url: MintUrl,
        state: State,
        unit: CurrencyUnit,
    ) -> Result<Self, Error> {
        let y = proof.y()?;

        let spending_condition: Option<SpendingConditions> = (&proof.secret).try_into().ok();

        Ok(Self {
            proof,
            y,
            mint_url,
            state,
            spending_condition,
            unit,
        })
    }

    /// Check if [`Proof`] matches conditions
    pub fn matches_conditions(
        &self,
        mint_url: &Option<MintUrl>,
        unit: &Option<CurrencyUnit>,
        state: &Option<Vec<State>>,
        spending_conditions: &Option<Vec<SpendingConditions>>,
    ) -> bool {
        if let Some(mint_url) = mint_url {
            if mint_url.ne(&self.mint_url) {
                return false;
            }
        }

        if let Some(unit) = unit {
            if unit.ne(&self.unit) {
                return false;
            }
        }

        if let Some(state) = state {
            if !state.contains(&self.state) {
                return false;
            }
        }

        if let Some(spending_conditions) = spending_conditions {
            match &self.spending_condition {
                None => {
                    if !spending_conditions.is_empty() {
                        return false;
                    }
                }
                Some(s) => {
                    if !spending_conditions.contains(s) {
                        return false;
                    }
                }
            }
        }

        true
    }
}

/// Key used in hashmap of ln backends to identify what unit and payment method
/// it is for
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentProcessorKey {
    /// Unit of Payment backend
    pub unit: CurrencyUnit,
    /// Method of payment backend
    pub method: PaymentMethod,
}

impl PaymentProcessorKey {
    /// Create new [`PaymentProcessorKey`]
    pub fn new(unit: CurrencyUnit, method: PaymentMethod) -> Self {
        Self { unit, method }
    }
}

/// Seconds quotes are valid
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteTTL {
    /// Seconds mint quote is valid
    pub mint_ttl: u64,
    /// Seconds melt quote is valid
    pub melt_ttl: u64,
}

impl QuoteTTL {
    /// Create new [`QuoteTTL`]
    pub fn new(mint_ttl: u64, melt_ttl: u64) -> QuoteTTL {
        Self { mint_ttl, melt_ttl }
    }
}

impl Default for QuoteTTL {
    fn default() -> Self {
        Self {
            mint_ttl: 60 * 60, // 1 hour
            melt_ttl: 60,      // 1 minute
        }
    }
}

/// Mint Fee Reserve
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeeReserve {
    /// Absolute expected min fee
    pub min_fee_reserve: Amount,
    /// Percentage expected fee
    pub percent_fee_reserve: f32,
}

/// CDK Version
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct IssuerVersion {
    /// Implementation name (e.g., "cdk", "nutshell")
    pub implementation: String,
    /// Major version
    pub major: u16,
    /// Minor version
    pub minor: u16,
    /// Patch version
    pub patch: u16,
}

impl IssuerVersion {
    /// Create new [`IssuerVersion`]
    pub fn new(implementation: String, major: u16, minor: u16, patch: u16) -> Self {
        Self {
            implementation,
            major,
            minor,
            patch,
        }
    }
}

impl std::fmt::Display for IssuerVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}.{}.{}",
            self.implementation, self.major, self.minor, self.patch
        )
    }
}

impl PartialOrd for IssuerVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        if self.implementation != other.implementation {
            return None;
        }

        match self.major.cmp(&other.major) {
            std::cmp::Ordering::Equal => match self.minor.cmp(&other.minor) {
                std::cmp::Ordering::Equal => Some(self.patch.cmp(&other.patch)),
                other => Some(other),
            },
            other => Some(other),
        }
    }
}

impl std::str::FromStr for IssuerVersion {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (implementation, version_str) = s
            .split_once('/')
            .ok_or(Error::Custom(format!("Invalid version string: {}", s)))?;
        let implementation = implementation.to_string();

        let parts: Vec<&str> = version_str.splitn(3, '.').collect();
        if parts.len() != 3 {
            return Err(Error::Custom(format!("Invalid version string: {}", s)));
        }

        let major = parts[0]
            .parse()
            .map_err(|_| Error::Custom(format!("Invalid major version: {}", parts[0])))?;
        let minor = parts[1]
            .parse()
            .map_err(|_| Error::Custom(format!("Invalid minor version: {}", parts[1])))?;

        // Handle patch version with optional suffixes like -rc1
        let patch_str = parts[2];
        let patch_end = patch_str
            .find(|c: char| !c.is_numeric())
            .unwrap_or(patch_str.len());
        let patch = patch_str[..patch_end]
            .parse()
            .map_err(|_| Error::Custom(format!("Invalid patch version: {}", parts[2])))?;

        Ok(Self {
            implementation,
            major,
            minor,
            patch,
        })
    }
}

impl Serialize for IssuerVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for IssuerVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        std::str::FromStr::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cashu::SecretKey;

    use super::{Melted, ProofInfo};
    use crate::mint_url::MintUrl;
    use crate::nuts::{CurrencyUnit, Id, Proof, PublicKey, SpendingConditions, State};
    use crate::secret::Secret;
    use crate::Amount;

    #[test]
    fn test_melted() {
        let keyset_id = Id::from_str("00deadbeef123456").unwrap();
        let proof = Proof::new(
            Amount::from(64),
            keyset_id,
            Secret::generate(),
            PublicKey::from_hex(
                "02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        );
        let melted = Melted::from_proofs(
            super::MeltQuoteState::Paid,
            Some("preimage".to_string()),
            Amount::from(64),
            vec![proof.clone()],
            None,
        )
        .unwrap();
        assert_eq!(melted.amount, Amount::from(64));
        assert_eq!(melted.fee_paid, Amount::ZERO);
        assert_eq!(melted.total_amount(), Amount::from(64));
    }

    #[test]
    fn test_melted_with_change() {
        let keyset_id = Id::from_str("00deadbeef123456").unwrap();
        let proof = Proof::new(
            Amount::from(64),
            keyset_id,
            Secret::generate(),
            PublicKey::from_hex(
                "02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        );
        let change_proof = Proof::new(
            Amount::from(32),
            keyset_id,
            Secret::generate(),
            PublicKey::from_hex(
                "03deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        );
        let melted = Melted::from_proofs(
            super::MeltQuoteState::Paid,
            Some("preimage".to_string()),
            Amount::from(31),
            vec![proof.clone()],
            Some(vec![change_proof.clone()]),
        )
        .unwrap();
        assert_eq!(melted.amount, Amount::from(31));
        assert_eq!(melted.fee_paid, Amount::from(1));
        assert_eq!(melted.total_amount(), Amount::from(32));
    }

    #[test]
    fn test_matches_conditions() {
        let keyset_id = Id::from_str("00deadbeef123456").unwrap();
        let proof = Proof::new(
            Amount::from(64),
            keyset_id,
            Secret::new("test_secret"),
            PublicKey::from_hex(
                "02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        );

        let mint_url = MintUrl::from_str("https://example.com").unwrap();
        let proof_info =
            ProofInfo::new(proof, mint_url.clone(), State::Unspent, CurrencyUnit::Sat).unwrap();

        // Test matching mint_url
        assert!(proof_info.matches_conditions(&Some(mint_url.clone()), &None, &None, &None));
        assert!(!proof_info.matches_conditions(
            &Some(MintUrl::from_str("https://different.com").unwrap()),
            &None,
            &None,
            &None
        ));

        // Test matching unit
        assert!(proof_info.matches_conditions(&None, &Some(CurrencyUnit::Sat), &None, &None));
        assert!(!proof_info.matches_conditions(&None, &Some(CurrencyUnit::Msat), &None, &None));

        // Test matching state
        assert!(proof_info.matches_conditions(&None, &None, &Some(vec![State::Unspent]), &None));
        assert!(proof_info.matches_conditions(
            &None,
            &None,
            &Some(vec![State::Unspent, State::Spent]),
            &None
        ));
        assert!(!proof_info.matches_conditions(&None, &None, &Some(vec![State::Spent]), &None));

        // Test with no conditions (should match)
        assert!(proof_info.matches_conditions(&None, &None, &None, &None));

        // Test with multiple conditions
        assert!(proof_info.matches_conditions(
            &Some(mint_url),
            &Some(CurrencyUnit::Sat),
            &Some(vec![State::Unspent]),
            &None
        ));
    }

    #[test]
    fn test_matches_conditions_with_spending_conditions() {
        // This test would need to be expanded with actual SpendingConditions
        // implementation, but we can test the basic case where no spending
        // conditions are present

        let keyset_id = Id::from_str("00deadbeef123456").unwrap();
        let proof = Proof::new(
            Amount::from(64),
            keyset_id,
            Secret::new("test_secret"),
            PublicKey::from_hex(
                "02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        );

        let mint_url = MintUrl::from_str("https://example.com").unwrap();
        let proof_info =
            ProofInfo::new(proof, mint_url, State::Unspent, CurrencyUnit::Sat).unwrap();

        // Test with empty spending conditions (should match when proof has none)
        assert!(proof_info.matches_conditions(&None, &None, &None, &Some(vec![])));

        // Test with non-empty spending conditions (should not match when proof has none)
        let dummy_condition = SpendingConditions::P2PKConditions {
            data: SecretKey::generate().public_key(),
            conditions: None,
        };
        assert!(!proof_info.matches_conditions(&None, &None, &None, &Some(vec![dummy_condition])));
    }

    use super::IssuerVersion;

    #[test]
    fn test_version_parsing() {
        // Test explicit cdk format
        let v = IssuerVersion::from_str("cdk/1.2.3").unwrap();
        assert_eq!(v.implementation, "cdk");
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
        assert_eq!(v.to_string(), "cdk/1.2.3");

        // Test nutshell format
        let v = IssuerVersion::from_str("nutshell/0.16.0").unwrap();
        assert_eq!(v.implementation, "nutshell");
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 16);
        assert_eq!(v.patch, 0);
        assert_eq!(v.to_string(), "nutshell/0.16.0");
    }

    #[test]
    fn test_version_ordering() {
        let v1 = IssuerVersion::from_str("cdk/0.1.0").unwrap();
        let v2 = IssuerVersion::from_str("cdk/0.1.1").unwrap();
        let v3 = IssuerVersion::from_str("cdk/0.2.0").unwrap();
        let v4 = IssuerVersion::from_str("cdk/1.0.0").unwrap();

        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 < v4);
        assert!(v1 < v4);

        // Test mixed implementations
        let v_nutshell = IssuerVersion::from_str("nutshell/0.1.0").unwrap();
        assert_eq!(v1.partial_cmp(&v_nutshell), None);
        assert!(!(v1 < v_nutshell));
        assert!(!(v1 > v_nutshell));
        assert!(!(v1 == v_nutshell));
    }

    #[test]
    fn test_version_serialization() {
        let v = IssuerVersion::from_str("cdk/0.14.2").unwrap();
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, "\"cdk/0.14.2\"");

        let v_deserialized: IssuerVersion = serde_json::from_str(&json).unwrap();
        assert_eq!(v, v_deserialized);
    }

    #[test]
    fn test_cdk_version_parsing_with_suffix() {
        let version_str = "cdk/0.15.0-rc1";
        let version = IssuerVersion::from_str(version_str).unwrap();
        assert_eq!(version.implementation, "cdk");
        assert_eq!(version.major, 0);
        assert_eq!(version.minor, 15);
        assert_eq!(version.patch, 0);
    }

    #[test]
    fn test_cdk_version_parsing_standard() {
        let version_str = "cdk/0.15.0";
        let version = IssuerVersion::from_str(version_str).unwrap();
        assert_eq!(version.implementation, "cdk");
        assert_eq!(version.major, 0);
        assert_eq!(version.minor, 15);
        assert_eq!(version.patch, 0);
    }

    #[test]
    fn test_cdk_version_parsing_complex_suffix() {
        let version_str = "cdk/0.15.0-beta.1+build123";
        let version = IssuerVersion::from_str(version_str).unwrap();
        assert_eq!(version.implementation, "cdk");
        assert_eq!(version.major, 0);
        assert_eq!(version.minor, 15);
        assert_eq!(version.patch, 0);
    }

    #[test]
    fn test_cdk_version_parsing_invalid() {
        // Missing prefix
        let version_str = "0.15.0";
        assert!(IssuerVersion::from_str(version_str).is_err());

        // Invalid version format
        let version_str = "cdk/0.15";
        assert!(IssuerVersion::from_str(version_str).is_err());

        let version_str = "cdk/0.15.a";
        assert!(IssuerVersion::from_str(version_str).is_err());
    }

    #[test]
    fn test_cdk_version_parsing_with_implementation() {
        let version_str = "nutshell/0.16.2";
        let version = IssuerVersion::from_str(version_str).unwrap();
        assert_eq!(version.implementation, "nutshell");
        assert_eq!(version.major, 0);
        assert_eq!(version.minor, 16);
        assert_eq!(version.patch, 2);
    }

    #[test]
    fn test_cdk_version_comparison_different_implementations() {
        let v1 = IssuerVersion::from_str("cdk/0.15.0").unwrap();
        let v2 = IssuerVersion::from_str("nutshell/0.15.0").unwrap();

        assert_eq!(v1.partial_cmp(&v2), None);
    }
}

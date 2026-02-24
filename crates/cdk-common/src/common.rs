//! Types

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{CurrencyUnit, MeltQuoteState, PaymentMethod, Proofs};
// Re-export ProofInfo from wallet module for backwards compatibility
#[cfg(feature = "wallet")]
pub use crate::wallet::ProofInfo;
use crate::Amount;

/// Result of a finalized melt operation
#[derive(Clone, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FinalizedMelt {
    /// Quote ID
    quote_id: String,
    /// State of quote
    state: MeltQuoteState,
    /// Payment proof (e.g., Lightning preimage)
    payment_proof: Option<String>,
    /// Melt change
    change: Option<Proofs>,
    /// Melt amount
    amount: Amount,
    /// Fee paid
    fee_paid: Amount,
}

impl FinalizedMelt {
    /// Create new [`FinalizedMelt`]
    pub fn new(
        quote_id: String,
        state: MeltQuoteState,
        payment_proof: Option<String>,
        amount: Amount,
        fee_paid: Amount,
        change: Option<Proofs>,
    ) -> Self {
        Self {
            quote_id,
            state,
            payment_proof,
            change,
            amount,
            fee_paid,
        }
    }

    /// Create new [`FinalizedMelt`] calculating fee from proofs
    pub fn from_proofs(
        quote_id: String,
        state: MeltQuoteState,
        payment_proof: Option<String>,
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
            quote_id,
            state,
            payment_proof,
            change: change_proofs,
            amount: quote_amount,
            fee_paid,
        })
    }

    /// Get the quote ID
    #[inline]
    pub fn quote_id(&self) -> &str {
        &self.quote_id
    }

    /// Get the state of the melt
    #[inline]
    pub fn state(&self) -> MeltQuoteState {
        self.state
    }

    /// Get the payment proof (e.g., Lightning preimage)
    #[inline]
    pub fn payment_proof(&self) -> Option<&str> {
        self.payment_proof.as_deref()
    }

    /// Get the change proofs
    #[inline]
    pub fn change(&self) -> Option<&Proofs> {
        self.change.as_ref()
    }

    /// Consume self and return the change proofs
    #[inline]
    pub fn into_change(self) -> Option<Proofs> {
        self.change
    }

    /// Get the amount melted
    #[inline]
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Get the fee paid
    #[inline]
    pub fn fee_paid(&self) -> Amount {
        self.fee_paid
    }

    /// Total amount melted (amount + fee)
    ///
    /// # Panics
    ///
    /// Panics if the sum of `amount` and `fee_paid` overflows. This should not
    /// happen as the fee is validated when calculated.
    #[inline]
    pub fn total_amount(&self) -> Amount {
        self.amount
            .checked_add(self.fee_paid)
            .expect("We check when calc fee paid")
    }
}

impl std::fmt::Debug for FinalizedMelt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinalizedMelt")
            .field("quote_id", &self.quote_id)
            .field("state", &self.state)
            .field("amount", &self.amount)
            .field("fee_paid", &self.fee_paid)
            .finish()
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

    use super::FinalizedMelt;
    use crate::nuts::{Id, Proof, PublicKey};
    use crate::secret::Secret;
    use crate::Amount;

    #[test]
    fn test_finalized_melt() {
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
        let finalized = FinalizedMelt::from_proofs(
            "test_quote_id".to_string(),
            super::MeltQuoteState::Paid,
            Some("preimage".to_string()),
            Amount::from(64),
            vec![proof.clone()],
            None,
        )
        .unwrap();
        assert_eq!(finalized.quote_id(), "test_quote_id");
        assert_eq!(finalized.amount(), Amount::from(64));
        assert_eq!(finalized.fee_paid(), Amount::ZERO);
        assert_eq!(finalized.total_amount(), Amount::from(64));
    }

    #[test]
    fn test_finalized_melt_with_change() {
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
        let finalized = FinalizedMelt::from_proofs(
            "test_quote_id".to_string(),
            super::MeltQuoteState::Paid,
            Some("preimage".to_string()),
            Amount::from(31),
            vec![proof.clone()],
            Some(vec![change_proof.clone()]),
        )
        .unwrap();
        assert_eq!(finalized.quote_id(), "test_quote_id");
        assert_eq!(finalized.amount(), Amount::from(31));
        assert_eq!(finalized.fee_paid(), Amount::from(1));
        assert_eq!(finalized.total_amount(), Amount::from(32));
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

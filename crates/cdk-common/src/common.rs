//! Types

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Error;
use crate::mint_url::MintUrl;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    CurrencyUnit, MeltQuoteState, PaymentMethod, Proof, Proofs, PublicKey, SpendingConditions,
    State,
};
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
    /// Operation ID that is using/spending this proof
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_by_operation: Option<Uuid>,
    /// Operation ID that created this proof
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by_operation: Option<Uuid>,
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
            used_by_operation: None,
            created_by_operation: None,
        })
    }

    /// Create new [`ProofInfo`] with operation tracking
    pub fn new_with_operations(
        proof: Proof,
        mint_url: MintUrl,
        state: State,
        unit: CurrencyUnit,
        used_by_operation: Option<Uuid>,
        created_by_operation: Option<Uuid>,
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
            used_by_operation,
            created_by_operation,
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cashu::SecretKey;

    use super::{FinalizedMelt, ProofInfo};
    use crate::mint_url::MintUrl;
    use crate::nuts::{CurrencyUnit, Id, Proof, PublicKey, SpendingConditions, State};
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
}

/// Mint Fee Reserve
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeeReserve {
    /// Absolute expected min fee
    pub min_fee_reserve: Amount,
    /// Percentage expected fee
    pub percent_fee_reserve: f32,
}

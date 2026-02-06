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
}

/// Mint Fee Reserve
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeeReserve {
    /// Absolute expected min fee
    pub min_fee_reserve: Amount,
    /// Percentage expected fee
    pub percent_fee_reserve: f32,
}

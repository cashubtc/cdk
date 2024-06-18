//! Types

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Error;
use crate::nuts::{
    CurrencyUnit, MeltQuoteState, MintQuoteState, Proof, Proofs, PublicKey, SpendingConditions,
    State,
};
use crate::url::UncheckedUrl;
use crate::Amount;

/// Melt response with proofs
#[derive(Debug, Clone, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Melted {
    pub state: MeltQuoteState,
    pub preimage: Option<String>,
    pub change: Option<Proofs>,
}

/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuote {
    pub id: String,
    pub mint_url: UncheckedUrl,
    pub amount: Amount,
    pub unit: CurrencyUnit,
    pub request: String,
    pub state: MintQuoteState,
    pub expiry: u64,
}

impl MintQuote {
    pub fn new(
        mint_url: UncheckedUrl,
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        expiry: u64,
    ) -> Self {
        let id = Uuid::new_v4();

        Self {
            mint_url,
            id: id.to_string(),
            amount,
            unit,
            request,
            state: MintQuoteState::Unpaid,
            expiry,
        }
    }
}

/// Melt Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuote {
    pub id: String,
    pub unit: CurrencyUnit,
    pub amount: Amount,
    pub request: String,
    pub fee_reserve: Amount,
    pub state: MeltQuoteState,
    pub expiry: u64,
    pub payment_preimage: Option<String>,
}

#[cfg(feature = "mint")]
impl MeltQuote {
    pub fn new(
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        fee_reserve: Amount,
        expiry: u64,
    ) -> Self {
        let id = Uuid::new_v4();

        Self {
            id: id.to_string(),
            amount,
            unit,
            request,
            fee_reserve,
            state: MeltQuoteState::Unpaid,
            expiry,
            payment_preimage: None,
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofInfo {
    pub proof: Proof,
    pub y: PublicKey,
    pub mint_url: UncheckedUrl,
    pub state: State,
    pub spending_condition: Option<SpendingConditions>,
    pub unit: CurrencyUnit,
}

impl ProofInfo {
    pub fn new(
        proof: Proof,
        mint_url: UncheckedUrl,
        state: State,
        unit: CurrencyUnit,
    ) -> Result<Self, Error> {
        let y = proof
            .y()
            .map_err(|_| Error::CustomError("Could not find y".to_string()))?;

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

    pub fn matches_conditions(
        &self,
        mint_url: &Option<UncheckedUrl>,
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
                None => return false,
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

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use cdk::mint::{MeltQuote as MintMeltQuote, Mint};
use cdk::nuts::{
    CurrencyUnit, Id, MeltOptions as NutMeltOptions, MeltQuoteState, MintQuoteState, State,
};
use cdk::Amount;
use cdk_common::mint::OperationKind;
use tonic::Status;

use crate::melt_options::Options::{Amountless, Mpp};
use crate::{AmountlessOptions, Balance, MeltOptions, MeltQuote, MppOptions};

/// Raw balance data fetched from the mint database
///
/// This is the base data structure - use the helper methods
/// to aggregate by unit or access per-keyset stats.
pub struct MintBalances {
    /// Issued amounts per keyset ID
    pub issued: HashMap<Id, Amount>,
    /// Redeemed amounts per keyset ID
    pub redeemed: HashMap<Id, Amount>,
    /// Fees collected per keyset ID
    pub fees: HashMap<Id, Amount>,
    /// Keyset ID to unit mapping
    pub keyset_units: HashMap<Id, CurrencyUnit>,
}

impl MintBalances {
    /// Fetch all balance data from the mint (3 DB calls)
    pub async fn fetch(mint: &Mint) -> Result<Self, Status> {
        let keyset_units: HashMap<_, _> = mint
            .keyset_infos()
            .into_iter()
            .map(|info| (info.id, info.unit))
            .collect();

        let issued = mint
            .total_issued()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let redeemed = mint
            .total_redeemed()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let fees = mint
            .total_fees()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Self {
            issued,
            redeemed,
            fees,
            keyset_units,
        })
    }

    /// Get stats for a specific keyset
    pub fn get_keyset_stats(&self, id: &Id) -> KeysetStats {
        KeysetStats {
            total_issued: self.issued.get(id).copied().unwrap_or(Amount::ZERO),
            total_redeemed: self.redeemed.get(id).copied().unwrap_or(Amount::ZERO),
            total_fees_collected: self.fees.get(id).copied().unwrap_or(Amount::ZERO),
        }
    }

    /// Aggregate balances by currency unit
    ///
    /// Returns None if an overflow occurs during aggregation.
    pub fn aggregate_by_unit(&self) -> Option<UnitBalances> {
        let issued = self.aggregate_amounts_by_unit(&self.issued)?;
        let redeemed = self.aggregate_amounts_by_unit(&self.redeemed)?;
        let fees = self.aggregate_amounts_by_unit(&self.fees)?;

        Some(UnitBalances {
            issued,
            redeemed,
            fees,
        })
    }

    /// Helper to aggregate a single amount map by unit
    ///
    /// Returns None if an overflow occurs.
    fn aggregate_amounts_by_unit(
        &self,
        amounts: &HashMap<Id, Amount>,
    ) -> Option<HashMap<CurrencyUnit, Amount>> {
        let mut by_unit: HashMap<CurrencyUnit, Amount> = HashMap::new();
        for (keyset_id, amount) in amounts {
            if let Some(unit) = self.keyset_units.get(keyset_id) {
                let entry = by_unit.entry(unit.clone()).or_insert(Amount::ZERO);
                *entry = entry.checked_add(*amount)?;
            }
        }
        Some(by_unit)
    }
}

/// Balances aggregated by currency unit
pub struct UnitBalances {
    pub issued: HashMap<CurrencyUnit, Amount>,
    pub redeemed: HashMap<CurrencyUnit, Amount>,
    pub fees: HashMap<CurrencyUnit, Amount>,
}

impl UnitBalances {
    /// Convert to proto Balance objects with optional unit filter
    pub fn to_balances(&self, unit_filter: Option<&CurrencyUnit>) -> Vec<Balance> {
        self.issued
            .iter()
            .filter(|(unit, _)| unit_filter.is_none_or(|f| f == *unit))
            .map(|(unit, &issued)| {
                let redeemed = self.redeemed.get(unit).copied().unwrap_or(Amount::ZERO);
                let fees = self.fees.get(unit).copied().unwrap_or(Amount::ZERO);

                Balance {
                    unit: unit.to_string(),
                    total_balance: issued.checked_sub(redeemed).unwrap_or(Amount::ZERO).into(),
                    total_issued: issued.into(),
                    total_redeemed: redeemed.into(),
                    total_fees_collected: fees.into(),
                }
            })
            .collect()
    }
}

/// Statistics for a single keyset
pub struct KeysetStats {
    pub total_issued: Amount,
    pub total_redeemed: Amount,
    pub total_fees_collected: Amount,
}

impl KeysetStats {
    /// Calculate net balance (issued - redeemed)
    pub fn total_balance(&self) -> Amount {
        self.total_issued
            .checked_sub(self.total_redeemed)
            .unwrap_or(Amount::ZERO)
    }
}

/// Convert a mint MeltQuote to proto MeltQuote
pub fn melt_quote_to_proto(quote: &MintMeltQuote) -> MeltQuote {
    let options = quote.options.map(|opt| {
        let options = match opt {
            NutMeltOptions::Mpp { mpp } => Mpp(MppOptions {
                amount: mpp.amount.into(),
            }),
            NutMeltOptions::Amountless { amountless } => Amountless(AmountlessOptions {
                amount_msat: amountless.amount_msat.into(),
            }),
        };
        MeltOptions {
            options: Some(options),
        }
    });

    MeltQuote {
        id: quote.id.to_string(),
        unit: quote.unit.to_string(),
        amount: quote.amount().value(),
        request: quote.request.to_string(),
        fee_reserve: quote.fee_reserve().value(),
        state: quote.state.to_string(),
        payment_preimage: quote.payment_preimage.clone(),
        request_lookup_id: quote.request_lookup_id.as_ref().map(|r| r.to_string()),
        created_time: quote.created_time,
        paid_time: quote.paid_time,
        payment_method: quote.payment_method.to_string(),
        options,
    }
}

/// Result of validating units against mint configuration
pub struct ValidateUnitsResult {
    /// Successfully parsed currency units
    pub parsed: Vec<CurrencyUnit>,
    /// Invalid unit strings that weren't recognized
    pub invalid: Vec<String>,
    /// Valid units configured in the mint (for error messages)
    pub valid_units: Vec<String>,
}

/// Validates unit strings against the mint's actual configured units
///
/// Returns parsed units, invalid unit strings, and the list of valid units for error messages.
pub fn validate_units_against_mint(units: &[String], mint: &Mint) -> ValidateUnitsResult {
    let valid_units: HashSet<String> = mint
        .keyset_infos()
        .into_iter()
        .filter(|info| info.unit != CurrencyUnit::Auth)
        .map(|info| info.unit.to_string().to_lowercase())
        .collect();

    if units.is_empty() {
        return ValidateUnitsResult {
            parsed: Vec::new(),
            invalid: Vec::new(),
            valid_units: valid_units.into_iter().collect(),
        };
    }

    let mut parsed = Vec::new();
    let mut invalid = Vec::new();

    for u in units {
        if valid_units.contains(&u.to_lowercase()) {
            if let Ok(unit) = CurrencyUnit::from_str(u) {
                parsed.push(unit);
            }
        } else {
            invalid.push(u.clone());
        }
    }

    ValidateUnitsResult {
        parsed,
        invalid,
        valid_units: valid_units.into_iter().collect(),
    }
}

/// Validates and parses mint quote state strings, returns parsed states and any invalid ones
pub fn parse_mint_quote_states(states: &[String]) -> (Vec<MintQuoteState>, Vec<String>) {
    let mut parsed = Vec::new();
    let mut invalid = Vec::new();

    for s in states {
        match MintQuoteState::from_str(s) {
            Ok(state) => parsed.push(state),
            Err(_) => invalid.push(s.clone()),
        }
    }

    (parsed, invalid)
}

/// Validates and parses melt quote state strings, returns parsed states and any invalid ones
pub fn parse_melt_quote_states(states: &[String]) -> (Vec<MeltQuoteState>, Vec<String>) {
    let mut parsed = Vec::new();
    let mut invalid = Vec::new();

    for s in states {
        match MeltQuoteState::from_str(s) {
            Ok(state) => parsed.push(state),
            Err(_) => invalid.push(s.clone()),
        }
    }

    (parsed, invalid)
}

/// Validates and parses proof state strings, returns parsed states and any invalid ones
pub fn parse_proof_states(states: &[String]) -> (Vec<State>, Vec<String>) {
    let mut parsed = Vec::new();
    let mut invalid = Vec::new();

    for s in states {
        match State::from_str(s) {
            Ok(state) => parsed.push(state),
            Err(_) => invalid.push(s.clone()),
        }
    }

    (parsed, invalid)
}

/// Validates and parses keyset ID strings, returns parsed IDs and any invalid ones
pub fn parse_keyset_ids(ids: &[String]) -> (Vec<Id>, Vec<String>) {
    let mut parsed = Vec::with_capacity(ids.len());
    let mut invalid = Vec::new();

    for id in ids {
        match Id::from_str(id) {
            Ok(keyset_id) => parsed.push(keyset_id),
            Err(_) => invalid.push(id.clone()),
        }
    }

    (parsed, invalid)
}

/// Validates operation kind strings, returns invalid ones if any
pub fn validate_operations(operations: &[String]) -> (Vec<String>, Vec<String>) {
    let mut invalid = Vec::new();

    for op in operations {
        if OperationKind::from_str(op).is_err() {
            invalid.push(op.clone());
        }
    }

    (operations.to_vec(), invalid)
}

/// Validates pagination parameters
///
/// Returns `true` if valid, `false` if index_offset is provided without a limit (num_max).
pub fn validate_pagination(index_offset: i64, num_max: i64) -> bool {
    !(index_offset > 0 && num_max <= 0)
}

/// Returns the effective limit, defaulting to 100 if not specified
pub fn effective_limit(num_max: i64) -> u64 {
    if num_max > 0 {
        num_max as u64
    } else {
        100
    }
}

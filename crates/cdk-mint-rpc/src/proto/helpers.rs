use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use cdk::cdk_database::{BlindSignatureRecord, OperationRecord, ProofRecord};
use cdk::mint::MeltQuote as MintMeltQuote;
use cdk::mint::Mint;
use cdk::mint::MintQuote as MintMintQuote;
use cdk::nuts::{CurrencyUnit, Id, MeltQuoteState, MintQuoteState, State};
use cdk::Amount;
use cdk_common::mint::OperationKind;
use tonic::Status;

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
        // Build keyset ID -> unit mapping
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
    pub fn aggregate_by_unit(&self) -> Result<UnitBalances, Status> {
        let issued = self.aggregate_amounts_by_unit(&self.issued)?;
        let redeemed = self.aggregate_amounts_by_unit(&self.redeemed)?;
        let fees = self.aggregate_amounts_by_unit(&self.fees)?;

        Ok(UnitBalances {
            issued,
            redeemed,
            fees,
        })
    }

    /// Helper to aggregate a single amount map by unit
    fn aggregate_amounts_by_unit(
        &self,
        amounts: &HashMap<Id, Amount>,
    ) -> Result<HashMap<CurrencyUnit, Amount>, Status> {
        let mut by_unit: HashMap<CurrencyUnit, Amount> = HashMap::new();
        for (keyset_id, amount) in amounts {
            if let Some(unit) = self.keyset_units.get(keyset_id) {
                let entry = by_unit.entry(unit.clone()).or_insert(Amount::ZERO);
                *entry = entry
                    .checked_add(*amount)
                    .ok_or_else(|| Status::internal("Overflow".to_string()))?;
            }
        }
        Ok(by_unit)
    }
}

/// Balances aggregated by currency unit
pub struct UnitBalances {
    pub issued: HashMap<CurrencyUnit, Amount>,
    pub redeemed: HashMap<CurrencyUnit, Amount>,
    pub fees: HashMap<CurrencyUnit, Amount>,
}

/// Statistics for a single keyset
#[derive(Default, Clone)]
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

// Legacy function wrappers for backwards compatibility

/// Fetches issued, redeemed, and fees aggregated by currency unit
pub async fn get_balances_by_unit(mint: &Mint) -> Result<UnitBalances, Status> {
    let balances = MintBalances::fetch(mint).await?;
    balances.aggregate_by_unit()
}

/// Convert a mint MintQuote to proto MintQuoteSummary (for list responses)
///
/// This version does not include paid_time/issued_time which require JOINs.
/// Use this for efficient list operations.
pub fn mint_quote_to_summary(quote: &MintMintQuote) -> crate::MintQuoteSummary {
    crate::MintQuoteSummary {
        id: quote.id.to_string(),
        amount: quote.amount.map(|a| a.into()),
        unit: quote.unit.to_string(),
        request: quote.request.clone(),
        state: quote.state().to_string(),
        request_lookup_id: Some(quote.request_lookup_id.to_string()),
        request_lookup_id_kind: quote.request_lookup_id.kind().to_string(),
        pubkey: quote.pubkey.map(|pk| pk.to_string()),
        created_time: quote.created_time,
        amount_paid: quote.amount_paid().into(),
        amount_issued: quote.amount_issued().into(),
        payment_method: quote.payment_method.to_string(),
    }
}

/// Convert a mint MintQuote to proto MintQuoteDetail (for single quote lookup)
///
/// This version includes full payment/issuance history.
/// Use this for detailed single-quote queries.
pub fn mint_quote_to_detail(quote: &MintMintQuote) -> crate::MintQuoteDetail {
    // Convert payments to proto
    let payments: Vec<crate::MintQuotePayment> = quote
        .payments
        .iter()
        .map(|p| crate::MintQuotePayment {
            amount: p.amount.into(),
            time: p.time,
            payment_id: p.payment_id.clone(),
        })
        .collect();

    // Convert issuances to proto
    let issuances: Vec<crate::MintQuoteIssuance> = quote
        .issuance
        .iter()
        .map(|i| crate::MintQuoteIssuance {
            amount: i.amount.into(),
            time: i.time,
        })
        .collect();

    crate::MintQuoteDetail {
        id: quote.id.to_string(),
        amount: quote.amount.map(|a| a.into()),
        unit: quote.unit.to_string(),
        request: quote.request.clone(),
        state: quote.state().to_string(),
        request_lookup_id: Some(quote.request_lookup_id.to_string()),
        request_lookup_id_kind: quote.request_lookup_id.kind().to_string(),
        pubkey: quote.pubkey.map(|pk| pk.to_string()),
        created_time: quote.created_time,
        payments,
        issuances,
        amount_paid: quote.amount_paid().into(),
        amount_issued: quote.amount_issued().into(),
        payment_method: quote.payment_method.to_string(),
    }
}

/// Convert a mint MeltQuote to proto MeltQuote
pub fn melt_quote_to_proto(quote: &MintMeltQuote) -> crate::MeltQuote {
    let options = quote.options.map(|opt| {
        use cdk::nuts::MeltOptions as RustMeltOptions;
        let options = match opt {
            RustMeltOptions::Mpp { mpp } => crate::melt_options::Options::Mpp(crate::MppOptions {
                amount: mpp.amount.into(),
            }),
            RustMeltOptions::Amountless { amountless } => {
                crate::melt_options::Options::Amountless(crate::AmountlessOptions {
                    amount_msat: amountless.amount_msat.into(),
                })
            }
        };
        crate::MeltOptions {
            options: Some(options),
        }
    });

    crate::MeltQuote {
        id: quote.id.to_string(),
        unit: quote.unit.to_string(),
        amount: quote.amount.into(),
        request: quote.request.to_string(),
        fee_reserve: quote.fee_reserve.into(),
        state: quote.state.to_string(),
        payment_preimage: quote.payment_preimage.clone(),
        request_lookup_id: quote.request_lookup_id.as_ref().map(|r| r.to_string()),
        created_time: quote.created_time,
        paid_time: quote.paid_time,
        payment_method: quote.payment_method.to_string(),
        options,
    }
}

/// Convert a ProofRecord to proto Proof
pub fn proof_record_to_proto(proof: &ProofRecord) -> crate::Proof {
    crate::Proof {
        amount: proof.amount.into(),
        keyset_id: proof.keyset_id.to_string(),
        state: proof.state.to_string(),
        quote_id: proof.quote_id.clone(),
        created_time: proof.created_time,
        operation_kind: proof.operation_kind.clone().unwrap_or_default(),
        operation_id: proof.operation_id.clone().unwrap_or_default(),
    }
}

/// Convert a BlindSignatureRecord to proto BlindSignature
pub fn blind_signature_record_to_proto(sig: &BlindSignatureRecord) -> crate::BlindSignature {
    crate::BlindSignature {
        amount: sig.amount.into(),
        keyset_id: sig.keyset_id.to_string(),
        quote_id: sig.quote_id.clone(),
        created_time: sig.created_time,
        signed_time: sig.signed_time,
        operation_kind: sig.operation_kind.clone().unwrap_or_default(),
        operation_id: sig.operation_id.clone().unwrap_or_default(),
    }
}

/// Convert an OperationRecord to proto Operations
pub fn operation_record_to_proto(op: &OperationRecord) -> crate::Operations {
    crate::Operations {
        operation_id: op.operation_id.clone(),
        operation_kind: op.operation_kind.clone(),
        completed_time: op.completed_time,
        total_issued: op.total_issued.into(),
        total_redeemed: op.total_redeemed.into(),
        fee_collected: op.fee_collected.into(),
        payment_amount: op.payment_amount.map(|a| a.into()),
        payment_fee: op.payment_fee.map(|a| a.into()),
        payment_method: op.payment_method.clone(),
        unit: op.unit.clone().unwrap_or_default(),
    }
}

// ============================================================================
// Request Validation Helpers
// ============================================================================

/// Validates unit strings against the mint's actual configured units
///
/// Returns an error if any unit is not configured in the mint's keysets.
pub fn validate_units_against_mint(
    units: &[String],
    mint: &Mint,
) -> Result<Vec<CurrencyUnit>, Status> {
    if units.is_empty() {
        return Ok(Vec::new());
    }

    // Get valid units from mint's keysets (excluding auth keysets)
    let valid_units: HashSet<String> = mint
        .keyset_infos()
        .into_iter()
        .filter(|info| info.unit != CurrencyUnit::Auth)
        .map(|info| info.unit.to_string().to_lowercase())
        .collect();

    let mut parsed = Vec::new();
    let mut invalid = Vec::new();

    for u in units {
        if valid_units.contains(&u.to_lowercase()) {
            // Safe to unwrap - CurrencyUnit::from_str never fails
            parsed.push(CurrencyUnit::from_str(u).unwrap());
        } else {
            invalid.push(u.as_str());
        }
    }

    if invalid.is_empty() {
        Ok(parsed)
    } else {
        let valid_list: Vec<_> = valid_units.into_iter().collect();
        Err(Status::invalid_argument(format!(
            "Invalid unit(s): {}. Valid units for this mint: {}",
            invalid.join(", "),
            valid_list.join(", ")
        )))
    }
}

/// Validates and parses mint quote state strings
pub fn parse_mint_quote_states(states: &[String]) -> Result<Vec<MintQuoteState>, Status> {
    let mut parsed = Vec::new();
    let mut invalid = Vec::new();

    for s in states {
        match MintQuoteState::from_str(s) {
            Ok(state) => parsed.push(state),
            Err(_) => invalid.push(s.as_str()),
        }
    }

    if invalid.is_empty() {
        Ok(parsed)
    } else {
        Err(Status::invalid_argument(format!(
            "Invalid mint quote state(s): {}. Valid states: unpaid, paid, issued, pending",
            invalid.join(", ")
        )))
    }
}

/// Validates and parses melt quote state strings
pub fn parse_melt_quote_states(states: &[String]) -> Result<Vec<MeltQuoteState>, Status> {
    let mut parsed = Vec::new();
    let mut invalid = Vec::new();

    for s in states {
        match MeltQuoteState::from_str(s) {
            Ok(state) => parsed.push(state),
            Err(_) => invalid.push(s.as_str()),
        }
    }

    if invalid.is_empty() {
        Ok(parsed)
    } else {
        Err(Status::invalid_argument(format!(
            "Invalid melt quote state(s): {}. Valid states: unpaid, pending, paid, unknown",
            invalid.join(", ")
        )))
    }
}

/// Validates and parses proof state strings
pub fn parse_proof_states(states: &[String]) -> Result<Vec<State>, Status> {
    let mut parsed = Vec::new();
    let mut invalid = Vec::new();

    for s in states {
        match State::from_str(s) {
            Ok(state) => parsed.push(state),
            Err(_) => invalid.push(s.as_str()),
        }
    }

    if invalid.is_empty() {
        Ok(parsed)
    } else {
        Err(Status::invalid_argument(format!(
            "Invalid proof state(s): {}. Valid states: unspent, spent, pending, reserved",
            invalid.join(", ")
        )))
    }
}

/// Validates and parses operation kind strings
pub fn parse_operations(operations: &[String]) -> Result<Vec<String>, Status> {
    let mut invalid = Vec::new();

    for op in operations {
        if OperationKind::from_str(op).is_err() {
            invalid.push(op.as_str());
        }
    }

    if invalid.is_empty() {
        Ok(operations.to_vec())
    } else {
        Err(Status::invalid_argument(format!(
            "Invalid operation(s): {}. Valid operations: mint, melt, swap",
            invalid.join(", ")
        )))
    }
}

/// Validates and parses keyset ID strings
pub fn parse_keyset_ids(ids: &[String]) -> Result<Vec<Id>, Status> {
    let mut parsed = Vec::new();
    let mut invalid = Vec::new();

    for id in ids {
        match Id::from_str(id) {
            Ok(keyset_id) => parsed.push(keyset_id),
            Err(_) => invalid.push(id.as_str()),
        }
    }

    if invalid.is_empty() {
        Ok(parsed)
    } else {
        Err(Status::invalid_argument(format!(
            "Invalid keyset ID(s): {}",
            invalid.join(", ")
        )))
    }
}

/// Validates pagination parameters
///
/// Returns error if index_offset is provided without a limit (num_max).
pub fn validate_pagination(
    index_offset: i64,
    num_max: i64,
    field_name: &str,
) -> Result<(), Status> {
    if index_offset > 0 && num_max <= 0 {
        return Err(Status::invalid_argument(format!(
            "{} is required when index_offset is provided",
            field_name
        )));
    }
    Ok(())
}

/// Default maximum number of results for list operations when no limit is specified
pub const DEFAULT_MAX_LIMIT: u64 = 100;

/// Returns the effective limit, defaulting to DEFAULT_MAX_LIMIT if not specified
pub fn effective_limit(num_max: i64) -> u64 {
    if num_max > 0 {
        num_max as u64
    } else {
        DEFAULT_MAX_LIMIT
    }
}

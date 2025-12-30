use std::collections::HashMap;

use cdk::mint::Mint;
use cdk::mint::MintQuote as MintMintQuote;
use cdk::nuts::{CurrencyUnit, Id};
use cdk::Amount;
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

/// Convert a mint MintQuote to proto MintQuote
pub fn mint_quote_to_proto(quote: &MintMintQuote) -> crate::MintQuote {
    crate::MintQuote {
        id: quote.id.to_string(),
        amount: quote.amount.map(|a| a.into()),
        unit: quote.unit.to_string(),
        request: quote.request.clone(),
        state: quote.state().to_string(),
        request_lookup_id: Some(quote.request_lookup_id.to_string()),
        request_lookup_id_kind: quote.request_lookup_id.kind().to_string(),
        pubkey: quote.pubkey.map(|pk| pk.to_string()),
        created_time: quote.created_time,
        paid_time: quote.payments.first().map(|p| p.time),
        issued_time: quote.issuance.first().map(|i| i.time),
        amount_paid: quote.amount_paid().into(),
        amount_issued: quote.amount_issued().into(),
        payment_method: quote.payment_method.to_string(),
    }
}

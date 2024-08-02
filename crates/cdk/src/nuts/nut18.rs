//! NUT-18: Melting Tokens Onchain
//!
//! <https://github.com/cashubtc/nuts/blob/main/18.md>

use serde::{Deserialize, Serialize};

use super::nut05::{self, MeltMethodSettings};
use super::CurrencyUnit;
#[cfg(feature = "mint")]
use crate::mint;
use crate::nuts::Proofs;
use crate::Amount;

/// Melt quote request [NUT-18]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteBtcOnchainRequest {
    /// Amount to be paid
    pub amount: Amount,
    /// Bitcoin onchain address to be paid
    pub address: String,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
}

/// Melt quote response [NUT-18]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteBtcOnchainResponse {
    /// Quote Id
    pub quote: String,
    /// Description
    pub amount: Amount,
    /// The fee that is required
    pub fee: Amount,
    /// Whether the the request has been paid
    pub state: nut05::QuoteState,
}

#[cfg(feature = "mint")]
impl From<mint::MeltQuote> for MeltQuoteBtcOnchainResponse {
    fn from(melt_quote: mint::MeltQuote) -> MeltQuoteBtcOnchainResponse {
        MeltQuoteBtcOnchainResponse {
            quote: melt_quote.id,
            amount: melt_quote.amount,
            fee: melt_quote.fee_reserve,
            state: melt_quote.state,
        }
    }
}

/// Melt BTC on chain Request [NUT-18]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltBtcOnchianRequest {
    /// Quote ID
    pub quote: String,
    /// Proofs
    pub inputs: Proofs,
}

impl MeltBtcOnchianRequest {
    /// Total amount of proofs in amounts
    pub fn proofs_amount(&self) -> Amount {
        self.inputs.iter().map(|proof| proof.amount).sum()
    }
}

/// Melt Response [NUT-18]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltBtcOnChainResponse {
    /// Indicate if payment was successful
    pub paid: bool,
    /// TXID
    pub txid: Option<String>,
}

/// Melt Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    methods: Vec<MeltMethodSettings>,
}

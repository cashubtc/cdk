//! Mint types

use bitcoin::bip32::DerivationPath;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::nuts::{MeltQuoteState, MintQuoteState};
use crate::{Amount, CurrencyUnit, Id, KeySetInfo, PublicKey};

/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuote {
    /// Quote id
    pub id: Uuid,
    /// Amount of quote
    pub amount: Amount,
    /// Unit of quote
    pub unit: CurrencyUnit,
    /// Quote payment request e.g. bolt11
    pub request: String,
    /// Quote state
    pub state: MintQuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Value used by ln backend to look up state of request
    pub request_lookup_id: String,
    /// Pubkey
    pub pubkey: Option<PublicKey>,
}

impl MintQuote {
    /// Create new [`MintQuote`]
    pub fn new(
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        expiry: u64,
        request_lookup_id: String,
        pubkey: Option<PublicKey>,
    ) -> Self {
        let id = Uuid::new_v4();

        Self {
            id,
            amount,
            unit,
            request,
            state: MintQuoteState::Unpaid,
            expiry,
            request_lookup_id,
            pubkey,
        }
    }
}

// Melt Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuote {
    /// Quote id
    pub id: Uuid,
    /// Quote unit
    pub unit: CurrencyUnit,
    /// Quote amount
    pub amount: Amount,
    /// Quote Payment request e.g. bolt11
    pub request: String,
    /// Quote fee reserve
    pub fee_reserve: Amount,
    /// Quote state
    pub state: MeltQuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Payment preimage
    pub payment_preimage: Option<String>,
    /// Value used by ln backend to look up state of request
    pub request_lookup_id: String,
    /// Msat to pay
    ///
    /// Used for an amountless invoice
    pub msat_to_pay: Option<Amount>,
}

impl MeltQuote {
    /// Create new [`MeltQuote`]
    pub fn new(
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        fee_reserve: Amount,
        expiry: u64,
        request_lookup_id: String,
        msat_to_pay: Option<Amount>,
    ) -> Self {
        let id = Uuid::new_v4();

        Self {
            id,
            amount,
            unit,
            request,
            fee_reserve,
            state: MeltQuoteState::Unpaid,
            expiry,
            payment_preimage: None,
            request_lookup_id,
            msat_to_pay,
        }
    }
}

/// Mint Keyset Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKeySetInfo {
    /// Keyset [`Id`]
    pub id: Id,
    /// Keyset [`CurrencyUnit`]
    pub unit: CurrencyUnit,
    /// Keyset active or inactive
    /// Mint will only issue new [`BlindSignature`] on active keysets
    pub active: bool,
    /// Starting unix time Keyset is valid from
    pub valid_from: u64,
    /// When the Keyset is valid to
    /// This is not shown to the wallet and can only be used internally
    pub valid_to: Option<u64>,
    /// [`DerivationPath`] keyset
    pub derivation_path: DerivationPath,
    /// DerivationPath index of Keyset
    pub derivation_path_index: Option<u32>,
    /// Max order of keyset
    pub max_order: u8,
    /// Input Fee ppk
    #[serde(default = "default_fee")]
    pub input_fee_ppk: u64,
}

/// Default fee
pub fn default_fee() -> u64 {
    0
}

impl From<MintKeySetInfo> for KeySetInfo {
    fn from(keyset_info: MintKeySetInfo) -> Self {
        Self {
            id: keyset_info.id,
            unit: keyset_info.unit,
            active: keyset_info.active,
            input_fee_ppk: keyset_info.input_fee_ppk,
        }
    }
}

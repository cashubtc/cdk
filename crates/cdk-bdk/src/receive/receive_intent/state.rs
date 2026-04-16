//! Typestate markers for ReceiveIntent state transitions

/// Marker for a detected incoming UTXO awaiting confirmation
#[derive(Debug, Clone)]
pub struct Detected {
    /// Quote ID linking this intent to a mint quote
    pub quote_id: String,
    /// Bitcoin address that received the payment
    pub address: String,
    /// Transaction ID containing the payment
    pub txid: String,
    /// Outpoint string (txid:vout) identifying the specific UTXO
    pub outpoint: String,
    /// Payment amount in satoshis
    pub amount_sat: u64,
    /// Block height at which the UTXO was first detected
    #[allow(dead_code)]
    pub block_height: u32,
}

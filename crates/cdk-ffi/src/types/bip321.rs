//! FFI-compatible types for BIP 321 payment instruction helpers.
//!
//! These types represent the result of parsing a BIP 321 `bitcoin:` URI (or a
//! standalone payment string) via
//! [`parse_bip321_payment_instruction`](crate::bip321::parse_bip321_payment_instruction).

use std::sync::Arc;

use cdk_common::bitcoin;

use super::payment_request::PaymentRequest;

/// Bitcoin network for on-chain address validation.
///
/// This determines which address prefixes are accepted when parsing a BIP 321
/// `bitcoin:` URI that contains an on-chain component.
///
/// ```text
/// val parsed = parseBip321PaymentInstruction(
///     "bitcoin:bc1qar0s...?creq=CREQB1...",
///     BitcoinNetwork.BITCOIN  // mainnet addresses only
/// )
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BitcoinNetwork {
    /// Bitcoin mainnet (addresses start with `bc1`, `1`, or `3`).
    Bitcoin,
    /// Bitcoin testnet (addresses start with `tb1`, `m`, or `n`).
    Testnet,
    /// Bitcoin signet (addresses start with `tb1`).
    Signet,
    /// Bitcoin regtest (addresses start with `bcrt1`).
    Regtest,
}

impl From<BitcoinNetwork> for bitcoin::Network {
    fn from(network: BitcoinNetwork) -> Self {
        match network {
            BitcoinNetwork::Bitcoin => bitcoin::Network::Bitcoin,
            BitcoinNetwork::Testnet => bitcoin::Network::Testnet,
            BitcoinNetwork::Signet => bitcoin::Network::Signet,
            BitcoinNetwork::Regtest => bitcoin::Network::Regtest,
        }
    }
}

impl From<bitcoin::Network> for BitcoinNetwork {
    fn from(network: bitcoin::Network) -> Self {
        match network {
            bitcoin::Network::Bitcoin => BitcoinNetwork::Bitcoin,
            bitcoin::Network::Testnet => BitcoinNetwork::Testnet,
            bitcoin::Network::Signet => BitcoinNetwork::Signet,
            bitcoin::Network::Regtest => BitcoinNetwork::Regtest,
            _ => BitcoinNetwork::Bitcoin,
        }
    }
}

/// A parsed BIP 321 payment instruction containing all payment methods found.
///
/// After parsing, inspect the lists to determine which payment methods are
/// available and choose the best one for your wallet. A single URI can contain
/// multiple methods (e.g. cashu + BOLT11 + on-chain) to give the payer options.
///
/// # Examples
///
/// ```text
/// // Parse a BIP 321 URI that bundles cashu, BOLT11, and an on-chain address
/// val parsed = parseBip321PaymentInstruction(
///     "bitcoin:bc1qar0s...?creq=CREQB1...&lightning=lnbc100n1p..."
/// )
///
/// // Check which payment methods are available and pick one
/// when {
///     parsed.cashuRequests.isNotEmpty() -> {
///         // Prefer ecash: instant settlement, zero fees
///         val request = parsed.cashuRequests.first()
///         val id = request.paymentId()         // e.g. "b7a90176"
///         val amount = request.amount()         // e.g. Amount(10)
///         val unit = request.unit()             // e.g. CurrencyUnit.Sat
///         val mints = request.mints()           // acceptable mint URLs
///         val transports = request.transports() // how to deliver proofs
///     }
///     parsed.bolt11Invoices.isNotEmpty() -> {
///         // Fall back to Lightning BOLT11
///         val invoice = parsed.bolt11Invoices.first()
///     }
///     parsed.bolt12Offers.isNotEmpty() -> {
///         // Fall back to Lightning BOLT12
///         val offer = parsed.bolt12Offers.first()
///     }
///     parsed.onchainAddresses.isNotEmpty() -> {
///         // Last resort: on-chain payment
///         val address = parsed.onchainAddresses.first()
///     }
/// }
///
/// // Amount info
/// val msats = parsed.amountMsats           // fixed amount in msats, or null
/// val flexible = parsed.isConfigurableAmount // true if payer chooses amount
/// val desc = parsed.description             // URI label/message, or null
/// ```
#[derive(Debug, Clone, uniffi::Record)]
pub struct ParsedPaymentInstruction {
    /// Cashu NUT-26 payment requests.
    pub cashu_requests: Vec<Arc<PaymentRequest>>,
    /// BOLT11 invoice strings.
    pub bolt11_invoices: Vec<String>,
    /// BOLT12 offer strings.
    pub bolt12_offers: Vec<String>,
    /// On-chain bitcoin addresses.
    pub onchain_addresses: Vec<String>,
    /// Description / label / message from the URI.
    pub description: Option<String>,
    /// Amount in millisatoshis (if a fixed-amount instruction).
    pub amount_msats: Option<u64>,
    /// Whether the amount is configurable (vs fixed).
    pub is_configurable_amount: bool,
}

impl From<cdk::wallet::bip321::ParsedPaymentInstruction> for ParsedPaymentInstruction {
    fn from(parsed: cdk::wallet::bip321::ParsedPaymentInstruction) -> Self {
        let cashu_requests = parsed
            .cashu_requests
            .into_iter()
            .map(|req| Arc::new(PaymentRequest::from_inner(req)))
            .collect();

        Self {
            cashu_requests,
            bolt11_invoices: parsed.bolt11_invoices,
            bolt12_offers: parsed.bolt12_offers,
            onchain_addresses: parsed.onchain_addresses,
            description: parsed.description,
            amount_msats: parsed.amount_msats,
            is_configurable_amount: parsed.is_configurable_amount,
        }
    }
}

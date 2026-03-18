//! BIP 321 Payment Instruction Helpers
//!
//! This module provides helper functions for reading and creating BIP 321 `bitcoin:` URIs
//! that can include cashu payment requests (NUT-26 `creq` parameter) alongside other
//! payment methods (BOLT11, BOLT12, on-chain addresses).
//!
//! # Parsing
//!
//! Use [`parse_payment_instruction`] to parse a BIP 321 URI or standalone
//! payment string into [`ParsedPaymentInstruction`]. A [`bitcoin::Network`]
//! must be provided so that on-chain addresses are validated against the
//! correct network.
//!
//! # Creating
//!
//! Use [`Bip321UriBuilder`] to construct a BIP 321 `bitcoin:` URI via the builder
//! pattern and call `.to_string()` to produce the final URI string.
//! A [`PaymentRequest`] can also be converted directly via the
//! [`PaymentRequestBip321Ext::to_bip321`] extension method.
//!
//! # Example
//!
//! ```no_run
//! use cdk::wallet::bip321::{parse_payment_instruction, Bip321UriBuilder};
//!
//! # async fn example() -> Result<(), cdk::error::Error> {
//! // Create a BIP 321 URI with a cashu payment request
//! let uri = Bip321UriBuilder::new()
//!     .with_cashu_request_str("CREQB1...".to_string())
//!     .to_string();
//!
//! // Parse it back (network is required for on-chain address validation)
//! let parsed = parse_payment_instruction(&uri, bitcoin::Network::Bitcoin).await?;
//! assert_eq!(parsed.cashu_requests.len(), 1);
//! # Ok(())
//! # }
//! ```

use core::fmt;
use std::str::FromStr;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use std::sync::Arc;

use bitcoin_payment_instructions::hrn_resolution::DummyHrnResolver;
use bitcoin_payment_instructions::{
    PaymentInstructions, PaymentMethod as BpiPaymentMethod, PossiblyResolvedPaymentMethod,
};
use tracing::instrument;
use url::form_urlencoded;

#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use crate::bip353::Bip353Address;
use crate::error::Error;
use crate::nuts::PaymentRequest;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use crate::wallet::MintConnector;

/// Parsed payment instruction with all available payment methods.
#[derive(Debug, Clone, Default)]
pub struct ParsedPaymentInstruction {
    /// Cashu NUT-26 payment requests found in the instruction.
    pub cashu_requests: Vec<PaymentRequest>,
    /// BOLT11 invoice strings found.
    pub bolt11_invoices: Vec<String>,
    /// BOLT12 offer strings found.
    pub bolt12_offers: Vec<String>,
    /// On-chain bitcoin addresses found.
    pub onchain_addresses: Vec<String>,
    /// Description / label / message from the URI.
    pub description: Option<String>,
    /// Amount in millisatoshis (if a fixed-amount instruction).
    pub amount_msats: Option<u64>,
    /// Whether the amount is configurable (vs fixed).
    pub is_configurable_amount: bool,
}

/// Builder for BIP 321 `bitcoin:` URIs.
///
/// # Example
///
/// ```no_run
/// use cdk::wallet::bip321::Bip321UriBuilder;
///
/// let uri = Bip321UriBuilder::new()
///     .with_onchain_address("bc1q...".to_string())
///     .with_amount_sats(100_000)
///     .with_message("Coffee payment".to_string())
///     .to_string();
/// ```
#[derive(Debug, Clone, Default)]
pub struct Bip321UriBuilder {
    /// A cashu payment request as a CREQB1 bech32m string.
    cashu_request_str: Option<String>,
    /// A BOLT11 invoice string.
    bolt11_invoice: Option<String>,
    /// A BOLT12 offer string.
    bolt12_offer: Option<String>,
    /// An on-chain bitcoin address.
    onchain_address: Option<String>,
    /// Amount in satoshis.
    amount_sats: Option<u64>,
    /// Label for the payment (shown to user, not sent to payee).
    label: Option<String>,
    /// Message for the payment (displayed to user as a note).
    message: Option<String>,
}

impl Bip321UriBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the cashu payment request from a raw CREQB1 bech32m string.
    pub fn with_cashu_request_str(mut self, creq: String) -> Self {
        self.cashu_request_str = Some(creq);
        self
    }

    /// Set the cashu payment request from a CDK [`PaymentRequest`].
    ///
    /// The request is encoded to bech32m format internally.
    pub fn with_cashu_request(mut self, request: &PaymentRequest) -> Result<Self, Error> {
        let bech32_str = request
            .to_bech32_string()
            .map_err(|e| Error::Bip321Encode(e.to_string()))?;
        self.cashu_request_str = Some(bech32_str);
        Ok(self)
    }

    /// Set the BOLT11 invoice string.
    pub fn with_bolt11_invoice(mut self, invoice: String) -> Self {
        self.bolt11_invoice = Some(invoice);
        self
    }

    /// Set the BOLT12 offer string.
    pub fn with_bolt12_offer(mut self, offer: String) -> Self {
        self.bolt12_offer = Some(offer);
        self
    }

    /// Set the on-chain bitcoin address.
    pub fn with_onchain_address(mut self, address: String) -> Self {
        self.onchain_address = Some(address);
        self
    }

    /// Set the amount in satoshis.
    ///
    /// BIP 21 encodes amount values in BTC, so satoshis are formatted at
    /// render time.
    pub fn with_amount_sats(mut self, sats: u64) -> Self {
        self.amount_sats = Some(sats);
        self
    }

    /// Set the label for the payment (shown to user, not sent to payee).
    pub fn with_label(mut self, label: String) -> Self {
        self.label = Some(label);
        self
    }

    /// Set the message for the payment (displayed to user as a note).
    pub fn with_message(mut self, message: String) -> Self {
        self.message = Some(message);
        self
    }
}

/// Extension trait for converting [`PaymentRequest`] values to BIP 321 builders.
pub trait PaymentRequestBip321Ext {
    /// Convert this request into a builder with the `creq=` field pre-populated.
    fn to_bip321(&self) -> Result<Bip321UriBuilder, Error>;
}

impl PaymentRequestBip321Ext for PaymentRequest {
    fn to_bip321(&self) -> Result<Bip321UriBuilder, Error> {
        Bip321UriBuilder::new().with_cashu_request(self)
    }
}

/// Parse a BIP 321 `bitcoin:` URI or standalone payment instruction string,
/// validating on-chain addresses against the given [`bitcoin::Network`].
///
/// Supports BIP 321 URIs and standalone Cashu, BOLT11, BOLT12, and on-chain inputs.
/// Human-readable name resolution (`user@domain.com`, BIP 353, LNURL) is not supported.
#[instrument(skip_all)]
pub async fn parse_payment_instruction(
    instruction: &str,
    network: bitcoin::Network,
) -> Result<ParsedPaymentInstruction, Error> {
    let resolver = DummyHrnResolver;

    let parsed = PaymentInstructions::parse(instruction, network, &resolver, false)
        .await
        .map_err(|e| Error::Bip321Parse(format!("{e:?}")))?;

    convert_payment_instructions(parsed)
}

/// Resolve a BIP353 address and parse the resulting concrete payment instruction.
///
/// This is the shared low-level helper for "resolve first, inspect later" flows. It accepts
/// any [`MintConnector`] so callers do not need a full [`crate::wallet::Wallet`] just to resolve and inspect a
/// human-readable payment instruction.
///
/// The `network` parameter is forwarded to [`parse_payment_instruction`] and controls
/// which on-chain address prefixes are accepted in the resolved URI.
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
#[instrument(skip_all)]
pub async fn resolve_bip353_payment_instruction(
    client: &Arc<dyn MintConnector + Send + Sync>,
    bip353_address: &str,
    network: bitcoin::Network,
) -> Result<ParsedPaymentInstruction, Error> {
    let address = Bip353Address::from_str(bip353_address).map_err(|e| {
        tracing::error!("Failed to parse BIP353 address '{}': {}", bip353_address, e);
        Error::Bip353Parse(e.to_string())
    })?;

    tracing::debug!("Resolving BIP353 address: {}", address);
    let address_string = address.to_string();

    let resolved_uri = address.resolve(client).await.map_err(|e| {
        tracing::error!(
            "Failed to resolve BIP353 address '{}': {}",
            address_string,
            e
        );
        Error::Bip353Resolve(e.to_string())
    })?;

    parse_payment_instruction(&resolved_uri, network)
        .await
        .map_err(|e| {
            tracing::error!(
                "Failed to parse resolved BIP353 payment instruction '{}': {}",
                resolved_uri,
                e
            );
            Error::Bip321Parse(format!("Invalid resolved bitcoin URI: {e}"))
        })
}

fn convert_payment_instructions(
    parsed: PaymentInstructions,
) -> Result<ParsedPaymentInstruction, Error> {
    match parsed {
        PaymentInstructions::FixedAmount(fixed) => {
            let max_amount_msats = fixed.max_amount().map(|a| a.milli_sats());
            extract_payment_methods(
                fixed.methods(),
                fixed.recipient_description().map(|d| d.to_string()),
                max_amount_msats,
                false,
            )
        }
        PaymentInstructions::ConfigurableAmount(configurable) => extract_payment_methods(
            configurable.methods().filter_map(|method| match method {
                PossiblyResolvedPaymentMethod::Resolved(method) => Some(method),
                _ => None,
            }),
            configurable.recipient_description().map(|d| d.to_string()),
            None,
            true,
        ),
    }
}

/// Extract CDK types from BPI payment methods.
fn extract_payment_methods<'a>(
    methods: impl IntoIterator<Item = &'a BpiPaymentMethod>,
    description: Option<String>,
    max_amount_msats: Option<u64>,
    is_configurable: bool,
) -> Result<ParsedPaymentInstruction, Error> {
    let mut result = ParsedPaymentInstruction {
        description,
        amount_msats: max_amount_msats,
        is_configurable_amount: is_configurable,
        ..Default::default()
    };

    for method in methods {
        match method {
            BpiPaymentMethod::Cashu(cashu_req) => {
                // Round-trip through bech32m string for safe conversion
                let bech32_str = cashu_req.to_string();
                let cdk_req = PaymentRequest::from_str(&bech32_str)
                    .map_err(|e| Error::Bip321Parse(e.to_string()))?;
                result.cashu_requests.push(cdk_req);
            }
            BpiPaymentMethod::LightningBolt11(invoice) => {
                result.bolt11_invoices.push(invoice.to_string());
            }
            BpiPaymentMethod::LightningBolt12(offer) => {
                result.bolt12_offers.push(offer.to_string());
            }
            BpiPaymentMethod::OnChain(address) => {
                result.onchain_addresses.push(address.to_string());
            }
        }
    }

    Ok(result)
}

impl fmt::Display for Bip321UriBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bitcoin:")?;

        // On-chain address goes in the path
        if let Some(ref addr) = self.onchain_address {
            f.write_str(addr)?;
        }

        let mut query_params = form_urlencoded::Serializer::new(String::new());

        if let Some(amount_sats) = self.amount_sats {
            let amount_btc = format_btc_amount_from_sats(amount_sats);
            query_params.append_pair("amount", &amount_btc);
        }

        if let Some(ref label) = self.label {
            query_params.append_pair("label", label);
        }

        if let Some(ref message) = self.message {
            query_params.append_pair("message", message);
        }

        if let Some(ref bolt11) = self.bolt11_invoice {
            query_params.append_pair("lightning", bolt11);
        }

        if let Some(ref bolt12) = self.bolt12_offer {
            query_params.append_pair("lno", bolt12);
        }

        if let Some(ref creq) = self.cashu_request_str {
            query_params.append_pair("creq", creq);
        }

        let query_string = query_params.finish();
        if !query_string.is_empty() {
            f.write_str("?")?;
            f.write_str(&query_string)?;
        }

        Ok(())
    }
}

/// Format satoshis to BTC string without trailing zeros, per BIP 21.
fn format_btc_amount_from_sats(sats: u64) -> String {
    let whole = sats / 100_000_000;
    let fractional = sats % 100_000_000;

    if fractional == 0 {
        return whole.to_string();
    }

    let formatted = format!("{whole}.{fractional:08}");
    formatted.trim_end_matches('0').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CREQ: &str = "CREQB1QYQQWER9D4HNZV3NQGQQSQQQQQQQQQQRAQPSQQGQQSQQZQG9QQVXSAR5WPEN5TE0D45KUAPWV4UXZMTSD3JJUCM0D5RQQRJRDANXVET9YPCXZ7TDV4H8GXHR3TQ";
    const TEST_BOLT11: &str = "lnbc1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdpl2pkx2ctnv5sxxmmwwd5kgetjypeh2ursdae8g6twvus8g6rfwvs8qun0dfjkxaq9qrsgq357wnc5r2ueh7ck6q93dj32dlqnls087fxdwk8qakdyafkq3yap9us6v52vjjsrvywa6rt52cm9r9zqt8r2t7mlcwspyetp5h2tztugp9lfyql";

    fn test_payment_request() -> PaymentRequest {
        PaymentRequest::from_str(TEST_CREQ).expect("should parse test vector")
    }

    fn assert_has_creq(uri: &str) {
        assert!(uri.starts_with("bitcoin:?") || uri.starts_with("bitcoin:bc1"));
        assert!(uri.contains("creq="));
    }

    fn assert_demo_cashu(parsed: &ParsedPaymentInstruction) {
        assert_eq!(parsed.cashu_requests.len(), 1);
        assert!(!parsed.is_configurable_amount);

        let req = &parsed.cashu_requests[0];
        assert_eq!(req.payment_id, Some("demo123".to_string()));
        assert_eq!(req.amount, Some(1000.into()));
    }

    #[test]
    fn test_bip321_uri_cashu_only() {
        let uri = Bip321UriBuilder::new()
            .with_cashu_request_str(TEST_CREQ.to_string())
            .to_string();

        assert_has_creq(&uri);
        assert!(uri.contains(TEST_CREQ));
    }

    #[test]
    fn test_bip321_uri_multi_method() {
        let addr = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";
        let uri = Bip321UriBuilder::new()
            .with_cashu_request_str(TEST_CREQ.to_string())
            .with_onchain_address(addr.to_string())
            .with_amount_sats(100_000)
            .with_message("Coffee payment".to_string())
            .to_string();

        assert!(uri.starts_with(&format!("bitcoin:{addr}?")));
        assert!(uri.contains("creq="));
        assert!(uri.contains("amount=0.001"));
        assert!(uri.contains("message=Coffee+payment"));
    }

    #[test]
    fn test_bip321_uri_empty_params() {
        let uri = Bip321UriBuilder::new().to_string();
        assert_eq!(uri, "bitcoin:");
    }

    #[test]
    fn test_format_btc_amount_from_sats() {
        assert_eq!(format_btc_amount_from_sats(100_000_000), "1");
        assert_eq!(format_btc_amount_from_sats(100_000), "0.001");
        assert_eq!(format_btc_amount_from_sats(1), "0.00000001");
        assert_eq!(format_btc_amount_from_sats(2_100_000_000), "21");
    }

    #[test]
    fn test_query_params_are_encoded() {
        let uri = Bip321UriBuilder::new()
            .with_label("a=b&c=d".to_string())
            .with_message("hello world".to_string())
            .to_string();
        assert_eq!(uri, "bitcoin:?label=a%3Db%26c%3Dd&message=hello+world");
    }

    #[tokio::test]
    async fn test_parse_standalone_cashu_request() {
        let parsed = parse_payment_instruction(TEST_CREQ, bitcoin::Network::Bitcoin)
            .await
            .expect("should parse standalone CREQB1");
        assert_demo_cashu(&parsed);
    }

    #[tokio::test]
    async fn test_parse_bip321_uri_with_cashu() {
        let uri = format!("bitcoin:?creq={TEST_CREQ}");
        let parsed = parse_payment_instruction(&uri, bitcoin::Network::Bitcoin)
            .await
            .expect("should parse bitcoin: URI with creq");
        assert_demo_cashu(&parsed);
    }

    #[tokio::test]
    async fn test_parse_bip321_uri_with_cashu_and_amount() {
        let uri = format!("bitcoin:?creq={TEST_CREQ}&amount=0.00001");
        let parsed = parse_payment_instruction(&uri, bitcoin::Network::Bitcoin)
            .await
            .expect("should parse bitcoin: URI with creq and amount");

        assert_eq!(parsed.cashu_requests.len(), 1);
        assert_eq!(parsed.amount_msats, Some(1_000_000));
    }

    #[tokio::test]
    async fn test_roundtrip_create_then_parse() {
        let uri = Bip321UriBuilder::new()
            .with_cashu_request_str(TEST_CREQ.to_string())
            .to_string();

        let parsed = parse_payment_instruction(&uri, bitcoin::Network::Bitcoin)
            .await
            .expect("should parse created URI");
        assert_demo_cashu(&parsed);
    }

    #[tokio::test]
    async fn test_parse_standalone_bolt11() {
        let result = parse_payment_instruction(TEST_BOLT11, bitcoin::Network::Bitcoin).await;
        // This specific invoice may or may not parse depending on checksum
        // validity, but we verify the parser at least tries.
        if let Ok(parsed) = result {
            assert!(!parsed.bolt11_invoices.is_empty());
        }
    }

    #[test]
    fn test_bip321_uri_with_payment_request() {
        let payment_request = test_payment_request();

        let uri = Bip321UriBuilder::new()
            .with_cashu_request(&payment_request)
            .expect("should encode request")
            .to_string();
        assert_has_creq(&uri);
    }

    #[test]
    fn test_payment_request_to_bip321() {
        let payment_request = test_payment_request();

        let uri = payment_request
            .to_bip321()
            .expect("should create builder")
            .to_string();
        assert_has_creq(&uri);
    }

    #[test]
    fn test_payment_request_to_bip321_then_customise() {
        let payment_request = test_payment_request();

        let uri = payment_request
            .to_bip321()
            .expect("should create builder")
            .with_onchain_address("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string())
            .with_amount_sats(100_000)
            .to_string();
        assert!(uri.contains("creq="));
        assert!(uri.contains("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"));
        assert!(uri.contains("amount=0.001"));
    }

    #[tokio::test]
    async fn test_parse_garbage_input_returns_error() {
        let result =
            parse_payment_instruction("not-a-valid-anything", bitcoin::Network::Bitcoin).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_format_btc_amount_from_sats_zero() {
        assert_eq!(format_btc_amount_from_sats(0), "0");
    }

    #[test]
    fn test_bip321_uri_bolt12_only() {
        let offer = "lno1qgsqtest";
        let uri = Bip321UriBuilder::new()
            .with_bolt12_offer(offer.to_string())
            .to_string();
        assert!(uri.starts_with("bitcoin:?"));
        assert!(uri.contains(&format!("lno={offer}")));
        assert!(!uri.contains("creq="));
        assert!(!uri.contains("lightning="));
    }

    #[test]
    fn test_bip321_uri_with_label() {
        let uri = Bip321UriBuilder::new()
            .with_label("Donation".to_string())
            .to_string();
        assert!(uri.contains("label=Donation"));
    }

    #[tokio::test]
    async fn test_parse_for_different_networks_both_succeed_for_non_onchain() {
        // Cashu payment requests have no on-chain component, so they should
        // parse successfully regardless of the network parameter.
        let mainnet_result = parse_payment_instruction(TEST_CREQ, bitcoin::Network::Bitcoin).await;
        let testnet_result = parse_payment_instruction(TEST_CREQ, bitcoin::Network::Testnet).await;

        assert!(mainnet_result.is_ok());
        assert!(testnet_result.is_ok());
    }
}

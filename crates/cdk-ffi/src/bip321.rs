//! BIP 321 payment instruction FFI bindings.
//!
//! Provides helpers to create and parse `bitcoin:` URIs containing Cashu
//! (`creq`), BOLT11 (`lightning`), BOLT12 (`lno`), and on-chain methods.

use cdk::wallet::bip321::Bip321UriBuilder;
use tracing::instrument;

use crate::error::FfiError;
use crate::types::bip321::{BitcoinNetwork, ParsedPaymentInstruction};

/// Apply optional BOLT11 and BOLT12 parameters to a [`Bip321UriBuilder`].
pub(crate) fn apply_optional_lightning_methods(
    mut builder: Bip321UriBuilder,
    bolt11: Option<String>,
    bolt12: Option<String>,
) -> Bip321UriBuilder {
    if let Some(invoice) = bolt11 {
        builder = builder.with_bolt11_invoice(invoice);
    }
    if let Some(offer) = bolt12 {
        builder = builder.with_bolt12_offer(offer);
    }

    builder
}

/// Create a BIP 321 `bitcoin:` URI from raw string components.
///
/// Combines optional `creq`, `lightning`, and `lno` query parameters into a
/// single URI without requiring a `PaymentRequest` object.
///
/// ```text
/// val uri = createBip321Uri(
///     creq = "CREQB1...",
///     bolt11 = "lnbc100n1p...",
///     bolt12 = "lno1qgsq..."
/// )
/// // => "bitcoin:?creq=CREQB1...&lightning=lnbc100n1p...&lno=lno1qgsq..."
/// ```
#[uniffi::export]
pub fn create_bip321_uri(
    creq: Option<String>,
    bolt11: Option<String>,
    bolt12: Option<String>,
) -> String {
    let mut builder = Bip321UriBuilder::new();
    if let Some(creq) = creq {
        builder = builder.with_cashu_request_str(creq);
    }
    builder = apply_optional_lightning_methods(builder, bolt11, bolt12);
    builder.to_string()
}

/// Parse a BIP 321 `bitcoin:` URI or standalone payment instruction string,
/// validating on-chain addresses against the given [`bitcoin::Network`].
///
/// Returns a [`ParsedPaymentInstruction`] from a BIP 321 URI or standalone
/// payment string (Cashu, BOLT11, BOLT12, or on-chain address).
///
/// ```text
/// val parsed = parseBip321PaymentInstruction(
///     "bitcoin:?creq=CREQB1...&lightning=lnbc100n1p...",
///     Network.BITCOIN
/// )
/// // parsed.cashuRequests and parsed.bolt11Invoices are populated when present
/// ```
#[uniffi::export(async_runtime = "tokio")]
#[instrument(skip_all)]
pub async fn parse_bip321_payment_instruction(
    instruction: String,
    network: BitcoinNetwork,
) -> Result<ParsedPaymentInstruction, FfiError> {
    let parsed =
        cdk::wallet::bip321::parse_payment_instruction(&instruction, network.into()).await?;
    Ok(parsed.into())
}

/// Resolve a BIP353 human-readable address into a parsed payment instruction.
///
/// This uses the wallet's configured connector to resolve the address, then parses the resolved
/// `bitcoin:` URI into a [`ParsedPaymentInstruction`] so callers can inspect the available
/// methods before deciding how to pay or whether a BIP353 melt is possible.
///
/// The `network` parameter controls which on-chain address prefixes are accepted
/// in the resolved URI.
#[cfg(not(target_arch = "wasm32"))]
#[uniffi::export(async_runtime = "tokio")]
#[instrument(skip_all)]
pub async fn resolve_bip353_payment_instruction(
    wallet: std::sync::Arc<crate::wallet::Wallet>,
    address: String,
    network: BitcoinNetwork,
) -> Result<ParsedPaymentInstruction, FfiError> {
    let client = wallet.inner().mint_connector();
    let parsed =
        cdk::wallet::resolve_bip353_payment_instruction(&client, &address, network.into()).await?;
    Ok(parsed.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CREQ: &str = "CREQB1QYQQWER9D4HNZV3NQGQQSQQQQQQQQQQRAQPSQQGQQSQQZQG9QQVXSAR5WPEN5TE0D45KUAPWV4UXZMTSD3JJUCM0D5RQQRJRDANXVET9YPCXZ7TDV4H8GXHR3TQ";
    const TEST_BOLT11: &str = "lnbc100n1ptest";

    fn assert_uri_prefix(uri: &str) {
        assert!(uri.starts_with("bitcoin:?"));
    }

    #[test]
    fn test_create_bip321_uri_cashu_and_bolt11() {
        let uri = create_bip321_uri(
            Some(TEST_CREQ.to_string()),
            Some(TEST_BOLT11.to_string()),
            None,
        );

        assert_uri_prefix(&uri);
        assert!(uri.contains(&format!("creq={TEST_CREQ}")));
        assert!(uri.contains(&format!("lightning={TEST_BOLT11}")));
    }

    #[test]
    fn test_create_bip321_uri_cashu_only() {
        let uri = create_bip321_uri(Some(TEST_CREQ.to_string()), None, None);

        assert_uri_prefix(&uri);
        assert!(uri.contains("creq="));
        assert!(!uri.contains("lightning="));
        assert!(!uri.contains("lno="));
    }

    #[test]
    fn test_create_bip321_uri_bolt11_only() {
        let uri = create_bip321_uri(None, Some(TEST_BOLT11.to_string()), None);

        assert_uri_prefix(&uri);
        assert!(uri.contains(&format!("lightning={TEST_BOLT11}")));
        assert!(!uri.contains("creq="));
    }

    #[test]
    fn test_create_bip321_uri_all_methods() {
        let bolt12 = "lno1qgsqtest";

        let uri = create_bip321_uri(
            Some(TEST_CREQ.to_string()),
            Some(TEST_BOLT11.to_string()),
            Some(bolt12.to_string()),
        );

        assert_uri_prefix(&uri);
        assert!(uri.contains("creq="));
        assert!(uri.contains("lightning="));
        assert!(uri.contains("lno="));
    }

    #[test]
    fn test_create_bip321_uri_empty() {
        let uri = create_bip321_uri(None, None, None);
        assert_eq!(uri, "bitcoin:");
    }
}

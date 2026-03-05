//! Melt Lightning Address
//!
//! Implementation of melt functionality for Lightning addresses

use std::str::FromStr;

use cdk_common::wallet::MeltQuote;
use tracing::instrument;

use crate::lightning_address::LightningAddress;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Melt Quote for Lightning address
    ///
    /// This method resolves a Lightning address (e.g., "alice@example.com") to a Lightning invoice
    /// and then creates a melt quote for that invoice.
    ///
    /// # Arguments
    ///
    /// * `lightning_address` - Lightning address in the format "user@domain.com"
    /// * `amount_msat` - Amount to pay in millisatoshis
    ///
    /// # Returns
    ///
    /// A `MeltQuote` that can be used to execute the payment
    ///
    /// # Errors
    ///
    /// This method will return an error if:
    /// - The Lightning address format is invalid
    /// - HTTP request to the Lightning address service fails
    /// - The amount is outside the acceptable range
    /// - The service returns an error
    /// - The mint fails to provide a quote for the invoice
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use cdk::Amount;
    /// # use cdk::Wallet;
    /// # async fn example(wallet: Wallet) -> Result<(), cdk::Error> {
    /// let quote = wallet
    ///     .melt_lightning_address_quote("alice@example.com", Amount::from(100_000)) // 100 sats in msat
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self, amount_msat), fields(lightning_address = %lightning_address))]
    pub async fn melt_lightning_address_quote(
        &self,
        lightning_address: &str,
        amount_msat: impl Into<Amount>,
    ) -> Result<MeltQuote, Error> {
        let amount = amount_msat.into();

        // Parse the Lightning address
        let ln_address = LightningAddress::from_str(lightning_address).map_err(|e| {
            tracing::error!(
                "Failed to parse Lightning address '{}': {}",
                lightning_address,
                e
            );
            Error::LightningAddressParse(e.to_string())
        })?;

        tracing::debug!("Resolving Lightning address: {}", ln_address);

        // Request an invoice from the Lightning address service
        let invoice = ln_address
            .request_invoice(&self.client, amount)
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to get invoice from Lightning address service: {}",
                    e
                );
                Error::LightningAddressRequest(e.to_string())
            })?;

        tracing::debug!(
            "Received invoice from Lightning address service: {}",
            invoice
        );

        // Create a melt quote for the invoice using the existing bolt11 functionality
        // The invoice from LNURL already contains the amount, so we don't need amountless options
        self.melt_bolt11_quote(invoice.to_string(), None).await
    }
}

#[cfg(all(test, feature = "wallet", not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;

    use cdk_common::database::WalletDatabase;

    use super::*;
    use crate::mint_url::MintUrl;
    use crate::nuts::{CurrencyUnit, MeltQuoteBolt11Response, MeltQuoteState};
    use crate::wallet::test_utils::MockMintConnector;
    use crate::wallet::WalletBuilder;

    const INVOICE_1000_SATS: &str = "lnbc10u1p3xtswzsp5a0pjcg5t042q3lk4mvjqv3x3ea8m9w2grswlxqk6dwlj6x4spphsdqqcqzzsxqyz5vqsp5f4w7aw8g0n7v9j4hrjz8fll2gk7wgpk9v7s0x3t8xmtt4f25mh9qxpqysgqfeh44plf5n2m4gq2a4v0y5ngd8lgfz2g06kknk2pu4v772ma3xjxfugm6mh8vk0j9j2qlenhtj5w0q0td5j0g2vm0r6zv0v2fsz0u4qqr28j6g";

    async fn test_wallet_with_connector(connector: Arc<MockMintConnector>) -> Wallet {
        let db = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("memory db"),
        ) as Arc<dyn WalletDatabase<_> + Send + Sync>;
        let seed = [1; 64];

        WalletBuilder::new()
            .mint_url(MintUrl::from_str("https://mint.example.com").expect("valid mint url"))
            .unit(CurrencyUnit::Sat)
            .localstore(db)
            .seed(seed)
            .shared_client(connector)
            .build()
            .expect("wallet builds")
    }

    #[tokio::test]
    async fn test_melt_lightning_address_quote_rejects_mismatched_invoice_amount() {
        let connector = Arc::new(MockMintConnector::new());
        connector.set_lnurl_pay_request_response(Ok(crate::lightning_address::LnurlPayResponse {
            callback: "https://example.com/callback".to_string(),
            min_sendable: 1,
            max_sendable: 2_000_000,
            metadata: "[]".to_string(),
            tag: Some("payRequest".to_string()),
            reason: None,
        }));
        connector.set_lnurl_invoice_response(Ok(
            crate::lightning_address::LnurlPayInvoiceResponse {
                pr: Some(INVOICE_1000_SATS.to_string()),
                success_action: None,
                routes: None,
                reason: None,
            },
        ));
        connector.set_bolt11_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            quote: "quote-should-not-be-used".to_string(),
            amount: Amount::from(1_000_u64),
            fee_reserve: Amount::from(0_u64),
            state: MeltQuoteState::Unpaid,
            expiry: 0,
            payment_preimage: None,
            change: None,
            request: None,
            unit: None,
        }));

        let wallet = test_wallet_with_connector(connector.clone()).await;
        let error = wallet
            .melt_lightning_address_quote("alice@example.com", Amount::from(100_000_u64))
            .await
            .expect_err("mismatched invoice amount should fail");

        assert!(matches!(error, Error::LightningAddressRequest(_)));
    }
}

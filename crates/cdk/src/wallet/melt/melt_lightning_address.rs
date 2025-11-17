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
        self.melt_quote(invoice.to_string(), None).await
    }
}

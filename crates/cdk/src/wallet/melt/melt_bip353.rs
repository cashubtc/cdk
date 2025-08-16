//! Melt BIP353
//!
//! Implementation of melt functionality for BIP353 human-readable addresses

use std::str::FromStr;

use cdk_common::wallet::MeltQuote;
use tracing::instrument;

#[cfg(feature = "bip353")]
use crate::bip353::{Bip353Address, PaymentType};
use crate::nuts::MeltOptions;
use crate::{Error, Wallet};

impl Wallet {
    /// Melt Quote for BIP353 human-readable address
    ///
    /// This method resolves a BIP353 address (e.g., "alice@example.com") to a Lightning offer
    /// and then creates a melt quote for that offer.
    ///
    /// # Arguments
    ///
    /// * `bip353_address` - Human-readable address in the format "user@domain.com"
    /// * `options` - Optional melt options (typically for amountless offers)
    ///
    /// # Returns
    ///
    /// A `MeltQuote` that can be used to execute the payment
    ///
    /// # Errors
    ///
    /// This method will return an error if:
    /// - The BIP353 address format is invalid
    /// - DNS resolution fails or DNSSEC validation fails
    /// - No Lightning offer is found in the payment instructions
    /// - The mint fails to provide a quote for the offer
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use cdk::nuts::MeltOptions;
    /// # use cdk::Wallet;
    /// # async fn example(wallet: Wallet) -> Result<(), cdk::Error> {
    /// let options = MeltOptions::new_amountless(100_000); // 100 sats in msat
    /// let quote = wallet
    ///     .melt_bip353_quote("alice@example.com", Some(options))
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "bip353")]
    #[instrument(skip(self), fields(address = %bip353_address))]
    pub async fn melt_bip353_quote(
        &self,
        bip353_address: &str,
        options: MeltOptions,
    ) -> Result<MeltQuote, Error> {
        // Parse the BIP353 address
        let address = Bip353Address::from_str(bip353_address).map_err(|e| {
            tracing::error!("Failed to parse BIP353 address '{}': {}", bip353_address, e);
            Error::Bip353Parse(e.to_string())
        })?;

        tracing::debug!("Resolving BIP353 address: {}", address);

        // Keep a copy for error reporting
        let address_string = address.to_string();

        // Resolve the address to get payment instructions
        let payment_instructions = address.resolve().await.map_err(|e| {
            tracing::error!(
                "Failed to resolve BIP353 address '{}': {}",
                address_string,
                e
            );
            Error::Bip353Resolve(e.to_string())
        })?;

        // Extract the Lightning offer from the payment instructions
        let offer = payment_instructions
            .get(&PaymentType::LightningOffer)
            .ok_or_else(|| {
                tracing::error!("No Lightning offer found in BIP353 payment instructions");
                Error::Bip353NoLightningOffer
            })?;

        tracing::debug!("Found Lightning offer in BIP353 instructions: {}", offer);

        // Create a melt quote for the BOLT12 offer
        self.melt_bolt12_quote(offer.clone(), Some(options)).await
    }
}

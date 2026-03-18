//! Melt BIP353
//!
//! Implementation of melt functionality for BIP353 human-readable addresses

use cdk_common::wallet::MeltQuote;
use tracing::instrument;

use crate::nuts::MeltOptions;
use crate::wallet::bip321::resolve_bip353_payment_instruction;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Melt Quote for BIP353 human-readable address
    ///
    /// This method resolves a BIP353 address (e.g., "alice@example.com") to a Bitcoin
    /// payment instruction, requires a BOLT12 offer in that instruction, and then creates
    /// a melt quote for that offer.
    ///
    /// # Arguments
    ///
    /// * `bip353_address` - Human-readable address in the format "user@domain.com"
    /// * `amount_msat` - Amount to pay in millisatoshis
    /// * `network` - Bitcoin network for on-chain address validation in the resolved URI
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
    /// - No BOLT12 offer is found in the payment instructions
    /// - The mint fails to provide a quote for the offer
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use cdk::Amount;
    /// # use cdk::Wallet;
    /// # async fn example(wallet: Wallet) -> Result<(), cdk::Error> {
    /// let quote = wallet
    ///     .melt_bip353_quote(
    ///         "alice@example.com",
    ///         Amount::from(100_000), // 100 sats in msat
    ///         bitcoin::Network::Bitcoin,
    ///     )
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    #[instrument(skip(self, amount_msat), fields(address = %bip353_address))]
    pub async fn melt_bip353_quote(
        &self,
        bip353_address: &str,
        amount_msat: impl Into<Amount>,
        network: bitcoin::Network,
    ) -> Result<MeltQuote, Error> {
        let parsed_instruction =
            resolve_bip353_payment_instruction(&self.client, bip353_address, network).await?;

        let offer = parsed_instruction.bolt12_offers.first().ok_or_else(|| {
            tracing::error!("No BOLT12 offer found in BIP353 payment instructions");
            Error::Bip353NoBolt12Offer
        })?;

        tracing::debug!("Found BOLT12 offer in BIP353 instructions: {}", offer);

        // Create melt options with the provided amount
        let options = MeltOptions::new_amountless(amount_msat);

        // Create a melt quote for the BOLT12 offer
        self.melt_bolt12_quote(offer.clone(), Some(options)).await
    }
}

#[cfg(all(
    test,
    feature = "bip353",
    feature = "wallet",
    not(target_arch = "wasm32")
))]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;

    use cdk_common::database::WalletDatabase;

    use super::*;
    use crate::mint_url::MintUrl;
    use crate::nuts::CurrencyUnit;
    use crate::wallet::test_utils::MockMintConnector;
    use crate::wallet::WalletBuilder;

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
    async fn test_melt_bip353_quote_errors_when_resolved_uri_has_only_bolt11() {
        let connector = Arc::new(MockMintConnector::new());
        connector.set_dns_txt_response(Ok(vec![
            "bitcoin:?lightning=lnbc1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdpl2pkx2ctnv5sxxmmwwd5kgetjypeh2ursdae8g6twvus8g6rfwvs8qun0dfjkxaq9qrsgq357wnc5r2ueh7ck6q93dj32dlqnls087fxdwk8qakdyafkq3yap9us6v52vjjsrvywa6rt52cm9r9zqt8r2t7mlcwspyetp5h2tztugp9lfyql".to_string(),
        ]));

        let wallet = test_wallet_with_connector(connector).await;
        let error = wallet
            .melt_bip353_quote(
                "alice@example.com",
                Amount::from(100_000_u64),
                bitcoin::Network::Bitcoin,
            )
            .await
            .expect_err("bolt11-only BIP353 should error");

        assert!(matches!(error, Error::Bip353NoBolt12Offer));
    }

    #[tokio::test]
    async fn test_melt_bip353_quote_errors_when_resolved_uri_has_only_cashu() {
        let connector = Arc::new(MockMintConnector::new());
        connector.set_dns_txt_response(Ok(vec![
            "bitcoin:?creq=CREQB1QYQQWER9D4HNZV3NQGQQSQQQQQQQQQQRAQPSQQGQQSQQZQG9QQVXSAR5WPEN5TE0D45KUAPWV4UXZMTSD3JJUCM0D5RQQRJRDANXVET9YPCXZ7TDV4H8GXHR3TQ".to_string(),
        ]));

        let wallet = test_wallet_with_connector(connector).await;
        let error = wallet
            .melt_bip353_quote(
                "alice@example.com",
                Amount::from(100_000_u64),
                bitcoin::Network::Bitcoin,
            )
            .await
            .expect_err("cashu-only BIP353 should error");

        assert!(matches!(error, Error::Bip353NoBolt12Offer));
    }

    #[tokio::test]
    async fn test_melt_bip353_quote_reports_invalid_resolved_uri() {
        let connector = Arc::new(MockMintConnector::new());
        connector.set_dns_txt_response(Ok(vec!["bitcoin:?lno=not-a-valid-offer".to_string()]));

        let wallet = test_wallet_with_connector(connector).await;
        let error = wallet
            .melt_bip353_quote(
                "alice@example.com",
                Amount::from(100_000_u64),
                bitcoin::Network::Bitcoin,
            )
            .await
            .expect_err("invalid resolved URI should error");

        assert!(matches!(error, Error::Bip321Parse(_)));
    }
}

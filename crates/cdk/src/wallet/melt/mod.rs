use std::collections::HashMap;

use cdk_common::database::DynWalletDatabaseTransaction;
use cdk_common::util::unix_time;
use cdk_common::wallet::{MeltQuote, Transaction, TransactionDirection};
use cdk_common::{Error, MeltQuoteBolt11Response, MeltQuoteState, ProofsMethods, State};
use tracing::instrument;

use crate::Wallet;

#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
mod melt_bip353;
mod melt_bolt11;
mod melt_bolt12;
#[cfg(feature = "wallet")]
mod melt_lightning_address;

impl Wallet {
    /// Check pending melt quotes
    #[instrument(skip_all)]
    pub async fn check_pending_melt_quotes(&self) -> Result<(), Error> {
        let quotes = self.get_pending_melt_quotes().await?;
        for quote in quotes {
            self.melt_quote_status(&quote.id).await?;
        }
        Ok(())
    }

    /// Get all active melt quotes from the wallet
    pub async fn get_active_melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let quotes = self.localstore.get_melt_quotes().await?;
        Ok(quotes
            .into_iter()
            .filter(|q| {
                q.state == MeltQuoteState::Pending
                    || (q.state == MeltQuoteState::Unpaid && q.expiry > unix_time())
            })
            .collect())
    }

    /// Get pending melt quotes
    pub async fn get_pending_melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let quotes = self.localstore.get_melt_quotes().await?;
        Ok(quotes
            .into_iter()
            .filter(|q| q.state == MeltQuoteState::Pending)
            .collect())
    }

    pub(crate) async fn add_transaction_for_pending_melt(
        &self,
        tx: &mut DynWalletDatabaseTransaction<'_>,
        quote: &MeltQuote,
        response: &MeltQuoteBolt11Response<String>,
    ) -> Result<(), Error> {
        if quote.state != response.state {
            tracing::info!(
                "Quote melt {} state changed from {} to {}",
                quote.id,
                quote.state,
                response.state
            );
            if response.state == MeltQuoteState::Paid {
                let pending_proofs = self
                    .get_proofs_with(Some(tx), Some(vec![State::Pending]), None)
                    .await?;
                let proofs_total = pending_proofs.total_amount().unwrap_or_default();
                let change_total = response.change_amount().unwrap_or_default();

                tx.add_transaction(Transaction {
                    mint_url: self.mint_url.clone(),
                    direction: TransactionDirection::Outgoing,
                    amount: response.amount,
                    fee: proofs_total
                        .checked_sub(response.amount)
                        .and_then(|amt| amt.checked_sub(change_total))
                        .unwrap_or_default(),
                    unit: quote.unit.clone(),
                    ys: pending_proofs.ys()?,
                    timestamp: unix_time(),
                    memo: None,
                    metadata: HashMap::new(),
                    quote_id: Some(quote.id.clone()),
                    payment_request: Some(quote.request.clone()),
                    payment_proof: response.payment_preimage.clone(),
                })
                .await?;
            }
        }
        Ok(())
    }

    /// Get a melt quote for a human-readable address
    ///
    /// This method accepts a human-readable address that could be either a BIP353 address
    /// or a Lightning address. It intelligently determines which to try based on mint support:
    ///
    /// 1. If the mint supports Bolt12, it tries BIP353 first
    /// 2. Falls back to Lightning address only if BIP353 DNS resolution fails
    /// 3. If BIP353 resolves but fails at the mint, it does NOT fall back to Lightning address
    /// 4. If the mint doesn't support Bolt12, it tries Lightning address directly
    #[cfg(all(feature = "bip353", feature = "wallet", not(target_arch = "wasm32")))]
    pub async fn melt_human_readable_quote(
        &self,
        address: &str,
        amount_msat: impl Into<crate::Amount>,
    ) -> Result<MeltQuote, Error> {
        use cdk_common::nuts::PaymentMethod;

        let amount = amount_msat.into();

        // Get mint info from cache to check bolt12 support (no network call)
        let mint_info = &self
            .metadata_cache
            .load(&self.localstore, &self.client, {
                let ttl = self.metadata_cache_ttl.read();
                *ttl
            })
            .await?
            .mint_info;

        // Check if mint supports bolt12 by looking at nut05 methods
        let supports_bolt12 = mint_info
            .nuts
            .nut05
            .methods
            .iter()
            .any(|m| m.method == PaymentMethod::Bolt12);

        if supports_bolt12 {
            // Mint supports bolt12, try BIP353 first
            match self.melt_bip353_quote(address, amount).await {
                Ok(quote) => Ok(quote),
                Err(Error::Bip353Resolve(_)) => {
                    // DNS resolution failed, fall back to Lightning address
                    tracing::debug!(
                        "BIP353 DNS resolution failed for {}, trying Lightning address",
                        address
                    );
                    return self.melt_lightning_address_quote(address, amount).await;
                }
                Err(e) => {
                    // BIP353 resolved but failed for another reason (e.g., mint error)
                    // Don't fall back to Lightning address
                    Err(e)
                }
            }
        } else {
            // Mint doesn't support bolt12, use Lightning address directly
            self.melt_lightning_address_quote(address, amount).await
        }
    }
}

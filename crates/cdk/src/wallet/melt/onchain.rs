use cdk_common::nut00::KnownMethod;
use cdk_common::wallet::MeltQuote;
use cdk_common::{
    MeltQuoteCreateResponse, MeltQuoteOnchainOptions, MeltQuoteRequest, PaymentMethod,
};
use tracing::instrument;

use crate::nuts::MeltQuoteOnchainRequest;
use crate::{Amount, Error, Wallet};

fn wallet_melt_quote_from_onchain_response(
    mint_url: &crate::mint_url::MintUrl,
    unit: &crate::nuts::CurrencyUnit,
    response: cdk_common::MeltQuoteOnchainResponse<String>,
) -> MeltQuote {
    MeltQuote {
        id: response.quote,
        mint_url: Some(mint_url.clone()),
        amount: response.amount,
        request: response.request,
        unit: unit.clone(),
        fee_reserve: response.fee,
        state: response.state,
        expiry: response.expiry,
        payment_proof: response.outpoint.clone(),
        estimated_blocks: Some(response.estimated_blocks),
        payment_method: PaymentMethod::Known(KnownMethod::Onchain),
        used_by_operation: None,
        version: 0,
    }
}

impl Wallet {
    /// Fetch available onchain melt quote options.
    #[instrument(skip(self, max_fee_amount))]
    pub async fn quote_onchain_melt_options(
        &self,
        address: &str,
        amount: Amount,
        max_fee_amount: Option<Amount>,
    ) -> Result<Vec<MeltQuote>, Error> {
        let quote_request = MeltQuoteOnchainRequest {
            request: address.to_string(),
            unit: self.unit.clone(),
            amount,
        };

        let quote_res = self
            .client
            .post_melt_quote(MeltQuoteRequest::Onchain(quote_request))
            .await?;

        let quote_res = match quote_res {
            MeltQuoteCreateResponse::Onchain(MeltQuoteOnchainOptions { quotes }) => quotes,
            _ => return Err(Error::InvalidPaymentMethod),
        };

        let mut filtered_quotes = Vec::new();

        for quote in quote_res {
            if let Some(max_fee) = max_fee_amount {
                if quote.fee > max_fee {
                    continue;
                }
            }

            filtered_quotes.push(wallet_melt_quote_from_onchain_response(
                &self.mint_url,
                &self.unit,
                quote,
            ));
        }

        if filtered_quotes.is_empty() {
            return Err(Error::MaxFeeExceeded);
        }

        Ok(filtered_quotes)
    }

    /// Persist a selected onchain melt quote.
    #[instrument(skip(self))]
    pub async fn select_onchain_melt_quote(&self, quote: MeltQuote) -> Result<MeltQuote, Error> {
        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }
}

use cdk_common::nut00::KnownMethod;
use cdk_common::wallet::MeltQuote;
use cdk_common::{MeltQuoteCreateResponse, MeltQuoteRequest, PaymentMethod};
use tracing::instrument;

use crate::nuts::MeltQuoteOnchainRequest;
use crate::{Amount, Error, Wallet};

fn wallet_melt_quote_from_onchain_response(
    mint_url: &crate::mint_url::MintUrl,
    unit: &crate::nuts::CurrencyUnit,
    response: cdk_common::MeltQuoteOnchainResponse<String>,
    fee_option: cdk_common::nuts::nut_onchain::MeltQuoteOnchainFeeOption,
) -> MeltQuote {
    MeltQuote {
        id: response.quote,
        mint_url: Some(mint_url.clone()),
        amount: response.amount,
        request: response.request,
        unit: unit.clone(),
        fee_reserve: fee_option.fee_reserve,
        state: response.state,
        expiry: response.expiry,
        payment_proof: response.outpoint.clone(),
        estimated_blocks: Some(fee_option.estimated_blocks),
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
            MeltQuoteCreateResponse::Onchain(quote) => quote,
            _ => return Err(Error::InvalidPaymentMethod),
        };

        let mut filtered_quotes = Vec::new();

        for fee_option in quote_res.fee_options.clone() {
            if let Some(max_fee) = max_fee_amount {
                if fee_option.fee_reserve > max_fee {
                    continue;
                }
            }

            filtered_quotes.push(wallet_melt_quote_from_onchain_response(
                &self.mint_url,
                &self.unit,
                quote_res.clone(),
                fee_option,
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

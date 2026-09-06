use cdk_common::nut00::KnownMethod;
use cdk_common::wallet::MeltQuote;
use cdk_common::{MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteState, PaymentMethod};
use tracing::instrument;

use crate::nuts::MeltQuoteOnchainRequest;
use crate::{Amount, Error, Wallet};

fn wallet_melt_quote_from_onchain_response(
    mint_url: &crate::mint_url::MintUrl,
    unit: &crate::nuts::CurrencyUnit,
    response: cdk_common::MeltQuoteOnchainResponse<String>,
    fee_option: cdk_common::nuts::nut30::MeltQuoteOnchainFeeOption,
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
        fee_index: Some(fee_option.fee_index),
        payment_method: PaymentMethod::Known(KnownMethod::Onchain),
        used_by_operation: None,
        version: 0,
    }
}

impl Wallet {
    /// Fetch available onchain melt quote options.
    #[instrument(skip(self, max_fee_amount))]
    pub(crate) async fn quote_onchain_melt_options(
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
    pub(crate) async fn select_onchain_melt_quote(
        &self,
        mut quote: MeltQuote,
    ) -> Result<MeltQuote, Error> {
        if let Some(current) = self.localstore.get_melt_quote(&quote.id).await? {
            if current.mint_url != quote.mint_url || current.unit != quote.unit {
                return Err(Error::InvalidOperationState);
            }
            if current.used_by_operation.is_some() {
                return Err(cdk_common::database::Error::QuoteAlreadyInUse.into());
            }
            if !matches!(
                current.state,
                MeltQuoteState::Unpaid | MeltQuoteState::Failed
            ) {
                return Err(Error::InvalidOperationState);
            }
            // Allow selecting another fee option after canceling a plan, but
            // never reset a quote using the session's original stale version.
            quote.version = current.version;
            quote.state = current.state;
        }
        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::*;
    use crate::wallet::advanced::{PaymentFunding, PaymentPrepareOptions};
    use crate::wallet::payment::PaymentSession;
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet_with_mock, test_keyset_id, test_melt_quote,
        test_mint_url, test_proof_info, MockMintConnector,
    };

    #[tokio::test]
    async fn onchain_selection_preserves_reservations_and_allows_reselection_after_cancel() {
        let db = create_test_db().await;
        let connector = Arc::new(MockMintConnector::new());
        connector.reset_default_mint_state();
        let wallet = create_test_wallet_with_mock(db.clone(), connector).await;
        let proof = test_proof_info(test_keyset_id(), 2000, test_mint_url());
        db.update_proofs(vec![proof.clone()], vec![]).await.unwrap();

        let mut quote = test_melt_quote();
        quote.payment_method = PaymentMethod::Known(KnownMethod::Onchain);
        quote.fee_index = Some(0);
        let first = PaymentSession::new(wallet.clone(), quote.clone(), HashMap::new(), true);
        quote.fee_index = Some(1);
        quote.fee_reserve = Amount::from(20);
        let second = PaymentSession::new(wallet, quote.clone(), HashMap::new(), true);
        let funding = PaymentPrepareOptions {
            funding: PaymentFunding::Proofs(vec![proof.proof]),
        };

        let plan = first.prepare_with(funding.clone()).await.unwrap();
        let owner = db
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap()
            .used_by_operation;
        assert!(owner.is_some());
        for session in [&first, &second] {
            assert!(matches!(
                session.prepare_with(funding.clone()).await,
                Err(Error::Database(
                    cdk_common::database::Error::QuoteAlreadyInUse
                ))
            ));
            let stored = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
            assert_eq!(stored.used_by_operation, owner);
            assert_eq!(stored.fee_index, Some(0));
        }

        plan.cancel().await.unwrap();
        let plan = second.prepare_with(funding).await.unwrap();
        let stored = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
        assert_eq!(stored.fee_index, Some(1));
        assert_eq!(stored.fee_reserve, Amount::from(20));
        plan.cancel().await.unwrap();

        let mut paid = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
        paid.state = MeltQuoteState::Paid;
        db.add_melt_quote(paid).await.unwrap();
        assert!(matches!(
            first.prepare().await,
            Err(Error::InvalidOperationState)
        ));
        assert_eq!(
            db.get_melt_quote(&quote.id).await.unwrap().unwrap().state,
            MeltQuoteState::Paid
        );
    }
}

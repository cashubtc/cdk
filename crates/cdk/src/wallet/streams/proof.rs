//! Mint Stream
//!
//! This will mint after a mint quote has been paid. If the quote is for a Bolt12 it will keep minting until a timeout is reached.
//!
//! Bolt11 will mint once

use std::collections::HashMap;
use std::task::Poll;

use cdk_common::amount::SplitTarget;
use cdk_common::wallet::MintQuote;
use cdk_common::{Error, PaymentMethod, Proofs, SpendingConditions};
use futures::{FutureExt, Stream, StreamExt};
use tokio_util::sync::CancellationToken;

use super::payment::PaymentStream;
use super::{RecvFuture, WaitableEvent};
use crate::Wallet;

/// Proofs for many mint quotes, as they are minted, in streams
pub struct MultipleMintQuoteProofStream<'a> {
    payment_stream: PaymentStream<'a>,
    wallet: &'a Wallet,
    quotes: HashMap<String, MintQuote>,
    amount_split_target: SplitTarget,
    spending_conditions: Option<SpendingConditions>,
    minting_future: Option<RecvFuture<'a, Result<(MintQuote, Proofs), Error>>>,
}

impl<'a> MultipleMintQuoteProofStream<'a> {
    /// Create a new Stream
    pub fn new(
        wallet: &'a Wallet,
        quotes: Vec<MintQuote>,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Self {
        let filter: WaitableEvent = quotes.as_slice().into();

        Self {
            payment_stream: PaymentStream::new(wallet, filter.into_subscription()),
            wallet,
            amount_split_target,
            spending_conditions,
            quotes: quotes
                .into_iter()
                .map(|mint_quote| (mint_quote.id.clone(), mint_quote))
                .collect(),
            minting_future: None,
        }
    }

    /// Get cancellation token
    pub fn get_cancel_token(&self) -> CancellationToken {
        self.payment_stream.get_cancel_token()
    }
}

impl Stream for MultipleMintQuoteProofStream<'_> {
    type Item = Result<(MintQuote, Proofs), Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if let Some(mut minting_future) = this.minting_future.take() {
            return match minting_future.poll_unpin(cx) {
                Poll::Pending => {
                    this.minting_future = Some(minting_future);
                    Poll::Pending
                }
                Poll::Ready(proofs) => Poll::Ready(Some(proofs)),
            };
        }

        match this.payment_stream.poll_next_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => match result {
                None => Poll::Ready(None),
                Some(result) => {
                    let (quote_id, amount) = match result {
                        Err(err) => {
                            tracing::error!(
                                "Error while waiting for payment for {:?}",
                                this.quotes.keys().collect::<Vec<_>>()
                            );
                            return Poll::Ready(Some(Err(err)));
                        }
                        Ok(amount) => amount,
                    };

                    let mint_quote = if let Some(quote) = this.quotes.get(&quote_id) {
                        quote.clone()
                    } else {
                        tracing::error!("Cannot find mint_quote {} internally", quote_id);
                        return Poll::Ready(Some(Err(Error::UnknownQuote)));
                    };

                    let amount_split_target = this.amount_split_target.clone();
                    let spending_conditions = this.spending_conditions.clone();
                    let wallet = this.wallet;

                    tracing::debug!(
                        "Received payment ({:?}) notification for {}. Minting...",
                        amount,
                        mint_quote.id
                    );

                    let mut minting_future = Box::pin(async move {
                        match mint_quote.payment_method {
                            PaymentMethod::Bolt11 => wallet
                                .mint(&mint_quote.id, amount_split_target, spending_conditions)
                                .await
                                .map(|proofs| (mint_quote, proofs)),
                            PaymentMethod::Bolt12 => wallet
                                .mint_bolt12(
                                    &mint_quote.id,
                                    amount,
                                    amount_split_target,
                                    spending_conditions,
                                )
                                .await
                                .map(|proofs| (mint_quote, proofs)),
                            PaymentMethod::MiningShare => {
                                let paid_amount = amount.ok_or(Error::AmountUndefined)?;
                                let keyset_id = mint_quote.keyset_id.ok_or(Error::UnknownKeySet)?;
                                let secret_key = mint_quote
                                    .secret_key
                                    .clone()
                                    .ok_or(Error::SecretKeyRequired)?;

                                wallet
                                    .mint_mining_share(
                                        &mint_quote.id,
                                        paid_amount,
                                        keyset_id,
                                        secret_key,
                                    )
                                    .await
                                    .map(|proofs| (mint_quote, proofs))
                            }
                            _ => Err(Error::UnsupportedPaymentMethod),
                        }
                    });

                    match minting_future.poll_unpin(cx) {
                        Poll::Pending => {
                            this.minting_future = Some(minting_future);
                            Poll::Pending
                        }
                        Poll::Ready(result) => Poll::Ready(Some(result)),
                    }
                }
            },
        }
    }
}

/// Proofs for a single mint quote
pub struct SingleMintQuoteProofStream<'a>(MultipleMintQuoteProofStream<'a>);

impl<'a> SingleMintQuoteProofStream<'a> {
    /// Create a new Stream
    pub fn new(
        wallet: &'a Wallet,
        quote: MintQuote,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Self {
        Self(MultipleMintQuoteProofStream::new(
            wallet,
            vec![quote],
            amount_split_target,
            spending_conditions,
        ))
    }

    /// Get cancellation token
    pub fn get_cancel_token(&self) -> CancellationToken {
        self.0.payment_stream.get_cancel_token()
    }
}

impl Stream for SingleMintQuoteProofStream<'_> {
    type Item = Result<Proofs, Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.0.poll_next_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => match result {
                None => Poll::Ready(None),
                Some(Err(err)) => Poll::Ready(Some(Err(err))),
                Some(Ok((_, proofs))) => Poll::Ready(Some(Ok(proofs))),
            },
        }
    }
}

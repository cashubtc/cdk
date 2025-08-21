//! Mint Stream
//!
//! This will mint after a mint quote has been paid. If the quote is for a Bolt12 it will keep minting until a timeout is reached.
//!
//! Bolt11 will mint once

use std::task::Poll;

use cdk_common::amount::SplitTarget;
use cdk_common::wallet::MintQuote;
use cdk_common::{Error, PaymentMethod, Proofs, SpendingConditions};
use futures::{FutureExt, Stream, StreamExt};
use tokio_util::sync::CancellationToken;

use super::payment::PaymentStream;
use super::{RecvFuture, WaitableEvent};
use crate::Wallet;

/// Mint waiter
pub struct ProofStream<'a> {
    payment_stream: PaymentStream<'a>,
    wallet: &'a Wallet,
    mint_quote: MintQuote,
    amount_split_target: SplitTarget,
    spending_conditions: Option<SpendingConditions>,
    minting_future: Option<RecvFuture<'a, Result<Proofs, Error>>>,
}

impl<'a> ProofStream<'a> {
    /// Create a new Stream
    pub fn new(
        wallet: &'a Wallet,
        mint_quote: MintQuote,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Self {
        let filter: WaitableEvent = (&mint_quote).into();
        Self {
            payment_stream: PaymentStream::new(wallet, filter.into()),
            wallet,
            amount_split_target,
            spending_conditions,
            mint_quote,
            minting_future: None,
        }
    }

    /// Get cancellation token
    pub fn get_cancel_token(&self) -> CancellationToken {
        self.payment_stream.get_cancel_token()
    }
}

impl Stream for ProofStream<'_> {
    type Item = Result<Proofs, Error>;

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
                    let amount = match result {
                        Err(err) => {
                            tracing::error!(
                                "Error while waiting for payment for {}",
                                this.mint_quote.id
                            );
                            return Poll::Ready(Some(Err(err)));
                        }
                        Ok(amount) => amount,
                    };

                    let mint_quote = this.mint_quote.clone();
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
                            PaymentMethod::Bolt11 => {
                                wallet
                                    .mint(&mint_quote.id, amount_split_target, spending_conditions)
                                    .await
                            }
                            PaymentMethod::Bolt12 => {
                                wallet
                                    .mint_bolt12(
                                        &mint_quote.id,
                                        amount,
                                        amount_split_target,
                                        spending_conditions,
                                    )
                                    .await
                            }
                            _ => Err(Error::UnsupportedPaymentMethod),
                        }
                    });

                    match minting_future.poll_unpin(cx) {
                        Poll::Pending => {
                            this.minting_future = Some(minting_future);
                            Poll::Pending
                        }
                        Poll::Ready(proofs) => Poll::Ready(Some(proofs)),
                    }
                }
            },
        }
    }
}

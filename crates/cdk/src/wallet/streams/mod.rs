//! Wallet waiter APIn
use std::future::Future;
use std::pin::Pin;

use cdk_common::amount::SplitTarget;
use cdk_common::wallet::{MeltQuote, MintQuote};
use cdk_common::{PaymentMethod, SpendingConditions};
use payment::PaymentStream;
use proof::{MultipleMintQuoteProofStream, SingleMintQuoteProofStream};

use super::{Wallet, WalletSubscription};

pub mod payment;
pub mod proof;
mod wait;

/// Shared type
type RecvFuture<'a, Ret> = Pin<Box<dyn Future<Output = Ret> + Send + 'a>>;

#[allow(private_bounds)]
#[allow(clippy::enum_variant_names)]
enum WaitableEvent {
    MeltQuote(Vec<String>),
    MintQuote(Vec<(String, PaymentMethod)>),
}

impl From<&[MeltQuote]> for WaitableEvent {
    fn from(events: &[MeltQuote]) -> Self {
        WaitableEvent::MeltQuote(events.iter().map(|event| event.id.to_owned()).collect())
    }
}

impl From<&MeltQuote> for WaitableEvent {
    fn from(event: &MeltQuote) -> Self {
        WaitableEvent::MeltQuote(vec![event.id.to_owned()])
    }
}

impl From<&[MintQuote]> for WaitableEvent {
    fn from(events: &[MintQuote]) -> Self {
        WaitableEvent::MintQuote(
            events
                .iter()
                .map(|event| (event.id.to_owned(), event.payment_method.clone()))
                .collect(),
        )
    }
}

impl From<&MintQuote> for WaitableEvent {
    fn from(event: &MintQuote) -> Self {
        WaitableEvent::MintQuote(vec![(event.id.to_owned(), event.payment_method.clone())])
    }
}

impl WaitableEvent {
    fn into_subscription(self) -> Vec<WalletSubscription> {
        match self {
            WaitableEvent::MeltQuote(quotes) => {
                vec![WalletSubscription::Bolt11MeltQuoteState(quotes)]
            }
            WaitableEvent::MintQuote(quotes) => {
                let (bolt11, bolt12) = quotes.into_iter().fold(
                    (Vec::new(), Vec::new()),
                    |mut acc, (quote_id, payment_method)| {
                        match payment_method {
                            PaymentMethod::Bolt11 => acc.0.push(quote_id),
                            PaymentMethod::Bolt12 => acc.1.push(quote_id),
                            PaymentMethod::Custom(_) => acc.0.push(quote_id),
                            PaymentMethod::MiningShare => acc.0.push(quote_id),
                        }
                        acc
                    },
                );

                let mut subscriptions = Vec::new();

                if !bolt11.is_empty() {
                    subscriptions.push(WalletSubscription::Bolt11MintQuoteState(bolt11));
                }

                if !bolt12.is_empty() {
                    subscriptions.push(WalletSubscription::Bolt12MintQuoteState(bolt12));
                }

                subscriptions
            }
        }
    }
}

impl Wallet {
    /// Streams all proofs from a single mint quote
    #[inline(always)]
    pub fn proof_stream(
        &self,
        quote: MintQuote,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> SingleMintQuoteProofStream<'_> {
        SingleMintQuoteProofStream::new(self, quote, amount_split_target, spending_conditions)
    }

    /// Streams all new proofs for a set of mints
    #[inline(always)]
    pub fn mints_proof_stream(
        &self,
        quotes: Vec<MintQuote>,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> MultipleMintQuoteProofStream<'_> {
        MultipleMintQuoteProofStream::new(self, quotes, amount_split_target, spending_conditions)
    }

    /// Returns a BoxFuture that will wait for payment on the given event with a timeout check
    #[allow(private_bounds)]
    pub fn payment_stream<T>(&self, events: T) -> PaymentStream<'_>
    where
        T: Into<WaitableEvent>,
    {
        PaymentStream::new(self, events.into().into_subscription())
    }
}
#[cfg(all(feature = "nostr", not(target_arch = "wasm32")))]
pub mod nostr;

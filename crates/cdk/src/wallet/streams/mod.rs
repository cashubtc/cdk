//! Wallet waiter APIs
use std::future::Future;
use std::pin::Pin;

use cdk_common::amount::SplitTarget;
use cdk_common::wallet::{MeltQuote, MintQuote};
use cdk_common::{PaymentMethod, SpendingConditions};
use payment::PaymentStream;
use proof::ProofStream;

use super::{Wallet, WalletSubscription};

pub mod payment;
pub mod proof;

/// Shared type
#[cfg(not(target_arch = "wasm32"))]
type RecvFuture<'a, Ret> = Pin<Box<dyn Future<Output = Ret> + Send + 'a>>;

#[cfg(target_arch = "wasm32")]
type RecvFuture<'a, Ret> = Pin<Box<dyn Future<Output = Ret> + 'a>>;

#[allow(private_bounds)]
#[allow(clippy::enum_variant_names)]
enum WaitableEvent {
    MeltQuote(String),
    MintQuote(String),
    Bolt12MintQuote(String),
}

impl From<&MeltQuote> for WaitableEvent {
    fn from(event: &MeltQuote) -> Self {
        WaitableEvent::MeltQuote(event.id.to_owned())
    }
}

impl From<&MintQuote> for WaitableEvent {
    fn from(event: &MintQuote) -> Self {
        match event.payment_method {
            PaymentMethod::Bolt11 => WaitableEvent::MintQuote(event.id.to_owned()),
            PaymentMethod::Bolt12 => WaitableEvent::Bolt12MintQuote(event.id.to_owned()),
            PaymentMethod::Custom(_) => WaitableEvent::MintQuote(event.id.to_owned()),
        }
    }
}

impl From<WaitableEvent> for WalletSubscription {
    fn from(val: WaitableEvent) -> Self {
        match val {
            WaitableEvent::MeltQuote(quote_id) => {
                WalletSubscription::Bolt11MeltQuoteState(vec![quote_id])
            }
            WaitableEvent::MintQuote(quote_id) => {
                WalletSubscription::Bolt11MintQuoteState(vec![quote_id])
            }
            WaitableEvent::Bolt12MintQuote(quote_id) => {
                WalletSubscription::Bolt12MintQuoteState(vec![quote_id])
            }
        }
    }
}

impl Wallet {
    #[inline(always)]
    /// Mints a mint quote once it is paid
    pub fn proof_stream(
        &self,
        quote: MintQuote,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> ProofStream<'_> {
        ProofStream::new(self, quote, amount_split_target, spending_conditions)
    }

    /// Returns a BoxFuture that will wait for payment on the given event with a timeout check
    #[allow(private_bounds)]
    pub fn payment_stream<T>(&self, event: T) -> PaymentStream<'_>
    where
        T: Into<WaitableEvent>,
    {
        PaymentStream::new(self, event.into().into())
    }
}

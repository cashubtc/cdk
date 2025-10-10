//! Specific Subscription for the cdk crate

use std::ops::Deref;
use std::sync::Arc;

use cdk_common::database::DynMintDatabase;
use cdk_common::mint::MintQuote;
use cdk_common::nut17::NotificationId;
use cdk_common::pub_sub::{Pubsub, Spec, Subscriber};
use cdk_common::subscription::SubId;
use cdk_common::{
    Amount, BlindSignature, MeltQuoteBolt11Response, MeltQuoteState, MintQuoteBolt11Response,
    MintQuoteBolt12Response, MintQuoteState, PaymentMethod, ProofState, PublicKey, QuoteId,
};

use crate::event::MintEvent;

/// Mint subtopics
#[derive(Clone)]
pub struct MintPubSubSpec {
    db: DynMintDatabase,
}

impl MintPubSubSpec {
    async fn get_events_from_db(
        &self,
        request: &[NotificationId<QuoteId>],
    ) -> Result<Vec<MintEvent<QuoteId>>, String> {
        let mut to_return = vec![];
        let mut public_keys: Vec<PublicKey> = Vec::new();
        let mut melt_queries = Vec::new();
        let mut mint_queries = Vec::new();

        for idx in request.iter() {
            match idx {
                NotificationId::ProofState(pk) => public_keys.push(*pk),
                NotificationId::MeltQuoteBolt11(uuid) => {
                    melt_queries.push(self.db.get_melt_quote(uuid))
                }
                NotificationId::MintQuoteBolt11(uuid) => {
                    mint_queries.push(self.db.get_mint_quote(uuid))
                }
                NotificationId::MintQuoteBolt12(uuid) => {
                    mint_queries.push(self.db.get_mint_quote(uuid))
                }
                NotificationId::MeltQuoteBolt12(uuid) => {
                    melt_queries.push(self.db.get_melt_quote(uuid))
                }
                NotificationId::MintQuoteMiningShare(uuid) => {
                    mint_queries.push(self.db.get_mint_quote(uuid))
                }
            }
        }

        if !melt_queries.is_empty() {
            to_return.extend(
                futures::future::try_join_all(melt_queries)
                    .await
                    .map(|quotes| {
                        quotes
                            .into_iter()
                            .filter_map(|quote| quote.map(|x| x.into()))
                            .map(|x: MeltQuoteBolt11Response<QuoteId>| x.into())
                            .collect::<Vec<_>>()
                    })
                    .map_err(|e| e.to_string())?,
            );
        }

        if !mint_queries.is_empty() {
            to_return.extend(
                futures::future::try_join_all(mint_queries)
                    .await
                    .map(|quotes| {
                        quotes
                            .into_iter()
                            .filter_map(|quote| {
                                quote.and_then(|x| match x.payment_method {
                                    PaymentMethod::Bolt11 => {
                                        let response: MintQuoteBolt11Response<QuoteId> = x.into();
                                        Some(response.into())
                                    }
                                    PaymentMethod::Bolt12 => match x.try_into() {
                                        Ok(response) => {
                                            let response: MintQuoteBolt12Response<QuoteId> =
                                                response;
                                            Some(response.into())
                                        }
                                        Err(_) => None,
                                    },
                                    PaymentMethod::MiningShare => None,
                                    PaymentMethod::Custom(_) => None,
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .map_err(|e| e.to_string())?,
            );
        }

        if !public_keys.is_empty() {
            to_return.extend(
                self.db
                    .get_proofs_states(public_keys.as_slice())
                    .await
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .enumerate()
                    .filter_map(|(idx, state)| state.map(|state| (public_keys[idx], state).into()))
                    .map(|state: ProofState| state.into()),
            );
        }

        Ok(to_return)
    }
}

#[async_trait::async_trait]
impl Spec for MintPubSubSpec {
    type SubscriptionId = SubId;

    type Topic = NotificationId<QuoteId>;

    type Event = MintEvent<QuoteId>;

    type Context = DynMintDatabase;

    fn new_instance(context: Self::Context) -> Arc<Self> {
        Arc::new(Self { db: context })
    }

    async fn fetch_events(self: &Arc<Self>, topics: Vec<Self::Topic>, reply_to: Subscriber<Self>) {
        for event in self
            .get_events_from_db(&topics)
            .await
            .inspect_err(|err| tracing::error!("Error reading events from db {err:?}"))
            .unwrap_or_default()
        {
            let _ = reply_to.send(event);
        }
    }
}

/// PubsubManager
pub struct PubSubManager(Pubsub<MintPubSubSpec>);

impl PubSubManager {
    /// Create a new instance
    pub fn new(db: DynMintDatabase) -> Arc<Self> {
        Arc::new(Self(Pubsub::new(MintPubSubSpec::new_instance(db))))
    }

    /// Helper function to emit a ProofState status
    pub fn proof_state<E: Into<ProofState>>(&self, event: E) {
        self.publish(event.into());
    }

    /// Helper function to publish even of a mint quote being paid
    pub fn mint_quote_issue(&self, mint_quote: &MintQuote, total_issued: Amount) {
        match mint_quote.payment_method {
            PaymentMethod::Bolt11 => {
                self.mint_quote_bolt11_status(mint_quote.clone(), MintQuoteState::Issued);
            }
            PaymentMethod::Bolt12 => {
                self.mint_quote_bolt12_status(
                    mint_quote.clone(),
                    mint_quote.amount_paid(),
                    total_issued,
                );
            }
            _ => {
                // We don't send ws updates for unknown methods
            }
        }
    }

    /// Helper function to publish even of a mint quote being paid
    pub fn mint_quote_payment(&self, mint_quote: &MintQuote, total_paid: Amount) {
        match mint_quote.payment_method {
            PaymentMethod::Bolt11 => {
                self.mint_quote_bolt11_status(mint_quote.clone(), MintQuoteState::Paid);
            }
            PaymentMethod::Bolt12 => {
                self.mint_quote_bolt12_status(
                    mint_quote.clone(),
                    total_paid,
                    mint_quote.amount_issued(),
                );
            }
            _ => {
                // We don't send ws updates for unknown methods
            }
        }
    }

    /// Helper function to emit a MintQuoteBolt11Response status
    pub fn mint_quote_bolt11_status<E: Into<MintQuoteBolt11Response<QuoteId>>>(
        &self,
        quote: E,
        new_state: MintQuoteState,
    ) {
        let mut event = quote.into();
        event.state = new_state;

        self.publish(event);
    }

    /// Helper function to emit a MintQuoteBolt11Response status
    pub fn mint_quote_bolt12_status<E: TryInto<MintQuoteBolt12Response<QuoteId>>>(
        &self,
        quote: E,
        amount_paid: Amount,
        amount_issued: Amount,
    ) {
        if let Ok(mut event) = quote.try_into() {
            event.amount_paid = amount_paid;
            event.amount_issued = amount_issued;

            self.publish(event);
        } else {
            tracing::warn!("Could not convert quote to MintQuoteResponse");
        }
    }

    /// Helper function to emit a MeltQuoteBolt11Response status
    pub fn melt_quote_status<E: Into<MeltQuoteBolt11Response<QuoteId>>>(
        &self,
        quote: E,
        payment_preimage: Option<String>,
        change: Option<Vec<BlindSignature>>,
        new_state: MeltQuoteState,
    ) {
        let mut quote = quote.into();
        quote.state = new_state;
        quote.paid = Some(new_state == MeltQuoteState::Paid);
        quote.payment_preimage = payment_preimage;
        quote.change = change;
        self.publish(quote);
    }
}

impl Deref for PubSubManager {
    type Target = Pubsub<MintPubSubSpec>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

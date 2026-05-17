//! Specific Subscription for the cdk crate

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use cdk_common::common::PaymentProcessorKey;
use cdk_common::database::DynMintDatabase;
use cdk_common::mint::{MeltQuote, MintQuote};
use cdk_common::nut17::NotificationId;
use cdk_common::payment::DynMintPayment;
use cdk_common::pub_sub::{Pubsub, Spec, Subscriber};
use cdk_common::subscription::SubId;
use cdk_common::{
    Amount, BlindSignature, CurrencyUnit, MeltQuoteBolt11Response, MeltQuoteBolt12Response,
    MeltQuoteState, MintQuoteBolt11Response, MintQuoteBolt12Response, MintQuoteCustomResponse,
    MintQuoteState, NotificationPayload, ProofState, PublicKey, QuoteId,
};

use super::Mint;
use crate::event::MintEvent;

/// Mint subtopics
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct MintPubSubSpec {
    db: DynMintDatabase,
    payment_processors: Arc<HashMap<PaymentProcessorKey, DynMintPayment>>,
}

impl MintPubSubSpec {
    /// Call Mint::check_mint_quote_payments to update quotes by pinging the payment backend
    async fn get_mint_quotes(
        &self,
        quote_ids: &[QuoteId],
    ) -> Result<HashMap<QuoteId, MintQuote>, cdk_common::Error> {
        if quote_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut quotes = HashMap::new();

        for mut quote in self
            .db
            .get_mint_quotes_by_ids(quote_ids)
            .await?
            .into_iter()
            .flatten()
        {
            Mint::check_mint_quote_payments(
                self.db.clone(),
                self.payment_processors.clone(),
                None,
                &mut quote,
            )
            .await?;

            quotes.insert(quote.id.clone(), quote);
        }

        Ok(quotes)
    }

    async fn get_events_from_db(
        &self,
        request: &[NotificationId<QuoteId>],
    ) -> Result<Vec<MintEvent<QuoteId>>, String> {
        let mut to_return = vec![];
        let mut public_keys: Vec<PublicKey> = Vec::new();
        let mint_quote_ids = request
            .iter()
            .filter_map(|idx| match idx {
                NotificationId::MintQuoteBolt11(uuid) | NotificationId::MintQuoteBolt12(uuid) => {
                    Some(uuid.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let mint_quotes = self
            .get_mint_quotes(&mint_quote_ids)
            .await
            .map_err(|e| e.to_string())?;

        for idx in request.iter() {
            match idx {
                NotificationId::ProofState(pk) => public_keys.push(*pk),
                NotificationId::MeltQuoteBolt11(uuid) => {
                    // TODO: In the HTTP handler, we check with the LN backend if a payment is in a pending quote state to resolve stuck payments.
                    // Implement similar logic here for WebSocket-only wallets.
                    if let Some(melt_quote) = self
                        .db
                        .get_melt_quote(uuid)
                        .await
                        .map_err(|e| e.to_string())?
                    {
                        let melt_quote: MeltQuoteBolt11Response<_> = melt_quote.into();
                        to_return.push(melt_quote.into());
                    }
                }
                NotificationId::MeltQuoteBolt12(uuid) => {
                    if let Some(melt_quote) = self
                        .db
                        .get_melt_quote(uuid)
                        .await
                        .map_err(|e| e.to_string())?
                    {
                        let melt_quote: MeltQuoteBolt12Response<_> = melt_quote.into();
                        to_return.push(melt_quote.into());
                    }
                }
                NotificationId::MintQuoteBolt11(uuid) | NotificationId::MintQuoteBolt12(uuid) => {
                    if let Some(mint_quote) = mint_quotes.get(uuid).cloned() {
                        let mint_quote = match idx {
                            NotificationId::MintQuoteBolt11(_) => {
                                let response: MintQuoteBolt11Response<QuoteId> = mint_quote.into();
                                response.into()
                            }
                            NotificationId::MintQuoteBolt12(_) => match mint_quote.try_into() {
                                Ok(response) => {
                                    let response: MintQuoteBolt12Response<QuoteId> = response;
                                    response.into()
                                }
                                Err(_) => continue,
                            },
                            _ => continue,
                        };

                        to_return.push(mint_quote);
                    }
                }
                NotificationId::MintQuoteCustom(_, _) | NotificationId::MeltQuoteCustom(_, _) => {
                    continue;
                }
            }
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

    type Context = (
        DynMintDatabase,
        Arc<HashMap<PaymentProcessorKey, DynMintPayment>>,
    );

    fn new_instance(context: Self::Context) -> Arc<Self> {
        Arc::new(Self {
            db: context.0,
            payment_processors: context.1,
        })
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
#[allow(missing_debug_implementations)]
pub struct PubSubManager(Pubsub<MintPubSubSpec>);

impl PubSubManager {
    /// Create a new instance
    pub fn new(
        context: (
            DynMintDatabase,
            Arc<HashMap<PaymentProcessorKey, DynMintPayment>>,
        ),
    ) -> Arc<Self> {
        Arc::new(Self(Pubsub::new(MintPubSubSpec::new_instance(context))))
    }

    /// Helper function to emit a ProofState status
    pub fn proof_state<E: Into<ProofState>>(&self, event: E) {
        self.publish(event.into());
    }

    /// Helper function to publish even of a mint quote being paid
    pub fn mint_quote_issue(&self, mint_quote: &MintQuote, total_issued: Amount<CurrencyUnit>) {
        match mint_quote.payment_method {
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt11) => {
                self.mint_quote_bolt11_status(mint_quote.clone(), MintQuoteState::Issued);
            }
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt12) => {
                self.mint_quote_bolt12_status(
                    mint_quote.clone(),
                    mint_quote.amount_paid().into(),
                    total_issued.into(),
                );
            }
            cdk_common::PaymentMethod::Custom(ref method) => {
                if let Ok(response) = MintQuoteCustomResponse::try_from(mint_quote.clone()) {
                    self.publish(NotificationPayload::CustomMintQuoteResponse(
                        method.clone(),
                        response,
                    ));
                }
            }
        }
    }

    /// Helper function to publish even of a mint quote being paid
    pub fn mint_quote_payment(&self, mint_quote: &MintQuote, total_paid: Amount<CurrencyUnit>) {
        match mint_quote.payment_method {
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt11) => {
                self.mint_quote_bolt11_status(mint_quote.clone(), MintQuoteState::Paid);
            }
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt12) => {
                self.mint_quote_bolt12_status(
                    mint_quote.clone(),
                    total_paid.into(),
                    mint_quote.amount_issued().into(),
                );
            }
            cdk_common::PaymentMethod::Custom(ref method) => {
                if let Ok(response) = MintQuoteCustomResponse::try_from(mint_quote.clone()) {
                    self.publish(NotificationPayload::CustomMintQuoteResponse(
                        method.clone(),
                        response,
                    ));
                }
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
    pub fn melt_quote_status(
        &self,
        quote: &MeltQuote,
        payment_proof: Option<String>,
        change: Option<Vec<BlindSignature>>,
        new_state: MeltQuoteState,
    ) {
        match quote.payment_method {
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt11) => {
                let mut event: MeltQuoteBolt11Response<QuoteId> = quote.clone().into();
                event.state = new_state;
                event.payment_preimage = payment_proof;
                event.change = change;
                self.publish(NotificationPayload::MeltQuoteBolt11Response(event));
            }
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt12) => {
                let mut event: MeltQuoteBolt12Response<QuoteId> = quote.clone().into();
                event.state = new_state;
                event.payment_preimage = payment_proof;
                event.change = change;
                self.publish(NotificationPayload::MeltQuoteBolt12Response(event));
            }
            cdk_common::PaymentMethod::Custom(ref method) => {
                let mut response: cdk_common::nuts::MeltQuoteCustomResponse<QuoteId> =
                    quote.clone().into();
                response.state = new_state;
                response.payment_preimage = payment_proof;
                response.change = change;

                self.publish(NotificationPayload::CustomMeltQuoteResponse(
                    method.clone(),
                    response,
                ));
            }
        }
    }

}

impl Deref for PubSubManager {
    type Target = Pubsub<MintPubSubSpec>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::database::DynMintDatabase;
    use cdk_common::mint::MintQuote;
    use cdk_common::payment::PaymentIdentifier;
    use cdk_common::QuoteId;

    use super::*;

    fn paid_bolt11_quote(id: QuoteId, amount: u64) -> MintQuote {
        MintQuote::new(
            Some(id),
            format!("lnbc1test{amount}"),
            CurrencyUnit::Sat,
            Some(Amount::new(amount, CurrencyUnit::Sat)),
            0,
            PaymentIdentifier::CustomId(format!("lookup-{amount}")),
            None,
            Amount::new(0, CurrencyUnit::Sat),
            Amount::new(0, CurrencyUnit::Sat),
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt11),
            0,
            vec![],
            vec![],
            None,
        )
    }

    async fn add_mint_quote(db: &DynMintDatabase, quote: MintQuote) {
        let payment_amount = quote.amount.clone().expect("quote amount");
        let payment_id = format!("payment-{}", quote.id);
        let mut tx = db.begin_transaction().await.expect("begin transaction");
        let mut quote = tx.add_mint_quote(quote).await.expect("add mint quote");
        quote
            .add_payment(payment_amount, payment_id, Some(0))
            .expect("add payment");
        tx.update_mint_quote(&mut quote)
            .await
            .expect("update mint quote");
        tx.commit().await.expect("commit transaction");
    }

    #[tokio::test]
    async fn get_events_from_db_batches_multiple_mint_quote_filters() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory mint database"),
        );
        let first_quote_id = QuoteId::new_uuid();
        let second_quote_id = QuoteId::new_uuid();
        add_mint_quote(&db, paid_bolt11_quote(first_quote_id.clone(), 21)).await;
        add_mint_quote(&db, paid_bolt11_quote(second_quote_id.clone(), 34)).await;

        let spec = MintPubSubSpec {
            db,
            payment_processors: Arc::new(HashMap::new()),
        };
        let events = spec
            .get_events_from_db(&[
                NotificationId::MintQuoteBolt11(first_quote_id.clone()),
                NotificationId::MintQuoteBolt11(second_quote_id.clone()),
            ])
            .await
            .expect("get events");

        let quote_ids = events
            .into_iter()
            .map(|event| match event.into_inner() {
                NotificationPayload::MintQuoteBolt11Response(response) => response.quote,
                payload => panic!("unexpected payload: {payload:?}"),
            })
            .collect::<Vec<_>>();

        assert_eq!(quote_ids, vec![first_quote_id, second_quote_id]);
    }
}

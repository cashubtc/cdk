//! Specific Subscription for the cdk crate

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Weak};

use cdk_common::common::PaymentProcessorKey;
use cdk_common::database::DynMintDatabase;
use cdk_common::mint::{MeltQuote, MintQuote};
use cdk_common::nut17::NotificationId;
use cdk_common::payment::DynMintPayment;
use cdk_common::pub_sub::{Bus, LocalBus, LocalDelivery, Pubsub, Spec, Subscriber};
use cdk_common::subscription::SubId;
use cdk_common::{
    Amount, BlindSignature, CurrencyUnit, MeltQuoteBolt11Response, MeltQuoteBolt12Response,
    MeltQuoteOnchainResponse, MeltQuoteResponse, MeltQuoteState, MintQuoteBolt11Response,
    MintQuoteBolt12Response, MintQuoteCustomResponse, MintQuoteOnchainResponse, MintQuoteState,
    NotificationPayload, ProofState, PublicKey, QuoteId,
};

use super::Mint;
use crate::event::MintEvent;

/// Mint subtopics
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct MintPubSubSpec {
    db: DynMintDatabase,
    payment_processors: Arc<HashMap<PaymentProcessorKey, DynMintPayment>>,
    // The manager owns this spec; a strong reference back would retain both forever.
    pubsub_manager: Weak<PubSubManager>,
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
                self.pubsub_manager.upgrade(),
                &mut quote,
            )
            .await?;

            quotes.insert(quote.id.clone(), quote);
        }

        Ok(quotes)
    }

    async fn get_melt_quote_response(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Option<MeltQuoteResponse<QuoteId>>, String> {
        let quote = match self
            .db
            .get_melt_quote(quote_id)
            .await
            .map_err(|e| e.to_string())?
        {
            Some(quote) => quote,
            None => return Ok(None),
        };
        let change = if matches!(
            quote.state,
            MeltQuoteState::Pending | MeltQuoteState::Unknown
        ) {
            None
        } else {
            let signatures = self
                .db
                .get_blind_signatures_for_quote(quote_id)
                .await
                .map_err(|e| e.to_string())?;
            (!signatures.is_empty()).then_some(signatures)
        };
        Ok(Some(quote.into_response(change)))
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
                NotificationId::MintQuoteBolt11(uuid)
                | NotificationId::MintQuoteBolt12(uuid)
                | NotificationId::MintQuoteOnchain(uuid)
                | NotificationId::MintQuoteCustom(_, uuid) => Some(uuid.clone()),
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
                NotificationId::MeltQuoteBolt11(uuid)
                | NotificationId::MeltQuoteBolt12(uuid)
                | NotificationId::MeltQuoteOnchain(uuid)
                | NotificationId::MeltQuoteCustom(_, uuid) => {
                    // TODO: Check pending payments with the backend, as the HTTP handler does.
                    if let Some(response) = self.get_melt_quote_response(uuid).await? {
                        let event: MintEvent<QuoteId> = match (idx, response) {
                            (NotificationId::MeltQuoteBolt11(_), MeltQuoteResponse::Bolt11(r)) => {
                                r.into()
                            }
                            (NotificationId::MeltQuoteBolt12(_), MeltQuoteResponse::Bolt12(r)) => {
                                r.into()
                            }
                            (
                                NotificationId::MeltQuoteOnchain(_),
                                MeltQuoteResponse::Onchain(r),
                            ) => r.into(),
                            (
                                NotificationId::MeltQuoteCustom(method, _),
                                MeltQuoteResponse::Custom((
                                    cdk_common::PaymentMethod::Custom(stored_method),
                                    response,
                                )),
                            ) if method == &stored_method => {
                                NotificationPayload::CustomMeltQuoteResponse(
                                    stored_method,
                                    response,
                                )
                                .into()
                            }
                            _ => continue,
                        };
                        to_return.push(event);
                    }
                }
                NotificationId::MintQuoteBolt11(uuid)
                | NotificationId::MintQuoteBolt12(uuid)
                | NotificationId::MintQuoteOnchain(uuid)
                | NotificationId::MintQuoteCustom(_, uuid) => {
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
                            NotificationId::MintQuoteOnchain(_) => match mint_quote.try_into() {
                                Ok(response) => {
                                    let response: MintQuoteOnchainResponse<QuoteId> = response;
                                    response.into()
                                }
                                Err(_) => continue,
                            },
                            NotificationId::MintQuoteCustom(method, _)
                                if mint_quote.payment_method
                                    == cdk_common::PaymentMethod::Custom(method.clone()) =>
                            {
                                match MintQuoteCustomResponse::try_from(mint_quote) {
                                    Ok(response) => NotificationPayload::CustomMintQuoteResponse(
                                        method.clone(),
                                        response,
                                    )
                                    .into(),
                                    Err(_) => continue,
                                }
                            }
                            _ => continue,
                        };

                        to_return.push(mint_quote);
                    }
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

    /// The trait signature cannot carry the owning manager, so the returned
    /// spec has a dangling back-reference and payments discovered during
    /// backfill are never published. [`PubSubManager`] builds the connected
    /// spec itself; use it for anything but an isolated read-only test.
    fn new_instance(context: Self::Context) -> Arc<Self> {
        Arc::new(Self {
            db: context.0,
            payment_processors: context.1,
            pubsub_manager: Weak::new(),
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

/// Builds the distribution bus for the mint's pub/sub from a local-delivery
/// handle. Passed to `PubSubManager::new_with_bus` to send NUT-17
/// notifications across mint instances instead of keeping them in-process.
pub type MintPubSubBusBuilder =
    Box<dyn FnOnce(LocalDelivery<MintPubSubSpec>) -> Arc<dyn Bus<MintPubSubSpec>> + Send>;

/// PubsubManager
#[allow(missing_debug_implementations)]
pub struct PubSubManager(Pubsub<MintPubSubSpec>);

impl PubSubManager {
    /// Create a new instance with the default in-process bus
    pub fn new(
        context: (
            DynMintDatabase,
            Arc<HashMap<PaymentProcessorKey, DynMintPayment>>,
        ),
    ) -> Arc<Self> {
        Self::new_with_bus(context, |local| Arc::new(LocalBus::new(local)))
    }

    /// Create a new instance with a custom distribution bus
    ///
    /// Use this to run several mint instances that share notifications. The
    /// default [`PubSubManager::new`] keeps events in-process, so a WebSocket
    /// subscriber only receives events published by the instance it is
    /// connected to. A distributed bus forwards events across instances.
    pub fn new_with_bus<F>(
        context: (
            DynMintDatabase,
            Arc<HashMap<PaymentProcessorKey, DynMintPayment>>,
        ),
        build_bus: F,
    ) -> Arc<Self>
    where
        F: FnOnce(LocalDelivery<MintPubSubSpec>) -> Arc<dyn Bus<MintPubSubSpec>>,
    {
        Arc::new_cyclic(|manager| {
            Self(Pubsub::new_with_bus(
                Arc::new(MintPubSubSpec {
                    db: context.0,
                    payment_processors: context.1,
                    pubsub_manager: manager.clone(),
                }),
                build_bus,
            ))
        })
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
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Onchain) => {
                if let Ok(res) = mint_quote.clone().try_into() {
                    let res: MintQuoteOnchainResponse<QuoteId> = res;
                    self.publish(NotificationPayload::MintQuoteOnchainResponse(res));
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
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Onchain) => {
                if let Ok(res) = mint_quote.clone().try_into() {
                    let res: MintQuoteOnchainResponse<QuoteId> = res;
                    self.publish(NotificationPayload::MintQuoteOnchainResponse(res));
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
            cdk_common::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Onchain) => {
                let mut event: MeltQuoteOnchainResponse<QuoteId> = quote.clone().into();
                event.state = new_state;
                event.change = change;
                self.publish(NotificationPayload::MeltQuoteOnchainResponse(event));
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
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use cdk_common::database::DynMintDatabase;
    use cdk_common::mint::MintQuote;
    use cdk_common::payment::{
        self, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse,
        MintPayment, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse,
        SettingsResponse, WaitPaymentResponse,
    };
    use cdk_common::subscription::Params;
    use cdk_common::QuoteId;
    use futures::Stream;
    use tokio::sync::Notify;
    use tokio::time::timeout;

    use super::*;

    fn bolt11_quote(id: QuoteId, amount: u64) -> MintQuote {
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
        let first_quote_id = QuoteId::new();
        let second_quote_id = QuoteId::new();
        add_mint_quote(&db, bolt11_quote(first_quote_id.clone(), 21)).await;
        add_mint_quote(&db, bolt11_quote(second_quote_id.clone(), 34)).await;

        let spec = MintPubSubSpec::new_instance((db, Arc::new(HashMap::new())));
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

    fn melt_quote(method: cdk_common::PaymentMethod, state: MeltQuoteState) -> MeltQuote {
        use cdk_common::mint::MeltPaymentRequest;
        use cdk_common::nut00::KnownMethod;

        let request = match &method {
            cdk_common::PaymentMethod::Known(KnownMethod::Bolt11) => MeltPaymentRequest::Bolt11 {
                bolt11: cdk_fake_wallet::create_fake_invoice(21_000, "backfill".to_owned()),
            },
            cdk_common::PaymentMethod::Known(KnownMethod::Bolt12) => {
                let key = cdk_common::SecretKey::generate().public_key();
                let offer = lightning::offers::offer::OfferBuilder::new(
                    bitcoin::secp256k1::PublicKey::from_slice(&key.to_bytes()).expect("public key"),
                )
                .build()
                .expect("offer");
                MeltPaymentRequest::Bolt12 {
                    offer: Box::new(offer),
                }
            }
            cdk_common::PaymentMethod::Known(KnownMethod::Onchain) => MeltPaymentRequest::Onchain {
                address: "bcrt1qtest".to_owned(),
            },
            cdk_common::PaymentMethod::Custom(method) => MeltPaymentRequest::Custom {
                method: method.clone(),
                request: "custom-payment".to_owned(),
            },
        };
        let mut quote = MeltQuote::new(
            None,
            request,
            CurrencyUnit::Sat,
            Amount::new(21, CurrencyUnit::Sat),
            Amount::new(2, CurrencyUnit::Sat),
            0,
            None,
            None,
            method,
            None,
            Some(1),
        );
        quote.state = state;
        if state == MeltQuoteState::Paid {
            quote.payment_proof = Some("payment-proof".to_owned());
        }
        quote
    }

    async fn add_melt_quote(
        db: &DynMintDatabase,
        quote: MeltQuote,
        with_change: bool,
    ) -> Option<Vec<BlindSignature>> {
        let mut tx = db.begin_transaction().await.expect("begin transaction");
        tx.add_melt_quote(quote.clone())
            .await
            .expect("add melt quote");
        let change = if with_change {
            let signature = BlindSignature {
                amount: 2.into(),
                keyset_id: "009a1f293253e41e".parse().expect("keyset id"),
                c: cdk_common::SecretKey::generate().public_key(),
                dleq: None,
            };
            let signatures = vec![signature];
            tx.add_blind_signatures(
                &[cdk_common::SecretKey::generate().public_key()],
                &signatures,
                Some(quote.id),
            )
            .await
            .expect("add change signatures");
            Some(signatures)
        } else {
            None
        };
        tx.commit().await.expect("commit transaction");
        change
    }

    #[tokio::test]
    async fn get_events_from_db_returns_persisted_melt_change() {
        use cdk_common::nut00::KnownMethod;

        let db: DynMintDatabase =
            Arc::new(cdk_sqlite::mint::memory::empty().await.expect("database"));
        let spec = MintPubSubSpec::new_instance((db.clone(), Arc::new(HashMap::new())));
        for method in [
            KnownMethod::Bolt11,
            KnownMethod::Bolt12,
            KnownMethod::Onchain,
        ] {
            for (state, with_change) in [
                (MeltQuoteState::Paid, true),
                (MeltQuoteState::Paid, false),
                (MeltQuoteState::Pending, true),
            ] {
                let quote = melt_quote(cdk_common::PaymentMethod::Known(method), state);
                let stored_change = add_melt_quote(&db, quote.clone(), with_change).await;
                let topic = match method {
                    KnownMethod::Bolt11 => NotificationId::MeltQuoteBolt11(quote.id.clone()),
                    KnownMethod::Bolt12 => NotificationId::MeltQuoteBolt12(quote.id.clone()),
                    KnownMethod::Onchain => NotificationId::MeltQuoteOnchain(quote.id.clone()),
                };
                let events = spec.get_events_from_db(&[topic]).await.expect("backfill");
                assert_eq!(events.len(), 1);
                let (id, actual_state, change) = match events[0].inner() {
                    NotificationPayload::MeltQuoteBolt11Response(r) => {
                        assert_eq!(r.payment_preimage, quote.payment_proof);
                        (&r.quote, r.state, &r.change)
                    }
                    NotificationPayload::MeltQuoteBolt12Response(r) => {
                        assert_eq!(r.payment_preimage, quote.payment_proof);
                        (&r.quote, r.state, &r.change)
                    }
                    NotificationPayload::MeltQuoteOnchainResponse(r) => {
                        (&r.quote, r.state, &r.change)
                    }
                    payload => panic!("unexpected payload: {payload:?}"),
                };
                assert_eq!(id, &quote.id);
                assert_eq!(actual_state, state);
                let expected = if state == MeltQuoteState::Paid {
                    stored_change
                } else {
                    None
                };
                assert_eq!(change, &expected);
            }
        }
    }

    #[tokio::test]
    async fn get_events_from_db_backfills_custom_mint_quotes() {
        let db: DynMintDatabase =
            Arc::new(cdk_sqlite::mint::memory::empty().await.expect("database"));
        let method = "test_method".to_owned();
        let mut quote = bolt11_quote(QuoteId::new(), 21);
        quote.payment_method = cdk_common::PaymentMethod::Custom(method.clone());
        quote.extra_json = Some(serde_json::json!({"receipt": "mint-receipt"}));
        add_mint_quote(&db, quote.clone()).await;
        // Model a recently checked payment; no backend call is needed for this snapshot.
        assert!(db
            .try_update_mint_quote_last_checked(&quote.id, cdk_common::util::unix_time(), 0)
            .await
            .expect("record payment check"));
        let spec = MintPubSubSpec::new_instance((db, Arc::new(HashMap::new())));
        let events = spec
            .get_events_from_db(&[
                NotificationId::MintQuoteCustom(method.clone(), quote.id.clone()),
                NotificationId::MintQuoteCustom("wrong_method".to_owned(), quote.id.clone()),
                NotificationId::MintQuoteCustom(method.clone(), QuoteId::new()),
            ])
            .await
            .expect("backfill");
        assert_eq!(events.len(), 1);
        match events[0].inner() {
            NotificationPayload::CustomMintQuoteResponse(actual_method, response) => {
                assert_eq!(actual_method, &method);
                assert_eq!(response.quote, quote.id);
                assert_eq!(response.method, quote.payment_method);
                assert_eq!(response.amount_paid, Amount::from(21));
                assert_eq!(response.amount_issued, Amount::ZERO);
                assert_eq!(response.extra, quote.extra_json.expect("extra fields"));
            }
            payload => panic!("unexpected payload: {payload:?}"),
        }
    }

    #[tokio::test]
    async fn get_events_from_db_backfills_custom_melt_quotes() {
        let db: DynMintDatabase =
            Arc::new(cdk_sqlite::mint::memory::empty().await.expect("database"));
        let method = "test_method".to_owned();
        let mut quote = melt_quote(
            cdk_common::PaymentMethod::Custom(method.clone()),
            MeltQuoteState::Paid,
        );
        quote.extra_json = Some(serde_json::json!({"receipt": "melt-receipt"}));
        let change = add_melt_quote(&db, quote.clone(), true).await;
        let spec = MintPubSubSpec::new_instance((db, Arc::new(HashMap::new())));
        let events = spec
            .get_events_from_db(&[
                NotificationId::MeltQuoteCustom(method.clone(), quote.id.clone()),
                NotificationId::MeltQuoteCustom("wrong_method".to_owned(), quote.id.clone()),
                NotificationId::MeltQuoteCustom(method.clone(), QuoteId::new()),
            ])
            .await
            .expect("backfill");
        assert_eq!(events.len(), 1);
        match events[0].inner() {
            NotificationPayload::CustomMeltQuoteResponse(actual_method, response) => {
                assert_eq!(actual_method, &method);
                assert_eq!(response.quote, quote.id);
                assert_eq!(response.method, quote.payment_method);
                assert_eq!(response.state, MeltQuoteState::Paid);
                assert_eq!(response.payment_preimage, quote.payment_proof);
                assert_eq!(response.change, change);
                assert_eq!(response.extra, quote.extra_json.expect("extra fields"));
            }
            payload => panic!("unexpected payload: {payload:?}"),
        }
    }

    #[derive(Default)]
    struct BlockingPaymentBackend {
        entered: Notify,
        release: Notify,
        checks: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl MintPayment for BlockingPaymentBackend {
        type Err = payment::Error;

        async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
            Err(payment::Error::UnsupportedPaymentOption)
        }

        async fn create_incoming_payment_request(
            &self,
            _options: IncomingPaymentOptions,
        ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
            Err(payment::Error::UnsupportedPaymentOption)
        }

        async fn get_payment_quote(
            &self,
            _unit: &CurrencyUnit,
            _options: OutgoingPaymentOptions,
        ) -> Result<PaymentQuoteResponse, Self::Err> {
            Err(payment::Error::UnsupportedPaymentOption)
        }

        async fn make_payment(
            &self,
            _unit: &CurrencyUnit,
            _options: OutgoingPaymentOptions,
        ) -> Result<MakePaymentResponse, Self::Err> {
            Err(payment::Error::UnsupportedPaymentOption)
        }

        async fn wait_payment_event(
            &self,
        ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
            Ok(Box::pin(futures::stream::pending()))
        }

        fn is_payment_event_stream_active(&self) -> bool {
            false
        }

        fn cancel_payment_event_stream(&self) {}

        async fn check_incoming_payment_status(
            &self,
            payment_identifier: &PaymentIdentifier,
        ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
            self.checks.fetch_add(1, Ordering::Relaxed);
            self.entered.notify_one();
            self.release.notified().await;
            Ok(vec![WaitPaymentResponse {
                payment_identifier: payment_identifier.clone(),
                payment_amount: Amount::new(21, CurrencyUnit::Sat),
                payment_id: "backfill-payment".to_owned(),
            }])
        }

        async fn check_outgoing_payment(
            &self,
            _payment_identifier: &PaymentIdentifier,
        ) -> Result<MakePaymentResponse, Self::Err> {
            Err(payment::Error::UnsupportedPaymentOption)
        }
    }

    #[tokio::test]
    async fn backfill_payment_transition_notifies_concurrent_subscriber() {
        use cdk_common::nut00::KnownMethod;
        use cdk_common::nut17::Kind;
        use cdk_common::PaymentMethod;

        fn with_local_bus(context: <MintPubSubSpec as Spec>::Context) -> Arc<PubSubManager> {
            PubSubManager::new_with_bus(context, |local| Arc::new(LocalBus::new(local)))
        }

        let builders: [fn(_) -> _; 2] = [PubSubManager::new, with_local_bus];

        for build_manager in builders {
            for (method, kind) in [
                (
                    PaymentMethod::Known(KnownMethod::Bolt11),
                    Kind::Bolt11MintQuote,
                ),
                (
                    PaymentMethod::Custom("test_method".to_owned()),
                    Kind::Custom("test_method_mint_quote".to_owned()),
                ),
            ] {
                timeout(Duration::from_secs(5), async {
                    let db: DynMintDatabase =
                        Arc::new(cdk_sqlite::mint::memory::empty().await.expect("database"));
                    let mut quote = bolt11_quote(QuoteId::new(), 21);
                    quote.payment_method = method.clone();
                    let mut tx = db.begin_transaction().await.expect("transaction");
                    tx.add_mint_quote(quote.clone())
                        .await
                        .expect("unpaid quote");
                    tx.commit().await.expect("commit");

                    let backend = Arc::new(BlockingPaymentBackend::default());
                    let processors = HashMap::from([(
                        PaymentProcessorKey::new(CurrencyUnit::Sat, method),
                        backend.clone() as DynMintPayment,
                    )]);
                    let manager = build_manager((db.clone(), Arc::new(processors)));
                    let params = Params {
                        kind,
                        filters: vec![quote.id.to_string()],
                        id: Arc::new(SubId::from("first")),
                    };
                    let mut first = manager
                        .subscribe(params.clone())
                        .expect("first subscription");
                    backend.entered.notified().await;

                    let mut second = manager
                        .subscribe(Params {
                            id: Arc::new(SubId::from("second")),
                            ..params
                        })
                        .expect("second subscription");
                    let initial = second.recv().await.expect("unpaid backfill");
                    match initial.inner() {
                        NotificationPayload::MintQuoteBolt11Response(r) => {
                            assert_eq!(r.state, MintQuoteState::Unpaid)
                        }
                        NotificationPayload::CustomMintQuoteResponse(_, r) => {
                            assert_eq!(r.amount_paid, Amount::ZERO)
                        }
                        payload => panic!("unexpected payload: {payload:?}"),
                    }
                    assert_eq!(backend.checks.load(Ordering::Relaxed), 1);

                    backend.release.notify_one();
                    for subscriber in [&mut first, &mut second] {
                        let event = subscriber.recv().await.expect("paid notification");
                        match event.inner() {
                            NotificationPayload::MintQuoteBolt11Response(r) => {
                                assert_eq!(r.quote, quote.id);
                                assert_eq!(r.state, MintQuoteState::Paid);
                            }
                            NotificationPayload::CustomMintQuoteResponse(method, r) => {
                                assert_eq!(method, "test_method");
                                assert_eq!(r.quote, quote.id);
                                assert_eq!(r.amount_paid, Amount::from(21));
                                assert_eq!(r.amount_issued, Amount::ZERO);
                            }
                            payload => panic!("unexpected payload: {payload:?}"),
                        }
                        let stored = db
                            .get_mint_quote(&quote.id)
                            .await
                            .expect("read quote")
                            .expect("quote");
                        assert_eq!(stored.amount_paid(), Amount::new(21, CurrencyUnit::Sat));
                    }
                    assert_eq!(backend.checks.load(Ordering::Relaxed), 1);
                })
                .await
                .expect("both subscribers must observe the committed payment");
            }
        }
    }

    #[tokio::test]
    async fn pubsub_manager_does_not_retain_its_database() {
        let db: DynMintDatabase =
            Arc::new(cdk_sqlite::mint::memory::empty().await.expect("database"));
        let db_weak = Arc::downgrade(&db);
        let manager = PubSubManager::new((db, Arc::new(HashMap::new())));
        let manager_weak = Arc::downgrade(&manager);
        drop(manager);
        assert!(manager_weak.upgrade().is_none());
        assert!(db_weak.upgrade().is_none());
    }
}

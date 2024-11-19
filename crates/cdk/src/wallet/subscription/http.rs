use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, RwLock};
use tokio::time;

use super::WsSubscriptionBody;
use crate::nuts::nut17::Kind;
use crate::nuts::{nut01, nut04, nut05, nut07, CheckStateRequest, NotificationPayload};
use crate::pub_sub::SubId;
use crate::wallet::client::MintConnector;

#[derive(Debug, Hash, PartialEq, Eq)]
enum UrlType {
    Mint(String),
    Melt(String),
    PublicKey(nut01::PublicKey),
}

#[derive(Debug, Eq, PartialEq)]
enum AnyState {
    MintQuoteState(nut04::QuoteState),
    MeltQuoteState(nut05::QuoteState),
    PublicKey(nut07::State),
    Empty,
}

type SubscribedTo = HashMap<UrlType, (mpsc::Sender<NotificationPayload<String>>, SubId, AnyState)>;

async fn convert_subscription(
    sub_id: SubId,
    subscriptions: &Arc<RwLock<HashMap<SubId, WsSubscriptionBody>>>,
    subscribed_to: &mut SubscribedTo,
) -> Option<()> {
    let subscription = subscriptions.read().await;
    let sub = subscription.get(&sub_id)?;
    tracing::debug!("New subscription: {:?}", sub);
    match sub.1.kind {
        Kind::Bolt11MintQuote => {
            for id in sub.1.filters.iter().map(|id| UrlType::Mint(id.clone())) {
                subscribed_to.insert(id, (sub.0.clone(), sub.1.id.clone(), AnyState::Empty));
            }
        }
        Kind::Bolt11MeltQuote => {
            for id in sub.1.filters.iter().map(|id| UrlType::Melt(id.clone())) {
                subscribed_to.insert(id, (sub.0.clone(), sub.1.id.clone(), AnyState::Empty));
            }
        }
        Kind::ProofState => {
            for id in sub
                .1
                .filters
                .iter()
                .map(|id| nut01::PublicKey::from_hex(id).map(UrlType::PublicKey))
            {
                match id {
                    Ok(id) => {
                        subscribed_to
                            .insert(id, (sub.0.clone(), sub.1.id.clone(), AnyState::Empty));
                    }
                    Err(err) => {
                        tracing::error!("Error parsing public key: {:?}. Subscription ignored, will never yield any result", err);
                    }
                }
            }
        }
    }

    Some(())
}

#[allow(clippy::incompatible_msrv)]
#[inline]
pub async fn http_main<S: IntoIterator<Item = SubId>>(
    initial_state: S,
    http_client: Arc<dyn MintConnector + Send + Sync>,
    subscriptions: Arc<RwLock<HashMap<SubId, WsSubscriptionBody>>>,
    mut new_subscription_recv: mpsc::Receiver<SubId>,
    mut on_drop: mpsc::Receiver<SubId>,
) {
    let mut interval = time::interval(Duration::from_secs(2));
    let mut subscribed_to = HashMap::<UrlType, (mpsc::Sender<_>, _, AnyState)>::new();

    for sub_id in initial_state {
        convert_subscription(sub_id, &subscriptions, &mut subscribed_to).await;
    }

    loop {
        tokio::select! {
            _ = interval.tick() => {
                for (url, (sender, _, last_state)) in subscribed_to.iter_mut() {
                    tracing::debug!("Polling: {:?}", url);
                    match url {
                        UrlType::Mint(id) => {
                            let response = http_client.get_mint_quote_status(id).await;
                            if let Ok(response) = response {
                                if *last_state == AnyState::MintQuoteState(response.state) {
                                    continue;
                                }
                                *last_state = AnyState::MintQuoteState(response.state);
                                if let Err(err) = sender.try_send(NotificationPayload::MintQuoteBolt11Response(response)) {
                                    tracing::error!("Error sending mint quote response: {:?}", err);
                                }
                            }
                        }
                        UrlType::Melt(id) => {
                            let response = http_client.get_melt_quote_status(id).await;
                            if let Ok(response) = response {
                                if *last_state == AnyState::MeltQuoteState(response.state) {
                                    continue;
                                }
                                *last_state = AnyState::MeltQuoteState(response.state);
                                if let Err(err) =  sender.try_send(NotificationPayload::MeltQuoteBolt11Response(response)) {
                                    tracing::error!("Error sending melt quote response: {:?}", err);
                                }
                            }
                        }
                        UrlType::PublicKey(id) => {
                            let responses = http_client.post_check_state(CheckStateRequest {
                                ys: vec![*id],
                            }).await;
                            if let Ok(mut responses) = responses {
                                let response = if let Some(state) = responses.states.pop() {
                                    state
                                } else {
                                    continue;
                                };

                                if *last_state == AnyState::PublicKey(response.state) {
                                    continue;
                                }
                                *last_state = AnyState::PublicKey(response.state);
                                if let Err(err) = sender.try_send(NotificationPayload::ProofState(response)) {
                                    tracing::error!("Error sending proof state response: {:?}", err);
                                }
                            }
                        }
                    }
                }
            }
            Some(subid) = new_subscription_recv.recv() => {
                convert_subscription(subid, &subscriptions, &mut subscribed_to).await;
            }
            Some(id) = on_drop.recv() => {
                subscribed_to.retain(|_, (_, sub_id, _)| *sub_id != id);
            }
        }
    }
}

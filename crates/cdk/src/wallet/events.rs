use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

use cdk_common::mint_url::MintUrl;
use cdk_common::pub_sub::index::{self, Indexable};
use cdk_common::pub_sub::{OnNewSubscription, SubId};
use cdk_common::{Amount, CurrencyUnit, State};
use tokio::sync::RwLock;

#[cfg(not(target_arch = "wasm32"))]
use super::ProofsMethods;
use super::Wallet;
#[cfg(not(target_arch = "wasm32"))]
use crate::pub_sub;
use crate::pub_sub::ActiveSubscription;

/// The internal event is `()` because the events() will accept no filter, all events all sent to
/// all subscriber with no option to filter.
///
/// To change this, change this type to an enum or something
type EventFilter = ();

/// Event types
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Balance(MintUrl, CurrencyUnit, State, Amount),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum EventId {
    Balance(MintUrl, CurrencyUnit, State),
}

impl Event {
    pub fn get_id(&self) -> EventId {
        match self {
            Event::Balance(url, currency_unit, state, _) => {
                EventId::Balance(url.to_owned(), currency_unit.to_owned(), state.to_owned())
            }
        }
    }
}

impl Indexable for Event {
    type Index = EventFilter;

    fn to_indexes(&self) -> Vec<index::Index<Self::Index>> {
        vec![index::Index::from(())]
    }
}

/// Keep in memory the latest events and send it over back to new subscribers
#[derive(Debug, Default)]
pub struct EventStore {
    last_events: RwLock<HashMap<EventId, Event>>,
}

#[async_trait::async_trait]
impl OnNewSubscription for EventStore {
    type Event = Event;
    type Index = EventFilter;

    async fn on_new_subscription(
        &self,
        _request: &[&Self::Index],
    ) -> Result<Vec<Self::Event>, String> {
        Ok(self
            .last_events
            .read()
            .await
            .values()
            .map(|x| x.to_owned())
            .collect())
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// The event manager is an alias manager
pub type EventManager = pub_sub::Manager<Event, EventStore>;

/// An type to subscribe, meaningless until EventFilter is an enum
#[derive(Debug, Default)]
struct SubscribeToAllEvents {
    inner: SubId,
}

impl AsRef<SubId> for SubscribeToAllEvents {
    fn as_ref(&self) -> &SubId {
        &self.inner
    }
}

impl From<SubscribeToAllEvents> for Vec<index::Index<EventFilter>> {
    fn from(_val: SubscribeToAllEvents) -> Self {
        vec![().into()]
    }
}

impl Wallet {
    /// Internal function to trigger an event. This function is private and must be called from
    /// within itself.
    #[inline(always)]
    #[cfg(not(target_arch = "wasm32"))]
    async fn trigger_events(event_manager: Arc<EventManager>, events: Vec<Event>) {
        let events = if let Some(event_store) = event_manager.on_new_subscription() {
            let mut last_events = event_store.last_events.write().await;

            events
                .into_iter()
                .filter_map(|event| {
                    if let Some(previous) = last_events.insert(event.get_id(), event.clone()) {
                        if previous == event {
                            // do nothing
                            return None;
                        }
                    }

                    Some(event)
                })
                .collect()
        } else {
            events
        };

        events
            .into_iter()
            .for_each(|event| event_manager.broadcast(event));
    }

    /// Notify all balances, because it is likely it has changed
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn notify_update_balance(&self) {}

    /// Notify all balances, because it is likely it has changed
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn notify_update_balance(&self) {
        let db = self.localstore.clone();
        let event_manager = self.event_manager.clone();
        let mint_url = self.mint_url.clone();
        let unit = self.unit.clone();

        if event_manager.active_subscriptions() == 0 {
            // Do not query the db is there are no listeners
            return;
        }

        tokio::spawn(async move {
            let unspent = db.get_proofs(
                Some(mint_url.clone()),
                Some(unit.clone()),
                Some(vec![State::Unspent]),
                None,
            );
            let reserved = db.get_proofs(
                Some(mint_url.clone()),
                Some(unit.clone()),
                Some(vec![State::Reserved]),
                None,
            );
            let pending = db.get_proofs(
                Some(mint_url.clone()),
                Some(unit.clone()),
                Some(vec![State::Pending]),
                None,
            );
            let (unspent, reserved, pending) = tokio::join!(unspent, reserved, pending);

            let events = vec![
                unspent.map(|x| {
                    x.into_iter()
                        .map(|x| x.proof)
                        .collect::<Vec<_>>()
                        .total_amount()
                        .map(|total| {
                            Event::Balance(mint_url.clone(), unit.clone(), State::Unspent, total)
                        })
                }),
                reserved.map(|x| {
                    x.into_iter()
                        .map(|x| x.proof)
                        .collect::<Vec<_>>()
                        .total_amount()
                        .map(|total| {
                            Event::Balance(mint_url.clone(), unit.clone(), State::Reserved, total)
                        })
                }),
                pending.map(|x| {
                    x.into_iter()
                        .map(|x| x.proof)
                        .collect::<Vec<_>>()
                        .total_amount()
                        .map(|total| {
                            Event::Balance(mint_url.clone(), unit.clone(), State::Pending, total)
                        })
                }),
            ]
            .into_iter()
            .filter_map(|event| event.ok()?.ok())
            .collect();

            Self::trigger_events(event_manager.clone(), events).await;
        });
    }

    /// Subscribe to wallet events
    #[cfg(target_arch = "wasm32")]
    pub async fn events(&self) -> Result<ActiveSubscription<Event, EventFilter>, super::Error> {
        Err(super::Error::NotSupported)
    }

    /// Subscribe to wallet events
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn events(&self) -> Result<ActiveSubscription<Event, EventFilter>, super::Error> {
        Ok(self
            .event_manager
            .subscribe(SubscribeToAllEvents::default())
            .await)
    }
}

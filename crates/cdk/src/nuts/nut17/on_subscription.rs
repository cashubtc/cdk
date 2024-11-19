//! On Subscription
//!
//! This module contains the code that is triggered when a new subscription is created.
use std::sync::Arc;

use uuid::Uuid;

use super::{Notification, NotificationPayload};
use crate::cdk_database::{self, MintDatabase};
use crate::nuts::{MeltQuoteBolt11Response, MintQuoteBolt11Response, ProofState, PublicKey};
use crate::pub_sub::OnNewSubscription;

#[derive(Default)]
/// Subscription Init
///
/// This struct triggers code when a new subscription is created.
///
/// It is used to send the initial state of the subscription to the client.
pub struct OnSubscription(
    pub(crate) Option<Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>>,
);

#[async_trait::async_trait]
impl OnNewSubscription for OnSubscription {
    type Event = NotificationPayload<Uuid>;
    type Index = Notification;

    async fn on_new_subscription(
        &self,
        request: &[&Self::Index],
    ) -> Result<Vec<Self::Event>, String> {
        let datastore = if let Some(localstore) = self.0.as_ref() {
            localstore
        } else {
            return Ok(vec![]);
        };

        let mut to_return = vec![];
        let mut public_keys: Vec<PublicKey> = Vec::new();
        let mut melt_queries = Vec::new();
        let mut mint_queries = Vec::new();

        for idx in request.iter() {
            match idx {
                Notification::ProofState(pk) => public_keys.push(*pk),
                Notification::MeltQuoteBolt11(uuid) => {
                    melt_queries.push(datastore.get_melt_quote(uuid))
                }
                Notification::MintQuoteBolt11(uuid) => {
                    mint_queries.push(datastore.get_mint_quote(uuid))
                }
            }
        }

        to_return.extend(
            futures::future::try_join_all(melt_queries)
                .await
                .map(|quotes| {
                    quotes
                        .into_iter()
                        .filter_map(|quote| quote.map(|x| x.into()))
                        .map(|x: MeltQuoteBolt11Response<Uuid>| x.into())
                        .collect::<Vec<_>>()
                })
                .map_err(|e| e.to_string())?,
        );
        to_return.extend(
            futures::future::try_join_all(mint_queries)
                .await
                .map(|quotes| {
                    quotes
                        .into_iter()
                        .filter_map(|quote| quote.map(|x| x.into()))
                        .map(|x: MintQuoteBolt11Response<Uuid>| x.into())
                        .collect::<Vec<_>>()
                })
                .map_err(|e| e.to_string())?,
        );

        to_return.extend(
            datastore
                .get_proofs_states(public_keys.as_slice())
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .enumerate()
                .filter_map(|(idx, state)| state.map(|state| (public_keys[idx], state).into()))
                .map(|state: ProofState| state.into()),
        );

        Ok(to_return)
    }
}

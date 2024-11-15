//! On Subscription
//!
//! This module contains the code that is triggered when a new subscription is created.
use std::collections::HashMap;
use std::sync::Arc;

use super::{Kind, NotificationPayload};
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
    type Event = NotificationPayload;
    type Index = (String, Kind);

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

        for (kind, values) in request.iter().fold(
            HashMap::new(),
            |mut acc: HashMap<&Kind, Vec<&String>>, (data, kind)| {
                acc.entry(kind).or_default().push(data);
                acc
            },
        ) {
            match kind {
                Kind::Bolt11MeltQuote => {
                    let queries = values
                        .iter()
                        .map(|id| datastore.get_melt_quote(id))
                        .collect::<Vec<_>>();

                    to_return.extend(
                        futures::future::try_join_all(queries)
                            .await
                            .map(|quotes| {
                                quotes
                                    .into_iter()
                                    .filter_map(|quote| quote.map(|x| x.into()))
                                    .map(|x: MeltQuoteBolt11Response| x.into())
                                    .collect::<Vec<_>>()
                            })
                            .map_err(|e| e.to_string())?,
                    );
                }
                Kind::Bolt11MintQuote => {
                    let queries = values
                        .iter()
                        .map(|id| datastore.get_mint_quote(id))
                        .collect::<Vec<_>>();

                    to_return.extend(
                        futures::future::try_join_all(queries)
                            .await
                            .map(|quotes| {
                                quotes
                                    .into_iter()
                                    .filter_map(|quote| quote.map(|x| x.into()))
                                    .map(|x: MintQuoteBolt11Response| x.into())
                                    .collect::<Vec<_>>()
                            })
                            .map_err(|e| e.to_string())?,
                    );
                }
                Kind::ProofState => {
                    let public_keys = values
                        .iter()
                        .map(PublicKey::from_hex)
                        .collect::<Result<Vec<PublicKey>, _>>()
                        .map_err(|e| e.to_string())?;

                    to_return.extend(
                        datastore
                            .get_proofs_states(&public_keys)
                            .await
                            .map_err(|e| e.to_string())?
                            .into_iter()
                            .enumerate()
                            .filter_map(|(idx, state)| {
                                state.map(|state| (public_keys[idx], state).into())
                            })
                            .map(|state: ProofState| state.into()),
                    );
                }
            }
        }

        Ok(to_return)
    }
}

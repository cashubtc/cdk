//! On Subscription
//!
//! This module contains the code that is triggered when a new subscription is created.
use std::sync::Arc;

use cdk_common::database::{self, MintDatabase};
use cdk_common::nut17::Notification;
use cdk_common::pub_sub::OnNewSubscription;
use cdk_common::{MintQuoteBolt12Response, NotificationPayload, PaymentMethod};
use uuid::Uuid;

use crate::nuts::{
    MeltQuoteBolt11Response, MeltQuoteOnchainResponse, MintQuoteBolt11Response,
    MintQuoteOnchainResponse, ProofState, PublicKey,
};

#[derive(Default)]
/// Subscription Init
///
/// This struct triggers code when a new subscription is created.
///
/// It is used to send the initial state of the subscription to the client.
pub struct OnSubscription(pub(crate) Option<Arc<dyn MintDatabase<database::Error> + Send + Sync>>);

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
                Notification::MintQuoteBolt12(uuid) => {
                    mint_queries.push(datastore.get_mint_quote(uuid))
                }
                Notification::MeltQuoteBolt12(uuid) => {
                    melt_queries.push(datastore.get_melt_quote(uuid))
                }
                Notification::MintQuoteOnchain(uuid) => {
                    mint_queries.push(datastore.get_mint_quote(uuid))
                }
                Notification::MeltQuoteOnchain(uuid) => {
                    melt_queries.push(datastore.get_melt_quote(uuid))
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
                            .filter_map(|quote| {
                                quote.and_then(|x| match x.payment_method {
                                    PaymentMethod::Bolt11 => {
                                        let response: MeltQuoteBolt11Response<Uuid> = x.into();
                                        Some(response.into())
                                    }
                                    PaymentMethod::Bolt12 => {
                                        let response: MeltQuoteBolt11Response<Uuid> = x.into();
                                        Some(response.into())
                                    }
                                    PaymentMethod::Onchain => {
                                        let response: MeltQuoteOnchainResponse<Uuid> = x.into();
                                        Some(response.into())
                                    }
                                    PaymentMethod::Custom(_) => None,
                                })
                            })
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
                                        let response: MintQuoteBolt11Response<Uuid> = x.into();
                                        Some(response.into())
                                    }
                                    PaymentMethod::Bolt12 => match x.try_into() {
                                        Ok(response) => {
                                            let response: MintQuoteBolt12Response<Uuid> = response;
                                            Some(response.into())
                                        }
                                        Err(_) => None,
                                    },
                                    PaymentMethod::Onchain => match x.try_into() {
                                        Ok(response) => {
                                            let response: MintQuoteOnchainResponse<Uuid> = response;
                                            Some(response.into())
                                        }
                                        Err(_) => None,
                                    },
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
                datastore
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

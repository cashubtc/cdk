use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use cdk::mint::QuoteId;
use cdk::nuts::nut17::NotificationPayload;
use cdk::subscription::SubId;
use cdk::ws::{
    notification_to_ws_message, NotificationInner, WsErrorBody, WsMessageOrResponse,
    WsMethodRequest, WsRequest,
};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::MintState;

mod error;
mod subscribe;
mod unsubscribe;

async fn process(
    context: &mut WsContext,
    body: WsRequest,
) -> Result<serde_json::Value, serde_json::Error> {
    let response = match body.method {
        WsMethodRequest::Subscribe(sub) => subscribe::handle(context, sub).await,
        WsMethodRequest::Unsubscribe(unsub) => unsubscribe::handle(context, unsub).await,
    }
    .map_err(WsErrorBody::from);

    let response: WsMessageOrResponse = (body.id, response).into();

    serde_json::to_value(response)
}

pub use error::WsError;

pub struct WsContext {
    state: MintState,
    subscriptions: HashMap<Arc<SubId>, tokio::task::JoinHandle<()>>,
    publisher: mpsc::Sender<(Arc<SubId>, NotificationPayload<QuoteId>)>,
}

impl Drop for WsContext {
    fn drop(&mut self) {
        for (_, handle) in self.subscriptions.drain() {
            handle.abort();
        }
    }
}

/// Main function for websocket connections
///
/// This function will handle all incoming websocket connections and keep them in their own loop.
///
/// For simplicity sake this function will spawn tasks for each subscription and
/// keep them in a hashmap, and will have a single subscriber for all of them.
pub async fn main_websocket(mut socket: WebSocket, state: MintState) {
    let (publisher, mut subscriber) = mpsc::channel(100);
    let mut context = WsContext {
        state,
        subscriptions: HashMap::new(),
        publisher,
    };

    loop {
        tokio::select! {
            Some((sub_id, payload)) = subscriber.recv() => {
                if !context.subscriptions.contains_key(&sub_id) {
                    // It may be possible an incoming message has come from a dropped Subscriptions that has not yet been
                    // unsubscribed from the subscription manager, just ignore it.
                    continue;
                }
                let notification = notification_to_ws_message(NotificationInner {
                    sub_id,
                    payload,
                });
                let message = match serde_json::to_string(&notification) {
                    Ok(message) => message,
                    Err(err) => {
                        tracing::error!("Could not serialize notification: {}", err);
                        continue;
                    }
                };

                if let Err(err)= socket.send(Message::Text(message.into())).await {
                    tracing::error!("Could not send websocket message: {}", err);
                    break;
                }
            }

            Some(from_ws) = socket.next() => {
                let text = match from_ws {
                    Ok(Message::Text(text)) => text.to_string(),
                    Ok(Message::Binary(bin)) => String::from_utf8_lossy(&bin).to_string(),
                    Ok(Message::Ping(payload)) => {
                        // Reply with Pong with same payload
                        if let Err(e) = socket.send(Message::Pong(payload)).await {
                            tracing::error!("failed to send pong: {e}");
                            break;
                        }
                        continue;
                    },
                    Ok(Message::Pong(_payload)) => {
                        tracing::error!("Unexpected pong");
                        continue;
                    },
                    Ok(Message::Close(frame)) => {
                        if let Some(CloseFrame { code, reason }) = frame {
                            tracing::info!("ws-close: code={code:?} reason='{reason}'");
                        } else {
                            tracing::info!("ws-close: no frame");
                        }

                        let _ = socket.send(Message::Close(Some(CloseFrame {
                            code: axum::extract::ws::close_code::NORMAL,
                            reason: "bye!".into(),
                        }))).await;
                        break;
                    }
                    Err(err) => {
                        tracing::error!("ws-error: {err}");
                        break;
                    }
                };


                let request = match serde_json::from_str::<WsRequest>(&text) {
                    Ok(request) => request,
                    Err(err) => {
                        tracing::error!("Could not parse request: {}", err);
                        continue;
                    }
                };

                match process(&mut context, request).await {
                    Ok(result) => {
                        if let Err(err) = socket
                            .send(Message::Text(result.to_string().into()))
                            .await
                        {
                            tracing::error!("Could not send request: {}", err);
                            break;
                        }
                    }
                    Err(err) => {
                        tracing::error!("Error serializing response: {}", err);
                        break;
                    }
                }
            }
            else =>  {
                // Unexpected, we should exit the loop
                tracing::warn!("Unexpected event, closing ws");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use cdk::mint::{Mint, QuoteId};
    use cdk::nuts::nut02::KeySetVersion;
    use cdk::nuts::{CurrencyUnit, MintInfo};
    use cdk::subscription::{Params, SubId};
    use cdk::ws::WsUnsubscribeRequest;
    use cdk_signatory::db_signatory::DbSignatory;
    use cdk_signatory::signatory::{RotateKeyArguments, Signatory};
    use cdk_sqlite::mint::memory;

    use super::*;
    use crate::cache::HttpCache;

    async fn create_test_mint() -> Arc<Mint> {
        let localstore = Arc::new(memory::empty().await.expect("in-memory db"));

        let seed = [0u8; 32];
        let mut supported_units = HashMap::new();
        let amounts: Vec<u64> = (0..8).map(|i| 2u64.pow(i)).collect();
        supported_units.insert(CurrencyUnit::Sat, (0u64, amounts));

        let signatory = Arc::new(
            DbSignatory::new(
                localstore.clone(),
                &seed,
                supported_units.clone(),
                HashMap::new(),
            )
            .await
            .expect("signatory"),
        );

        for (unit, (fee, amounts)) in &supported_units {
            signatory
                .rotate_keyset(RotateKeyArguments {
                    unit: unit.clone(),
                    amounts: amounts.clone(),
                    input_fee_ppk: *fee,
                    keyset_id_type: KeySetVersion::Version00,
                    final_expiry: None,
                })
                .await
                .expect("rotate keyset");
        }

        Arc::new(
            Mint::new(
                MintInfo::default(),
                signatory,
                localstore,
                HashMap::new(),
                1000,
                1000,
            )
            .await
            .expect("mint"),
        )
    }

    fn make_params(sub_id: &str) -> Params {
        // A non-empty filter is required so the subscription is registered in
        // the TopicTree and the internal channel stays open.  Without a filter
        // the channel closes immediately and the ActiveSubscription is dropped
        // before the test can observe the active_subscribers count.
        Params {
            kind: cdk::nuts::nut17::Kind::Bolt11MintQuote,
            filters: vec![QuoteId::new_uuid().to_string()],
            id: Arc::new(SubId::from(sub_id)),
        }
    }

    fn make_context(mint: Arc<Mint>) -> WsContext {
        let state = MintState {
            mint,
            cache: Arc::new(HttpCache::default()),
        };
        let (publisher, _receiver) = tokio::sync::mpsc::channel(100);
        WsContext {
            state,
            subscriptions: HashMap::new(),
            publisher,
        }
    }

    /// Verify that unsubscribing leaks the background task and leaves the
    /// subscription registered in the pub/sub manager.
    ///
    /// This test is expected to FAIL until the fix is applied: after an
    /// explicit unsubscribe the `active_subscribers` count must return to 0,
    /// but the current code only removes the `JoinHandle` from the map without
    /// aborting the task (which owns the `ActiveSubscription`).
    #[tokio::test]
    async fn test_unsubscribe_cleans_up_active_subscription() {
        let mint = create_test_mint().await;
        let pubsub = mint.pubsub_manager();
        let mut context = make_context(mint);

        // Subscribe
        subscribe::handle(&mut context, make_params("sub-1"))
            .await
            .expect("subscribe");

        // Give the spawned task a moment to register
        tokio::task::yield_now().await;

        assert_eq!(
            pubsub.active_subscribers(),
            1,
            "should have 1 active subscriber after subscribe"
        );

        // Unsubscribe
        unsubscribe::handle(
            &mut context,
            WsUnsubscribeRequest {
                sub_id: Arc::new(SubId::from("sub-1")),
            },
        )
        .await
        .expect("unsubscribe");

        // The task must be aborted and the ActiveSubscription dropped so the
        // pub/sub index is cleaned up.  Without the fix this will be 1.
        tokio::task::yield_now().await;
        assert_eq!(
            pubsub.active_subscribers(),
            0,
            "active_subscribers should be 0 after explicit unsubscribe"
        );
    }

    /// Verify that dropping the `WsContext` (i.e. client disconnect) leaks
    /// background tasks and leaves subscriptions registered in the pub/sub
    /// manager.
    ///
    /// This test is expected to FAIL until the fix is applied: when the
    /// context is dropped all spawned tasks must be aborted so the
    /// `ActiveSubscription` destructor cleans up the pub/sub indexes.
    #[tokio::test]
    async fn test_context_drop_cleans_up_active_subscriptions() {
        let mint = create_test_mint().await;
        let pubsub = mint.pubsub_manager();
        let mut context = make_context(mint);

        // Subscribe twice with different IDs
        subscribe::handle(&mut context, make_params("sub-A"))
            .await
            .expect("subscribe A");
        subscribe::handle(&mut context, make_params("sub-B"))
            .await
            .expect("subscribe B");

        tokio::task::yield_now().await;
        assert_eq!(
            pubsub.active_subscribers(),
            2,
            "should have 2 active subscribers"
        );

        // Simulate client disconnect by dropping the context
        drop(context);

        // All tasks must be aborted and both ActiveSubscriptions dropped.
        // Without the fix this will remain 2.
        tokio::task::yield_now().await;
        assert_eq!(
            pubsub.active_subscribers(),
            0,
            "active_subscribers should be 0 after context drop (disconnect)"
        );
    }
}

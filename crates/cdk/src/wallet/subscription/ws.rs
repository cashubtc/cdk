use cdk_common::nut17::ws::WsMessageOrResponse;
use cdk_common::pub_sub::remote_consumer::{InternalRelay, StreamCtrl, SubscribeMessage};
use cdk_common::pub_sub::Error as PubsubError;
#[cfg(feature = "auth")]
use cdk_common::{Method, RoutePath};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::{MintSubTopics, SubscriptionClient};

#[inline(always)]
pub(crate) async fn stream_client(
    client: &SubscriptionClient,
    mut ctrl: mpsc::Receiver<StreamCtrl<MintSubTopics>>,
    topics: Vec<SubscribeMessage<MintSubTopics>>,
    reply_to: InternalRelay<MintSubTopics>,
) -> Result<(), PubsubError> {
    let mut url = client
        .mint_url
        .join_paths(&["v1", "ws"])
        .expect("Could not join paths");

    if url.scheme() == "https" {
        url.set_scheme("wss").expect("Could not set scheme");
    } else {
        url.set_scheme("ws").expect("Could not set scheme");
    }

    #[cfg(not(feature = "auth"))]
    let request = url.to_string().into_client_request().map_err(|err| {
        tracing::error!("Failed to create client request: {:?}", err);
        // Fallback to HTTP client if we can't create the WebSocket request
        cdk_common::pub_sub::Error::NotSupported
    })?;

    #[cfg(feature = "auth")]
    let mut request = url.to_string().into_client_request().map_err(|err| {
        tracing::error!("Failed to create client request: {:?}", err);
        // Fallback to HTTP client if we can't create the WebSocket request
        cdk_common::pub_sub::Error::NotSupported
    })?;

    #[cfg(feature = "auth")]
    {
        let auth_wallet = client.http_client.get_auth_wallet().await;
        let token = match auth_wallet.as_ref() {
            Some(auth_wallet) => {
                let endpoint = cdk_common::ProtectedEndpoint::new(Method::Get, RoutePath::Ws);
                match auth_wallet.get_auth_for_request(&endpoint).await {
                    Ok(token) => token,
                    Err(err) => {
                        tracing::warn!("Failed to get auth token: {:?}", err);
                        None
                    }
                }
            }
            None => None,
        };

        if let Some(auth_token) = token {
            let header_key = match &auth_token {
                cdk_common::AuthToken::ClearAuth(_) => "Clear-auth",
                cdk_common::AuthToken::BlindAuth(_) => "Blind-auth",
            };

            match auth_token.to_string().parse() {
                Ok(header_value) => {
                    request.headers_mut().insert(header_key, header_value);
                }
                Err(err) => {
                    tracing::warn!("Failed to parse auth token as header value: {:?}", err);
                }
            }
        }
    }

    tracing::debug!("Connecting to {}", url);
    let ws_stream = connect_async(request)
        .await
        .map(|(ws_stream, _)| ws_stream)
        .map_err(|err| {
            tracing::error!("Error connecting: {err:?}");

            cdk_common::pub_sub::Error::Internal(Box::new(err))
        })?;

    tracing::debug!("Connected to {}", url);
    let (mut write, mut read) = ws_stream.split();

    for (name, index) in topics {
        let (_, req) = if let Some(req) = client.get_sub_request(name, index) {
            req
        } else {
            continue;
        };

        let _ = write.send(Message::Text(req.into())).await;
    }

    loop {
        tokio::select! {
            Some(msg) = ctrl.recv() => {
                match msg {
                    StreamCtrl::Subscribe(msg) => {
                        let (_, req) = if let Some(req) = client.get_sub_request(msg.0, msg.1) {
                            req
                        } else {
                            continue;
                        };
                        let _ = write.send(Message::Text(req.into())).await;
                    }
                    StreamCtrl::Unsubscribe(msg) => {
                        let req = if let Some(req) = client.get_unsub_request(msg) {
                            req
                        } else {
                            continue;
                        };
                        let _ = write.send(Message::Text(req.into())).await;
                    }
                    StreamCtrl::Stop => {
                        if let Err(err) = write.send(Message::Close(None)).await {
                            tracing::error!("Closing error {err:?}");
                        }
                        break;
                    }
                };
            }
            Some(msg) = read.next() => {
                let msg = match msg {
                    Ok(msg) => msg,
                    Err(_) => {
                        if let Err(err) = write.send(Message::Close(None)).await {
                            tracing::error!("Closing error {err:?}");
                        }
                        break;
                    }
                };
                let msg = match msg {
                    Message::Text(msg) => msg,
                    _ => continue,
                };
                let msg = match serde_json::from_str::<WsMessageOrResponse<String>>(&msg) {
                    Ok(msg) => msg,
                    Err(_) => continue,
                };

                match msg {
                    WsMessageOrResponse::Notification(payload) => {
                        reply_to.send(payload.params.payload);
                    }
                    WsMessageOrResponse::Response(response) => {
                        tracing::debug!("Received response from server: {:?}", response);
                    }
                    WsMessageOrResponse::ErrorResponse(error) => {
                        tracing::debug!("Received an error from server: {:?}", error);
                        return Err(PubsubError::InternalStr(error.error.message));
                    }
                }

            }
        }
    }

    Ok(())
}

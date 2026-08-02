//! One-stream-per-transaction HTTP and WebSocket client.

use std::sync::atomic::Ordering;

use bytes::{Bytes, BytesMut};
use cashu::nuts::nut22::AuthToken;
use cdk_http_client::{HttpError, RawResponse};
use http_body_util::{BodyExt, Full};
use hyper::{
    body::Incoming,
    header::{self, HeaderName, HeaderValue},
    Method, Request, Response,
};
use hyper_util::rt::TokioIo;
use serde::{de::DeserializeOwned, Serialize};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use url::Url;

use crate::{address::IrohTarget, protocol, Error, IrohNode, IrohStream};

/// HTTP client backed by one clone-shared Iroh endpoint and connection pool.
#[derive(Debug, Clone)]
pub struct IrohClient {
    node: IrohNode,
}

impl IrohClient {
    /// Creates a client using an explicitly managed Iroh node.
    pub fn new(node: IrohNode) -> Self {
        Self { node }
    }

    /// Returns the underlying node.
    pub fn node(&self) -> &IrohNode {
        &self.node
    }

    /// Sends an arbitrary byte request without route-specific mapping.
    pub async fn request_raw(
        &self,
        method: Method,
        url: Url,
        headers: &[(HeaderName, HeaderValue)],
        body: Bytes,
        max_response_bytes: usize,
    ) -> Result<RawResponse, Error> {
        if headers
            .iter()
            .any(|(name, _)| name == header::HOST || name == header::CONTENT_LENGTH)
        {
            return Err(Error::InvalidRequest {
                reason: "Host and Content-Length are transport-owned",
            });
        }
        if body.len() > self.node.config().limits.max_request_body_bytes {
            return Err(Error::RequestTooLarge {
                actual: body.len(),
                max: self.node.config().limits.max_request_body_bytes,
            });
        }
        let target = IrohTarget::parse(&url)?;
        let connection = self
            .node
            .pool()
            .connect(self.node.endpoint(), target.endpoint_id, protocol::ALPN)
            .await?;
        let (send, recv) = match tokio::time::timeout(
            self.node.config().timeouts.stream_open,
            connection.open_bi(),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(_)) => {
                self.node
                    .pool()
                    .evict(target.endpoint_id, protocol::ALPN)
                    .await;
                return Err(Error::Stream);
            }
            Err(_) => {
                self.node
                    .metrics_inner()
                    .timeouts
                    .fetch_add(1, Ordering::Relaxed);
                return Err(Error::Timeout {
                    operation: "stream open",
                });
            }
        };
        let stream = TokioIo::new(IrohStream::new(send, recv));
        let (mut sender, driver) = hyper::client::conn::http1::Builder::new()
            .handshake(stream)
            .await
            .map_err(|_| Error::Http)?;
        let driver = tokio::spawn(driver);

        let mut builder = Request::builder()
            .method(method)
            .uri(target.request_target)
            .header(header::HOST, target.authority);
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        let request = builder.body(Full::new(body)).map_err(|_| Error::Http)?;
        let response = match tokio::time::timeout(
            self.node.config().timeouts.headers,
            sender.send_request(request),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                driver.abort();
                return Err(Error::Http);
            }
            Err(_) => {
                driver.abort();
                self.node
                    .metrics_inner()
                    .timeouts
                    .fetch_add(1, Ordering::Relaxed);
                return Err(Error::Timeout {
                    operation: "response headers",
                });
            }
        };
        let status = response.status().as_u16();
        let response_body = self
            .collect_response(response, max_response_bytes, driver, sender)
            .await?;
        let metrics = self.node.metrics_inner();
        metrics.requests.fetch_add(1, Ordering::Relaxed);
        metrics.record_status(status);
        Ok(RawResponse::new(status, response_body))
    }

    async fn collect_response(
        &self,
        response: Response<Incoming>,
        max_response_bytes: usize,
        driver: JoinHandle<Result<(), hyper::Error>>,
        sender: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
    ) -> Result<Vec<u8>, Error> {
        if let Some(actual) = response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|actual| *actual > max_response_bytes)
        {
            driver.abort();
            return Err(Error::ResponseTooLarge {
                actual,
                max: max_response_bytes,
            });
        }
        let mut body = response.into_body();
        let mut collected = BytesMut::new();
        loop {
            let frame =
                match tokio::time::timeout(self.node.config().timeouts.body_progress, body.frame())
                    .await
                {
                    Ok(frame) => frame,
                    Err(_) => {
                        driver.abort();
                        self.node
                            .metrics_inner()
                            .timeouts
                            .fetch_add(1, Ordering::Relaxed);
                        return Err(Error::Timeout {
                            operation: "response body progress",
                        });
                    }
                };
            let Some(frame) = frame else {
                break;
            };
            let frame = match frame {
                Ok(frame) => frame,
                Err(_) => {
                    driver.abort();
                    return Err(Error::Http);
                }
            };
            if let Some(data) = frame.data_ref() {
                let actual = collected.len().saturating_add(data.len());
                if actual > max_response_bytes {
                    driver.abort();
                    return Err(Error::ResponseTooLarge {
                        actual,
                        max: max_response_bytes,
                    });
                }
                collected.extend_from_slice(data);
            }
        }
        drop(body);
        drop(sender);
        let mut driver = driver;
        match tokio::time::timeout(self.node.config().timeouts.shutdown, &mut driver).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(_))) | Ok(Err(_)) => return Err(Error::Http),
            Err(_) => {
                driver.abort();
                let _ = driver.await;
                return Err(Error::ShutdownTimeout);
            }
        }
        Ok(collected.to_vec())
    }

    /// Sends a GET request and returns status plus bytes.
    pub async fn get_raw(&self, url: Url, auth: Option<AuthToken>) -> Result<RawResponse, Error> {
        let headers = auth_headers(auth)?;
        self.request_raw(
            Method::GET,
            url,
            &headers,
            Bytes::new(),
            self.node.config().limits.max_response_body_bytes,
        )
        .await
    }

    /// Sends a JSON POST and collects a bounded raw response.
    pub async fn post_json_raw<P>(
        &self,
        url: Url,
        auth: Option<AuthToken>,
        payload: &P,
        max_response_bytes: usize,
    ) -> Result<RawResponse, Error>
    where
        P: Serialize + Send + Sync,
    {
        let body = serde_json::to_vec(payload).map_err(|_| Error::Serialization)?;
        let mut headers = auth_headers(auth)?;
        headers.push((
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        ));
        self.request_raw(
            Method::POST,
            url,
            &headers,
            Bytes::from(body),
            max_response_bytes,
        )
        .await
    }

    /// Sends a form POST and returns status plus bytes.
    pub async fn post_form_raw<P>(
        &self,
        url: Url,
        auth: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, Error>
    where
        P: Serialize + Send + Sync,
    {
        let body = serde_urlencoded::to_string(payload).map_err(|_| Error::Serialization)?;
        let mut headers = auth_headers(auth)?;
        headers.push((
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        ));
        self.request_raw(
            Method::POST,
            url,
            &headers,
            Bytes::from(body),
            self.node.config().limits.max_response_body_bytes,
        )
        .await
    }

    /// Opens a WebSocket transaction over its own Iroh stream.
    pub async fn ws_connect(
        &self,
        url: &Url,
        headers: &[(&str, &str)],
    ) -> Result<
        (
            cdk_http_client::ws::WsSender,
            cdk_http_client::ws::WsReceiver,
        ),
        cdk_http_client::ws::WsError,
    > {
        use cdk_http_client::ws::WsError;

        let target =
            IrohTarget::parse(url).map_err(|error| WsError::Connection(error.to_string()))?;
        let connection = self
            .node
            .pool()
            .connect(self.node.endpoint(), target.endpoint_id, protocol::ALPN)
            .await
            .map_err(|error| WsError::Connection(error.to_string()))?;
        let (send, recv) = match tokio::time::timeout(
            self.node.config().timeouts.stream_open,
            connection.open_bi(),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(_)) => {
                self.node
                    .pool()
                    .evict(target.endpoint_id, protocol::ALPN)
                    .await;
                return Err(WsError::Connection(
                    "Iroh request stream failed".to_string(),
                ));
            }
            Err(_) => {
                self.node
                    .metrics_inner()
                    .timeouts
                    .fetch_add(1, Ordering::Relaxed);
                return Err(WsError::Connection(
                    "Iroh stream open timed out".to_string(),
                ));
            }
        };
        let ws_url = format!("ws://{}{}", target.authority, target.request_target);
        let mut request = ws_url
            .into_client_request()
            .map_err(|_| WsError::Connection("invalid WebSocket request".to_string()))?;
        for &(name, value) in headers {
            let Ok(name) = HeaderName::try_from(name) else {
                return Err(WsError::Connection("invalid WebSocket header".to_string()));
            };
            let Ok(value) = HeaderValue::try_from(value) else {
                return Err(WsError::Connection("invalid WebSocket header".to_string()));
            };
            request.headers_mut().insert(name, value);
        }
        let handshake = tokio_tungstenite::client_async(request, IrohStream::new(send, recv));
        let (stream, _) = tokio::time::timeout(self.node.config().timeouts.headers, handshake)
            .await
            .map_err(|_| {
                self.node
                    .metrics_inner()
                    .timeouts
                    .fetch_add(1, Ordering::Relaxed);
                WsError::Connection("Iroh WebSocket handshake timed out".to_string())
            })?
            .map_err(|_| WsError::Connection("Iroh WebSocket handshake failed".to_string()))?;
        Ok(cdk_http_client::ws::from_websocket_stream(stream))
    }
}

fn auth_headers(auth: Option<AuthToken>) -> Result<Vec<(HeaderName, HeaderValue)>, Error> {
    let Some(auth) = auth else {
        return Ok(Vec::new());
    };
    let name = HeaderName::try_from(auth.header_key()).map_err(|_| Error::Http)?;
    let value = HeaderValue::try_from(auth.to_string()).map_err(|_| Error::Http)?;
    Ok(vec![(name, value)])
}

pub(crate) fn decode_json<T>(response: RawResponse) -> Result<T, HttpError>
where
    T: DeserializeOwned,
{
    response.json_or_status_error()
}

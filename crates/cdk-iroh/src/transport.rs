//! Hybrid CDK transport with explicit URL-scheme dispatch.

use std::{fmt, sync::Arc};

use async_trait::async_trait;
use cashu::nuts::nut22::AuthToken;
use cdk_http_client::{HttpError, RawResponse, Transport};
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::OnceCell;
use url::Url;

use crate::{client::decode_json, Error, IrohClient, IrohConfig, IrohNode};

/// Hybrid transport which delegates HTTP(S) unchanged and routes only `iroh` URLs to Iroh.
#[derive(Clone)]
pub struct IrohTransport<H = cdk_http_client::Async> {
    inner: H,
    node: Arc<OnceCell<IrohNode>>,
    default_config: IrohConfig,
}

impl<H> IrohTransport<H> {
    /// Creates a hybrid transport from a runtime-owned Iroh node and HTTP transport.
    pub fn with_node(node: IrohNode, inner: H) -> Self {
        let cell = OnceCell::new();
        let _ = cell.set(node);
        Self {
            inner,
            node: Arc::new(cell),
            default_config: IrohConfig::default(),
        }
    }

    /// Returns the delegated HTTP transport.
    pub fn inner(&self) -> &H {
        &self.inner
    }

    async fn iroh_client(&self) -> Result<IrohClient, Error> {
        let node = self
            .node
            .get_or_try_init(|| IrohNode::ephemeral(self.default_config.clone()))
            .await?;
        Ok(IrohClient::new(node.clone()))
    }
}

impl<H> Default for IrohTransport<H>
where
    H: Default,
{
    fn default() -> Self {
        Self {
            inner: H::default(),
            node: Arc::new(OnceCell::new()),
            default_config: IrohConfig::default(),
        }
    }
}

impl<H> fmt::Debug for IrohTransport<H>
where
    H: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IrohTransport")
            .field("inner", &self.inner)
            .field("iroh_endpoint_initialized", &self.node.initialized())
            .field("default_config", &self.default_config)
            .finish()
    }
}

#[async_trait]
impl<H> Transport for IrohTransport<H>
where
    H: Transport + 'static,
{
    async fn ws_connect(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<
        (
            cdk_http_client::ws::WsSender,
            cdk_http_client::ws::WsReceiver,
        ),
        cdk_http_client::ws::WsError,
    > {
        let parsed = Url::parse(url).map_err(|_| {
            cdk_http_client::ws::WsError::Connection("invalid WebSocket URL".to_string())
        })?;
        match parsed.scheme() {
            "iroh" => {
                self.iroh_client()
                    .await
                    .map_err(|error| cdk_http_client::ws::WsError::Connection(error.to_string()))?
                    .ws_connect(&parsed, headers)
                    .await
            }
            "ws" | "wss" => self.inner.ws_connect(url, headers).await,
            scheme => Err(cdk_http_client::ws::WsError::Connection(
                Error::UnsupportedScheme {
                    scheme: scheme.to_owned(),
                }
                .to_string(),
            )),
        }
    }

    fn with_proxy(
        &mut self,
        proxy: Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        self.inner
            .with_proxy(proxy, host_matcher, accept_invalid_certs)
    }

    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, HttpError> {
        self.inner.resolve_dns_txt(domain).await
    }

    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        match url.scheme() {
            "iroh" => decode_json(self.iroh_client().await?.get_raw(url, auth).await?),
            "http" | "https" => self.inner.http_get(url, auth).await,
            scheme => Err(Error::UnsupportedScheme {
                scheme: scheme.to_owned(),
            }
            .into()),
        }
    }

    async fn http_get_raw(
        &self,
        url: Url,
        auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        match url.scheme() {
            "iroh" => Ok(self.iroh_client().await?.get_raw(url, auth).await?),
            "http" | "https" => self.inner.http_get_raw(url, auth).await,
            scheme => Err(Error::UnsupportedScheme {
                scheme: scheme.to_owned(),
            }
            .into()),
        }
    }

    async fn http_post<P, R>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        match url.scheme() {
            "iroh" => {
                let client = self.iroh_client().await?;
                let max_response_bytes = client.node().config().limits.max_response_body_bytes;
                decode_json(
                    client
                        .post_json_raw(url, auth_token, payload, max_response_bytes)
                        .await?,
                )
            }
            "http" | "https" => self.inner.http_post(url, auth_token, payload).await,
            scheme => Err(Error::UnsupportedScheme {
                scheme: scheme.to_owned(),
            }
            .into()),
        }
    }

    async fn http_post_bounded<P, R>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
        max_response_bytes: usize,
    ) -> Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned + Serialize,
    {
        match url.scheme() {
            "iroh" => decode_json(
                self.iroh_client()
                    .await?
                    .post_json_raw(url, auth_token, payload, max_response_bytes)
                    .await?,
            ),
            "http" | "https" => {
                self.inner
                    .http_post_bounded(url, auth_token, payload, max_response_bytes)
                    .await
            }
            scheme => Err(Error::UnsupportedScheme {
                scheme: scheme.to_owned(),
            }
            .into()),
        }
    }

    async fn http_post_form_raw<P>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        match url.scheme() {
            "iroh" => Ok(self
                .iroh_client()
                .await?
                .post_form_raw(url, auth_token, payload)
                .await?),
            "http" | "https" => {
                self.inner
                    .http_post_form_raw(url, auth_token, payload)
                    .await
            }
            scheme => Err(Error::UnsupportedScheme {
                scheme: scheme.to_owned(),
            }
            .into()),
        }
    }
}

//! Hybrid CDK transport with explicit URL-scheme dispatch.

use std::fmt;

use async_trait::async_trait;
use cashu::nuts::nut22::AuthToken;
use cdk_http_client::{HttpError, RawResponse, Transport};
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::OnceCell;
use url::Url;

use crate::{client::decode_json, Error, IrohClient, IrohConfig, IrohNode};

static DEFAULT_NODE: OnceCell<IrohNode> = OnceCell::const_new();

#[derive(Clone)]
enum NodeSource {
    ProcessDefault,
    Runtime(IrohNode),
}

/// Hybrid transport which delegates HTTP(S) unchanged and routes only `iroh` URLs to Iroh.
#[derive(Clone)]
pub struct IrohTransport<H = cdk_http_client::Async> {
    inner: H,
    node: NodeSource,
    default_config: IrohConfig,
}

impl<H> IrohTransport<H> {
    /// Creates a hybrid transport from a runtime-owned Iroh node and HTTP transport.
    pub fn with_node(node: IrohNode, inner: H) -> Self {
        Self {
            inner,
            node: NodeSource::Runtime(node),
            default_config: IrohConfig::default(),
        }
    }

    /// Returns the delegated HTTP transport.
    pub fn inner(&self) -> &H {
        &self.inner
    }

    async fn iroh_client(&self) -> Result<IrohClient, Error> {
        let node = match &self.node {
            NodeSource::ProcessDefault => DEFAULT_NODE
                .get_or_try_init(|| IrohNode::ephemeral(self.default_config.clone()))
                .await?
                .clone(),
            NodeSource::Runtime(node) => node.clone(),
        };
        Ok(IrohClient::new(node))
    }
}

impl<H> Default for IrohTransport<H>
where
    H: Default,
{
    fn default() -> Self {
        Self {
            inner: H::default(),
            node: NodeSource::ProcessDefault,
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
            .field(
                "iroh_endpoint_source",
                &match self.node {
                    NodeSource::ProcessDefault => "process-default",
                    NodeSource::Runtime(_) => "runtime",
                },
            )
            .field(
                "iroh_endpoint_initialized",
                &match self.node {
                    NodeSource::ProcessDefault => DEFAULT_NODE.initialized(),
                    NodeSource::Runtime(_) => true,
                },
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_transports_share_one_process_endpoint() {
        let first = IrohTransport::<cdk_http_client::Async>::default();
        let second = IrohTransport::<cdk_http_client::Async>::default();

        let first_client = first.iroh_client().await.expect("first default endpoint");
        let second_client = second.iroh_client().await.expect("second default endpoint");

        assert_eq!(
            first_client.node().endpoint_id(),
            second_client.node().endpoint_id()
        );
    }
}

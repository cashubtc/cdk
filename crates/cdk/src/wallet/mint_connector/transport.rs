//! HTTP Transport trait with a default implementation
use std::fmt::Debug;

use cdk_common::AuthToken;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use super::Error;
use crate::error::ErrorResponse;

/// Expected HTTP Transport
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait Transport: Default + Send + Sync + Debug + Clone {
    /// Make the transport to use a given proxy
    fn with_proxy(
        &mut self,
        proxy: url::Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<(), super::Error>;

    /// HTTP Get request
    async fn http_get<R>(
        &self,
        url: url::Url,
        auth: Option<cdk_common::AuthToken>,
    ) -> Result<R, super::Error>
    where
        R: serde::de::DeserializeOwned;

    /// HTTP Post request
    async fn http_post<P, R>(
        &self,
        url: url::Url,
        auth_token: Option<cdk_common::AuthToken>,
        payload: &P,
    ) -> Result<R, super::Error>
    where
        P: serde::Serialize + ?Sized + Send + Sync,
        R: serde::de::DeserializeOwned;
}

/// Async transport for Http
#[derive(Debug, Clone)]
pub struct Async {
    inner: Client,
}

impl Default for Async {
    fn default() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        Self {
            inner: Client::new(),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl Transport for Async {
    #[cfg(target_arch = "wasm32")]
    fn with_proxy(
        &mut self,
        _proxy: Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), Error> {
        panic!("Not supported in wasm");
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn with_proxy(
        &mut self,
        proxy: Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<(), Error> {
        let builder = reqwest::Client::builder().danger_accept_invalid_certs(accept_invalid_certs);

        let builder = match host_matcher {
            Some(pattern) => {
                // When a matcher is provided, only apply the proxy to matched hosts
                let regex = regex::Regex::new(pattern).map_err(|e| Error::Custom(e.to_string()))?;
                builder.proxy(reqwest::Proxy::custom(move |url| {
                    url.host_str()
                        .filter(|host| regex.is_match(host))
                        .map(|_| proxy.clone())
                }))
            }
            // Apply proxy to all requests when no matcher is provided
            None => {
                builder.proxy(reqwest::Proxy::all(proxy).map_err(|e| Error::Custom(e.to_string()))?)
            }
        };

        self.inner = builder
            .build()
            .map_err(|e| Error::HttpError(e.status().map(|s| s.as_u16()), e.to_string()))?;
        Ok(())
    }

    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, Error>
    where
        R: DeserializeOwned,
    {
        let mut request = self.inner.get(url);

        if let Some(auth) = auth {
            request = request.header(auth.header_key(), auth.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|e| {
                Error::HttpError(
                    e.status().map(|status_code| status_code.as_u16()),
                    e.to_string(),
                )
            })?
            .text()
            .await
            .map_err(|e| {
                Error::HttpError(
                    e.status().map(|status_code| status_code.as_u16()),
                    e.to_string(),
                )
            })?;

        serde_json::from_str::<R>(&response).map_err(|err| {
            tracing::warn!("Http Response error: {}", err);
            match ErrorResponse::from_json(&response) {
                Ok(ok) => <ErrorResponse as Into<Error>>::into(ok),
                Err(err) => err.into(),
            }
        })
    }

    async fn http_post<P, R>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, Error>
    where
        P: Serialize + ?Sized + Send + Sync,
        R: DeserializeOwned,
    {
        let mut request = self.inner.post(url).json(&payload);

        if let Some(auth) = auth_token {
            request = request.header(auth.header_key(), auth.to_string());
        }

        let response = request.send().await.map_err(|e| {
            Error::HttpError(
                e.status().map(|status_code| status_code.as_u16()),
                e.to_string(),
            )
        })?;

        let response = response.text().await.map_err(|e| {
            Error::HttpError(
                e.status().map(|status_code| status_code.as_u16()),
                e.to_string(),
            )
        })?;

        serde_json::from_str::<R>(&response).map_err(|err| {
            tracing::warn!("Http Response error: {}", err);
            match ErrorResponse::from_json(&response) {
                Ok(ok) => <ErrorResponse as Into<Error>>::into(ok),
                Err(err) => err.into(),
            }
        })
    }
}

#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub mod tor_transport {
    use std::sync::Arc;

    use arti_client::{IsolationToken, StreamPrefs, TorClient, TorClientConfig};
    use arti_hyper::ArtiHttpConnector;
    use async_trait::async_trait;
    use cdk_common::AuthToken;
    use http::header::{self, HeaderName, HeaderValue};
    use hyper::http::{Method, Request, Uri};
    use hyper::{Body, Client};
    use serde::de::DeserializeOwned;
    use tls_api::{TlsConnector as _, TlsConnectorBuilder as _};
    use tokio::sync::OnceCell;
    use url::Url;

    use super::Error;
    use crate::error::ErrorResponse;

    pub struct TorAsync {
        tor: std::sync::Arc<OnceCell<arti_client::TorClient<tor_rtcompat::PreferredRuntime>>>,
        isolation: IsolationToken,
    }

    impl Clone for TorAsync {
        fn clone(&self) -> Self {
            Self {
                tor: self.tor.clone(),
                isolation: IsolationToken::new(),
            }
        }
    }

    impl std::fmt::Debug for TorAsync {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("TorAsync")
                .field("isolation", &self.isolation)
                .finish()
        }
    }

    impl Default for TorAsync {
        fn default() -> Self {
            Self {
                tor: Arc::new(OnceCell::new()),
                isolation: IsolationToken::new(),
            }
        }
    }

    #[async_trait]
    impl super::Transport for TorAsync {
        fn with_proxy(
            &mut self,
            _proxy: Url,
            _host_matcher: Option<&str>,
            _accept_invalid_certs: bool,
        ) -> Result<(), Error> {
            panic!("not supported with TorAsync transport");
        }

        async fn http_get<R>(
            &self,
            url: url::Url,
            auth: Option<cdk_common::AuthToken>,
        ) -> Result<R, super::Error>
        where
            R: serde::de::DeserializeOwned,
        {
            self.request::<Vec<u8>, R>(Method::GET, url, auth, None)
                .await
        }

        async fn http_post<P, R>(
            &self,
            url: url::Url,
            auth_token: Option<cdk_common::AuthToken>,
            payload: &P,
        ) -> Result<R, super::Error>
        where
            P: serde::Serialize + ?Sized + Send + Sync,
            R: serde::de::DeserializeOwned,
        {
            let body = serde_json::to_vec(payload).map_err(|e| Error::Custom(e.to_string()))?;
            self.request::<Vec<u8>, R>(Method::POST, url, auth_token, Some(body))
                .await
        }
    }

    impl TorAsync {
        async fn request<B, R>(
            &self,
            method: http::Method,
            url: Url,
            auth: Option<AuthToken>,
            body: Option<B>,
        ) -> Result<R, Error>
        where
            B: Into<Vec<u8>>,
            R: DeserializeOwned,
        {
            let tor_client = self
                .tor
                .get_or_init(|| async move {
                    TorClient::create_bootstrapped(TorClientConfig::default())
                        .await
                        .expect("bootstrap")
                })
                .await
                .clone();

            let tls = tls_api_native_tls::TlsConnector::builder()
                .map_err(|e| Error::Custom(format!("{e:?}")))?
                .build()
                .map_err(|e| Error::Custom(format!("{e:?}")))?;

            // Set isolation
            let mut prefs = StreamPrefs::new();
            prefs.set_isolation(self.isolation.clone());
            let client_for_request = tor_client.clone_with_prefs(prefs);

            let connector = ArtiHttpConnector::new(client_for_request, tls);
            let client: Client<_> = Client::builder().build(connector);

            let uri: Uri = url
                .as_str()
                .parse::<Uri>()
                .map_err(|e| Error::Custom(e.to_string()))?;

            let mut builder = Request::builder().method(method).uri(uri);
            builder = builder.header(header::ACCEPT, "application/json");

            let mut req = if let Some(b) = body {
                builder
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(b.into()))
                    .map_err(|e| Error::Custom(e.to_string()))?
            } else {
                builder
                    .body(Body::empty())
                    .map_err(|e| Error::Custom(e.to_string()))?
            };

            if let Some(auth) = auth {
                let key = auth.header_key();
                let val = auth.to_string();
                // Insert after building request due to builder API ergonomics
                req.headers_mut().insert(
                    HeaderName::from_bytes(key.as_bytes())
                        .map_err(|e| Error::Custom(e.to_string()))?,
                    HeaderValue::from_str(&val).map_err(|e| Error::Custom(e.to_string()))?,
                );
            }

            let resp = client
                .request(req)
                .await
                .map_err(|e| Error::HttpError(None, e.to_string()))?;

            let status = resp.status().as_u16();
            let bytes = hyper::body::to_bytes(resp.into_body())
                .await
                .map_err(|e| Error::HttpError(None, e.to_string()))?;

            if !(200..300).contains(&status) {
                let text = String::from_utf8_lossy(&bytes).to_string();
                return Err(Error::HttpError(Some(status), text));
            }

            serde_json::from_slice::<R>(&bytes).map_err(|err| {
                let text = String::from_utf8_lossy(&bytes).to_string();
                tracing::warn!("Http Response error: {}", err);
                match ErrorResponse::from_json(&text) {
                    Ok(ok) => <ErrorResponse as Into<Error>>::into(ok),
                    Err(err) => err.into(),
                }
            })
        }
    }
}

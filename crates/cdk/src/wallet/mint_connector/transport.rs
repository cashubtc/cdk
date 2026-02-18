//! HTTP Transport trait with a default implementation
use std::fmt::Debug;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use std::str::FromStr;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use cdk_common::HttpClientBuilder;
use cdk_common::{AuthToken, HttpClient};
use cdk_http_client::RequestBuilderExt;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use hickory_resolver::name_server::TokioConnectionProvider;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use hickory_resolver::Resolver;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use super::Error;
use crate::error::ErrorResponse;

/// Expected HTTP Transport
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait Transport: Default + Send + Sync + Debug + Clone {
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    /// DNS resolver to get a TXT record from a domain name
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error>;

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
    inner: HttpClient,
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    resolver: Arc<Resolver<TokioConnectionProvider>>,
}

impl Default for Async {
    fn default() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        Self {
            inner: HttpClient::new(),
            #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
            resolver: Arc::new(default_resolver()),
        }
    }
}

#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
fn default_resolver() -> Resolver<TokioConnectionProvider> {
    let mut resolver_opts = ResolverOpts::default();
    resolver_opts.validate = true;

    Resolver::builder_with_config(
        ResolverConfig::default(),
        TokioConnectionProvider::default(),
    )
    .with_options(resolver_opts)
    .build()
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
        let builder =
            HttpClientBuilder::default().danger_accept_invalid_certs(accept_invalid_certs);

        let builder = match host_matcher {
            Some(pattern) => {
                // When a matcher is provided, only apply the proxy to matched hosts
                builder
                    .proxy_with_matcher(proxy, pattern)
                    .map_err(|e| Error::Custom(e.to_string()))?
            }
            // Apply proxy to all requests when no matcher is provided
            None => builder.proxy(proxy),
        };

        self.inner = builder
            .build()
            .map_err(|e| Error::HttpError(None, e.to_string()))?;
        Ok(())
    }

    /// DNS resolver to get a TXT record from a domain name
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, Error> {
        let name = hickory_resolver::Name::from_str(domain)
            .map_err(|e| Error::Custom(format!("Invalid domain name: {}", e)))?;

        Ok(self
            .resolver
            .txt_lookup(name)
            .await
            .map_err(|e| Error::Custom(e.to_string()))?
            .into_iter()
            .map(|txt| {
                txt.txt_data()
                    .iter()
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .collect::<Vec<_>>())
    }

    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, Error>
    where
        R: DeserializeOwned,
    {
        let url_str = url.to_string();
        let mut request = self.inner.get(&url_str);

        if let Some(auth) = auth {
            request = request.header(auth.header_key(), auth.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|e| Error::HttpError(None, e.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| Error::HttpError(None, e.to_string()))?;

        if !(200..300).contains(&status) {
            // Attempt to strictly parse as a Cashu ErrorResponse (requires the 'code' field).
            // This avoids laundering generic errors into ErrorCode::Unknown(999).
            if let Ok(err_resp) = serde_json::from_str::<ErrorResponse>(&body) {
                return Err(err_resp.into());
            }
            return Err(Error::HttpError(Some(status), body));
        }

        serde_json::from_str::<R>(&body).map_err(|err| {
            tracing::warn!("Http Response error: {}", err);
            match ErrorResponse::from_json(&body) {
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
        let url_str = url.to_string();
        let mut request = self.inner.post(&url_str).json(&payload);

        if let Some(auth) = auth_token {
            request = request.header(auth.header_key(), auth.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|e| Error::HttpError(None, e.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| Error::HttpError(None, e.to_string()))?;

        if !(200..300).contains(&status) {
            // Attempt to strictly parse as a Cashu ErrorResponse (requires the 'code' field).
            // This avoids laundering generic errors into ErrorCode::Unknown(999).
            if let Ok(err_resp) = serde_json::from_str::<ErrorResponse>(&body) {
                return Err(err_resp.into());
            }
            return Err(Error::HttpError(Some(status), body));
        }

        serde_json::from_str::<R>(&body).map_err(|err| {
            tracing::warn!("Http Response error: {}", err);
            match ErrorResponse::from_json(&body) {
                Ok(ok) => <ErrorResponse as Into<Error>>::into(ok),
                Err(err) => err.into(),
            }
        })
    }
}

#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub mod tor_transport;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    /// Spawn a one-shot HTTP server that replies with the given status line and
    /// body to the first request it receives. Returns the URL to connect to.
    async fn spawn_canned_response(status_line: &'static str, body: &'static str) -> Url {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");

        let response = format!(
            "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );

        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });

        Url::parse(&format!("http://{}/", addr)).expect("valid url")
    }

    /// Regression test for https://github.com/cashubtc/cdk/issues/1914
    ///
    /// A 429 response with a body that does not match the Cashu ErrorResponse
    /// schema (e.g. nutshell's `{"detail":"Rate limit exceeded."}`) must surface
    /// as `Error::HttpError(Some(429), body)` instead of being laundered into
    /// `ErrorCode::Unknown(999)` by `ErrorResponse::from_value`.
    #[tokio::test]
    async fn http_post_surfaces_429_status() {
        let url = spawn_canned_response(
            "HTTP/1.1 429 Too Many Requests",
            r#"{"detail":"Rate limit exceeded."}"#,
        )
        .await;

        let transport = Async::default();
        let result: Result<serde_json::Value, Error> =
            transport.http_post(url, None, &serde_json::json!({})).await;

        match result {
            Err(Error::HttpError(Some(429), body)) => {
                assert!(
                    body.contains("Rate limit exceeded"),
                    "body should be preserved, got: {body}"
                );
            }
            other => panic!("expected HttpError(Some(429), _), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn http_get_surfaces_429_status() {
        let url = spawn_canned_response(
            "HTTP/1.1 429 Too Many Requests",
            r#"{"detail":"Rate limit exceeded."}"#,
        )
        .await;

        let transport = Async::default();
        let result: Result<serde_json::Value, Error> = transport.http_get(url, None).await;

        match result {
            Err(Error::HttpError(Some(429), body)) => {
                assert!(body.contains("Rate limit exceeded"));
            }
            other => panic!("expected HttpError(Some(429), _), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn http_post_surfaces_400_mint_error() {
        let url = spawn_canned_response(
            "HTTP/1.1 400 Bad Request",
            r#"{"code": 1000, "detail": "Token already spent"}"#,
        )
        .await;

        let transport = Async::default();
        let result: Result<serde_json::Value, Error> =
            transport.http_post(url, None, &serde_json::json!({})).await;

        match result {
            Err(Error::UnknownErrorResponse(msg)) if msg.contains("1000") => {}
            other => panic!("expected Error::UnknownErrorResponse containing 1000, got {other:?}"),
        }
    }
}

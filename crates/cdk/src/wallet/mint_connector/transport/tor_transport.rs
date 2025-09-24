///! Tor transport implementation (non-wasm32 only)
use std::sync::Arc;

use arti_client::{TorClient, TorClientConfig};
use arti_hyper::ArtiHttpConnector;
use async_trait::async_trait;
use cdk_common::AuthToken;
use http::header::{self, HeaderName, HeaderValue};
use hyper::http::{Method, Request, Uri};
use hyper::{Body, Client};
use serde::de::DeserializeOwned;
use tls_api::{TlsConnector as _, TlsConnectorBuilder as _};
use url::Url;

use super::super::Error;
use crate::wallet::mint_connector::transport::{ErrorResponse, Transport};

/// Fixed-size pool size
const DEFAULT_TOR_POOL_SIZE: usize = 10;

/// Tor transport that maintains a pool of isolated TorClient handles
#[derive(Clone)]
pub struct TorAsync {
    /// Pool of isolated clients created from a single bootstrapped base client
    pool: Arc<Vec<TorClient<tor_rtcompat::PreferredRuntime>>>,
}

impl std::fmt::Debug for TorAsync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TorAsync")
            .field("pool_size", &self.pool.len())
            .finish()
    }
}

impl Default for TorAsync {
    fn default() -> Self {
        // Default builds a TorAsync with the default pool size by blocking on bootstrap.
        // If a Tokio runtime is present, use its handle; otherwise, create a temporary runtime.
        let fut = Self::with_pool_size(DEFAULT_TOR_POOL_SIZE);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle
                .block_on(fut)
                .unwrap_or_else(|e| panic!("TorAsync::default() bootstrap failed: {e}")),
            Err(_) => tokio::runtime::Runtime::new()
                .expect("failed to create temporary Tokio runtime for TorAsync::default()")
                .block_on(fut)
                .unwrap_or_else(|e| panic!("TorAsync::default() bootstrap failed: {e}")),
        }
    }
}

impl TorAsync {
    /// Create a TorAsync with default pool size
    pub async fn new() -> Result<Self, Error> {
        Self::with_pool_size(DEFAULT_TOR_POOL_SIZE).await
    }

    /// Create a TorAsync with the given pool size. Bootstraps arti and builds isolated clients.
    pub async fn with_pool_size(size: usize) -> Result<Self, Error> {
        let n = size.max(1);
        let base = TorClient::create_bootstrapped(TorClientConfig::default())
            .await
            .map_err(|e| Error::Custom(e.to_string()))?;
        let mut clients = Vec::with_capacity(n);
        for _ in 0..n {
            clients.push(base.isolated_client());
        }
        Ok(Self {
            pool: Arc::new(clients),
        })
    }

    /// Choose client index deterministically per endpoint path + query and payload
    #[inline]
    fn index_for_request(&self, url: &Url, body: Option<&[u8]>) -> usize {
        // Use a tiny, dependency-free, stable hash (FNV-1a 64-bit)
        const FNV_OFFSET: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;
        fn fnv1a(mut h: u64, bytes: &[u8]) -> u64 {
            for &b in bytes {
                h ^= b as u64;
                h = h.wrapping_mul(FNV_PRIME);
            }
            h
        }

        let mut h = FNV_OFFSET;
        h = fnv1a(h, url.path().as_bytes());
        if let Some(q) = url.query() {
            h = fnv1a(h, b"?");
            h = fnv1a(h, q.as_bytes());
        }
        if let Some(b) = body {
            h = fnv1a(h, b);
        }
        (h as usize) % self.pool.len()
    }

    async fn request<R>(
        &self,
        method: http::Method,
        url: Url,
        auth: Option<AuthToken>,
        mut body: Option<Vec<u8>>,
    ) -> Result<R, Error>
    where
        R: DeserializeOwned,
    {
        let tls = tls_api_native_tls::TlsConnector::builder()
            .map_err(|e| Error::Custom(format!("{e:?}")))?
            .build()
            .map_err(|e| Error::Custom(format!("{e:?}")))?;

        // Deterministically select an isolated client for request affinity
        let idx = self.index_for_request(&url, body.as_deref());
        let client_for_request = self.pool[idx].clone();

        let connector = ArtiHttpConnector::new(client_for_request, tls);
        let client: Client<_> = Client::builder().build(connector);

        let uri: Uri = url
            .as_str()
            .parse::<Uri>()
            .map_err(|e| Error::Custom(e.to_string()))?;

        let mut builder = Request::builder().method(method).uri(uri);
        builder = builder.header(header::ACCEPT, "application/json");

        let mut req = if let Some(b) = body.take() {
            builder
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(b))
                .map_err(|e| Error::Custom(e.to_string()))?
        } else {
            builder
                .body(Body::empty())
                .map_err(|e| Error::Custom(e.to_string()))?
        };

        if let Some(auth) = auth {
            let key = auth.header_key();
            let val = auth.to_string();
            req.headers_mut().insert(
                HeaderName::from_bytes(key.as_bytes()).map_err(|e| Error::Custom(e.to_string()))?,
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

#[async_trait]
impl Transport for TorAsync {
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
    ) -> Result<R, super::super::Error>
    where
        R: serde::de::DeserializeOwned,
    {
        self.request::<R>(Method::GET, url, auth, None).await
    }

    async fn http_post<P, R>(
        &self,
        url: url::Url,
        auth_token: Option<cdk_common::AuthToken>,
        payload: &P,
    ) -> Result<R, super::super::Error>
    where
        P: serde::Serialize + ?Sized + Send + Sync,
        R: serde::de::DeserializeOwned,
    {
        let body = serde_json::to_vec(payload).map_err(|e| Error::Custom(e.to_string()))?;
        self.request::<R>(Method::POST, url, auth_token, Some(body)).await
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, Error> {
        #[derive(serde::Deserialize)]
        struct Answer {
            #[serde(default)]
            data: String,
            #[allow(dead_code)]
            #[serde(default)]
            name: String,
            #[allow(dead_code)]
            #[serde(default)]
            r#type: u32,
        }

        #[allow(non_snake_case)]
        #[derive(serde::Deserialize)]
        struct DnsResp {
            #[serde(default)]
            Answer: Option<Vec<Answer>>,
            #[allow(dead_code)]
            #[serde(default)]
            Status: Option<u32>,
        }

        fn dequote_txt(s: &str) -> String {
            let mut result = String::new();
            let mut in_quote = false;
            let mut buf = String::new();
            for ch in s.chars() {
                if ch == '"' {
                    if in_quote {
                        result.push_str(&buf);
                        buf.clear();
                        in_quote = false;
                    } else {
                        in_quote = true;
                    }
                } else if in_quote {
                    buf.push(ch);
                }
            }
            if !result.is_empty() {
                result
            } else {
                s.trim_matches('"').to_string()
            }
        }

        let mut url =
            Url::parse("https://dns.google/resolve").map_err(|e| Error::Custom(e.to_string()))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("name", domain);
            qp.append_pair("type", "TXT");
        }

        let resp: DnsResp = self
            .request::<DnsResp>(Method::GET, url, None, None::<Vec<u8>>)
            .await?;

        let answers = resp.Answer.unwrap_or_default();
        let txts = answers
            .into_iter()
            .filter(|a| !a.data.is_empty())
            .map(|a| dequote_txt(&a.data))
            .collect::<Vec<_>>();

        Ok(txts)
    }
}

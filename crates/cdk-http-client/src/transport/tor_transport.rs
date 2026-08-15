//! Tor transport implementation (non-wasm32 only)

use std::fmt;
use std::sync::Arc;

use arti_client::{TorClient, TorClientConfig};
use arti_hyper::ArtiHttpConnector;
use async_trait::async_trait;
use cashu::nuts::nut22::AuthToken;
#[cfg(feature = "bip353")]
use dnssec_prover::query::{ProofBuilder, QueryBuf};
#[cfg(feature = "bip353")]
use dnssec_prover::rr::TXT_TYPE;
use http::header::{self, HeaderName, HeaderValue};
use hyper::http::{Method, Request, Uri};
use hyper::{Body, Client};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tls_api::{TlsConnector as _, TlsConnectorBuilder as _};
use tokio::sync::OnceCell;
use url::Url;

use crate::transport::Transport;
use crate::{HttpError, RawResponse};

/// Fixed-size pool size.
pub const DEFAULT_TOR_POOL_SIZE: usize = 5;

/// Tor transport that maintains a pool of isolated TorClient handles.
#[derive(Clone)]
pub struct TorAsync {
    salt: [u8; 4],
    size: usize,
    pool: Arc<OnceCell<Vec<TorClient<tor_rtcompat::PreferredRuntime>>>>,
}

impl fmt::Debug for TorAsync {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pool_len = self.pool.get().map(|p| p.len());
        f.debug_struct("TorAsync")
            .field("configured_pool_size", &self.size)
            .field("initialized_pool_size", &pool_len)
            .finish()
    }
}

#[inline]
fn gen_salt() -> [u8; 4] {
    let mut s = [0u8; 4];
    getrandom::getrandom(&mut s).expect("failed to obtain random bytes for TorAsync salt");
    s
}

impl Default for TorAsync {
    fn default() -> Self {
        Self {
            size: DEFAULT_TOR_POOL_SIZE,
            pool: Arc::new(OnceCell::new()),
            salt: gen_salt(),
        }
    }
}

impl TorAsync {
    /// Create a TorAsync with default pool size.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a TorAsync with the given pool size.
    pub fn with_pool_size(size: usize) -> Self {
        let size = size.max(1);
        Self {
            size,
            pool: Arc::new(OnceCell::new()),
            salt: gen_salt(),
        }
    }

    async fn ensure_pool(
        &self,
    ) -> Result<Vec<TorClient<tor_rtcompat::PreferredRuntime>>, HttpError> {
        let size = self.size;
        let pool_ref = self
            .pool
            .get_or_try_init(|| async move {
                let base = TorClient::create_bootstrapped(TorClientConfig::default())
                    .await
                    .map_err(|e| HttpError::Other(e.to_string()))?;
                let mut clients = Vec::with_capacity(size);
                for _ in 0..size {
                    clients.push(base.isolated_client());
                }
                Ok::<Vec<TorClient<tor_rtcompat::PreferredRuntime>>, HttpError>(clients)
            })
            .await?;
        Ok(pool_ref.clone())
    }

    #[inline]
    fn index_for_request(
        &self,
        method: &http::Method,
        url: &Url,
        body: Option<&[u8]>,
        pool_len: usize,
    ) -> usize {
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

        h = fnv1a(h, &self.salt);
        h = fnv1a(h, url.scheme().as_bytes());
        h = fnv1a(h, b"://");
        if let Some(host) = url.host_str() {
            h = fnv1a(h, host.as_bytes());
        }
        if let Some(port) = url.port() {
            h = fnv1a(h, b":");
            let p = port.to_string();
            h = fnv1a(h, p.as_bytes());
        }

        h = fnv1a(h, method.as_str().as_bytes());
        h = fnv1a(h, b" ");
        h = fnv1a(h, url.path().as_bytes());
        if let Some(q) = url.query() {
            h = fnv1a(h, b"?");
            h = fnv1a(h, q.as_bytes());
        }

        if let Some(b) = body {
            h = fnv1a(h, b);
        }

        (h as usize) % pool_len.max(1)
    }

    async fn raw_request(
        &self,
        method: http::Method,
        url: Url,
        auth: Option<AuthToken>,
        body: Option<(Vec<u8>, &'static str)>,
    ) -> Result<RawResponse, HttpError> {
        self.raw_request_with_accept(method, url, auth, body, "application/json")
            .await
    }

    async fn raw_request_with_accept(
        &self,
        method: http::Method,
        url: Url,
        auth: Option<AuthToken>,
        mut body: Option<(Vec<u8>, &'static str)>,
        accept: &'static str,
    ) -> Result<RawResponse, HttpError> {
        let tls = tls_api_native_tls::TlsConnector::builder()
            .map_err(|e| HttpError::Other(format!("{e:?}")))?
            .build()
            .map_err(|e| HttpError::Other(format!("{e:?}")))?;

        let pool = self.ensure_pool().await?;
        let idx = self.index_for_request(
            &method,
            &url,
            body.as_ref().map(|(bytes, _)| bytes.as_slice()),
            pool.len(),
        );
        let client_for_request = pool[idx].clone();

        let connector = ArtiHttpConnector::new(client_for_request, tls);
        let client: Client<_> = Client::builder().build(connector);

        let uri: Uri = url
            .as_str()
            .parse::<Uri>()
            .map_err(|e| HttpError::Other(e.to_string()))?;

        let mut builder = Request::builder().method(method).uri(uri);
        builder = builder.header(header::ACCEPT, accept);

        let mut req = match body.take() {
            Some((body, content_type)) => builder
                .header(http::header::CONTENT_TYPE, content_type)
                .body(Body::from(body))
                .map_err(|e| HttpError::Other(e.to_string()))?,
            None => builder
                .body(Body::empty())
                .map_err(|e| HttpError::Other(e.to_string()))?,
        };

        if let Some(auth) = auth {
            let key = auth.header_key();
            let val = auth.to_string();
            req.headers_mut().insert(
                HeaderName::from_bytes(key.as_bytes())
                    .map_err(|e| HttpError::Other(e.to_string()))?,
                HeaderValue::from_str(&val).map_err(|e| HttpError::Other(e.to_string()))?,
            );
        }

        let resp = client
            .request(req)
            .await
            .map_err(|e| HttpError::Connection(e.to_string()))?;

        let status = resp.status().as_u16();
        let bytes = hyper::body::to_bytes(resp.into_body())
            .await
            .map_err(|e| HttpError::Other(e.to_string()))?;

        Ok(RawResponse::new(status, bytes.to_vec()))
    }

    async fn request<R>(
        &self,
        method: http::Method,
        url: Url,
        auth: Option<AuthToken>,
        body: Option<(Vec<u8>, &'static str)>,
    ) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        self.raw_request(method, url, auth, body)
            .await?
            .json_or_status_error()
    }
}

#[async_trait]
impl Transport for TorAsync {
    fn with_proxy(
        &mut self,
        _proxy: Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        Err(HttpError::Proxy(
            "proxy configuration is not supported with TorAsync transport".to_string(),
        ))
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, HttpError> {
        let name = crate::dns::name_from_domain(domain)?;
        let (mut proof_builder, initial_query) = ProofBuilder::new(&name, TXT_TYPE);
        let mut pending_queries = vec![initial_query];
        let url = Url::parse("https://dns.google/dns-query")
            .map_err(|e| HttpError::Other(e.to_string()))?;

        while let Some(query) = pending_queries.pop() {
            let response = self
                .raw_request_with_accept(
                    Method::POST,
                    url.clone(),
                    None,
                    Some((query.into_vec(), "application/dns-message")),
                    "application/dns-message",
                )
                .await?;
            if !response.is_success() {
                return Err(HttpError::Status {
                    status: response.status(),
                    message: response.body_lossy(),
                });
            }

            let mut answer = QueryBuf::new_zeroed(0);
            answer.extend_from_slice(&response.bytes().await?);
            let queries = proof_builder
                .process_response(&answer)
                .map_err(|_| HttpError::Other("Invalid DNS-over-HTTPS response".to_owned()))?;
            pending_queries.extend(queries);
        }

        let (proof, _ttl) = proof_builder.finish_proof().map_err(|()| {
            HttpError::Other("Too many queries required to build DNSSEC proof".to_owned())
        })?;

        crate::dns::validated_txt_records(&name, &proof)
    }

    async fn ws_connect(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<(crate::ws::WsSender, crate::ws::WsReceiver), crate::ws::WsError> {
        let parsed_url = Url::parse(url)
            .map_err(|e| crate::ws::WsError::Terminal(format!("Invalid URL: {e}")))?;
        let pool = self
            .ensure_pool()
            .await
            .map_err(|e| crate::ws::WsError::Transient(e.to_string()))?;
        let idx = self.index_for_request(&http::Method::GET, &parsed_url, None, pool.len());

        crate::ws::connect_tor(pool[idx].clone(), url, headers).await
    }

    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        self.request::<R>(Method::GET, url, auth, None).await
    }

    async fn http_get_raw(
        &self,
        url: Url,
        auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        self.raw_request(Method::GET, url, auth, None).await
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
        let body =
            serde_json::to_vec(payload).map_err(|e| HttpError::Serialization(e.to_string()))?;
        self.request::<R>(
            Method::POST,
            url,
            auth_token,
            Some((body, "application/json")),
        )
        .await
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
        let body = serde_urlencoded::to_string(payload)
            .map_err(|e| HttpError::Serialization(e.to_string()))?;
        self.raw_request(
            Method::POST,
            url,
            auth_token,
            Some((body.into_bytes(), "application/x-www-form-urlencoded")),
        )
        .await
    }
}

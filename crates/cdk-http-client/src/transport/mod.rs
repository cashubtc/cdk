//! HTTP transport trait and implementations

use std::fmt::Debug;

use async_trait::async_trait;
use cashu::nuts::nut22::AuthToken;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

#[cfg(any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest"))]
use crate::{HttpClient, HttpClientBuilder};
use crate::{HttpError, RawResponse};

pub(super) fn bind_auth(
    mut auth: AuthToken,
    method: &str,
    url: &Url,
    body: &[u8],
) -> Result<AuthToken, HttpError> {
    let target = &url[url::Position::BeforePath..url::Position::AfterQuery];
    auth.bind_request(method, target, body)
        .map_err(|_| HttpError::Other("Could not authorize HTTP request".to_owned()))?;
    Ok(auth)
}

/// Expected HTTP transport.
///
/// Callers that construct a transport implicitly may add a [`Default`] bound,
/// while configured transports can be supplied directly without implementing
/// a meaningless default configuration.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait Transport: Send + Sync + Debug + Clone {
    /// Connect to a WebSocket endpoint using this transport.
    async fn ws_connect(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<(crate::ws::WsSender, crate::ws::WsReceiver), crate::ws::WsError> {
        crate::ws::connect(url, headers).await
    }

    /// Make the transport use a proxy.
    ///
    /// SOCKS proxy schemes such as `socks5h` are available only when this crate
    /// is built with the `reqwest` feature. The default `bitreq` backend accepts
    /// HTTP proxy URLs only.
    fn with_proxy(
        &mut self,
        proxy: Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<(), HttpError>;

    /// DNS resolver to get TXT records from a domain name.
    ///
    /// Transports that support DNS resolution should override this method. The
    /// default implementation keeps the trait API stable when the `bip353`
    /// feature is disabled.
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
        Err(HttpError::Other(
            "DNS TXT resolution is not enabled for this transport".to_owned(),
        ))
    }

    /// HTTP GET request.
    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: DeserializeOwned;

    /// HTTP GET request returning a raw response.
    async fn http_get_raw(
        &self,
        url: Url,
        auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError>;

    /// HTTP POST request.
    async fn http_post<P, R>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned;

    /// HTTP POST request with a form body returning a raw response.
    async fn http_post_form_raw<P>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync;
}

/// Default async transport backed by the crate `HttpClient`.
#[cfg(any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest"))]
#[derive(Debug, Clone)]
pub struct Async {
    inner: HttpClient,
}

#[cfg(any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest"))]
impl Default for Async {
    fn default() -> Self {
        Self {
            inner: HttpClient::builder()
                .no_redirects()
                .build()
                .expect("default no-redirect client"),
        }
    }
}

#[cfg(any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest"))]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl Transport for Async {
    fn with_proxy(
        &mut self,
        proxy: Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        let builder = HttpClientBuilder::default()
            .no_redirects()
            .danger_accept_invalid_certs(accept_invalid_certs);

        let builder = match host_matcher {
            Some(pattern) => builder.proxy_with_matcher(proxy, pattern)?,
            None => builder.proxy(proxy),
        };

        self.inner = builder.build()?;
        Ok(())
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, HttpError> {
        crate::dns::resolve_dns_txt(domain).await
    }

    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        self.http_get_raw(url, auth).await?.json_or_status_error()
    }

    async fn http_get_raw(
        &self,
        url: Url,
        auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        let url_str = url.to_string();
        let mut request = self.inner.get(&url_str);

        if let Some(auth) = auth {
            let auth = bind_auth(auth, "GET", &url, &[])?;
            request = request.header(auth.header_key(), auth.to_string());
        }

        request.send().await
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
        let url_str = url.to_string();
        let body =
            serde_json::to_vec(payload).map_err(|e| HttpError::Serialization(e.to_string()))?;
        let mut request = self
            .inner
            .post(&url_str)
            .body_bytes(body.clone())
            .header("Content-Type", "application/json");

        if let Some(auth) = auth_token {
            let auth = bind_auth(auth, "POST", &url, &body)?;
            request = request.header(auth.header_key(), auth.to_string());
        }

        request.send_json::<R>().await
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
        let url_str = url.to_string();
        let body = serde_urlencoded::to_string(payload)
            .map_err(|e| HttpError::Serialization(e.to_string()))?
            .into_bytes();
        let mut request = self
            .inner
            .post(&url_str)
            .body_bytes(body.clone())
            .header("Content-Type", "application/x-www-form-urlencoded");

        if let Some(auth) = auth_token {
            let auth = bind_auth(auth, "POST", &url, &body)?;
            request = request.header(auth.header_key(), auth.to_string());
        }

        request.send().await
    }
}

#[cfg(all(
    feature = "bitreq",
    not(feature = "reqwest"),
    not(target_arch = "wasm32")
))]
/// Bitreq-backed transport implementation.
pub type BitreqTransport = Async;

#[cfg(all(feature = "reqwest", not(target_arch = "wasm32")))]
/// Reqwest-backed transport implementation.
pub type ReqwestTransport = Async;

#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
mod tor_transport;

#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub use self::tor_transport::TorAsync;

#[cfg(all(
    test,
    not(target_arch = "wasm32"),
    any(feature = "bitreq", feature = "reqwest")
))]
mod tests {
    use cashu::nuts::nut10::nutroot::SpendInfo;
    use cashu::nuts::{BlindAuthToken, Id, Proof, SecretKey};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn nutroot_transport_signs_the_exact_post_body_and_query() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url: Url = format!("http://{}/v1/swap?x=%2F", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![];
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
            }
            let headers = String::from_utf8(request).unwrap();
            let mut length = 0;
            let mut token = None;
            for line in headers.lines().skip(1) {
                if let Some((key, value)) = line.split_once(':') {
                    if key.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                    if key.eq_ignore_ascii_case("blind-auth") {
                        token = Some(value.trim().parse::<BlindAuthToken>().unwrap());
                    }
                }
            }
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            let first: Vec<_> = headers.lines().next().unwrap().split_whitespace().collect();
            let mut token = token.unwrap();
            token
                .set_request_context(first[0], first[1], &body)
                .unwrap();
            token.verify_request().unwrap();
            assert_eq!(first[1], "/v1/swap?x=%2F");
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({"z": "last", "a": "first"})
            );
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await.unwrap();
        });
        let key = SecretKey::from_slice(&[9; 32]).unwrap();
        let id = Id::from_bytes(&[vec![2], vec![1; 32]].concat()).unwrap();
        let c = cashu::nuts::nut01::BlsG1PublicKey::hash_to_curve(b"signature").into();
        let mut proof = Proof::new(
            1.into(),
            id,
            key.public_key().to_string().parse().unwrap(),
            c,
        );
        proof.spend_info = Some(SpendInfo {
            bearer_key: Some(key),
            ..Default::default()
        });
        let auth = AuthToken::BlindAuth(BlindAuthToken::from_proof(proof).unwrap());
        #[derive(Serialize)]
        struct Payload<'a> {
            z: &'a str,
            a: &'a str,
        }
        let transport = Async::default();
        let result: serde_json::Value = transport
            .http_post(
                url,
                Some(auth),
                &Payload {
                    z: "last",
                    a: "first",
                },
            )
            .await
            .unwrap();
        assert_eq!(result, serde_json::json!({}));
        server.await.unwrap();
    }
}

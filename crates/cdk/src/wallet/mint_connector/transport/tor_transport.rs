///! Tor transport implementation (non-wasm32 only)
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

use super::super::Error;
use crate::wallet::mint_connector::transport::{ErrorResponse, Transport};

pub struct TorAsync {
    tor: Arc<OnceCell<arti_client::TorClient<tor_rtcompat::PreferredRuntime>>>,
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
        self.request::<Vec<u8>, R>(Method::GET, url, auth, None)
            .await
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
        self.request::<Vec<u8>, R>(Method::POST, url, auth_token, Some(body))
            .await
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, Error> {
        // Use DNS over HTTPS through Tor to avoid clearnet DNS leaks
        // Google DoH JSON API: https://dns.google/resolve?name=<domain>&type=TXT
        // We parse the JSON and extract TXT strings, concatenating quoted chunks if present
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
            // TXT answers often look like: "part1" "part2" or just "value"
            // Concatenate segments between quotes. Fallback: trim surrounding quotes.
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

        let mut url = Url::parse("https://dns.google/resolve")
            .map_err(|e| Error::Custom(e.to_string()))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("name", domain);
            qp.append_pair("type", "TXT");
        }

        let resp: DnsResp = self
            .request::<Vec<u8>, DnsResp>(Method::GET, url, None, None::<Vec<u8>>)
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

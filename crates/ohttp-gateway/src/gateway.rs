use std::str::FromStr;

use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use url::Url;
use {reqwest, serde_json};

use crate::key_config::OhttpConfig;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Magic Cashu purpose string for gateway prober
const MAGIC_CASHU_PURPOSE: &[u8] = b"CASHU 2253f530-151f-4800-a58e-c852a8dc8cff";

#[derive(Debug)]
struct BackendResponse {
    status: u16,
    headers: Vec<(reqwest::header::HeaderName, reqwest::header::HeaderValue)>,
    body: Vec<u8>,
}

/// Handle OHTTP gateway requests
pub async fn handle_ohttp_request(
    axum::extract::Extension(ohttp): axum::extract::Extension<OhttpConfig>,
    axum::extract::Extension(backend_url): axum::extract::Extension<Url>,
    body: Bytes,
) -> Result<Response, GatewayError> {
    tracing::trace!("Received OHTTP request, size: {}", body.len());

    // Decapsulate the OHTTP request
    let (bhttp_req, response_context) = match ohttp.server.decapsulate(&body) {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("Failed to decapsulate OHTTP request: {}", e);
            return Err(GatewayError::OhttpDecapsulation);
        }
    };

    // Parse the inner BHTTP request
    let inner_req = match parse_bhttp_request(&bhttp_req) {
        Ok(req) => req,
        Err(e) => {
            tracing::error!("Failed to parse BHTTP request: {}", e);
            return Err(GatewayError::InvalidRequest);
        }
    };

    // Forward the request to the configured backend
    let response = match forward_request(&backend_url, &inner_req).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("Failed to forward request: {}", e);
            return Err(GatewayError::ForwardingFailed);
        }
    };

    // Convert the response back to BHTTP format
    let bhttp_resp = match convert_to_bhttp_response(&response).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("Failed to convert response to BHTTP: {}", e);
            return Err(GatewayError::ResponseEncodingFailed);
        }
    };

    // Re-encapsulate the response
    let ohttp_resp = match response_context.encapsulate(&bhttp_resp) {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("Failed to re-encapsulate OHTTP response: {}", e);
            return Err(GatewayError::OhttpEncapsulation);
        }
    };

    tracing::trace!("Sending OHTTP response, size: {}", ohttp_resp.len());

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "message/ohttp-res")
        .body(axum::body::Body::from(ohttp_resp))
        .unwrap())
}

/// Handle requests for OHTTP keys
pub async fn handle_ohttp_keys(
    axum::extract::Extension(ohttp): axum::extract::Extension<OhttpConfig>,
) -> Result<Response, GatewayError> {
    let keys = match ohttp.server.config().encode() {
        Ok(keys) => keys,
        Err(e) => {
            tracing::error!("Failed to encode OHTTP keys: {}", e);
            return Err(GatewayError::KeyEncodingFailed);
        }
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/ohttp-keys")
        .body(axum::body::Body::from(keys))
        .unwrap())
}

/// Handle GET requests to /.well-known/ohttp-gateway
///
/// This endpoint handles two scenarios:
/// 1. Without query params: returns OHTTP keys (standard behavior)
/// 2. With ?allowed_purposes: returns Cashu opt-in information (gateway prober)
pub async fn handle_gateway_get(
    axum::extract::Extension(ohttp): axum::extract::Extension<OhttpConfig>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Response, GatewayError> {
    tracing::debug!(
        "Received GET request to /.well-known/ohttp-gateway with params: {:?}",
        params
    );

    // Check if the allowed_purposes query parameter is present (gateway prober)
    if params.contains_key("allowed_purposes") {
        tracing::debug!("Received gateway prober request for allowed purposes");

        // Encode the magic string in the same format as a TLS ALPN protocol list (a
        // U16BE count of strings followed by U8 length encoded strings).
        //
        // The string is just "CASHU" followed by a UUID, that signals to relays
        // that this OHTTP gateway will accept any requests associated with this
        // purpose.
        let mut alpn_encoded = Vec::new();

        // Add 16-bit big-endian count of strings in the list
        // We have 1 string
        let num_strings = 1u16;
        alpn_encoded.extend_from_slice(&num_strings.to_be_bytes());

        // Add the Cashu purpose string with its length prefix
        let purpose_len = MAGIC_CASHU_PURPOSE.len() as u8;
        alpn_encoded.push(purpose_len);
        alpn_encoded.extend_from_slice(MAGIC_CASHU_PURPOSE);

        tracing::debug!(
            "Responding with Cashu opt-in, purpose string: {}",
            String::from_utf8_lossy(MAGIC_CASHU_PURPOSE)
        );

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/x-ohttp-allowed-purposes")
            .body(axum::body::Body::from(alpn_encoded))
            .unwrap())
    } else {
        // Standard OHTTP keys request
        tracing::debug!("Returning OHTTP keys");

        let keys = match ohttp.server.config().encode() {
            Ok(keys) => keys,
            Err(e) => {
                tracing::error!("Failed to encode OHTTP keys: {}", e);
                return Err(GatewayError::KeyEncodingFailed);
            }
        };

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/ohttp-keys")
            .body(axum::body::Body::from(keys))
            .unwrap())
    }
}

#[derive(Clone)]
pub struct InnerRequest {
    pub method: String,
    pub uri: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub enum GatewayError {
    OhttpDecapsulation,
    OhttpEncapsulation,
    InvalidRequest,
    ForwardingFailed,
    ResponseEncodingFailed,
    KeyEncodingFailed,
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            GatewayError::OhttpDecapsulation => (StatusCode::BAD_REQUEST, "Invalid OHTTP request"),
            GatewayError::OhttpEncapsulation => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "OHTTP encapsulation failed",
            ),
            GatewayError::InvalidRequest => (StatusCode::BAD_REQUEST, "Invalid inner request"),
            GatewayError::ForwardingFailed => {
                (StatusCode::BAD_GATEWAY, "Failed to forward request")
            }
            GatewayError::ResponseEncodingFailed => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Response encoding failed",
            ),
            GatewayError::KeyEncodingFailed => {
                (StatusCode::INTERNAL_SERVER_ERROR, "Key encoding failed")
            }
        };

        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_string(&serde_json::json!({
                    "error": message,
                    "type": "gateway_error"
                }))
                .unwrap_or_else(|_| "{}".to_string()),
            ))
            .unwrap()
    }
}

fn parse_bhttp_request(bhttp_bytes: &[u8]) -> Result<InnerRequest, BoxError> {
    use bhttp::Message;

    tracing::trace!("Parsing BHTTP request, size: {} bytes", bhttp_bytes.len());

    let mut cursor = std::io::Cursor::new(bhttp_bytes);
    let req = Message::read_bhttp(&mut cursor)?;

    let method =
        String::from_utf8_lossy(req.control().method().ok_or("Missing method")?).to_string();

    let scheme = req.control().scheme().unwrap_or(b"https");
    let authority = req.control().authority().unwrap_or(b"");
    let path = req.control().path().unwrap_or(b"/");

    let uri = format!(
        "{}://{}{}",
        String::from_utf8_lossy(scheme),
        String::from_utf8_lossy(authority),
        String::from_utf8_lossy(path)
    );

    tracing::info!("Gateway request: {} {}", method, uri);
    tracing::trace!(
        "URI components - scheme: '{}', authority: '{}', path: '{}'",
        String::from_utf8_lossy(scheme),
        String::from_utf8_lossy(authority),
        String::from_utf8_lossy(path)
    );

    let mut headers = Vec::new();
    for header in req.header().fields() {
        headers.push((
            String::from_utf8_lossy(header.name()).to_string(),
            String::from_utf8_lossy(header.value()).to_string(),
        ));
    }

    let body = req.content().to_vec();
    tracing::trace!("Inner request body size: {} bytes", body.len());

    Ok(InnerRequest {
        method,
        uri,
        headers,
        body,
    })
}

async fn forward_request(
    backend_url: &Url,
    inner_req: &InnerRequest,
) -> Result<BackendResponse, BoxError> {
    // Extract path from inner request's URI for forwarding
    let inner_uri = Uri::from_str(&inner_req.uri)?;
    let path_and_query = inner_uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    // Construct backend URL with the path from the inner request
    let mut backend_url_with_path = backend_url.clone();
    backend_url_with_path.set_path(path_and_query);
    if let Some(query) = inner_uri.query() {
        backend_url_with_path.set_query(Some(query));
        tracing::trace!("Added query parameters: '{}'", query);
    }

    tracing::debug!(
        "Forwarding {} {} to {}",
        inner_req.method,
        inner_req.uri,
        backend_url_with_path
    );
    tracing::trace!("Request headers: {:?}", inner_req.headers);
    tracing::trace!("Request body size: {} bytes", inner_req.body.len());

    // Use reqwest for the actual HTTP request (simpler than hyper's low-level API)
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?;

    let mut req_builder = client.request(
        reqwest::Method::from_str(&inner_req.method)?,
        backend_url_with_path.as_str(),
    );

    // Add headers from the inner request
    for (name, value) in &inner_req.headers {
        req_builder = req_builder.header(name, value);
    }

    // Add body if present
    let request = if inner_req.body.is_empty() {
        req_builder.build()?
    } else {
        req_builder.body(inner_req.body.clone()).build()?
    };

    let response = client.execute(request).await?;
    let status = response.status();
    let headers = response.headers().clone();
    let body_bytes = response.bytes().await?;

    tracing::debug!("Backend response: {}", status);
    tracing::trace!("Response headers: {:?}", headers);
    tracing::trace!("Response body size: {} bytes", body_bytes.len());

    // Create a simple response structure for processing
    let backend_response = BackendResponse {
        status: status.as_u16(),
        headers: headers
            .into_iter()
            .filter_map(|(k, v)| k.map(|key| (key, v.clone())))
            .collect(),
        body: body_bytes.to_vec(),
    };

    Ok(backend_response)
}

async fn convert_to_bhttp_response(resp: &BackendResponse) -> Result<Vec<u8>, BoxError> {
    use bhttp::{Message, StatusCode as BhttpStatus};

    let status_code = BhttpStatus::try_from(resp.status).map_err(|_| "Invalid status code")?;

    let mut bhttp_resp = Message::response(status_code);

    // Add response headers
    for (name, value) in &resp.headers {
        bhttp_resp.put_header(name.as_str(), value.to_str()?);
    }

    // Write the response body
    bhttp_resp.write_content(&resp.body);

    let mut bhttp_bytes = Vec::new();
    bhttp_resp.write_bhttp(bhttp::Mode::KnownLength, &mut bhttp_bytes)?;

    Ok(bhttp_bytes)
}

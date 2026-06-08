use axum::body::to_bytes;
use axum::extract::{FromRequest, Request};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use maud::{html, Markup};
use rand::Rng;
use serde::de::DeserializeOwned;

const CSRF_COOKIE_NAME: &str = "ldk_node_dashboard_csrf";
const CSRF_TOKEN_BYTES: usize = 32;
const CSRF_TOKEN_HEX_LEN: usize = CSRF_TOKEN_BYTES * 2;
const FORM_BODY_LIMIT: usize = 1024 * 1024;

/// Active CSRF token for the dashboard request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsrfToken(String);

impl CsrfToken {
    /// Return the token value.
    pub fn value(&self) -> &str {
        &self.0
    }
}

/// Form extractor that validates the dashboard double-submit CSRF token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsrfForm<T>(pub T);

impl<S, T> FromRequest<S> for CsrfForm<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = StatusCode;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let cookie_token = csrf_cookie_value(request.headers())
            .ok_or(StatusCode::FORBIDDEN)?
            .to_owned();

        let body = to_bytes(request.into_body(), FORM_BODY_LIMIT)
            .await
            .map_err(|_| StatusCode::BAD_REQUEST)?;

        let fields: Vec<(String, String)> =
            serde_urlencoded::from_bytes(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
        let form_token = fields
            .iter()
            .find_map(|(name, value)| (name == "_csrf").then_some(value.as_str()))
            .ok_or(StatusCode::FORBIDDEN)?;

        if cookie_token != form_token {
            return Err(StatusCode::FORBIDDEN);
        }

        let form = serde_urlencoded::from_bytes(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
        Ok(Self(form))
    }
}

/// Ensure each dashboard request has an active CSRF token available to render.
pub async fn ensure_csrf_token(mut request: Request, next: Next) -> Response {
    let existing_token = csrf_cookie_value(request.headers());
    let (token, should_set_cookie) = match existing_token {
        Some(token) => (token.to_owned(), false),
        None => (generate_token(), true),
    };

    request.extensions_mut().insert(CsrfToken(token.clone()));

    let mut response = next.run(request).await;

    if should_set_cookie {
        if let Ok(header_value) = HeaderValue::from_str(&csrf_cookie_header_value(&token)) {
            response.headers_mut().append(SET_COOKIE, header_value);
        }
    }

    response
}

/// Render a hidden CSRF form field.
pub fn csrf_input(token: &CsrfToken) -> Markup {
    html! {
        input type="hidden" name="_csrf" value=(token.value()) {}
    }
}

fn generate_token() -> String {
    let bytes = rand::rng().random::<[u8; CSRF_TOKEN_BYTES]>();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn csrf_cookie_value(headers: &HeaderMap) -> Option<&str> {
    let cookie_header = headers.get(COOKIE)?.to_str().ok()?;

    cookie_header
        .split(';')
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find_map(|(name, value)| {
            (name == CSRF_COOKIE_NAME && is_valid_token(value)).then_some(value)
        })
}

fn csrf_cookie_header_value(token: &str) -> String {
    format!("{CSRF_COOKIE_NAME}={token}; HttpOnly; SameSite=Strict; Path=/")
}

fn is_valid_token(token: &str) -> bool {
    token.len() == CSRF_TOKEN_HEX_LEN && token.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::{Extension, Router};
    use serde::Deserialize;
    use tower::ServiceExt;

    use super::{ensure_csrf_token, CsrfForm, CsrfToken, CSRF_COOKIE_NAME};

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct TestForm {
        value: String,
    }

    async fn submit(CsrfForm(form): CsrfForm<TestForm>) -> String {
        form.value
    }

    fn test_app() -> Router {
        Router::new().route("/submit", post(submit))
    }

    #[tokio::test]
    async fn missing_csrf_cookie_rejects_post() {
        let response = test_app()
            .oneshot(
                Request::post("/submit")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("_csrf=abc&value=test"))
                    .expect("test request should build"),
            )
            .await
            .expect("test service should respond");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn missing_csrf_field_rejects_post() {
        let token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let response = test_app()
            .oneshot(
                Request::post("/submit")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", format!("{CSRF_COOKIE_NAME}={token}"))
                    .body(Body::from("value=test"))
                    .expect("test request should build"),
            )
            .await
            .expect("test service should respond");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn mismatched_csrf_tokens_reject_post() {
        let cookie_token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let form_token = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let response = test_app()
            .oneshot(
                Request::post("/submit")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", format!("{CSRF_COOKIE_NAME}={cookie_token}"))
                    .body(Body::from(format!("_csrf={form_token}&value=test")))
                    .expect("test request should build"),
            )
            .await
            .expect("test service should respond");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn matching_csrf_tokens_allow_form_deserialization() {
        let token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let response = test_app()
            .oneshot(
                Request::post("/submit")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", format!("{CSRF_COOKIE_NAME}={token}"))
                    .body(Body::from(format!("_csrf={token}&value=test")))
                    .expect("test request should build"),
            )
            .await
            .expect("test service should respond");

        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("test response body should read");

        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"test");
    }

    #[tokio::test]
    async fn middleware_sets_cookie_and_exposes_token_for_rendering() {
        async fn render(Extension(token): Extension<CsrfToken>) -> impl IntoResponse {
            token.value().to_owned()
        }

        let response = Router::new()
            .route("/", get(render))
            .layer(axum::middleware::from_fn(ensure_csrf_token))
            .oneshot(
                Request::get("/")
                    .body(Body::empty())
                    .expect("test request should build"),
            )
            .await
            .expect("test service should respond");

        assert_eq!(response.status(), StatusCode::OK);

        let cookie = response
            .headers()
            .get("set-cookie")
            .expect("csrf cookie should be set")
            .to_str()
            .expect("csrf cookie should be valid ASCII")
            .to_owned();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("test response body should read");
        let rendered_token =
            String::from_utf8(body.to_vec()).expect("rendered token should be valid UTF-8");

        assert!(cookie.contains(&format!("{CSRF_COOKIE_NAME}={rendered_token}")));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Path=/"));
    }
}

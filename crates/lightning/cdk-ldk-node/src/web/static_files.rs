use axum::extract::Path;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "static/"]
pub struct Assets;

fn get_content_type(path: &str) -> &'static str {
    if let Some(extension) = path.rsplit('.').next() {
        match extension.to_lowercase().as_str() {
            "css" => "text/css",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "svg" => "image/svg+xml",
            "ico" => "image/x-icon",
            _ => "application/octet-stream",
        }
    } else {
        "application/octet-stream"
    }
}

pub async fn static_handler(Path(path): Path<String>) -> impl IntoResponse {
    let cleaned_path = path.trim_start_matches('/');

    match Assets::get(cleaned_path) {
        Some(content) => {
            let content_type = get_content_type(cleaned_path);
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, content_type.parse().unwrap());

            // Add cache headers for static assets
            headers.insert(
                header::CACHE_CONTROL,
                "public, max-age=31536000".parse().unwrap(),
            );

            (headers, content.data).into_response()
        }
        None => {
            tracing::warn!("Static file not found: {}", cleaned_path);
            (StatusCode::NOT_FOUND, "404 Not Found").into_response()
        }
    }
}

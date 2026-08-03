//! Frozen constants for the first CDK HTTP-over-Iroh protocol.

use std::time::Duration;

/// Versioned Iroh ALPN for CDK's HTTP bridge.
pub const ALPN: &[u8] = b"cashu-cdk-http/1";
/// HTTP version carried on every admitted Iroh stream.
pub const HTTP_VERSION: &str = "HTTP/1.1";
/// Maximum request header bytes admitted by the bridge.
pub const MAX_HEADER_BYTES: usize = 64 * 1024;
/// Maximum request body bytes admitted before route-specific Axum limits.
pub const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
/// Default maximum response bytes collected by an uncustomized caller.
pub const MAX_RESPONSE_BODY_BYTES: usize = 16 * 1024 * 1024;
/// Maximum connection-establishment duration.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Maximum duration for opening a bidirectional stream.
pub const STREAM_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum duration for receiving request or response HTTP headers.
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(15);
/// Maximum duration without request or response body progress.
pub const BODY_PROGRESS_TIMEOUT: Duration = Duration::from_secs(30);
/// Maximum duration of one non-WebSocket HTTP request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Maximum duration an admitted connection may remain without active streams.
pub const CONNECTION_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Maximum graceful shutdown drain duration.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

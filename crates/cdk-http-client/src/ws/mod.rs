//! WebSocket client abstraction for CDK
//!
//! Provides a platform-agnostic WebSocket client. On native targets this uses
//! `tokio-tungstenite`; on WASM it uses `ws_stream_wasm`.

mod error;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

pub use error::WsError;
#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub(crate) use native::connect_tor;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{connect, from_websocket_stream, WsReceiver, WsSender};
#[cfg(target_arch = "wasm32")]
pub use wasm::{connect, WsReceiver, WsSender};

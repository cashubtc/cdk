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
#[cfg(not(target_arch = "wasm32"))]
pub use native::{connect, WsReceiver, WsSender};
#[cfg(target_arch = "wasm32")]
pub use wasm::{connect, WsReceiver, WsSender};

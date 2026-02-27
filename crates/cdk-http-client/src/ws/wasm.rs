//! WASM WebSocket implementation using web-sys + wasm-bindgen

use std::cell::RefCell;
use std::rc::Rc;

use futures::StreamExt;
use futures_channel::{mpsc, oneshot};
use wasm_bindgen::prelude::*;
use web_sys::{BinaryType, CloseEvent, ErrorEvent, MessageEvent, WebSocket};

use super::WsError;

/// WebSocket sender half
pub struct WsSender {
    ws: WebSocket,
    // Store closures to prevent leak; dropped when WsSender is dropped
    _onopen: Closure<dyn FnMut(JsValue)>,
    _onerror: Closure<dyn FnMut(ErrorEvent)>,
}

impl std::fmt::Debug for WsSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsSender").finish_non_exhaustive()
    }
}

/// WebSocket receiver half
pub struct WsReceiver {
    ws: WebSocket,
    rx: mpsc::UnboundedReceiver<Result<String, WsError>>,
    _onmessage: Closure<dyn FnMut(MessageEvent)>,
    _onclose: Closure<dyn FnMut(CloseEvent)>,
}

impl std::fmt::Debug for WsReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsReceiver").finish_non_exhaustive()
    }
}

impl Drop for WsSender {
    fn drop(&mut self) {
        self.ws.set_onopen(None);
        self.ws.set_onerror(None);
    }
}

impl Drop for WsReceiver {
    fn drop(&mut self) {
        self.ws.set_onmessage(None);
        self.ws.set_onclose(None);
    }
}

impl WsSender {
    /// Send a text message over the WebSocket
    pub async fn send(&mut self, text: String) -> Result<(), WsError> {
        self.ws
            .send_with_str(&text)
            .map_err(|e| WsError::Send(format!("{:?}", e)))
    }

    /// Send a close frame
    pub async fn close(&mut self) -> Result<(), WsError> {
        self.ws
            .close()
            .map_err(|e| WsError::Send(format!("{:?}", e)))
    }
}

impl WsReceiver {
    /// Receive the next text message. Returns `None` when the connection is closed.
    pub async fn recv(&mut self) -> Option<Result<String, WsError>> {
        self.rx.next().await
    }
}

/// Connect to a WebSocket endpoint.
///
/// On WASM, custom headers are not supported by the browser WebSocket API.
/// If `headers` is non-empty, a warning is logged and headers are ignored.
pub async fn connect(
    url: &str,
    headers: &[(&str, &str)],
) -> Result<(WsSender, WsReceiver), WsError> {
    if !headers.is_empty() {
        tracing::warn!(
            "WebSocket headers are not supported on WASM (browser limitation). \
             {} header(s) will be ignored.",
            headers.len()
        );
    }

    let ws = WebSocket::new(url).map_err(|e| WsError::Connection(format!("{:?}", e)))?;
    ws.set_binary_type(BinaryType::Arraybuffer);

    let (msg_tx, msg_rx) = mpsc::unbounded::<Result<String, WsError>>();
    let (open_tx, open_rx) = oneshot::channel::<Result<(), WsError>>();

    // Shared oneshot sender for both onopen and onerror (whichever fires first)
    let open_tx: Rc<RefCell<Option<oneshot::Sender<Result<(), WsError>>>>> =
        Rc::new(RefCell::new(Some(open_tx)));

    // onopen — signal that the connection is ready
    let open_tx_open = Rc::clone(&open_tx);
    let onopen = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| {
        if let Some(tx) = open_tx_open.borrow_mut().take() {
            let _ = tx.send(Ok(()));
        }
    });
    ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));

    // onmessage — push text messages into the channel
    let msg_tx_msg: mpsc::UnboundedSender<Result<String, WsError>> = msg_tx.clone();
    let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        if let Some(text) = e.data().as_string() {
            let _ = msg_tx_msg.unbounded_send(Ok(text));
        }
    });
    ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

    // onerror — send error and signal open failure if still pending
    let open_tx_err = Rc::clone(&open_tx);
    let msg_tx_err: mpsc::UnboundedSender<Result<String, WsError>> = msg_tx.clone();
    let onerror = Closure::<dyn FnMut(ErrorEvent)>::new(move |_e: ErrorEvent| {
        let err = WsError::Connection("WebSocket error".to_string());
        if let Some(tx) = open_tx_err.borrow_mut().take() {
            let _ = tx.send(Err(err));
        } else {
            let _ = msg_tx_err.unbounded_send(Err(err));
        }
    });
    ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));

    // onclose — close the message channel
    let onclose = Closure::<dyn FnMut(CloseEvent)>::new(move |_e: CloseEvent| {
        msg_tx.close_channel();
    });
    ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));

    // Wait for the connection to open (or fail)
    open_rx
        .await
        .map_err(|_| WsError::Connection("open channel dropped".to_string()))??;

    let ws_clone = ws.clone();
    Ok((
        WsSender {
            ws,
            _onopen: onopen,
            _onerror: onerror,
        },
        WsReceiver {
            ws: ws_clone,
            rx: msg_rx,
            _onmessage: onmessage,
            _onclose: onclose,
        },
    ))
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Source: https://www.jsonrpc.org/specification#error_object
pub enum WsError {
    /// Invalid JSON was received by the server.
    /// An error occurred on the server while parsing the JSON text.
    ParseError,
    /// The JSON sent is not a valid Request object.
    InvalidRequest,
    /// The method does not exist / is not available.
    MethodNotFound,
    /// Invalid method parameter(s).
    InvalidParams,
    /// Internal JSON-RPC error.
    InternalError,
    /// Custom error
    ServerError(i32, String),
}

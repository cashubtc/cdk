use cdk::nuts::nut17::ws::WsErrorBody;
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

impl From<WsError> for WsErrorBody {
    fn from(val: WsError) -> Self {
        let (id, message) = match val {
            WsError::ParseError => (-32700, "Parse error".to_string()),
            WsError::InvalidRequest => (-32600, "Invalid Request".to_string()),
            WsError::MethodNotFound => (-32601, "Method not found".to_string()),
            WsError::InvalidParams => (-32602, "Invalid params".to_string()),
            WsError::InternalError => (-32603, "Internal error".to_string()),
            WsError::ServerError(code, message) => (code, message),
        };
        WsErrorBody { code: id, message }
    }
}

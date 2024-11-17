use serde::Serialize;

use super::{WsContext, WsError, JSON_RPC_VERSION};

impl From<WsError> for WsErrorResponse {
    fn from(val: WsError) -> Self {
        let (id, message) = match val {
            WsError::ParseError => (-32700, "Parse error".to_string()),
            WsError::InvalidRequest => (-32600, "Invalid Request".to_string()),
            WsError::MethodNotFound => (-32601, "Method not found".to_string()),
            WsError::InvalidParams => (-32602, "Invalid params".to_string()),
            WsError::InternalError => (-32603, "Internal error".to_string()),
            WsError::ServerError(code, message) => (code, message),
        };
        WsErrorResponse { code: id, message }
    }
}

#[derive(Debug, Clone, Serialize)]
struct WsErrorResponse {
    code: i32,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct WsResponse<T: Serialize + Sized> {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<WsErrorResponse>,
    id: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct WsNotification<T> {
    pub jsonrpc: String,
    pub method: String,
    pub params: T,
}

#[async_trait::async_trait]
pub trait WsHandle {
    type Response: Serialize + Sized;

    async fn process(
        self,
        req_id: usize,
        context: &mut WsContext,
    ) -> Result<serde_json::Value, serde_json::Error>
    where
        Self: Sized,
    {
        serde_json::to_value(&match self.handle(context).await {
            Ok(response) => WsResponse {
                jsonrpc: JSON_RPC_VERSION.to_owned(),
                result: Some(response),
                error: None,
                id: req_id,
            },
            Err(error) => WsResponse {
                jsonrpc: JSON_RPC_VERSION.to_owned(),
                result: None,
                error: Some(error.into()),
                id: req_id,
            },
        })
    }

    async fn handle(self, context: &mut WsContext) -> Result<Self::Response, WsError>;
}

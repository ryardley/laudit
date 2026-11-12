use serde_json::{json, Map, Value};

pub const JSONRPC_VERSION: &str = "2.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,
    NoModelConfigured = -32001,
    TourNotFound = -32002,
    StopNotFound = -32003,
    Cancelled = -32004,
    LlmFailure = -32005,
}

impl ErrorCode {
    #[must_use]
    pub const fn code(self) -> i64 {
        self as i64
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    #[must_use]
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ParseError, message)
    }

    #[must_use]
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message)
    }

    #[must_use]
    pub fn method_not_found(method: &str) -> Self {
        Self::new(
            ErrorCode::MethodNotFound,
            format!("method not found: {method}"),
        )
    }

    #[must_use]
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidParams, message)
    }

    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InternalError, message)
    }

    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut error = Map::new();
        error.insert("code".to_owned(), json!(self.code.code()));
        error.insert("message".to_owned(), json!(self.message));
        if let Some(data) = &self.data {
            error.insert("data".to_owned(), data.clone());
        }
        Value::Object(error)
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = &self.message;
        write!(f, "{message} (code {})", self.code.code())
    }
}

impl std::error::Error for RpcError {}

#[must_use]
pub fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "result": result,
    })
}

#[must_use]
pub fn error_response(id: Value, error: &RpcError) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "error": error.to_value(),
    })
}

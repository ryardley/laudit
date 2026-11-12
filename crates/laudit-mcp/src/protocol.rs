//! Minimal MCP/JSON-RPC 2.0 protocol helpers.
//!
//! MCP is JSON-RPC 2.0 with a small set of methods. This module keeps the
//! framing and response shapes in one place so the server and tools stay
//! focused on behavior.

use serde_json::{json, Value};

/// JSON-RPC version string.
pub const JSONRPC_VERSION: &str = "2.0";

/// Protocol version advertised when the client does not request a supported
/// version.
pub const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// MCP protocol versions this server understands.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

/// JSON-RPC parse error.
pub const PARSE_ERROR: i64 = -32700;
/// JSON-RPC invalid request.
pub const INVALID_REQUEST: i64 = -32600;
/// JSON-RPC method not found.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC invalid params.
pub const INVALID_PARAMS: i64 = -32602;
/// JSON-RPC internal error.
pub const INTERNAL_ERROR: i64 = -32603;

/// A JSON-RPC error returned to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    /// JSON-RPC error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
}

impl RpcError {
    /// Build an error with the given code and message.
    #[must_use]
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Build a successful JSON-RPC response.
#[must_use]
pub fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "result": result,
    })
}

/// Build a JSON-RPC error response.
#[must_use]
pub fn error_response(id: Value, error: &RpcError) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "error": {
            "code": error.code,
            "message": error.message,
        },
    })
}

/// Wrap a tool's structured output in an MCP `tools/call` result.
///
/// The structured value is serialized as a single text content block so any
/// MCP client can consume it without understanding a custom schema.
#[must_use]
pub fn tool_result(value: &Value) -> Value {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned());
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
    })
}

/// Build an MCP `tools/call` result for a tool execution failure.
#[must_use]
pub fn tool_error(message: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": message.into() }],
        "isError": true,
    })
}

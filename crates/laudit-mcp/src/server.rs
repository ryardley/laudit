//! Newline-delimited MCP over stdio.
//!
//! The server reads one JSON-RPC message per line from stdin and writes one
//! response per line to stdout. Logs and diagnostics go to stderr so stdout
//! stays a clean protocol channel.

use std::io;

use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

use crate::protocol::{
    error_response, success_response, tool_error, tool_result, RpcError, DEFAULT_PROTOCOL_VERSION,
    INVALID_PARAMS, INVALID_REQUEST, JSONRPC_VERSION, METHOD_NOT_FOUND, PARSE_ERROR,
    SUPPORTED_PROTOCOL_VERSIONS,
};
use crate::tools::{self, ToolError};

/// An MCP stdio server.
#[derive(Debug, Default)]
pub struct Server;

impl Server {
    /// Create a server with no client session yet.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Serve MCP messages until stdin reaches EOF.
    pub async fn serve(self) -> io::Result<()> {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let mut lines = BufReader::new(stdin).lines();
        let mut writer = BufWriter::new(stdout);

        while let Some(line) = lines.next_line().await? {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some(response) = self.handle_line(line).await else {
                continue;
            };
            let encoded = serde_json::to_string(&response).map_err(io::Error::other)?;
            writer.write_all(encoded.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }

        writer.flush().await?;
        Ok(())
    }

    /// Run the server on a current-thread tokio runtime.
    pub fn run(self) -> io::Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(self.serve())
    }

    async fn handle_line(&self, line: &str) -> Option<Value> {
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                return Some(error_response(
                    Value::Null,
                    &RpcError::new(PARSE_ERROR, error.to_string()),
                ));
            }
        };

        let Value::Object(mut object) = value else {
            return Some(error_response(
                Value::Null,
                &RpcError::new(INVALID_REQUEST, "request must be a JSON object"),
            ));
        };

        let id = object.remove("id");
        let is_notification = id.is_none();
        let response_id = id.unwrap_or(Value::Null);

        match object.remove("jsonrpc") {
            Some(Value::String(version)) if version == JSONRPC_VERSION => {}
            _ => {
                return Some(error_response(
                    response_id,
                    &RpcError::new(INVALID_REQUEST, "jsonrpc must be \"2.0\""),
                ));
            }
        }

        let method = match object.remove("method") {
            Some(Value::String(method)) => method,
            _ => {
                return Some(error_response(
                    response_id,
                    &RpcError::new(INVALID_REQUEST, "method must be a string"),
                ));
            }
        };

        let params = match object.remove("params") {
            None | Some(Value::Null) => Value::Object(Map::new()),
            Some(params @ (Value::Object(_) | Value::Array(_))) => params,
            Some(_) => {
                if is_notification {
                    return None;
                }
                return Some(error_response(
                    response_id,
                    &RpcError::new(INVALID_PARAMS, "params must be an object or array"),
                ));
            }
        };

        let result = self.dispatch(&method, &params).await;
        if is_notification {
            return None;
        }

        match result {
            Ok(result) => Some(success_response(response_id, result)),
            Err(error) => Some(error_response(response_id, &error)),
        }
    }

    async fn dispatch(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        match method {
            "initialize" => self.initialize(params),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(tools::list()),
            "tools/call" => self.tools_call(params).await,
            "notifications/initialized" => Ok(Value::Null),
            "notifications/cancelled" => Ok(Value::Null),
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        }
    }

    fn initialize(&self, params: &Value) -> Result<Value, RpcError> {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_PROTOCOL_VERSION);
        let version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
            requested
        } else {
            DEFAULT_PROTOCOL_VERSION
        };
        Ok(json!({
            "protocolVersion": version,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "laudit",
                "version": laudit_core::version(),
            },
        }))
    }

    async fn tools_call(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "name is required"))?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !arguments.is_object() {
            return Err(RpcError::new(INVALID_PARAMS, "arguments must be an object"));
        }

        match tools::call(name, &arguments).await {
            Ok(value) => Ok(tool_result(&value)),
            Err(ToolError::Unknown(name)) => Err(RpcError::new(
                INVALID_PARAMS,
                format!("unknown tool: {name}"),
            )),
            Err(ToolError::Failed(message)) => Ok(tool_error(message)),
        }
    }
}

/// Serve MCP over stdio until EOF.
pub async fn serve() -> io::Result<()> {
    Server::new().serve().await
}

/// Run the MCP stdio server on a current-thread runtime.
pub fn run() -> io::Result<()> {
    Server::new().run()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn request(server: &Server, value: Value) -> Value {
        server
            .handle_line(&value.to_string())
            .await
            .expect("response")
    }

    #[tokio::test]
    async fn initialize_advertises_tools_capability() {
        let server = Server::new();
        let response = request(
            &server,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1" }
                }
            }),
        )
        .await;

        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
        assert!(response["result"]["capabilities"]["tools"].is_object());
        assert_eq!(response["result"]["serverInfo"]["name"], "laudit");
    }

    #[tokio::test]
    async fn tools_list_returns_every_tool() {
        let server = Server::new();
        let response = request(
            &server,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
        )
        .await;

        let tools = response["result"]["tools"].as_array().expect("tools array");
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert_eq!(names, tools::TOOL_NAMES);
        for tool in tools {
            assert!(tool["inputSchema"].is_object());
        }
    }

    #[tokio::test]
    async fn notifications_do_not_receive_responses() {
        let server = Server::new();
        let response = server
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await;
        assert!(response.is_none());
    }

    #[tokio::test]
    async fn unknown_methods_are_errors() {
        let server = Server::new();
        let response = request(
            &server,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "nope", "params": {} }),
        )
        .await;
        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn unknown_tools_are_invalid_params() {
        let server = Server::new();
        let response = request(
            &server,
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": { "name": "nope", "arguments": {} }
            }),
        )
        .await;
        assert_eq!(response["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn malformed_json_is_a_parse_error() {
        let server = Server::new();
        let response = server.handle_line("not json").await.expect("response");
        assert_eq!(response["error"]["code"], PARSE_ERROR);
    }
}

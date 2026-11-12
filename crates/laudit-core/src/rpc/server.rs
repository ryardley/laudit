use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::pin::Pin;

use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::mpsc;

use super::protocol::{error_response, success_response, RpcError, JSONRPC_VERSION};

pub type HandlerFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send + 'a>>;
pub type Handler = for<'a> fn(&'a Value, &'a ServerContext) -> HandlerFuture<'a>;

#[derive(Clone)]
pub struct Client {
    tx: mpsc::UnboundedSender<Value>,
}

impl Client {
    pub fn notify(&self, method: &str, params: Value) -> Result<(), RpcError> {
        let message = json!({
            "jsonrpc": JSONRPC_VERSION,
            "method": method,
            "params": params,
        });
        self.tx
            .send(message)
            .map_err(|_| RpcError::internal("connection closed"))
    }
}

#[derive(Clone)]
pub struct ServerContext {
    version: &'static str,
    client: Client,
}

impl ServerContext {
    #[must_use]
    pub fn version(&self) -> &'static str {
        self.version
    }

    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }
}

struct MethodRegistry {
    handlers: HashMap<&'static str, Handler>,
}

impl MethodRegistry {
    fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    fn insert(&mut self, method: &'static str, handler: Handler) {
        self.handlers.insert(method, handler);
    }

    fn get(&self, method: &str) -> Option<Handler> {
        self.handlers.get(method).copied()
    }

    fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.insert("ping", ping);
        registry.insert("status", status);
        registry
    }
}

pub struct Server {
    registry: MethodRegistry,
}

impl Server {
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: MethodRegistry::with_defaults(),
        }
    }

    pub fn register(&mut self, method: &'static str, handler: Handler) -> &mut Self {
        self.registry.insert(method, handler);
        self
    }

    pub async fn serve(&self) -> io::Result<()> {
        let (tx, rx) = mpsc::unbounded_channel();
        let writer = tokio::spawn(write_loop(rx));
        let context = ServerContext {
            version: crate::VERSION,
            client: Client { tx: tx.clone() },
        };
        let stdin = tokio::io::stdin();
        let mut lines = BufReader::new(stdin).lines();

        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let Some(response) = handle_line(line, &self.registry, &context).await else {
                        continue;
                    };
                    if tx.send(response).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    drop(context);
                    drop(tx);
                    let _ = writer.await;
                    return Err(error);
                }
            }
        }

        drop(context);
        drop(tx);
        match writer.await {
            Ok(result) => result,
            Err(error) => Err(io::Error::other(error)),
        }
    }

    pub fn run(&self) -> io::Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(self.serve())
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn serve() -> io::Result<()> {
    Server::new().serve().await
}

pub fn run() -> io::Result<()> {
    Server::new().run()
}

async fn write_loop(mut rx: mpsc::UnboundedReceiver<Value>) -> io::Result<()> {
    let stdout = tokio::io::stdout();
    let mut writer = BufWriter::new(stdout);
    while let Some(message) = rx.recv().await {
        let line = serde_json::to_string(&message).map_err(io::Error::other)?;
        writer.write_all(line.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }
    writer.flush().await?;
    Ok(())
}

async fn handle_line(
    line: &str,
    registry: &MethodRegistry,
    context: &ServerContext,
) -> Option<Value> {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return Some(error_response(
                Value::Null,
                &RpcError::parse_error(error.to_string()),
            ));
        }
    };

    match value {
        Value::Array(items) => handle_batch(items, registry, context).await,
        Value::Object(_) => handle_single(value, registry, context).await,
        _ => Some(error_response(
            Value::Null,
            &RpcError::invalid_request("request must be a JSON object"),
        )),
    }
}

async fn handle_batch(
    items: Vec<Value>,
    registry: &MethodRegistry,
    context: &ServerContext,
) -> Option<Value> {
    if items.is_empty() {
        return Some(error_response(
            Value::Null,
            &RpcError::invalid_request("batch must not be empty"),
        ));
    }

    let mut responses = Vec::new();
    for item in items {
        if let Some(response) = handle_single(item, registry, context).await {
            responses.push(response);
        }
    }

    if responses.is_empty() {
        None
    } else {
        Some(Value::Array(responses))
    }
}

async fn handle_single(
    value: Value,
    registry: &MethodRegistry,
    context: &ServerContext,
) -> Option<Value> {
    let Value::Object(mut object) = value else {
        return Some(error_response(
            Value::Null,
            &RpcError::invalid_request("request must be a JSON object"),
        ));
    };

    let id = object.remove("id");
    if let Some(id_value) = &id {
        match id_value {
            Value::String(_) | Value::Number(_) | Value::Null => {}
            _ => {
                return Some(error_response(
                    Value::Null,
                    &RpcError::invalid_request("id must be a string, number, or null"),
                ));
            }
        }
    }
    let is_notification = id.is_none();
    let response_id = id.unwrap_or(Value::Null);

    match object.remove("jsonrpc") {
        Some(Value::String(version)) if version == JSONRPC_VERSION => {}
        _ => {
            return Some(error_response(
                response_id,
                &RpcError::invalid_request("jsonrpc must be \"2.0\""),
            ));
        }
    }

    let method = match object.remove("method") {
        Some(Value::String(method)) => method,
        _ => {
            return Some(error_response(
                response_id,
                &RpcError::invalid_request("method must be a string"),
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
                &RpcError::invalid_params("params must be an object or array"),
            ));
        }
    };

    let Some(handler) = registry.get(&method) else {
        if is_notification {
            return None;
        }
        return Some(error_response(
            response_id,
            &RpcError::method_not_found(&method),
        ));
    };

    match handler(&params, context).await {
        Ok(result) => {
            if is_notification {
                None
            } else {
                Some(success_response(response_id, result))
            }
        }
        Err(error) => {
            if is_notification {
                None
            } else {
                Some(error_response(response_id, &error))
            }
        }
    }
}

fn ping<'a>(_params: &'a Value, context: &'a ServerContext) -> HandlerFuture<'a> {
    Box::pin(async move {
        Ok(json!({
            "pong": true,
            "version": context.version(),
        }))
    })
}

fn status<'a>(params: &'a Value, _context: &'a ServerContext) -> HandlerFuture<'a> {
    Box::pin(async move {
        if params
            .get("tour_id")
            .is_some_and(|tour_id| !tour_id.is_string())
        {
            return Err(RpcError::invalid_params("tour_id must be a string"));
        }
        Ok(json!({ "engine": "ok" }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ServerContext {
        let (tx, _rx) = mpsc::unbounded_channel();
        ServerContext {
            version: crate::VERSION,
            client: Client { tx },
        }
    }

    #[tokio::test]
    async fn ping_returns_pong_and_version() {
        let response = handle_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#,
            &MethodRegistry::with_defaults(),
            &context(),
        )
        .await
        .expect("response");

        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["pong"], true);
        assert_eq!(response["result"]["version"], crate::VERSION);
    }

    #[tokio::test]
    async fn status_reports_engine_ok() {
        let response = handle_line(
            r#"{"jsonrpc":"2.0","id":"s1","method":"status","params":{}}"#,
            &MethodRegistry::with_defaults(),
            &context(),
        )
        .await
        .expect("response");

        assert_eq!(response["id"], "s1");
        assert_eq!(response["result"]["engine"], "ok");
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error() {
        let response = handle_line("not json", &MethodRegistry::with_defaults(), &context())
            .await
            .expect("response");

        assert!(response["id"].is_null());
        assert_eq!(response["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let response = handle_line(
            r#"{"jsonrpc":"2.0","id":2,"method":"nope","params":{}}"#,
            &MethodRegistry::with_defaults(),
            &context(),
        )
        .await
        .expect("response");

        assert_eq!(response["id"], 2);
        assert_eq!(response["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn notifications_do_not_produce_responses() {
        let response = handle_line(
            r#"{"jsonrpc":"2.0","method":"ping","params":{}}"#,
            &MethodRegistry::with_defaults(),
            &context(),
        )
        .await;

        assert!(response.is_none());
    }

    #[tokio::test]
    async fn client_notify_emits_notification() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let client = Client { tx };

        client
            .notify("progress", json!({ "pct": 50 }))
            .expect("notify");

        let message = rx.recv().await.expect("notification");
        assert_eq!(message["jsonrpc"], "2.0");
        assert_eq!(message["method"], "progress");
        assert_eq!(message["params"]["pct"], 50);
        assert!(message.get("id").is_none());
    }
}

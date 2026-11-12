use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SECRET: &str = "AKIAIOSFODNN7EXAMPLE";

struct McpServer {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
}

impl McpServer {
    fn spawn(config_home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_laudit-mcp"))
            .env("XDG_CONFIG_HOME", config_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to spawn laudit-mcp");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, messages) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Ok(value) = serde_json::from_str(&line) {
                            if tx.send(value).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        Self {
            child,
            stdin: Some(stdin),
            messages,
        }
    }

    fn send(&mut self, value: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{value}").expect("write request");
        stdin.flush().expect("flush request");
    }

    fn read(&mut self) -> Value {
        self.messages
            .recv_timeout(Duration::from_secs(10))
            .expect("timed out waiting for an MCP message")
    }

    fn request(&mut self, id: Value, method: &str, params: Value) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        self.read()
    }

    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            if start.elapsed() > timeout {
                let _ = self.child.kill();
                panic!("server did not exit within {timeout:?}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("laudit-mcp-test-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, name: &str, contents: &str) {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write file");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn start_mock() -> (tokio::runtime::Runtime, MockServer) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("mock runtime");
    let server = runtime.block_on(MockServer::start());
    (runtime, server)
}

fn sse_body(chunks: &[&str]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        let value = json!({
            "choices": [{
                "delta": { "content": chunk },
                "finish_reason": null
            }]
        });
        body.push_str(&format!("data: {value}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    body
}

fn tool_value(response: &Value) -> Value {
    assert_eq!(
        response["result"]["isError"], false,
        "tool failed: {response}"
    );
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    serde_json::from_str(text).expect("tool result json")
}

#[test]
fn handshake_lists_tools_and_explains_without_secrets() {
    let temp = TempDir::new();
    let config_home = TempDir::new();
    temp.write("src/main.rs", "fn main() {}\n");

    let (runtime, mock) = start_mock();
    runtime.block_on(async {
        let body = format!("TITLE: Entry\nWHY: It starts here.\n---\nThe body contains {SECRET}.");
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body(&[&body])),
            )
            .mount(&mock)
            .await;
    });
    temp.write(
        ".laudit/config.toml",
        &format!(
            "[provider]\nbase_url = \"{}\"\nmodel = \"test-model\"\n",
            mock.uri()
        ),
    );

    let mut server = McpServer::spawn(config_home.path());

    let response = server.request(
        json!(1),
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-harness", "version": "1" }
        }),
    );
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
    assert!(response["result"]["capabilities"]["tools"].is_object());
    assert_eq!(response["result"]["serverInfo"]["name"], "laudit");

    server.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }));

    let response = server.request(json!(2), "tools/list", json!({}));
    let tools = response["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names, ["build_tour", "explain", "outline", "status"]);
    for tool in tools {
        assert!(tool["inputSchema"].is_object());
    }

    let response = server.request(
        json!(3),
        "tools/call",
        json!({
            "name": "build_tour",
            "arguments": {
                "repo": temp.path().to_string_lossy(),
                "offline": true
            }
        }),
    );
    let built = tool_value(&response);
    assert!(built["tour_id"].is_string());
    assert!(built["outline"]["chapters"].is_array());

    let response = server.request(
        json!(4),
        "tools/call",
        json!({
            "name": "explain",
            "arguments": {
                "repo": temp.path().to_string_lossy(),
                "symbol": "main"
            }
        }),
    );
    assert_eq!(
        response["result"]["isError"], false,
        "explain failed: {response}"
    );
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    assert!(!text.contains(SECRET), "secret leaked in response: {text}");
    assert!(
        text.contains('«'),
        "expected a redaction placeholder in: {text}"
    );
    let explained: Value = serde_json::from_str(text).expect("explain json");
    assert_eq!(explained["title"], "Entry");
    assert!(explained["explanation"]
        .as_str()
        .expect("explanation")
        .contains('«'));

    let response = server.request(
        json!(5),
        "tools/call",
        json!({
            "name": "status",
            "arguments": { "repo": temp.path().to_string_lossy() }
        }),
    );
    let status = tool_value(&response);
    assert_eq!(status["model_present"], true);
    assert_eq!(status["tour_present"], true);
    assert!(status["counts"]["stops"].as_u64().unwrap_or(0) >= 1);

    let response = server.request(
        json!(6),
        "tools/call",
        json!({
            "name": "outline",
            "arguments": { "repo": temp.path().to_string_lossy() }
        }),
    );
    let outline = tool_value(&response);
    assert!(outline["chapters"].is_array());

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn eof_shuts_down_cleanly() {
    let config_home = TempDir::new();
    let mut server = McpServer::spawn(config_home.path());
    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn unknown_tool_is_a_jsonrpc_error() {
    let config_home = TempDir::new();
    let mut server = McpServer::spawn(config_home.path());
    let response = server.request(
        json!(1),
        "tools/call",
        json!({ "name": "nope", "arguments": {} }),
    );
    assert_eq!(response["error"]["code"], -32602);
    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

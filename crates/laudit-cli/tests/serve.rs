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

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
}

impl Server {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_laudit"))
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn laudit serve");

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

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{line}").expect("write request");
        stdin.flush().expect("flush request");
    }

    fn send_json(&mut self, value: &Value) {
        self.send(&value.to_string());
    }

    fn read(&mut self) -> Value {
        self.messages
            .recv_timeout(Duration::from_secs(10))
            .expect("timed out waiting for a server message")
    }

    fn read_until(&mut self, mut predicate: impl FnMut(&[Value]) -> bool) -> Vec<Value> {
        let mut messages = Vec::new();
        loop {
            let message = self.read();
            messages.push(message);
            if predicate(&messages) {
                return messages;
            }
        }
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

#[test]
fn ping_returns_valid_jsonrpc_response() {
    let mut server = Server::spawn();

    server.send(r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#);
    let response = server.read();

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["pong"], true);
    assert!(response["result"]["version"].is_string());

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn status_returns_engine_ok() {
    let mut server = Server::spawn();

    server.send(r#"{"jsonrpc":"2.0","id":"status-1","method":"status","params":{}}"#);
    let response = server.read();

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], "status-1");
    assert_eq!(response["result"]["engine"], "ok");

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn malformed_input_returns_jsonrpc_error() {
    let mut server = Server::spawn();

    server.send("this is not json");
    let response = server.read();

    assert_eq!(response["jsonrpc"], "2.0");
    assert!(response["id"].is_null());
    assert_eq!(response["error"]["code"], -32700);
    assert!(response["error"]["message"].is_string());

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn invalid_request_returns_jsonrpc_error() {
    let mut server = Server::spawn();

    server.send("42");
    let response = server.read();

    assert_eq!(response["jsonrpc"], "2.0");
    assert!(response["id"].is_null());
    assert_eq!(response["error"]["code"], -32600);

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn notifications_do_not_receive_responses() {
    let mut server = Server::spawn();

    server.send(r#"{"jsonrpc":"2.0","method":"ping","params":{}}"#);
    server.send(r#"{"jsonrpc":"2.0","id":2,"method":"ping","params":{}}"#);
    let response = server.read();

    assert_eq!(response["id"], 2);
    assert_eq!(response["result"]["pong"], true);

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn eof_shuts_down_cleanly() {
    let mut server = Server::spawn();

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));

    assert!(status.success());
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("laudit-serve-test-{}-{id}", std::process::id()));
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

fn write_config(temp: &TempDir, mock: &MockServer) {
    temp.write(
        ".laudit/config.toml",
        &format!(
            "[provider]\nbase_url = \"{}\"\nmodel = \"test-model\"\n",
            mock.uri()
        ),
    );
}

fn completion(content: &str) -> Value {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }]
    })
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

#[test]
fn build_tour_offline_emits_progress_and_stop_ready() {
    let temp = TempDir::new();
    temp.write("src/main.rs", "fn main() {}\n");
    let mut server = Server::spawn();

    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "build_tour",
        "params": { "repo": temp.path().to_string_lossy(), "offline": true }
    }));

    let messages = server.read_until(|messages| {
        messages.iter().any(|message| message["id"] == 1)
            && messages
                .iter()
                .any(|message| message["method"] == "stop_ready")
    });

    let response = messages
        .iter()
        .find(|message| message["id"] == 1)
        .expect("build_tour response");
    let tour_id = response["result"]["tour_id"]
        .as_str()
        .expect("tour_id string");
    assert!(messages
        .iter()
        .any(|message| message["method"] == "progress"));
    let stop_ready = messages
        .iter()
        .find(|message| message["method"] == "stop_ready")
        .expect("stop_ready notification");
    assert_eq!(stop_ready["params"]["tour_id"], tour_id);
    assert!(stop_ready["params"]["stop_id"].is_string());

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn get_outline_returns_the_planned_tour() {
    let temp = TempDir::new();
    temp.write("src/main.rs", "fn main() {}\n");
    let mut server = Server::spawn();

    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "build_tour",
        "params": { "repo": temp.path().to_string_lossy(), "offline": true }
    }));
    let messages = server.read_until(|messages| {
        messages.iter().any(|message| message["id"] == 1)
            && messages
                .iter()
                .any(|message| message["method"] == "stop_ready")
    });
    let tour_id = messages
        .iter()
        .find(|message| message["id"] == 1)
        .expect("build_tour response")["result"]["tour_id"]
        .as_str()
        .expect("tour_id")
        .to_owned();

    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "get_outline",
        "params": { "tour_id": tour_id }
    }));
    let messages = server.read_until(|messages| messages.iter().any(|message| message["id"] == 2));
    let response = messages
        .iter()
        .find(|message| message["id"] == 2)
        .expect("get_outline response");
    let chapters = response["result"]["chapters"]
        .as_array()
        .expect("chapters array");
    assert!(!chapters.is_empty());
    assert!(chapters[0]["stops"][0]["file"].is_string());

    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "status",
        "params": { "tour_id": tour_id }
    }));
    let messages = server.read_until(|messages| messages.iter().any(|message| message["id"] == 3));
    let response = messages
        .iter()
        .find(|message| message["id"] == 3)
        .expect("status response");
    assert_eq!(response["result"]["engine"], "ok");
    assert!(response["result"]["tour"]["stops"].as_u64().unwrap_or(0) >= 1);

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn cancel_stops_build_tour_work() {
    let temp = TempDir::new();
    temp.write("src/main.rs", "fn main() {}\n");
    let (runtime, mock) = start_mock();
    write_config(&temp, &mock);
    runtime.block_on(async {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(5))
                    .set_body_json(completion(
                        r#"{"subsystems":[],"chapters":[],"flows":[],"glossary":[]}"#,
                    )),
            )
            .mount(&mock)
            .await;
    });

    let mut server = Server::spawn();
    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "build_tour",
        "params": { "repo": temp.path().to_string_lossy() }
    }));
    let messages = server.read_until(|messages| messages.iter().any(|message| message["id"] == 1));
    let tour_id = messages
        .iter()
        .find(|message| message["id"] == 1)
        .expect("build_tour response")["result"]["tour_id"]
        .as_str()
        .expect("tour_id")
        .to_owned();

    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "cancel",
        "params": { "tour_id": tour_id }
    }));
    let messages = server.read_until(|messages| {
        messages.iter().any(|message| message["id"] == 2)
            && messages
                .iter()
                .any(|message| message["method"] == "error" && message["params"]["code"] == -32004)
    });
    assert!(messages
        .iter()
        .any(|message| message["id"] == 2 && message["result"]["cancelled"] == true));
    assert!(!messages
        .iter()
        .any(|message| message["method"] == "stop_ready"));

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn explain_stop_streams_explanation_chunks() {
    let temp = TempDir::new();
    temp.write("src/main.rs", "fn main() {}\n");
    let (runtime, mock) = start_mock();
    write_config(&temp, &mock);
    runtime.block_on(async {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body(&[
                        "TITLE: Entry\nWHY: It starts here.\n---\nThe body.",
                    ])),
            )
            .mount(&mock)
            .await;
    });

    let mut server = Server::spawn();
    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "build_tour",
        "params": { "repo": temp.path().to_string_lossy(), "offline": true }
    }));
    let messages = server.read_until(|messages| {
        messages.iter().any(|message| message["id"] == 1)
            && messages
                .iter()
                .any(|message| message["method"] == "stop_ready")
    });
    let tour_id = messages
        .iter()
        .find(|message| message["id"] == 1)
        .expect("build_tour response")["result"]["tour_id"]
        .as_str()
        .expect("tour_id")
        .to_owned();
    let stop_id = messages
        .iter()
        .find(|message| message["method"] == "stop_ready")
        .expect("stop_ready")["params"]["stop_id"]
        .as_str()
        .expect("stop_id")
        .to_owned();

    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "explain_stop",
        "params": { "tour_id": tour_id, "stop_id": stop_id }
    }));
    let messages = server.read_until(|messages| {
        messages.iter().any(|message| message["id"] == 2)
            && messages
                .iter()
                .any(|message| message["method"] == "explanation_chunk")
    });
    let response = messages
        .iter()
        .find(|message| message["id"] == 2)
        .expect("explain_stop response");
    assert!(response["result"]["stop_id"].is_string());
    let chunk = messages
        .iter()
        .find(|message| message["method"] == "explanation_chunk")
        .expect("explanation_chunk");
    assert_eq!(chunk["params"]["delta"], "The body.");

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn lifecycle_errors_are_jsonrpc_errors() {
    let mut server = Server::spawn();

    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "get_outline",
        "params": { "tour_id": "tour-missing" }
    }));
    let response = server.read();
    assert_eq!(response["id"], 1);
    assert_eq!(response["error"]["code"], -32002);

    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "build_tour",
        "params": {}
    }));
    let response = server.read();
    assert_eq!(response["id"], 2);
    assert_eq!(response["error"]["code"], -32602);

    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "explain_stop",
        "params": { "tour_id": "tour-missing", "stop_id": "stop-1" }
    }));
    let response = server.read();
    assert_eq!(response["id"], 3);
    assert_eq!(response["error"]["code"], -32002);

    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "cancel",
        "params": { "tour_id": "tour-missing" }
    }));
    let response = server.read();
    assert_eq!(response["id"], 4);
    assert_eq!(response["error"]["code"], -32002);

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn build_tour_streams_explanations() {
    let temp = TempDir::new();
    temp.write("src/main.rs", "fn main() {}\n");
    let (runtime, mock) = start_mock();
    write_config(&temp, &mock);
    runtime.block_on(async {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(|request: &wiremock::Request| {
                let body: Value = request.body_json().expect("json body");
                if body["stream"].as_bool().unwrap_or(false) {
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string(sse_body(&[
                            "TITLE: Entry\nWHY: It starts here.\n---\nThe body.",
                        ]))
                } else {
                    ResponseTemplate::new(200).set_body_json(completion(
                        r#"{"subsystems":[],"chapters":[],"flows":[],"glossary":[]}"#,
                    ))
                }
            })
            .mount(&mock)
            .await;
    });

    let mut server = Server::spawn();
    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "build_tour",
        "params": { "repo": temp.path().to_string_lossy() }
    }));
    let messages = server.read_until(|messages| {
        messages.iter().any(|message| message["id"] == 1)
            && messages
                .iter()
                .any(|message| message["method"] == "comprehension_done")
            && messages
                .iter()
                .any(|message| message["method"] == "stop_ready")
            && messages
                .iter()
                .any(|message| message["method"] == "explanation_chunk")
    });

    let response = messages
        .iter()
        .find(|message| message["id"] == 1)
        .expect("build_tour response");
    assert!(response["result"]["tour_id"].is_string());
    assert!(messages
        .iter()
        .any(|message| message["method"] == "progress"));
    let chunk = messages
        .iter()
        .find(|message| message["method"] == "explanation_chunk")
        .expect("explanation_chunk");
    assert_eq!(chunk["params"]["delta"], "The body.");

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

#[test]
fn cancel_stops_explain_stop_work() {
    let temp = TempDir::new();
    temp.write("src/main.rs", "fn main() {}\n");
    let (runtime, mock) = start_mock();
    write_config(&temp, &mock);
    runtime.block_on(async {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(5))
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body(&[
                        "TITLE: Entry\nWHY: It starts here.\n---\nThe body.",
                    ])),
            )
            .mount(&mock)
            .await;
    });

    let mut server = Server::spawn();
    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "build_tour",
        "params": { "repo": temp.path().to_string_lossy(), "offline": true }
    }));
    let messages = server.read_until(|messages| {
        messages.iter().any(|message| message["id"] == 1)
            && messages
                .iter()
                .any(|message| message["method"] == "stop_ready")
    });
    let tour_id = messages
        .iter()
        .find(|message| message["id"] == 1)
        .expect("build_tour response")["result"]["tour_id"]
        .as_str()
        .expect("tour_id")
        .to_owned();
    let stop_id = messages
        .iter()
        .find(|message| message["method"] == "stop_ready")
        .expect("stop_ready")["params"]["stop_id"]
        .as_str()
        .expect("stop_id")
        .to_owned();

    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "explain_stop",
        "params": { "tour_id": tour_id, "stop_id": stop_id }
    }));
    let messages = server.read_until(|messages| messages.iter().any(|message| message["id"] == 2));
    assert!(messages
        .iter()
        .any(|message| message["id"] == 2 && message["result"]["stop_id"] == stop_id));

    server.send_json(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "cancel",
        "params": { "tour_id": tour_id, "stop_id": stop_id }
    }));
    let messages = server.read_until(|messages| {
        messages.iter().any(|message| message["id"] == 3)
            && messages
                .iter()
                .any(|message| message["method"] == "error" && message["params"]["code"] == -32004)
    });
    assert!(messages
        .iter()
        .any(|message| message["id"] == 3 && message["result"]["cancelled"] == true));
    assert!(!messages
        .iter()
        .any(|message| message["method"] == "explanation_chunk"));

    server.close_stdin();
    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success());
}

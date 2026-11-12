use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
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
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));

        Self {
            child,
            stdin: Some(stdin),
            stdout,
        }
    }

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{line}").expect("write request");
        stdin.flush().expect("flush request");
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read response");
        serde_json::from_str(&line).expect("valid JSON response")
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

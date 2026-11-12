//! Redaction-before-send integration test.
//!
//! A local mock LLM server captures the request body. The test asserts that
//! secrets in the prompt never reach the server and that placeholders do.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;

use laudit_core::redact::Redactor;

const AWS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";
const GITHUB_TOKEN: &str = "ghp_1234567890abcdefghijklmnopqrstuvwxyz";
const JWT: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
const PEM: &str =
    "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA...\n-----END RSA PRIVATE KEY-----";
const ENV_SECRET: &str = "hunter2";

#[test]
fn secret_never_reaches_mock_llm_server() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock llm");
    let address = listener.local_addr().expect("mock llm address");
    let (tx, rx) = mpsc::channel();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept request");
        let mut request = Vec::new();
        stream.read_to_end(&mut request).expect("read request");
        let request = String::from_utf8_lossy(&request).into_owned();
        let body = request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_owned())
            .unwrap_or_default();
        tx.send(body).expect("send body");
        let response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    });

    let prompt = format!(
        "Explain this code.\nAWS_ACCESS_KEY_ID={AWS_KEY}\nGITHUB_TOKEN={GITHUB_TOKEN}\nJWT={JWT}\nDATABASE_PASSWORD={ENV_SECRET}\n{PEM}\n"
    );

    let mut redactor = Redactor::default();
    let (redacted, _) = redactor.redact(&prompt);
    for secret in [AWS_KEY, GITHUB_TOKEN, JWT, ENV_SECRET] {
        assert!(!redacted.contains(secret), "redaction missed {secret}");
    }
    assert!(!redacted.contains("BEGIN RSA PRIVATE KEY"));

    let mut client = TcpStream::connect(address).expect("connect mock llm");
    let request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        redacted.len(),
        redacted
    );
    client.write_all(request.as_bytes()).expect("write request");
    client.shutdown(Shutdown::Write).expect("shutdown write");
    let mut response = String::new();
    client.read_to_string(&mut response).expect("read response");
    assert!(response.contains("200 OK"));

    let received = rx.recv().expect("received body");
    server.join().expect("mock llm thread");

    for secret in [AWS_KEY, GITHUB_TOKEN, JWT, ENV_SECRET] {
        assert!(
            !received.contains(secret),
            "secret reached mock LLM: {secret}"
        );
    }
    assert!(!received.contains("BEGIN RSA PRIVATE KEY"));
    assert!(
        received.contains('«'),
        "expected placeholders in request body: {received}"
    );
}

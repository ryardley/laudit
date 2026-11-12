use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use laudit_core::config::Config;
use laudit_core::llm::{
    ChatMessage, ChatRequest, LlmClient, LlmError, ResponseFormat, RetryPolicy,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn completion(content: &str) -> serde_json::Value {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15
        }
    })
}

fn fast_retry(max_retries: u32) -> RetryPolicy {
    RetryPolicy {
        max_retries,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
    }
}

fn client(server: &MockServer) -> LlmClient {
    LlmClient::new(server.uri(), "test-model", Some("test-key".to_owned()))
        .expect("client")
        .with_retry_policy(fast_retry(2))
}

#[tokio::test]
async fn successful_completion_returns_content_and_usage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion("hello")))
        .mount(&server)
        .await;

    let client = client(&server);
    let request = ChatRequest::new(vec![ChatMessage::user("hi")]);
    let response = client.chat(&request).await.expect("completion");

    assert_eq!(response.content, "hello");
    assert_eq!(response.finish_reason.as_deref(), Some("stop"));
    assert_eq!(response.usage.expect("usage").total_tokens, 15);

    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = requests[0].body_json().expect("json body");
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["stream"], false);
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(
        requests[0]
            .headers
            .get("authorization")
            .expect("authorization header")
            .to_str()
            .expect("header text"),
        "Bearer test-key"
    );
}

#[tokio::test]
async fn retries_on_429_then_succeeds() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |_request: &wiremock::Request| {
            if responder_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(429).set_body_string("rate limited")
            } else {
                ResponseTemplate::new(200).set_body_json(completion("after retry"))
            }
        })
        .mount(&server)
        .await;

    let client = client(&server);
    let request = ChatRequest::new(vec![ChatMessage::user("hi")]);
    let response = client.chat(&request).await.expect("completion");

    assert_eq!(response.content, "after retry");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn retries_on_5xx_then_succeeds() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |_request: &wiremock::Request| {
            if responder_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(503).set_body_string("unavailable")
            } else {
                ResponseTemplate::new(200).set_body_json(completion("recovered"))
            }
        })
        .mount(&server)
        .await;

    let client = client(&server);
    let request = ChatRequest::new(vec![ChatMessage::user("hi")]);
    let response = client.chat(&request).await.expect("completion");

    assert_eq!(response.content, "recovered");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn timeout_is_reported_after_retries_are_exhausted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(500))
                .set_body_json(completion("too late")),
        )
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-model", None)
        .expect("client")
        .with_retry_policy(fast_retry(0))
        .with_timeout(Duration::from_millis(50));
    let request = ChatRequest::new(vec![ChatMessage::user("hi")]);
    let error = client.chat(&request).await.expect_err("timeout");

    assert!(matches!(error, LlmError::Timeout), "got: {error}");
}

#[tokio::test]
async fn streaming_yields_chunks_in_order() {
    let server = MockServer::start().await;
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let client = client(&server);
    let request = ChatRequest::new(vec![ChatMessage::user("hi")]);
    let mut stream = client.chat_stream(&request).await.expect("stream");

    let mut text = String::new();
    let mut finish_reason = None;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk");
        text.push_str(&chunk.delta);
        if chunk.finish_reason.is_some() {
            finish_reason = chunk.finish_reason;
        }
    }

    assert_eq!(text, "Hello");
    assert_eq!(finish_reason.as_deref(), Some("stop"));

    let requests = server.received_requests().await.expect("requests");
    let body: serde_json::Value = requests[0].body_json().expect("json body");
    assert_eq!(body["stream"], true);
}

#[tokio::test]
async fn structured_output_falls_back_to_json_object() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            responder_calls.fetch_add(1, Ordering::SeqCst);
            let body: serde_json::Value = request.body_json().expect("json body");
            let format = body["response_format"]["type"].as_str().unwrap_or_default();
            if format == "json_schema" {
                ResponseTemplate::new(400)
                    .set_body_string("unsupported response_format: json_schema")
            } else {
                ResponseTemplate::new(200).set_body_json(completion(r#"{"answer":42}"#))
            }
        })
        .mount(&server)
        .await;

    let client = client(&server);
    let value: serde_json::Value = client
        .complete_json(
            vec![ChatMessage::user("answer")],
            "answer",
            json!({"type": "object"}),
        )
        .await
        .expect("structured output");

    assert_eq!(value["answer"], 42);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn malformed_json_is_repaired() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |_request: &wiremock::Request| {
            if responder_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200).set_body_json(completion(r#"{"answer": 42"#))
            } else {
                ResponseTemplate::new(200).set_body_json(completion(r#"{"answer": 42}"#))
            }
        })
        .mount(&server)
        .await;

    let client = client(&server);
    let value: serde_json::Value = client
        .complete_json(
            vec![ChatMessage::user("answer")],
            "answer",
            json!({"type": "object"}),
        )
        .await
        .expect("repaired output");

    assert_eq!(value["answer"], 42);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn missing_model_is_a_clear_error() {
    let error = LlmClient::new("http://localhost:11434/v1", "", None)
        .expect_err("missing model should fail");
    assert!(matches!(error, LlmError::MissingModel));
    assert!(error.to_string().contains("model"));
}

#[tokio::test]
async fn localhost_requires_no_api_key() {
    let dir = std::env::temp_dir().join(format!("laudit-llm-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        "[provider]\nbase_url = \"http://localhost:11434/v1\"\nmodel = \"llama3\"\n",
    )
    .expect("write config");

    let config = Config::load_from(None, Some(&path)).expect("load config");
    let client = LlmClient::from_config(&config).expect("local client without key");
    assert_eq!(client.model(), "llama3");
    assert_eq!(
        client.endpoint(),
        "http://localhost:11434/v1/chat/completions"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn response_format_is_sent_when_requested() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion("ok")))
        .mount(&server)
        .await;

    let client = client(&server);
    let request = ChatRequest {
        messages: vec![ChatMessage::user("hi")],
        response_format: Some(ResponseFormat::JsonObject),
        ..ChatRequest::default()
    };
    client.chat(&request).await.expect("completion");

    let requests = server.received_requests().await.expect("requests");
    let body: serde_json::Value = requests[0].body_json().expect("json body");
    assert_eq!(body["response_format"]["type"], "json_object");
}

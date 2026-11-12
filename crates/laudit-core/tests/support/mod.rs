//! Shared helpers for integration tests: a mock OpenAI-compatible LLM server.
//!
//! The mock returns canned structured responses for comprehension and a canned
//! streamed front-matter response for narration. It never touches the network
//! beyond the loopback wiremock server (D21).

#![allow(dead_code)]

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A non-streaming chat-completion response body.
pub fn completion(content: &str) -> Value {
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

/// Build a server-sent-events body from ordered content chunks.
pub fn sse_body<S: AsRef<str>>(chunks: &[S]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        let value = json!({
            "choices": [{
                "delta": { "content": chunk.as_ref() },
                "finish_reason": null
            }]
        });
        body.push_str(&format!("data: {value}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// Mount a mock that always returns a non-streaming completion.
pub async fn mount_completion(server: &MockServer, content: String) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion(&content)))
        .mount(server)
        .await;
}

/// Mount a mock that always returns a streamed completion.
pub async fn mount_stream(server: &MockServer, body: String) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(server)
        .await;
}

/// Mount a mock that serves both stages of the pipeline.
///
/// Non-streaming requests receive `model_content` as the assistant message.
/// Streaming requests receive a front-matter envelope whose body references the
/// stop's own symbol ID, so the narration ref validator has something valid to
/// keep.
pub async fn mount_pipeline(server: &MockServer, model_content: String) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().expect("json body");
            if body["stream"].as_bool().unwrap_or(false) {
                let symbol_id = stop_symbol_id(&body);
                let chunks = [
                    "TITLE: Fixture stop\nWHY: It is part of the fixture tour.\n---\nThis stop explains `"
                        .to_owned(),
                    symbol_id,
                    "`.\n".to_owned(),
                ];
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body(&chunks))
            } else {
                ResponseTemplate::new(200).set_body_json(completion(&model_content))
            }
        })
        .mount(server)
        .await;
}

fn stop_symbol_id(body: &Value) -> String {
    let user = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .unwrap_or_default();
    let start = user.find('{').unwrap_or(0);
    let context: Value = serde_json::from_str(&user[start..]).unwrap_or(Value::Null);
    context["stop"]["symbol_id"]
        .as_str()
        .unwrap_or("sym-unknown")
        .to_owned()
}

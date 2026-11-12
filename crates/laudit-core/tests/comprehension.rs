use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use laudit_core::comprehension::{comprehend, comprehend_with_options, ComprehensionOptions};
use laudit_core::config::Config;
use laudit_core::llm::LlmClient;
use laudit_core::model::{CodebaseModel, Skeleton};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "laudit-comprehension-it-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/graph-tree")
}

fn copy_dir_all(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn fixture() -> TempDir {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    temp
}

fn config_for(temp: &TempDir, server: &MockServer, budget: Option<u32>) -> Config {
    let path = temp.path().join(".laudit/config.toml");
    fs::create_dir_all(path.parent().expect("config parent")).expect("create .laudit");
    let mut contents = format!(
        "[provider]\nbase_url = \"{}\"\nmodel = \"test-model\"\n",
        server.uri()
    );
    if let Some(budget) = budget {
        contents.push_str(&format!("\n[budgets]\nmax_context_tokens = {budget}\n"));
    }
    fs::write(&path, contents).expect("write config");
    Config::load_from(None, Some(&path)).expect("load config")
}

fn completion(content: &str) -> Value {
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

async fn mount_completion(server: &MockServer, content: String) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion(&content)))
        .mount(server)
        .await;
}

fn recon_skeleton(temp: &TempDir, config: &Config) -> Skeleton {
    laudit_core::build::recon(temp.path(), config)
        .expect("recon")
        .skeleton
}

fn model_response(valid_id: &str, valid_file: &str) -> Value {
    json!({
        "subsystems": [{
            "id": "sub-1",
            "title": "Core",
            "summary": "Core subsystem.",
            "symbol_ids": [valid_id, "sym-unknown"],
            "files": [valid_file, "src/unknown.rs"]
        }],
        "chapters": [{
            "id": "ch-1",
            "title": "On-ramp",
            "intent": "Get oriented.",
            "stop_ids": [valid_id, "sym-unknown"]
        }],
        "flows": [{
            "id": "flow-1",
            "title": "Request flow",
            "steps": [valid_id, "sym-unknown"]
        }],
        "glossary": [{
            "term": "Term",
            "definition": "A definition."
        }]
    })
}

#[tokio::test]
async fn mock_llm_emits_valid_model_and_drops_unknown_ids() {
    let temp = fixture();
    let server = MockServer::start().await;
    let config = config_for(&temp, &server, None);
    let skeleton = recon_skeleton(&temp, &config);
    let valid_id = skeleton.symbols[0].id.clone();
    let valid_file = skeleton.files[0].path.to_string_lossy().into_owned();

    mount_completion(&server, model_response(&valid_id, &valid_file).to_string()).await;

    let client = LlmClient::new(server.uri(), "test-model", None).expect("client");
    let report = comprehend(temp.path(), &config, &skeleton, &client)
        .await
        .expect("comprehend");

    assert!(!report.cached);
    assert!(!report.hierarchical);
    assert_eq!(report.dropped_ids, vec!["sym-unknown"]);
    assert_eq!(report.dropped_files, vec!["src/unknown.rs"]);

    let model = &report.model;
    assert_eq!(model.subsystems[0].symbol_ids, vec![valid_id.clone()]);
    assert_eq!(model.subsystems[0].files, vec![valid_file.clone()]);
    assert_eq!(model.chapters[0].stop_ids, vec![valid_id.clone()]);
    assert_eq!(model.flows[0].steps, vec![valid_id.clone()]);
    assert_eq!(model.glossary[0].term, "Term");

    let bytes = fs::read(temp.path().join(".laudit/model.json")).expect("read model");
    let persisted: CodebaseModel = serde_json::from_slice(&bytes).expect("parse model");
    persisted.validate_schema_version().expect("schema version");
    assert_eq!(&persisted, model);
}

#[tokio::test]
async fn oversized_input_uses_hierarchical_path() {
    let temp = fixture();
    let server = MockServer::start().await;
    let config = config_for(&temp, &server, Some(1));
    let skeleton = recon_skeleton(&temp, &config);
    let valid_id = skeleton.symbols[0].id.clone();
    let valid_file = skeleton.files[0].path.to_string_lossy().into_owned();

    let subsystem = json!({
        "subsystems": [{
            "id": "sub-1",
            "title": "Core",
            "summary": "Core subsystem.",
            "symbol_ids": [valid_id],
            "files": [valid_file]
        }],
        "chapters": [],
        "flows": [],
        "glossary": []
    })
    .to_string();
    let synthesis = model_response(&valid_id, &valid_file).to_string();

    let subsystem_response = subsystem.clone();
    let synthesis_response = synthesis.clone();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().expect("json body");
            let user = body["messages"]
                .as_array()
                .and_then(|messages| messages.last())
                .and_then(|message| message["content"].as_str())
                .unwrap_or_default();
            let content = if user.contains("hierarchical synthesis pass") {
                synthesis_response.clone()
            } else {
                subsystem_response.clone()
            };
            ResponseTemplate::new(200).set_body_json(completion(&content))
        })
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-model", None).expect("client");
    let report = comprehend(temp.path(), &config, &skeleton, &client)
        .await
        .expect("comprehend");

    assert!(report.hierarchical);
    assert!(!report.cached);
    assert_eq!(report.model.subsystems[0].symbol_ids, vec![valid_id]);

    let requests = server.received_requests().await.expect("requests");
    assert_eq!(
        requests.len(),
        skeleton.communities.len() + 1,
        "expected one call per community plus synthesis"
    );
}

#[tokio::test]
async fn cache_invalidates_on_head_change() {
    let temp = fixture();
    let server = MockServer::start().await;
    let config = config_for(&temp, &server, None);
    let mut skeleton = recon_skeleton(&temp, &config);
    let valid_id = skeleton.symbols[0].id.clone();
    let valid_file = skeleton.files[0].path.to_string_lossy().into_owned();

    mount_completion(&server, model_response(&valid_id, &valid_file).to_string()).await;

    let client = LlmClient::new(server.uri(), "test-model", None).expect("client");

    skeleton.head = Some("commit-1".to_owned());
    let first = comprehend(temp.path(), &config, &skeleton, &client)
        .await
        .expect("first comprehension");
    assert!(!first.cached);
    assert_eq!(first.model.head.as_deref(), Some("commit-1"));
    let after_first = server.received_requests().await.expect("requests").len();

    let second = comprehend(temp.path(), &config, &skeleton, &client)
        .await
        .expect("second comprehension");
    assert!(second.cached);
    assert_eq!(second.model.head.as_deref(), Some("commit-1"));
    let after_second = server.received_requests().await.expect("requests").len();
    assert_eq!(
        after_first, after_second,
        "cache hit should not call the LLM"
    );

    skeleton.head = Some("commit-2".to_owned());
    let third = comprehend(temp.path(), &config, &skeleton, &client)
        .await
        .expect("third comprehension");
    assert!(!third.cached);
    assert_eq!(third.model.head.as_deref(), Some("commit-2"));
    let after_third = server.received_requests().await.expect("requests").len();
    assert!(
        after_third > after_second,
        "HEAD change should invalidate the cache"
    );
}

#[tokio::test]
async fn redaction_is_applied_before_send() {
    const SECRET: &str = "AKIAIOSFODNN7EXAMPLE";

    let temp = fixture();
    fs::write(
        temp.path().join("README.md"),
        format!("Deployment notes\n\nAWS key: {SECRET}\n"),
    )
    .expect("write readme");

    let server = MockServer::start().await;
    let config = config_for(&temp, &server, None);
    let skeleton = recon_skeleton(&temp, &config);
    let valid_id = skeleton.symbols[0].id.clone();
    let valid_file = skeleton.files[0].path.to_string_lossy().into_owned();

    mount_completion(&server, model_response(&valid_id, &valid_file).to_string()).await;

    let client = LlmClient::new(server.uri(), "test-model", None).expect("client");
    comprehend(temp.path(), &config, &skeleton, &client)
        .await
        .expect("comprehend");

    let requests = server.received_requests().await.expect("requests");
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(!body.contains(SECRET), "secret reached the LLM: {body}");
    assert!(body.contains("«aws_key#1»"), "missing placeholder: {body}");
}

#[tokio::test]
async fn forced_hierarchical_uses_the_hierarchical_path() {
    let temp = fixture();
    let server = MockServer::start().await;
    let config = config_for(&temp, &server, None);
    let skeleton = recon_skeleton(&temp, &config);
    let valid_id = skeleton.symbols[0].id.clone();
    let valid_file = skeleton.files[0].path.to_string_lossy().into_owned();

    let subsystem = json!({
        "subsystems": [{
            "id": "sub-1",
            "title": "Core",
            "summary": "Core subsystem.",
            "symbol_ids": [valid_id],
            "files": [valid_file]
        }],
        "chapters": [],
        "flows": [],
        "glossary": []
    })
    .to_string();
    let synthesis = model_response(&valid_id, &valid_file).to_string();

    let subsystem_response = subsystem.clone();
    let synthesis_response = synthesis.clone();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().expect("json body");
            let user = body["messages"]
                .as_array()
                .and_then(|messages| messages.last())
                .and_then(|message| message["content"].as_str())
                .unwrap_or_default();
            let content = if user.contains("hierarchical synthesis pass") {
                synthesis_response.clone()
            } else {
                subsystem_response.clone()
            };
            ResponseTemplate::new(200).set_body_json(completion(&content))
        })
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-model", None).expect("client");
    let options = ComprehensionOptions {
        force: false,
        force_hierarchical: true,
    };
    let report = comprehend_with_options(temp.path(), &config, &skeleton, &client, &options)
        .await
        .expect("comprehend");

    assert!(report.hierarchical);
}

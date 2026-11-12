use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use laudit_core::config::Config;
use laudit_core::llm::{LlmClient, RetryPolicy};
use laudit_core::model::{
    CodebaseModel, Position, Skeleton, Stop, StopStatus, Subsystem, Tour, TourChapter, TourSource,
    SCHEMA_VERSION,
};
use laudit_core::narration::{
    narrate, narrate_from_artifacts, resume_from_artifacts, DroppedRef, NarrationSink,
};
use laudit_core::recon::{Community, FileNode, SymbolNode};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

const SYMBOL: &str = "sym-0123456789abcdef";
const SYMBOL_TWO: &str = "sym-fedcba9876543210";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("laudit-narration-it-{}-{id}", std::process::id()));
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

#[derive(Default)]
struct CollectingSink {
    chunks: Vec<(String, String)>,
    stops: Vec<Stop>,
}

impl NarrationSink for CollectingSink {
    fn on_chunk(&mut self, stop_id: &str, delta: &str) {
        self.chunks.push((stop_id.to_owned(), delta.to_owned()));
    }

    fn on_stop(&mut self, stop: &Stop) {
        self.stops.push(stop.clone());
    }
}

fn config_for(temp: &TempDir, server: &MockServer) -> Config {
    let path = temp.path().join(".laudit/config.toml");
    fs::create_dir_all(path.parent().expect("config parent")).expect("create .laudit");
    fs::write(
        &path,
        format!(
            "[provider]\nbase_url = \"{}\"\nmodel = \"test-model\"\n",
            server.uri()
        ),
    )
    .expect("write config");
    Config::load_from(None, Some(&path)).expect("load config")
}

fn client(server: &MockServer) -> LlmClient {
    LlmClient::new(server.uri(), "test-model", None)
        .expect("client")
        .with_retry_policy(RetryPolicy {
            max_retries: 0,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
        })
}

fn skeleton(symbols: &[(&str, &str, u32)], file: &str) -> Skeleton {
    Skeleton {
        schema_version: SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        repo_id: "fnv1a128:test".to_owned(),
        head: None,
        dirty_hash: "fnv1a128:test".to_owned(),
        files: vec![FileNode {
            path: PathBuf::from(file),
            rank: 1.0,
            community: 0,
        }],
        symbols: symbols
            .iter()
            .map(|(id, name, line)| SymbolNode {
                id: (*id).to_owned(),
                file: PathBuf::from(file),
                name: (*name).to_owned(),
                line: *line,
                rank: 1.0,
                community: 0,
            })
            .collect(),
        communities: vec![Community {
            id: 0,
            files: vec![PathBuf::from(file)],
            symbols: symbols.iter().map(|(id, _, _)| (*id).to_owned()).collect(),
        }],
        entrypoints: Vec::new(),
        ranked_files: vec![file.to_owned()],
        ranked_symbols: symbols.iter().map(|(id, _, _)| (*id).to_owned()).collect(),
        provenance: BTreeMap::new(),
    }
}

fn model(symbols: &[&str], file: &str) -> CodebaseModel {
    CodebaseModel {
        schema_version: SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        repo_id: "fnv1a128:test".to_owned(),
        head: None,
        dirty_hash: "fnv1a128:test".to_owned(),
        generator: "laudit".to_owned(),
        model: "test-model".to_owned(),
        prompt_version: "1".to_owned(),
        generated_at: "2026-11-12T00:00:00Z".to_owned(),
        subsystems: vec![Subsystem {
            id: "core".to_owned(),
            title: "Core".to_owned(),
            summary: "Core subsystem.".to_owned(),
            symbol_ids: symbols.iter().map(|id| (*id).to_owned()).collect(),
            files: vec![file.to_owned()],
        }],
        chapters: Vec::new(),
        flows: Vec::new(),
        glossary: Vec::new(),
    }
}

type FileSymbols<'a> = (&'a str, &'a [(&'a str, &'a str, u32)]);

fn skeleton_files(files: &[FileSymbols<'_>]) -> Skeleton {
    let mut file_nodes = Vec::new();
    let mut symbols = Vec::new();
    let mut community_files = Vec::new();
    let mut community_symbols = Vec::new();
    let mut ranked_files = Vec::new();
    let mut ranked_symbols = Vec::new();
    for (file, syms) in files {
        file_nodes.push(FileNode {
            path: PathBuf::from(*file),
            rank: 1.0,
            community: 0,
        });
        community_files.push(PathBuf::from(*file));
        ranked_files.push((*file).to_owned());
        for (id, name, line) in *syms {
            symbols.push(SymbolNode {
                id: (*id).to_owned(),
                file: PathBuf::from(*file),
                name: (*name).to_owned(),
                line: *line,
                rank: 1.0,
                community: 0,
            });
            community_symbols.push((*id).to_owned());
            ranked_symbols.push((*id).to_owned());
        }
    }
    Skeleton {
        schema_version: SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        repo_id: "fnv1a128:test".to_owned(),
        head: None,
        dirty_hash: "fnv1a128:test".to_owned(),
        files: file_nodes,
        symbols,
        communities: vec![Community {
            id: 0,
            files: community_files,
            symbols: community_symbols,
        }],
        entrypoints: Vec::new(),
        ranked_files,
        ranked_symbols,
        provenance: BTreeMap::new(),
    }
}

fn model_files(files: &[(&str, &[&str])]) -> CodebaseModel {
    let mut symbol_ids = Vec::new();
    let mut file_paths = Vec::new();
    for (file, syms) in files {
        file_paths.push((*file).to_owned());
        symbol_ids.extend(syms.iter().map(|id| (*id).to_owned()));
    }
    CodebaseModel {
        schema_version: SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        repo_id: "fnv1a128:test".to_owned(),
        head: None,
        dirty_hash: "fnv1a128:test".to_owned(),
        generator: "laudit".to_owned(),
        model: "test-model".to_owned(),
        prompt_version: "1".to_owned(),
        generated_at: "2026-11-12T00:00:00Z".to_owned(),
        subsystems: vec![Subsystem {
            id: "core".to_owned(),
            title: "Core".to_owned(),
            summary: "Core subsystem.".to_owned(),
            symbol_ids,
            files: file_paths,
        }],
        chapters: Vec::new(),
        flows: Vec::new(),
        glossary: Vec::new(),
    }
}

fn stop(id: &str, symbol_id: &str, file: &str, start: u32, end: u32) -> Stop {
    Stop {
        id: id.to_owned(),
        file: file.to_owned(),
        start_line: start,
        end_line: end,
        symbol_id: symbol_id.to_owned(),
        title: "main".to_owned(),
        why_here: "Entrypoint.".to_owned(),
        explanation: String::new(),
        refs: Vec::new(),
        file_hash: String::new(),
        status: StopStatus::Pending,
    }
}

fn tour(stops: Vec<Stop>) -> Tour {
    Tour {
        schema_version: SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        tour_id: "tour-test".to_owned(),
        source: TourSource::codebase(),
        model: "test-model".to_owned(),
        prompt_version: "1".to_owned(),
        redaction_profile: "default".to_owned(),
        position: Position::default(),
        chapters: vec![TourChapter {
            id: "on-ramp".to_owned(),
            title: "On-ramp".to_owned(),
            stops,
        }],
    }
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

async fn mount_stream(server: &MockServer, body: String) {
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

fn persisted_tour(temp: &TempDir) -> Tour {
    let path = temp.path().join(".laudit/tours/tour-test.json");
    serde_json::from_slice(&fs::read(path).expect("read tour")).expect("parse tour")
}

#[tokio::test]
async fn mock_llm_streams_chunks_and_fills_tour_json() {
    let temp = TempDir::new();
    temp.write("src/main.rs", "fn main() {\n    helper();\n}\n");
    let server = MockServer::start().await;
    let config = config_for(&temp, &server);
    let skeleton = skeleton(&[(SYMBOL, "main", 1)], "src/main.rs");
    let model = model(&[SYMBOL], "src/main.rs");
    let mut tour = tour(vec![stop("stop-1", SYMBOL, "src/main.rs", 1, 3)]);

    let body = format!("The `main` function calls `{SYMBOL}`.");
    let chunks = [
        "TITLE: Program entry\nWHY: It starts the program.\n---\nThe ",
        "`main` function calls `",
        SYMBOL,
        "`.",
    ];
    mount_stream(&server, sse_body(&chunks)).await;

    let client = client(&server);
    let mut sink = CollectingSink::default();
    let report = narrate(
        temp.path(),
        &config,
        &skeleton,
        &model,
        &mut tour,
        &client,
        &mut sink,
    )
    .await
    .expect("narrate");

    assert_eq!(report.narrated, 1);
    assert!(report.skipped.is_empty());
    assert!(report.dropped_refs.is_empty());
    assert!(sink.chunks.len() > 1, "expected streamed chunks");
    let streamed: String = sink
        .chunks
        .iter()
        .map(|(_, delta)| delta.as_str())
        .collect();
    assert_eq!(streamed, body);
    assert_eq!(sink.stops.len(), 1);
    assert_eq!(sink.stops[0].status, StopStatus::Explained);

    let persisted = persisted_tour(&temp);
    let stop = &persisted.chapters[0].stops[0];
    assert_eq!(stop.title, "Program entry");
    assert_eq!(stop.why_here, "It starts the program.");
    assert_eq!(stop.explanation, body);
    assert_eq!(stop.refs, vec![SYMBOL.to_owned()]);
    assert_eq!(stop.status, StopStatus::Explained);
    assert_eq!(persisted, report.tour);
}

#[tokio::test]
async fn unknown_refs_are_dropped_with_a_warning() {
    let temp = TempDir::new();
    temp.write("src/main.rs", "fn main() {}\n");
    let server = MockServer::start().await;
    let config = config_for(&temp, &server);
    let skeleton = skeleton(&[(SYMBOL, "main", 1)], "src/main.rs");
    let model = model(&[SYMBOL], "src/main.rs");
    let mut tour = tour(vec![stop("stop-1", SYMBOL, "src/main.rs", 1, 1)]);

    let body = format!("See `{SYMBOL}` and `sym-unknown`.");
    let chunks = [
        "TITLE: Entry\nWHY: Why.\n---\nSee `",
        SYMBOL,
        "` and `sym-unknown`.",
    ];
    mount_stream(&server, sse_body(&chunks)).await;

    let client = client(&server);
    let mut sink = CollectingSink::default();
    let report = narrate(
        temp.path(),
        &config,
        &skeleton,
        &model,
        &mut tour,
        &client,
        &mut sink,
    )
    .await
    .expect("narrate");

    assert_eq!(
        report.dropped_refs,
        vec![DroppedRef {
            stop_id: "stop-1".to_owned(),
            symbol_id: "sym-unknown".to_owned(),
        }]
    );
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.contains("sym-unknown")));

    let persisted = persisted_tour(&temp);
    let stop = &persisted.chapters[0].stops[0];
    assert_eq!(stop.refs, vec![SYMBOL.to_owned()]);
    assert_eq!(stop.explanation, body);
}

#[tokio::test]
async fn failing_stop_is_skipped_and_marked() {
    let temp = TempDir::new();
    temp.write(
        "src/main.rs",
        "fn main() {\n    helper();\n}\n\nfn helper() {}\n",
    );
    let server = MockServer::start().await;
    let config = config_for(&temp, &server);
    let skeleton = skeleton(
        &[(SYMBOL, "main", 1), (SYMBOL_TWO, "helper", 5)],
        "src/main.rs",
    );
    let model = model(&[SYMBOL, SYMBOL_TWO], "src/main.rs");
    let mut tour = tour(vec![
        stop("stop-1", SYMBOL, "src/main.rs", 1, 3),
        stop("stop-2", SYMBOL_TWO, "src/main.rs", 5, 5),
    ]);

    let success = sse_body(&["TITLE: Helper\nWHY: It helps.\n---\nHelper body."]);
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().expect("json body");
            let user = body["messages"]
                .as_array()
                .and_then(|messages| messages.last())
                .and_then(|message| message["content"].as_str())
                .unwrap_or_default();
            if user.contains(&format!("\"symbol_id\": \"{SYMBOL}\"")) {
                ResponseTemplate::new(500).set_body_string("boom")
            } else {
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(success.clone())
            }
        })
        .mount(&server)
        .await;

    let client = client(&server);
    let mut sink = CollectingSink::default();
    let report = narrate(
        temp.path(),
        &config,
        &skeleton,
        &model,
        &mut tour,
        &client,
        &mut sink,
    )
    .await
    .expect("narrate");

    assert_eq!(report.narrated, 1);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].stop_id, "stop-1");
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.contains("skipped stop stop-1")));

    let persisted = persisted_tour(&temp);
    assert_eq!(persisted.chapters[0].stops[0].status, StopStatus::Skipped);
    assert_eq!(persisted.chapters[0].stops[1].status, StopStatus::Explained);
    assert_eq!(sink.stops.len(), 2);
}

#[tokio::test]
async fn redaction_is_applied_to_the_prompt() {
    const SECRET: &str = "AKIAIOSFODNN7EXAMPLE";

    let temp = TempDir::new();
    temp.write(
        "src/main.rs",
        &format!("const AWS_KEY: &str = \"{SECRET}\";\nfn main() {{}}\n"),
    );
    let server = MockServer::start().await;
    let config = config_for(&temp, &server);
    let skeleton = skeleton(&[(SYMBOL, "main", 2)], "src/main.rs");
    let model = model(&[SYMBOL], "src/main.rs");
    let mut tour = tour(vec![stop("stop-1", SYMBOL, "src/main.rs", 1, 2)]);

    let chunks = ["TITLE: Entry\nWHY: Why.\n---\nThe key is `«aws_key#1»`."];
    mount_stream(&server, sse_body(&chunks)).await;

    let client = client(&server);
    let mut sink = CollectingSink::default();
    narrate(
        temp.path(),
        &config,
        &skeleton,
        &model,
        &mut tour,
        &client,
        &mut sink,
    )
    .await
    .expect("narrate");

    let requests = server.received_requests().await.expect("requests");
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(!body.contains(SECRET), "secret reached the LLM: {body}");
    assert!(body.contains("«aws_key#1»"), "missing placeholder: {body}");

    let persisted = persisted_tour(&temp);
    assert!(persisted.chapters[0].stops[0]
        .explanation
        .contains("«aws_key#1»"));

    let redactions =
        fs::read_to_string(temp.path().join(".laudit/redactions.json")).expect("read redactions");
    assert!(redactions.contains("«aws_key#1»"));
    assert!(!redactions.contains(SECRET));
}

#[tokio::test]
async fn malformed_front_matter_is_retried_once() {
    let temp = TempDir::new();
    temp.write("src/main.rs", "fn main() {}\n");
    let server = MockServer::start().await;
    let config = config_for(&temp, &server);
    let skeleton = skeleton(&[(SYMBOL, "main", 1)], "src/main.rs");
    let model = model(&[SYMBOL], "src/main.rs");
    let mut tour = tour(vec![stop("stop-1", SYMBOL, "src/main.rs", 1, 1)]);

    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);
    let valid = sse_body(&["TITLE: Entry\nWHY: Why.\n---\nBody."]);
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |_request: &wiremock::Request| {
            if responder_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body(&["TITLE: Entry\nWHY: Why.\n"]))
            } else {
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(valid.clone())
            }
        })
        .mount(&server)
        .await;

    let client = client(&server);
    let mut sink = CollectingSink::default();
    let report = narrate(
        temp.path(),
        &config,
        &skeleton,
        &model,
        &mut tour,
        &client,
        &mut sink,
    )
    .await
    .expect("narrate");

    assert_eq!(report.narrated, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        persisted_tour(&temp).chapters[0].stops[0].status,
        StopStatus::Explained
    );
}

#[tokio::test]
async fn narrate_from_artifacts_loads_and_persists() {
    let temp = TempDir::new();
    temp.write("src/main.rs", "fn main() {}\n");
    let server = MockServer::start().await;
    let config = config_for(&temp, &server);
    let skeleton = skeleton(&[(SYMBOL, "main", 1)], "src/main.rs");
    let model = model(&[SYMBOL], "src/main.rs");
    let tour = tour(vec![stop("stop-1", SYMBOL, "src/main.rs", 1, 1)]);

    temp.write(
        ".laudit/skeleton.json",
        &serde_json::to_string_pretty(&skeleton).expect("serialize skeleton"),
    );
    temp.write(
        ".laudit/model.json",
        &serde_json::to_string_pretty(&model).expect("serialize model"),
    );
    temp.write(
        ".laudit/tours/tour-test.json",
        &serde_json::to_string_pretty(&tour).expect("serialize tour"),
    );

    mount_stream(&server, sse_body(&["TITLE: Entry\nWHY: Why.\n---\nBody."])).await;

    let client = client(&server);
    let mut sink = CollectingSink::default();
    let report = narrate_from_artifacts(temp.path(), &config, "tour-test", &client, &mut sink)
        .await
        .expect("narrate from artifacts");

    assert_eq!(report.narrated, 1);
    assert_eq!(
        persisted_tour(&temp).chapters[0].stops[0].status,
        StopStatus::Explained
    );
}

#[tokio::test]
async fn resume_skips_explained_stops_and_restores_position() {
    let temp = TempDir::new();
    temp.write("src/main.rs", "fn main() {}\nfn helper() {}\n");
    let server = MockServer::start().await;
    let config = config_for(&temp, &server);
    let skeleton = skeleton(
        &[(SYMBOL, "main", 1), (SYMBOL_TWO, "helper", 2)],
        "src/main.rs",
    );
    let model = model(&[SYMBOL, SYMBOL_TWO], "src/main.rs");
    let tour = tour(vec![
        stop("stop-1", SYMBOL, "src/main.rs", 1, 1),
        stop("stop-2", SYMBOL_TWO, "src/main.rs", 2, 2),
    ]);

    temp.write(
        ".laudit/skeleton.json",
        &serde_json::to_string_pretty(&skeleton).expect("serialize skeleton"),
    );
    temp.write(
        ".laudit/model.json",
        &serde_json::to_string_pretty(&model).expect("serialize model"),
    );
    temp.write(
        ".laudit/tours/tour-test.json",
        &serde_json::to_string_pretty(&tour).expect("serialize tour"),
    );

    mount_stream(&server, sse_body(&["TITLE: Entry\nWHY: Why.\n---\nBody."])).await;

    let client = client(&server);
    let mut sink = CollectingSink::default();
    let first = narrate_from_artifacts(temp.path(), &config, "tour-test", &client, &mut sink)
        .await
        .expect("first narration");
    assert_eq!(first.narrated, 2);
    assert_eq!(first.cached, 0);
    let requests_after_first = server.received_requests().await.expect("requests").len();
    assert_eq!(requests_after_first, 2);

    laudit_core::cache::set_position(
        temp.path(),
        "tour-test",
        Position {
            chapter: 0,
            stop: 1,
        },
    )
    .expect("set position");

    let mut sink = CollectingSink::default();
    let resumed = resume_from_artifacts(temp.path(), &config, "tour-test", &client, &mut sink)
        .await
        .expect("resume");

    assert_eq!(resumed.narrated, 0);
    assert_eq!(resumed.cached, 2);
    assert!(resumed.invalidated.is_empty());
    assert!(resumed.skipped.is_empty());
    let requests_after_resume = server.received_requests().await.expect("requests").len();
    assert_eq!(
        requests_after_resume, requests_after_first,
        "resume should not call the LLM for explained stops"
    );

    let persisted = persisted_tour(&temp);
    assert_eq!(
        persisted.position,
        Position {
            chapter: 0,
            stop: 1
        }
    );
    assert_eq!(persisted.chapters[0].stops[0].status, StopStatus::Explained);
    assert_eq!(persisted.chapters[0].stops[1].status, StopStatus::Explained);
    assert_eq!(persisted.chapters[0].stops[0].explanation, "Body.");
    assert_eq!(persisted.chapters[0].stops[1].explanation, "Body.");
}

#[tokio::test]
async fn editing_one_stop_file_invalidates_only_that_stop() {
    let temp = TempDir::new();
    temp.write("src/main.rs", "fn main() {}\n");
    temp.write("src/lib.rs", "fn helper() {}\n");
    let server = MockServer::start().await;
    let config = config_for(&temp, &server);
    let skeleton = skeleton_files(&[
        ("src/main.rs", &[(SYMBOL, "main", 1)]),
        ("src/lib.rs", &[(SYMBOL_TWO, "helper", 1)]),
    ]);
    let model = model_files(&[("src/main.rs", &[SYMBOL]), ("src/lib.rs", &[SYMBOL_TWO])]);
    let tour = tour(vec![
        stop("stop-1", SYMBOL, "src/main.rs", 1, 1),
        stop("stop-2", SYMBOL_TWO, "src/lib.rs", 1, 1),
    ]);

    temp.write(
        ".laudit/skeleton.json",
        &serde_json::to_string_pretty(&skeleton).expect("serialize skeleton"),
    );
    temp.write(
        ".laudit/model.json",
        &serde_json::to_string_pretty(&model).expect("serialize model"),
    );
    temp.write(
        ".laudit/tours/tour-test.json",
        &serde_json::to_string_pretty(&tour).expect("serialize tour"),
    );

    mount_stream(&server, sse_body(&["TITLE: Entry\nWHY: Why.\n---\nBody."])).await;

    let client = client(&server);
    let mut sink = CollectingSink::default();
    let first = narrate_from_artifacts(temp.path(), &config, "tour-test", &client, &mut sink)
        .await
        .expect("first narration");
    assert_eq!(first.narrated, 2);
    let requests_after_first = server.received_requests().await.expect("requests").len();
    assert_eq!(requests_after_first, 2);

    let before = persisted_tour(&temp);
    let first_hash = before.chapters[0].stops[0].file_hash.clone();
    let second_hash = before.chapters[0].stops[1].file_hash.clone();
    assert!(!first_hash.is_empty());
    assert!(!second_hash.is_empty());

    temp.write("src/lib.rs", "fn helper() {}\nfn extra() {}\n");

    let mut sink = CollectingSink::default();
    let resumed = resume_from_artifacts(temp.path(), &config, "tour-test", &client, &mut sink)
        .await
        .expect("resume");

    assert_eq!(resumed.invalidated, vec!["stop-2"]);
    assert_eq!(resumed.narrated, 1);
    assert_eq!(resumed.cached, 1);
    let requests_after_resume = server.received_requests().await.expect("requests").len();
    assert_eq!(
        requests_after_resume,
        requests_after_first + 1,
        "only the changed stop should be re-narrated"
    );

    let after = persisted_tour(&temp);
    assert_eq!(after.chapters[0].stops[0].status, StopStatus::Explained);
    assert_eq!(after.chapters[0].stops[1].status, StopStatus::Explained);
    assert_eq!(after.chapters[0].stops[0].file_hash, first_hash);
    assert_ne!(after.chapters[0].stops[1].file_hash, second_hash);
}

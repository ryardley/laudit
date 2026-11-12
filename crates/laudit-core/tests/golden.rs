//! Golden-artifact and fixture integration tests.
//!
//! These tests run the full pipeline against the committed fixture repos in
//! `tests/fixtures/` with a mock OpenAI-compatible server. They never touch the
//! network (D21). Snapshots are normalized for the few fields that legitimately
//! vary between runs (repository path, timestamps, content hashes) so the
//! golden artifacts stay stable.

mod support;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use laudit_core::build::build_offline;
use laudit_core::comprehension::comprehend;
use laudit_core::config::Config;
use laudit_core::llm::LlmClient;
use laudit_core::model::{CodebaseModel, Skeleton, Tour};
use laudit_core::narration::{narrate, NoopSink};
use laudit_core::plan::{load_skeleton, plan, tour_path, PlanOptions};
use laudit_core::recon::mine_file;
use serde_json::{json, Value};
use wiremock::MockServer;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

const AWS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";
const GITHUB_TOKEN: &str = "ghp_1234567890abcdefghijklmnopqrstuvwxyz";
const JWT: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
const PEM_MARKER: &str = "BEGIN RSA PRIVATE KEY";
const ENV_SECRET: &str = "hunter2";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("laudit-golden-it-{}-{id}", std::process::id()));
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
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

fn copy_fixture(name: &str) -> TempDir {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root().join(name), temp.path()).expect("copy fixture");
    temp
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

fn model_response(skeleton: &Skeleton) -> Value {
    let symbol_ids: Vec<String> = skeleton
        .symbols
        .iter()
        .map(|symbol| symbol.id.clone())
        .collect();
    let files: Vec<String> = skeleton
        .files
        .iter()
        .map(|file| file.path.to_string_lossy().into_owned())
        .collect();
    let first = symbol_ids.first().cloned().unwrap_or_default();

    json!({
        "subsystems": [{
            "id": "sub-core",
            "title": "Core",
            "summary": "The core of the fixture application.",
            "symbol_ids": symbol_ids.clone(),
            "files": files,
        }],
        "chapters": [{
            "id": "ch-on-ramp",
            "title": "On-ramp",
            "intent": "Orient the reader in the fixture.",
            "stop_ids": [first],
        }],
        "flows": [{
            "id": "flow-main",
            "title": "Main flow",
            "steps": symbol_ids,
        }],
        "glossary": [{
            "term": "fixture",
            "definition": "A small repository used for deterministic tests."
        }]
    })
}

fn normalize_model(value: &Value) -> Value {
    let mut value = value.clone();
    value["repo_id"] = json!("<repo_id>");
    value["dirty_hash"] = json!("<dirty_hash>");
    value["generated_at"] = json!("<generated_at>");
    value["prompt_version"] = json!("<prompt_version>");
    value
}

fn normalize_tour(value: &Value) -> Value {
    let mut value = value.clone();
    value["tour_id"] = json!("<tour_id>");
    value["prompt_version"] = json!("<prompt_version>");
    if let Some(chapters) = value["chapters"].as_array_mut() {
        for chapter in chapters {
            if let Some(stops) = chapter["stops"].as_array_mut() {
                for stop in stops {
                    stop["file_hash"] = json!("<file_hash>");
                }
            }
        }
    }
    value
}

struct PipelineOutput {
    model: Value,
    tour: Value,
}

async fn run_pipeline(fixture: &str) -> PipelineOutput {
    let temp = copy_fixture(fixture);
    let server = MockServer::start().await;
    let config = config_for(&temp, &server);
    let build = build_offline(temp.path(), &config).expect("offline build");
    assert!(build.files > 0, "fixture should contain source files");
    let skeleton = load_skeleton(temp.path()).expect("load skeleton");

    support::mount_pipeline(&server, model_response(&skeleton).to_string()).await;
    let client = LlmClient::new(server.uri(), "test-model", None).expect("client");

    let report = comprehend(temp.path(), &config, &skeleton, &client)
        .await
        .expect("comprehend");
    let persisted_model: CodebaseModel = serde_json::from_slice(
        &fs::read(temp.path().join(".laudit/model.json")).expect("read model"),
    )
    .expect("parse model");
    assert_eq!(persisted_model, report.model);
    let model = normalize_model(&serde_json::to_value(&report.model).expect("model json"));

    let mut plan_report = plan(
        temp.path(),
        &config,
        &skeleton,
        &report.model,
        &PlanOptions::default(),
    )
    .expect("plan");
    let mut sink = NoopSink;
    let narration = narrate(
        temp.path(),
        &config,
        &skeleton,
        &report.model,
        &mut plan_report.tour,
        &client,
        &mut sink,
    )
    .await
    .expect("narrate");
    assert!(
        narration.skipped.is_empty(),
        "unexpected skipped stops: {:?}",
        narration.skipped
    );
    let persisted_tour: Tour = serde_json::from_slice(
        &fs::read(tour_path(temp.path(), &narration.tour.tour_id)).expect("read tour"),
    )
    .expect("parse tour");
    assert_eq!(persisted_tour, narration.tour);
    let tour = normalize_tour(&serde_json::to_value(&narration.tour).expect("tour json"));

    PipelineOutput { model, tour }
}

#[tokio::test]
async fn rust_fixture_model_and_tour_match_golden() {
    let output = run_pipeline("rust-app").await;
    insta::assert_json_snapshot!("rust_app_model", output.model);
    insta::assert_json_snapshot!("rust_app_tour", output.tour);
}

#[tokio::test]
async fn ts_fixture_model_and_tour_match_golden() {
    let output = run_pipeline("ts-app").await;
    insta::assert_json_snapshot!("ts_app_model", output.model);
    insta::assert_json_snapshot!("ts_app_tour", output.tour);
}

#[tokio::test]
async fn mock_llm_never_receives_secrets_from_the_secrets_fixture() {
    let temp = copy_fixture("rust-app");
    let secrets = fixture_root().join("secrets");
    fs::copy(secrets.join("README.md"), temp.path().join("README.md")).expect("copy readme");
    fs::create_dir_all(temp.path().join("docs")).expect("create docs");
    fs::copy(
        secrets.join("docs/security.md"),
        temp.path().join("docs/security.md"),
    )
    .expect("copy security notes");
    fs::copy(secrets.join("notes.md"), temp.path().join("docs/notes.md")).expect("copy notes");

    let server = MockServer::start().await;
    let config = config_for(&temp, &server);
    let build = build_offline(temp.path(), &config).expect("offline build");
    assert!(build.files > 0, "fixture should contain source files");
    let skeleton = load_skeleton(temp.path()).expect("load skeleton");
    support::mount_completion(&server, model_response(&skeleton).to_string()).await;
    let client = LlmClient::new(server.uri(), "test-model", None).expect("client");
    comprehend(temp.path(), &config, &skeleton, &client)
        .await
        .expect("comprehend");

    let requests = server.received_requests().await.expect("requests");
    assert!(!requests.is_empty(), "expected at least one LLM request");
    for request in &requests {
        let body = String::from_utf8_lossy(&request.body);
        for secret in [AWS_KEY, GITHUB_TOKEN, JWT, ENV_SECRET] {
            assert!(
                !body.contains(secret),
                "secret reached the LLM: {secret}\nbody: {body}"
            );
        }
        assert!(!body.contains(PEM_MARKER), "PEM key reached the LLM");
        assert!(
            body.contains('«'),
            "expected redaction placeholders in the request body"
        );
    }

    let redactions =
        fs::read_to_string(temp.path().join(".laudit/redactions.json")).expect("read redactions");
    for secret in [AWS_KEY, GITHUB_TOKEN, JWT, ENV_SECRET] {
        assert!(
            !redactions.contains(secret),
            "secret persisted in redactions.json: {secret}"
        );
    }
    assert!(!redactions.contains(PEM_MARKER));
}

fn run_git(repo: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Alice")
        .env("GIT_AUTHOR_EMAIL", "alice@example.com")
        .env("GIT_COMMITTER_NAME", "Alice")
        .env("GIT_COMMITTER_EMAIL", "alice@example.com")
        .output()
        .expect("run git")
}

fn git_ok(repo: &Path, args: &[&str]) {
    let output = run_git(repo, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit(repo: &Path, message: &str, date: &str) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", message])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Alice")
        .env("GIT_AUTHOR_EMAIL", "alice@example.com")
        .env("GIT_COMMITTER_NAME", "Alice")
        .env("GIT_COMMITTER_EMAIL", "alice@example.com")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .expect("run git commit");
    assert!(
        output.status.success(),
        "git commit {message:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn git_fixture_provides_provenance() {
    let temp = copy_fixture("git-repo");
    git_ok(temp.path(), &["init", "-q"]);
    git_ok(temp.path(), &["config", "user.name", "Alice"]);
    git_ok(temp.path(), &["config", "user.email", "alice@example.com"]);
    git_ok(temp.path(), &["add", "."]);
    commit(temp.path(), "initial import", "2020-01-01T00:00:00Z");

    let lib = temp.path().join("src/lib.rs");
    let mut source = fs::read_to_string(&lib).expect("read lib");
    source.push_str("\npub fn multiply(left: i32, right: i32) -> i32 {\n    left * right\n}\n");
    fs::write(&lib, source).expect("write lib");
    git_ok(temp.path(), &["add", "src/lib.rs"]);
    commit(temp.path(), "add multiply helper", "2021-01-01T00:00:00Z");

    let provenance = mine_file(temp.path(), "src/lib.rs");
    assert!(provenance.blame.is_some());
    assert_eq!(provenance.churn.commits, 2);
    let subjects: Vec<&str> = provenance
        .recent_commits
        .iter()
        .map(|commit| commit.subject.as_str())
        .collect();
    assert_eq!(subjects, vec!["add multiply helper", "initial import"]);
}

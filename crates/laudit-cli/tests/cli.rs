use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn laudit(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_laudit"))
        .args(args)
        .output()
        .expect("run laudit")
}

fn laudit_with_config_dir(config_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_laudit"))
        .env("XDG_CONFIG_HOME", config_dir)
        .args(args)
        .output()
        .expect("run laudit")
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("laudit-cli-test-{}-{id}", std::process::id()));
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../laudit-core/tests/fixtures/graph-tree")
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

#[test]
fn help_lists_every_subcommand() {
    let output = laudit(&["--help"]);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in [
        "build", "serve", "tour", "explain", "export", "ask", "mcp", "outline", "status",
        "prompts", "clear",
    ] {
        assert!(stdout.contains(command), "missing {command} in:\n{stdout}");
    }
}

#[test]
fn version_prints_the_crate_version() {
    let output = laudit(&["--version"]);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "got: {stdout}");
}

#[test]
fn unknown_flags_are_rejected() {
    let output = laudit(&["build", "--bogus"]);
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument"), "got: {stderr}");
}

#[test]
fn stubs_parse_documented_flags_and_exit_non_zero() {
    let cases: &[&[&str]] = &[
        &[
            "build",
            "--offline",
            "--check",
            "--quiet",
            "--repo",
            ".",
            "--json",
        ],
        &["ask", "what does this do?"],
        &["mcp"],
        &["outline"],
        &["clear"],
    ];

    for args in cases {
        let output = laudit(args);
        assert!(
            !output.status.success(),
            "expected non-zero exit for {args:?}"
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("not implemented yet"),
            "missing stub message for {args:?}: {stderr}"
        );
    }
}

#[test]
fn export_requires_a_format() {
    let output = laudit(&["export"]);
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--md"), "got: {stderr}");
    assert!(stderr.contains("--html"), "got: {stderr}");
}

#[test]
fn export_md_renders_the_tour_to_stdout_and_a_file() {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let config_dir = temp.path().join("config");

    let build = laudit_with_config_dir(&config_dir, &["build", "--offline", "--repo", &repo]);
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let tour = laudit_with_config_dir(
        &config_dir,
        &["tour", "--print", "--offline", "--repo", &repo],
    );
    assert!(
        tour.status.success(),
        "tour failed: {}",
        String::from_utf8_lossy(&tour.stderr)
    );

    let output = laudit_with_config_dir(&config_dir, &["export", "--md", "--repo", &repo]);
    assert!(
        output.status.success(),
        "export failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("# "), "got: {stdout}");
    assert!(stdout.contains("## "), "got: {stdout}");
    assert!(stdout.contains("### "), "got: {stdout}");
    assert!(stdout.contains("```"), "got: {stdout}");
    assert!(stdout.contains("**Why here:**"), "got: {stdout}");

    let out_path = temp.path().join("tour.md");
    let output = laudit_with_config_dir(
        &config_dir,
        &[
            "export",
            "--md",
            "--out",
            out_path.to_str().expect("utf-8 output path"),
            "--repo",
            &repo,
        ],
    );
    assert!(
        output.status.success(),
        "export --out failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_path.is_file(), "missing {}", out_path.display());
    let written = fs::read_to_string(&out_path).expect("read markdown");
    assert_eq!(written, stdout);
}

#[test]
fn export_md_without_a_tour_fails_clearly() {
    let temp = TempDir::new();
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let config_dir = temp.path().join("config");

    let output = laudit_with_config_dir(&config_dir, &["export", "--md", "--repo", &repo]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no tour artifact"), "got: {stderr}");
}

#[test]
fn export_html_renders_the_tour_to_stdout_and_a_file_or_directory() {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let config_dir = temp.path().join("config");

    let build = laudit_with_config_dir(&config_dir, &["build", "--offline", "--repo", &repo]);
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let tour = laudit_with_config_dir(
        &config_dir,
        &["tour", "--print", "--offline", "--repo", &repo],
    );
    assert!(
        tour.status.success(),
        "tour failed: {}",
        String::from_utf8_lossy(&tour.stderr)
    );

    let output = laudit_with_config_dir(&config_dir, &["export", "--html", "--repo", &repo]);
    assert!(
        output.status.success(),
        "export failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("<!DOCTYPE html>"), "got: {stdout}");
    assert!(stdout.contains("class=\"sidebar\""), "got: {stdout}");
    assert!(stdout.contains("hl-keyword"), "got: {stdout}");

    let out_path = temp.path().join("tour.html");
    let output = laudit_with_config_dir(
        &config_dir,
        &[
            "export",
            "--html",
            "--out",
            out_path.to_str().expect("utf-8 output path"),
            "--repo",
            &repo,
        ],
    );
    assert!(
        output.status.success(),
        "export --out failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_path.is_file(), "missing {}", out_path.display());
    let written = fs::read_to_string(&out_path).expect("read html");
    assert_eq!(written, stdout);

    let out_dir = temp.path().join("site");
    let output = laudit_with_config_dir(
        &config_dir,
        &[
            "export",
            "--html",
            "--out",
            out_dir.to_str().expect("utf-8 output dir"),
            "--repo",
            &repo,
        ],
    );
    assert!(
        output.status.success(),
        "export --out dir failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_dir.join("index.html").is_file());
}

#[test]
fn export_html_without_a_tour_fails_clearly() {
    let temp = TempDir::new();
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let config_dir = temp.path().join("config");

    let output = laudit_with_config_dir(&config_dir, &["export", "--html", "--repo", &repo]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no tour artifact"), "got: {stderr}");
}

#[test]
fn offline_build_writes_index_and_skeleton_artifacts() {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let config_dir = temp.path().join("config");

    let output = laudit_with_config_dir(&config_dir, &["build", "--offline", "--repo", &repo]);
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let index = temp.path().join(".laudit/index/index.json");
    assert!(index.is_file(), "missing index at {}", index.display());

    let skeleton_path = temp.path().join(".laudit/skeleton.json");
    let skeleton: serde_json::Value =
        serde_json::from_slice(&fs::read(&skeleton_path).expect("read skeleton"))
            .expect("parse skeleton");
    assert_eq!(skeleton["schema_version"], 1);
    assert_eq!(skeleton["tool_version"], env!("CARGO_PKG_VERSION"));
    assert!(skeleton["files"]
        .as_array()
        .is_some_and(|files| !files.is_empty()));
    assert!(skeleton["symbols"]
        .as_array()
        .is_some_and(|symbols| !symbols.is_empty()));
    assert!(skeleton["communities"].is_array());
    assert!(skeleton["entrypoints"].is_array());
    assert!(skeleton["ranked_files"].is_array());
    assert!(skeleton["ranked_symbols"].is_array());

    let model_path = temp.path().join(".laudit/model.json");
    let model: serde_json::Value =
        serde_json::from_slice(&fs::read(&model_path).expect("read model")).expect("parse model");
    assert_eq!(model["schema_version"], 1);
    assert_eq!(model["subsystems"], serde_json::json!([]));
    assert_eq!(model["chapters"], serde_json::json!([]));
    assert_eq!(model["flows"], serde_json::json!([]));
    assert_eq!(model["glossary"], serde_json::json!([]));

    let first_skeleton = fs::read(&skeleton_path).expect("read first skeleton");
    let second = laudit_with_config_dir(&config_dir, &["build", "--offline", "--repo", &repo]);
    assert!(
        second.status.success(),
        "second build failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_skeleton = fs::read(&skeleton_path).expect("read second skeleton");
    assert_eq!(
        first_skeleton, second_skeleton,
        "skeleton is not deterministic"
    );
}

#[tokio::test]
async fn non_offline_build_writes_model_with_mock_llm() {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let config_dir = temp.path().join("config");

    let server = MockServer::start().await;
    let content = json!({
        "subsystems": [],
        "chapters": [],
        "flows": [],
        "glossary": [{ "term": "Term", "definition": "Definition." }]
    })
    .to_string();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 1,
                "completion_tokens": 1,
                "total_tokens": 2
            }
        })))
        .mount(&server)
        .await;

    let config_path = temp.path().join(".laudit/config.toml");
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("create .laudit");
    fs::write(
        &config_path,
        format!(
            "[provider]\nbase_url = \"{}\"\nmodel = \"test-model\"\n",
            server.uri()
        ),
    )
    .expect("write config");

    let output = laudit_with_config_dir(&config_dir, &["build", "--repo", &repo]);
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let model_path = temp.path().join(".laudit/model.json");
    let model: serde_json::Value =
        serde_json::from_slice(&fs::read(&model_path).expect("read model")).expect("parse model");
    assert_eq!(model["schema_version"], 1);
    assert_eq!(model["model"], "test-model");
    assert_eq!(model["glossary"][0]["term"], "Term");
    assert!(model["subsystems"].is_array());

    let tours_dir = temp.path().join(".laudit/tours");
    let tour_path = fs::read_dir(&tours_dir)
        .expect("tours dir")
        .next()
        .expect("a tour artifact")
        .expect("tour entry")
        .path();
    let tour: serde_json::Value =
        serde_json::from_slice(&fs::read(&tour_path).expect("read tour")).expect("parse tour");
    assert_eq!(tour["schema_version"], 1);
    assert_eq!(tour["source"]["kind"], "codebase");
    assert!(tour["chapters"]
        .as_array()
        .is_some_and(|chapters| !chapters.is_empty()));
    for chapter in tour["chapters"].as_array().expect("chapters") {
        for stop in chapter["stops"].as_array().expect("stops") {
            assert!(stop["start_line"].as_u64().is_some_and(|line| line >= 1));
            assert!(stop["end_line"].as_u64().is_some_and(|line| line >= 1));
        }
    }
}

#[test]
fn status_reports_tour_counts() {
    let temp = TempDir::new();
    fs::create_dir_all(temp.path().join(".laudit/tours")).expect("create tours dir");
    fs::create_dir_all(temp.path().join("src")).expect("create src dir");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write source");
    let hash = laudit_core::recon::content_hash(b"fn main() {}\n");
    let tour = json!({
        "schema_version": 1,
        "tool_version": env!("CARGO_PKG_VERSION"),
        "tour_id": "tour-test",
        "source": { "kind": "codebase" },
        "model": "test-model",
        "prompt_version": "1",
        "redaction_profile": "default",
        "position": { "chapter": 0, "stop": 1 },
        "chapters": [{
            "id": "ch-1",
            "title": "On-ramp",
            "stops": [
                {
                    "id": "stop-1",
                    "file": "src/main.rs",
                    "start_line": 1,
                    "end_line": 1,
                    "symbol_id": "sym-1",
                    "title": "main",
                    "why_here": "Entrypoint.",
                    "explanation": "Body.",
                    "refs": [],
                    "file_hash": hash,
                    "status": "explained"
                },
                {
                    "id": "stop-2",
                    "file": "src/main.rs",
                    "start_line": 1,
                    "end_line": 1,
                    "symbol_id": "sym-2",
                    "title": "helper",
                    "why_here": "Helper.",
                    "explanation": "",
                    "refs": [],
                    "file_hash": "",
                    "status": "pending"
                }
            ]
        }]
    });
    fs::write(
        temp.path().join(".laudit/tours/tour-test.json"),
        serde_json::to_vec_pretty(&tour).expect("serialize tour"),
    )
    .expect("write tour");

    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let output = laudit(&["status", "--repo", &repo, "--json"]);
    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("parse status");
    assert_eq!(report["tour_present"], true);
    assert_eq!(report["tour_id"], "tour-test");
    assert_eq!(report["chapters"], 1);
    assert_eq!(report["stops"], 2);
    assert_eq!(report["explained"], 1);
    assert_eq!(report["pending"], 1);
    assert_eq!(report["stale"], 0);
    assert_eq!(report["position"]["stop"], 1);
}

#[test]
fn status_without_artifacts_succeeds() {
    let temp = TempDir::new();
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let output = laudit(&["status", "--repo", &repo, "--json"]);
    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("parse status");
    assert_eq!(report["tour_present"], false);
    assert_eq!(report["stops"], 0);
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

async fn mount_narration(server: &MockServer, body: String) {
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

fn write_provider_config(repo: &Path, server: &MockServer) {
    let path = repo.join(".laudit/config.toml");
    fs::create_dir_all(path.parent().expect("config parent")).expect("create .laudit");
    fs::write(
        &path,
        format!(
            "[provider]\nbase_url = \"{}\"\nmodel = \"test-model\"\n",
            server.uri()
        ),
    )
    .expect("write config");
}

#[test]
fn tour_print_offline_prints_structure_without_network() {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let config_dir = temp.path().join("config");

    let build = laudit_with_config_dir(&config_dir, &["build", "--offline", "--repo", &repo]);
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let output = laudit_with_config_dir(
        &config_dir,
        &[
            "tour",
            "--print",
            "--offline",
            "--color",
            "never",
            "--repo",
            &repo,
        ],
    );
    assert!(
        output.status.success(),
        "tour failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Chapter 1:"), "got: {stdout}");
    assert!(stdout.contains("src/"), "got: {stdout}");
    assert!(stdout.contains(':'), "got: {stdout}");
    assert!(!stdout.contains('\u{1b}'), "unexpected ANSI: {stdout}");
}

#[tokio::test]
async fn tour_print_narrates_with_mock_llm() {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let config_dir = temp.path().join("config");

    let build = laudit_with_config_dir(&config_dir, &["build", "--offline", "--repo", &repo]);
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let server = MockServer::start().await;
    mount_narration(
        &server,
        sse_body(&["TITLE: Program entry\nWHY: It starts the program.\n---\nThe body."]),
    )
    .await;
    write_provider_config(temp.path(), &server);

    let output = laudit_with_config_dir(
        &config_dir,
        &["tour", "--print", "--color", "never", "--repo", &repo],
    );
    assert!(
        output.status.success(),
        "tour failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Chapter 1:"), "got: {stdout}");
    assert!(
        stdout.contains("Why: It starts the program."),
        "got: {stdout}"
    );
    assert!(stdout.contains("The body."), "got: {stdout}");
}

#[tokio::test]
async fn explain_prints_grounded_explanation_with_mock_llm() {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let config_dir = temp.path().join("config");

    let build = laudit_with_config_dir(&config_dir, &["build", "--offline", "--repo", &repo]);
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let server = MockServer::start().await;
    mount_narration(
        &server,
        sse_body(&["TITLE: Program entry\nWHY: It starts the program.\n---\nThe body."]),
    )
    .await;
    write_provider_config(temp.path(), &server);

    let output = laudit_with_config_dir(&config_dir, &["explain", "main", "--repo", &repo]);
    assert!(
        output.status.success(),
        "explain failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Program entry"), "got: {stdout}");
    assert!(
        stdout.contains("Why: It starts the program."),
        "got: {stdout}"
    );
    assert!(stdout.contains("The body."), "got: {stdout}");
}

#[test]
fn explain_ambiguous_query_lists_candidates_and_fails() {
    let temp = TempDir::new();
    fs::create_dir_all(temp.path().join("src")).expect("create src");
    fs::write(temp.path().join("src/a.rs"), "pub fn dup() {}\n").expect("write a");
    fs::write(temp.path().join("src/b.rs"), "pub fn dup() {}\n").expect("write b");
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let config_dir = temp.path().join("config");

    let build = laudit_with_config_dir(&config_dir, &["build", "--offline", "--repo", &repo]);
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let output = laudit_with_config_dir(
        &config_dir,
        &["explain", "dup", "--offline", "--repo", &repo],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ambiguous"), "got: {stderr}");
    assert!(stderr.contains("src/a.rs"), "got: {stderr}");
    assert!(stderr.contains("src/b.rs"), "got: {stderr}");
}

#[test]
fn explain_unknown_symbol_fails() {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let config_dir = temp.path().join("config");

    let build = laudit_with_config_dir(&config_dir, &["build", "--offline", "--repo", &repo]);
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let output = laudit_with_config_dir(
        &config_dir,
        &["explain", "does_not_exist", "--offline", "--repo", &repo],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no symbol matches"), "got: {stderr}");
}

#[test]
fn tour_print_color_always_emits_ansi_and_never_does_not() {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    let repo = temp.path().to_str().expect("utf-8 temp path").to_owned();
    let config_dir = temp.path().join("config");

    let build = laudit_with_config_dir(&config_dir, &["build", "--offline", "--repo", &repo]);
    assert!(build.status.success());

    let always = laudit_with_config_dir(
        &config_dir,
        &[
            "tour",
            "--print",
            "--offline",
            "--color",
            "always",
            "--repo",
            &repo,
        ],
    );
    assert!(always.status.success());
    assert!(String::from_utf8_lossy(&always.stdout).contains('\u{1b}'));

    let never = laudit_with_config_dir(
        &config_dir,
        &[
            "tour",
            "--print",
            "--offline",
            "--color",
            "never",
            "--repo",
            &repo,
        ],
    );
    assert!(never.status.success());
    assert!(!String::from_utf8_lossy(&never.stdout).contains('\u{1b}'));
}

#[test]
fn tour_rejects_unimplemented_source_adapters() {
    let output = laudit(&["tour", "--print", "--uncommitted"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not implemented yet"), "got: {stderr}");
}

#[test]
fn interactive_tour_requires_a_terminal() {
    let temp = TempDir::new();
    let config_dir = temp.path().join("config");
    let output = laudit_with_config_dir(&config_dir, &["tour"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("requires a terminal"), "got: {stderr}");
}

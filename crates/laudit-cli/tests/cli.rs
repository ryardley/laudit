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
        &[
            "tour",
            "--print",
            "--uncommitted",
            "--range",
            "main..HEAD",
            "--per-commit",
            "--color",
            "never",
        ],
        &["explain", "some_symbol"],
        &["export", "--md", "--out", "tour.md"],
        &["export", "--html"],
        &["ask", "what does this do?"],
        &["mcp"],
        &["outline"],
        &["status"],
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
}

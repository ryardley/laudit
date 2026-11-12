//! CLI integration tests for `laudit tour --uncommitted` and `--range`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("laudit-cli-source-{}-{id}", std::process::id()));
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../laudit-core/tests/fixtures/git-repo")
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

fn run_git(repo: &Path, args: &[&str]) -> Output {
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

fn git_ok(repo: &Path, args: &[&str]) -> String {
    let output = run_git(repo, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit(repo: &Path, message: &str, date: &str) -> String {
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
    git_ok(repo, &["rev-parse", "HEAD"])
}

fn init_repo() -> TempDir {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    git_ok(temp.path(), &["init", "-q"]);
    git_ok(temp.path(), &["config", "user.name", "Alice"]);
    git_ok(temp.path(), &["config", "user.email", "alice@example.com"]);
    git_ok(temp.path(), &["add", "."]);
    commit(temp.path(), "initial import", "2020-01-01T00:00:00Z");
    temp
}

fn laudit(repo: &Path, config_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_laudit"))
        .env("XDG_CONFIG_HOME", config_dir)
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .expect("run laudit")
}

#[test]
fn tour_uncommitted_prints_changes() {
    let repo = init_repo();
    let lib = repo.path().join("src/lib.rs");
    let source = fs::read_to_string(&lib).expect("read lib");
    fs::write(&lib, source.replace("Hello, {name}!", "Hi, {name}!")).expect("edit lib");
    let config_dir = repo.path().join("config");

    let output = laudit(
        repo.path(),
        &config_dir,
        &["tour", "--print", "--offline", "--uncommitted"],
    );
    assert!(
        output.status.success(),
        "tour failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Uncommitted changes"), "got: {stdout}");
    assert!(stdout.contains("src/lib.rs"), "got: {stdout}");
}

#[test]
fn tour_range_prints_changes() {
    let repo = init_repo();
    let base = git_ok(repo.path(), &["rev-parse", "HEAD"]);

    let lib = repo.path().join("src/lib.rs");
    let mut source = fs::read_to_string(&lib).expect("read lib");
    source.push_str("\npub fn multiply(left: i32, right: i32) -> i32 {\n    left * right\n}\n");
    fs::write(&lib, source).expect("edit lib");
    git_ok(repo.path(), &["add", "src/lib.rs"]);
    let head = commit(repo.path(), "add multiply helper", "2021-01-01T00:00:00Z");

    let config_dir = repo.path().join("config");
    let range = format!("{base}..{head}");
    let output = laudit(
        repo.path(),
        &config_dir,
        &["tour", "--print", "--offline", "--range", &range],
    );
    assert!(
        output.status.success(),
        "tour failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Changes"), "got: {stdout}");
    assert!(stdout.contains("src/lib.rs"), "got: {stdout}");
}

#[test]
fn tour_per_commit_prints_one_chapter_per_commit() {
    let repo = init_repo();
    let base = git_ok(repo.path(), &["rev-parse", "HEAD"]);

    let lib = repo.path().join("src/lib.rs");
    let mut source = fs::read_to_string(&lib).expect("read lib");
    source.push_str("\npub fn multiply(left: i32, right: i32) -> i32 {\n    left * right\n}\n");
    fs::write(&lib, source).expect("edit lib");
    git_ok(repo.path(), &["add", "src/lib.rs"]);
    commit(repo.path(), "add multiply helper", "2021-01-01T00:00:00Z");

    let mut source = fs::read_to_string(&lib).expect("read lib");
    source = source.replace("Hello, {name}!", "Hello, {name}!!");
    fs::write(&lib, source).expect("edit lib");
    git_ok(repo.path(), &["add", "src/lib.rs"]);
    let head = commit(repo.path(), "emphasize greeting", "2022-01-01T00:00:00Z");

    let config_dir = repo.path().join("config");
    let range = format!("{base}..{head}");
    let output = laudit(
        repo.path(),
        &config_dir,
        &[
            "tour",
            "--print",
            "--offline",
            "--range",
            &range,
            "--per-commit",
        ],
    );
    assert!(
        output.status.success(),
        "tour failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Chapter 1:"), "got: {stdout}");
    assert!(stdout.contains("Chapter 2:"), "got: {stdout}");
    assert!(stdout.contains("add multiply helper"), "got: {stdout}");
    assert!(stdout.contains("emphasize greeting"), "got: {stdout}");
}

#[test]
fn tour_per_commit_requires_a_range() {
    let repo = init_repo();
    let config_dir = repo.path().join("config");

    let output = laudit(
        repo.path(),
        &config_dir,
        &["tour", "--print", "--offline", "--per-commit"],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("requires --range"), "got: {stderr}");
}

#[test]
fn tour_rejects_an_invalid_range() {
    let repo = init_repo();
    let config_dir = repo.path().join("config");

    let output = laudit(
        repo.path(),
        &config_dir,
        &["tour", "--print", "--offline", "--range", "not-a-range"],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid range"), "got: {stderr}");
}

#[test]
fn tour_range_markdown_is_stable_and_redacted() {
    let repo = init_repo();
    let base = git_ok(repo.path(), &["rev-parse", "HEAD"]);

    let lib = repo.path().join("src/lib.rs");
    let mut source = fs::read_to_string(&lib).expect("read lib");
    source.push_str(
        "\npub fn multiply(left: i32, right: i32) -> i32 {\n    left * right\n}\n\npub fn token() -> &'static str {\n    \"ghp_0123456789abcdefghijklmnopqrstuvwxyz\"\n}\n",
    );
    fs::write(&lib, source).expect("edit lib");
    git_ok(repo.path(), &["add", "src/lib.rs"]);
    let head = commit(
        repo.path(),
        "add multiply and token",
        "2021-01-01T00:00:00Z",
    );

    let config_dir = repo.path().join("config");
    let range = format!("{base}..{head}");
    let first = repo.path().join("tour.md");
    let output = laudit(
        repo.path(),
        &config_dir,
        &[
            "tour",
            "--offline",
            "--range",
            &range,
            "--md",
            "--out",
            first.to_str().expect("path"),
        ],
    );
    assert!(
        output.status.success(),
        "tour failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let markdown = fs::read_to_string(&first).expect("read markdown");
    assert!(markdown.contains("## Changes"), "got:\n{markdown}");
    assert!(markdown.contains("multiply"), "got:\n{markdown}");
    assert!(
        !markdown.contains("ghp_0123456789abcdefghijklmnopqrstuvwxyz"),
        "secret leaked:\n{markdown}"
    );
    assert!(
        markdown.contains("«github_token#1»"),
        "missing placeholder:\n{markdown}"
    );

    let second = repo.path().join("tour-again.md");
    let output = laudit(
        repo.path(),
        &config_dir,
        &[
            "tour",
            "--offline",
            "--range",
            &range,
            "--md",
            "--out",
            second.to_str().expect("path"),
        ],
    );
    assert!(
        output.status.success(),
        "second tour failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        markdown,
        fs::read_to_string(&second).expect("read second markdown")
    );
}

#[test]
fn tour_markdown_prints_to_stdout() {
    let repo = init_repo();
    let base = git_ok(repo.path(), &["rev-parse", "HEAD"]);

    let lib = repo.path().join("src/lib.rs");
    let mut source = fs::read_to_string(&lib).expect("read lib");
    source.push_str("\npub fn multiply(left: i32, right: i32) -> i32 {\n    left * right\n}\n");
    fs::write(&lib, source).expect("edit lib");
    git_ok(repo.path(), &["add", "src/lib.rs"]);
    let head = commit(repo.path(), "add multiply helper", "2021-01-01T00:00:00Z");

    let config_dir = repo.path().join("config");
    let range = format!("{base}..{head}");
    let output = laudit(
        repo.path(),
        &config_dir,
        &["tour", "--offline", "--range", &range, "--md"],
    );
    assert!(
        output.status.success(),
        "tour failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("# "), "got:\n{stdout}");
    assert!(stdout.contains("## Changes"), "got:\n{stdout}");
    assert!(stdout.contains("```rust"), "got:\n{stdout}");
}

#[test]
fn tour_out_requires_markdown() {
    let repo = init_repo();
    let config_dir = repo.path().join("config");

    let output = laudit(
        repo.path(),
        &config_dir,
        &["tour", "--offline", "--out", "tour.md"],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--md"), "got: {stderr}");
}

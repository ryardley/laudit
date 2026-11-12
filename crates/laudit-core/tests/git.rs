use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use laudit_core::recon::git::{
    head, mine_file, mine_region, Churn, LineRange, MAX_RECENT_COMMITS, MAX_SUBJECT_CHARS,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("laudit-git-test-{}-{id}", std::process::id()));
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/git-repo")
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

fn git_ok(repo: &Path, args: &[&str]) {
    let output = run_git(repo, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit(repo: &Path, message: &str, date: &str, author: &str, email: &str) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", message])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", author)
        .env("GIT_AUTHOR_EMAIL", email)
        .env("GIT_COMMITTER_NAME", author)
        .env("GIT_COMMITTER_EMAIL", email)
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

fn init_fixture_repo() -> TempDir {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    git_ok(temp.path(), &["init", "-q"]);
    git_ok(temp.path(), &["config", "user.name", "Alice"]);
    git_ok(temp.path(), &["config", "user.email", "alice@example.com"]);
    git_ok(temp.path(), &["add", "."]);
    commit(
        temp.path(),
        "initial import",
        "2020-01-01T00:00:00Z",
        "Alice",
        "alice@example.com",
    );

    let lib = temp.path().join("src/lib.rs");
    let mut source = fs::read_to_string(&lib).expect("read lib");
    source.push_str("\npub fn multiply(left: i32, right: i32) -> i32 {\n    left * right\n}\n");
    fs::write(&lib, source).expect("write lib");
    git_ok(temp.path(), &["add", "src/lib.rs"]);
    commit(
        temp.path(),
        "add multiply helper",
        "2021-01-01T00:00:00Z",
        "Bob",
        "bob@example.com",
    );

    let mut source = fs::read_to_string(&lib).expect("read lib");
    source = source.replace("Hello, {name}!", "Hello, {name}!!");
    fs::write(&lib, source).expect("write lib");
    git_ok(temp.path(), &["add", "src/lib.rs"]);
    commit(
        temp.path(),
        "emphasize greeting",
        "2022-01-01T00:00:00Z",
        "Alice",
        "alice@example.com",
    );

    temp
}

fn long_subject(index: usize) -> String {
    format!("commit {index} {}", "x".repeat(200))
}

fn init_bounded_repo() -> TempDir {
    let temp = TempDir::new();
    copy_dir_all(&fixture_root(), temp.path()).expect("copy fixture");
    git_ok(temp.path(), &["init", "-q"]);
    git_ok(temp.path(), &["config", "user.name", "Alice"]);
    git_ok(temp.path(), &["config", "user.email", "alice@example.com"]);
    git_ok(temp.path(), &["add", "."]);
    commit(
        temp.path(),
        &long_subject(0),
        "2010-01-01T00:00:00Z",
        "Alice",
        "alice@example.com",
    );

    let lib = temp.path().join("src/lib.rs");
    for index in 1..8 {
        let mut source = fs::read_to_string(&lib).expect("read lib");
        source.push_str(&format!(
            "\npub fn generated_{index}() -> u32 {{\n    {index}\n}}\n"
        ));
        fs::write(&lib, source).expect("write lib");
        git_ok(temp.path(), &["add", "src/lib.rs"]);
        let date = format!("20{:02}-01-01T00:00:00Z", 10 + index);
        commit(
            temp.path(),
            &long_subject(index),
            &date,
            "Alice",
            "alice@example.com",
        );
    }

    temp
}

#[test]
fn fixture_repo_yields_blame_age_and_recent_subjects() {
    let repo = init_fixture_repo();
    let provenance = mine_file(repo.path(), "src/lib.rs");

    let blame = provenance.blame.expect("blame summary");
    assert!(blame.line_count > 0);
    assert_eq!(blame.author_count, 2);
    assert!(blame.oldest_timestamp < blame.newest_timestamp);
    assert!(blame.oldest_age_days >= blame.newest_age_days);
    assert!(blame.newest_age_days > 0);
    assert_eq!(blame.oldest_commit.len(), 40);
    assert_eq!(blame.newest_commit.len(), 40);

    let subjects: Vec<&str> = provenance
        .recent_commits
        .iter()
        .map(|commit| commit.subject.as_str())
        .collect();
    assert_eq!(
        subjects,
        vec![
            "emphasize greeting",
            "add multiply helper",
            "initial import"
        ]
    );

    assert_eq!(provenance.churn.commits, 3);
    assert!(provenance.churn.insertions > 0);
    assert!(provenance.churn.deletions > 0);
}

#[test]
fn region_blame_is_limited_to_the_region() {
    let repo = init_fixture_repo();
    let provenance = mine_region(repo.path(), "src/lib.rs", 1, 3);

    assert_eq!(provenance.region, Some(LineRange::new(1, 3)));
    let blame = provenance.blame.expect("blame summary");
    assert_eq!(blame.line_count, 3);
    assert_eq!(blame.author_count, 1);

    let subjects: Vec<&str> = provenance
        .recent_commits
        .iter()
        .map(|commit| commit.subject.as_str())
        .collect();
    assert!(
        subjects.contains(&"emphasize greeting"),
        "subjects: {subjects:?}"
    );
    assert!(
        subjects.contains(&"initial import"),
        "subjects: {subjects:?}"
    );
    assert!(
        !subjects.contains(&"add multiply helper"),
        "subjects: {subjects:?}"
    );
}

#[test]
fn head_returns_the_current_commit_or_none() {
    let repo = init_fixture_repo();
    let sha = head(repo.path()).expect("head");
    assert_eq!(sha.len(), 40);
    assert!(sha.bytes().all(|byte| byte.is_ascii_hexdigit()));

    let temp = TempDir::new();
    assert_eq!(head(temp.path()), None);
}

#[test]
fn non_git_directory_returns_empty_provenance() {
    let temp = TempDir::new();
    fs::write(temp.path().join("file.rs"), "fn main() {}\n").expect("write file");

    let provenance = mine_file(temp.path(), "file.rs");
    assert!(provenance.is_empty());
    assert!(provenance.blame.is_none());
    assert!(provenance.recent_commits.is_empty());
    assert_eq!(provenance.churn, Churn::default());
}

#[test]
fn untracked_file_returns_empty_provenance() {
    let repo = init_fixture_repo();
    fs::write(repo.path().join("src/untracked.rs"), "fn untracked() {}\n").expect("write file");

    let provenance = mine_file(repo.path(), "src/untracked.rs");
    assert!(provenance.is_empty());
}

#[test]
fn invalid_region_returns_empty_provenance() {
    let repo = init_fixture_repo();
    let provenance = mine_region(repo.path(), "src/lib.rs", 0, 3);
    assert!(provenance.is_empty());
}

#[test]
fn output_is_bounded() {
    let repo = init_bounded_repo();
    let provenance = mine_file(repo.path(), "src/lib.rs");

    assert_eq!(provenance.recent_commits.len(), MAX_RECENT_COMMITS);
    for commit in &provenance.recent_commits {
        assert!(
            commit.subject.chars().count() <= MAX_SUBJECT_CHARS,
            "subject too long: {} chars",
            commit.subject.chars().count()
        );
    }
    assert_eq!(provenance.churn.commits, 8);

    let json = serde_json::to_string(&provenance).expect("serialize provenance");
    assert!(
        json.len() < 8 * 1024,
        "provenance JSON too large: {} bytes",
        json.len()
    );
}

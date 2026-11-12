//! Integration tests for the uncommitted and commit-range source adapters.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use laudit_core::build;
use laudit_core::config::Config;
use laudit_core::model::Skeleton;
use laudit_core::plan;
use laudit_core::source::{self, SourceError, SourceSpec};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("laudit-source-test-{}-{id}", std::process::id()));
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

fn config() -> Config {
    Config::load_from_offline(None, None).expect("offline config")
}

fn recon(repo: &Path) -> Skeleton {
    let config = config();
    let recon = build::recon(repo, &config).expect("recon");
    build::write_skeleton(repo, &recon.skeleton).expect("write skeleton");
    recon.skeleton
}

fn symbol_id(skeleton: &Skeleton, name: &str) -> String {
    skeleton
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("missing symbol {name}"))
        .id
        .clone()
}

fn all_stops(tour: &laudit_core::model::Tour) -> Vec<&laudit_core::model::Stop> {
    tour.chapters
        .iter()
        .flat_map(|chapter| chapter.stops.iter())
        .collect()
}

#[test]
fn uncommitted_tour_maps_hunks_to_symbols_and_includes_untracked_files() {
    let repo = init_repo();
    let lib = repo.path().join("src/lib.rs");
    let source = fs::read_to_string(&lib).expect("read lib");
    fs::write(&lib, source.replace("Hello, {name}!", "Hi, {name}!")).expect("edit lib");
    fs::write(
        repo.path().join("src/new.rs"),
        "pub fn brand_new() -> u32 {\n    42\n}\n",
    )
    .expect("write untracked");

    let skeleton = recon(repo.path());
    let greet = symbol_id(&skeleton, "greet");
    let model = build::offline_model(&skeleton);

    let report = source::plan(
        repo.path(),
        &config(),
        &skeleton,
        &model,
        &SourceSpec::Uncommitted,
    )
    .expect("plan uncommitted");

    assert_eq!(report.tour.source.kind, "uncommitted");
    assert_eq!(report.tour.chapters.len(), 1);
    assert!(report.tour_path.is_file());
    plan::validate_tour(repo.path(), &report.tour).expect("valid tour");

    let stops = all_stops(&report.tour);
    assert!(
        stops
            .iter()
            .any(|stop| stop.file == "src/lib.rs" && stop.symbol_id == greet),
        "expected a stop anchored to greet: {stops:?}"
    );
    assert!(
        stops.iter().any(|stop| stop.file == "src/new.rs"),
        "expected a stop for the untracked file: {stops:?}"
    );
}

#[test]
fn hunk_outside_a_symbol_falls_back_to_a_line_range() {
    let repo = init_repo();
    let lib = repo.path().join("src/lib.rs");
    let source = fs::read_to_string(&lib).expect("read lib");
    fs::write(&lib, format!("// leading comment\n{source}")).expect("edit lib");

    let skeleton = recon(repo.path());
    let model = build::offline_model(&skeleton);

    let report = source::plan(
        repo.path(),
        &config(),
        &skeleton,
        &model,
        &SourceSpec::Uncommitted,
    )
    .expect("plan uncommitted");

    let stops = all_stops(&report.tour);
    let stop = stops
        .iter()
        .find(|stop| stop.file == "src/lib.rs" && stop.start_line == 1)
        .expect("leading comment stop");
    assert_eq!(stop.end_line, 1);
    assert!(stop.symbol_id.is_empty(), "unexpected symbol: {stop:?}");
}

#[test]
fn range_tour_uses_the_net_diff_by_default() {
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

    let skeleton = recon(repo.path());
    let model = build::offline_model(&skeleton);
    let spec = SourceSpec::Range {
        base,
        head,
        per_commit: false,
    };

    let report =
        source::plan(repo.path(), &config(), &skeleton, &model, &spec).expect("plan range");

    assert_eq!(report.tour.source.kind, "range");
    assert_eq!(report.tour.chapters.len(), 1);
    assert!(report.tour.chapters[0].title.contains("Changes"));
    assert!(!all_stops(&report.tour).is_empty());
    plan::validate_tour(repo.path(), &report.tour).expect("valid tour");
}

#[test]
fn per_commit_expands_a_range_into_one_chapter_per_commit() {
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

    let skeleton = recon(repo.path());
    let model = build::offline_model(&skeleton);
    let spec = SourceSpec::Range {
        base,
        head,
        per_commit: true,
    };

    let report = source::plan(repo.path(), &config(), &skeleton, &model, &spec)
        .expect("plan per-commit range");

    assert_eq!(report.tour.chapters.len(), 2);
    assert!(report.tour.chapters[0]
        .title
        .contains("add multiply helper"));
    assert!(report.tour.chapters[1].title.contains("emphasize greeting"));
    for chapter in &report.tour.chapters {
        assert!(!chapter.stops.is_empty(), "empty chapter: {chapter:?}");
    }
    plan::validate_tour(repo.path(), &report.tour).expect("valid tour");
}

#[test]
fn no_changes_is_an_error() {
    let repo = init_repo();
    let skeleton = recon(repo.path());
    let model = build::offline_model(&skeleton);

    let error = source::plan(
        repo.path(),
        &config(),
        &skeleton,
        &model,
        &SourceSpec::Uncommitted,
    )
    .expect_err("no changes");

    assert!(matches!(error, SourceError::NoChanges));
}

#[test]
fn non_git_directory_is_rejected() {
    let temp = TempDir::new();
    fs::write(temp.path().join("src.rs"), "fn main() {}\n").expect("write source");
    let skeleton = Skeleton {
        schema_version: laudit_core::model::SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        repo_id: "fnv1a128:abc".to_owned(),
        head: None,
        dirty_hash: "fnv1a128:def".to_owned(),
        files: Vec::new(),
        symbols: Vec::new(),
        communities: Vec::new(),
        entrypoints: Vec::new(),
        ranked_files: Vec::new(),
        ranked_symbols: Vec::new(),
        provenance: std::collections::BTreeMap::new(),
    };
    let model = build::offline_model(&skeleton);

    let error = source::plan(
        temp.path(),
        &config(),
        &skeleton,
        &model,
        &SourceSpec::Uncommitted,
    )
    .expect_err("not a git repo");

    assert!(matches!(error, SourceError::NotGitRepository(_)));
}

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use laudit_core::recon::index::{build_index, index_path, load, GITIGNORE_FILE, INDEX_DIR};
use laudit_core::recon::walk::{walk_with, WalkOptions};
use laudit_core::recon::TagKind;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("laudit-index-it-{}-{id}", std::process::id()));
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

fn walk(root: &Path) -> Vec<laudit_core::recon::SourceFile> {
    walk_with(root, &WalkOptions::default()).expect("walk fixture")
}

#[test]
fn second_build_reparses_only_the_edited_file() {
    let temp = fixture();
    let files = walk(temp.path());

    let first = build_index(temp.path(), &files).expect("first build");
    assert_eq!(first.stats.parsed, files.len());
    assert_eq!(first.stats.reused, 0);
    assert_eq!(first.stats.added, files.len());
    assert_eq!(first.stats.changed, 0);
    assert_eq!(first.stats.removed, 0);
    assert_eq!(first.index.file_edges, first.graph.file_edges);
    assert_eq!(first.index.symbol_edges, first.graph.symbol_edges);

    let second = build_index(temp.path(), &files).expect("second build");
    assert_eq!(second.stats.parsed, 0);
    assert_eq!(second.stats.reused, files.len());
    assert_eq!(second.stats.removed, 0);
    assert_eq!(second.tags, first.tags);

    let edited = temp.path().join("src/a1.rs");
    let mut source = fs::read_to_string(&edited).expect("read a1");
    source.push_str("\npub fn added_by_edit() {}\n");
    fs::write(&edited, source).expect("edit a1");

    let third = build_index(temp.path(), &files).expect("third build");
    assert_eq!(third.stats.parsed, 1);
    assert_eq!(third.stats.changed, 1);
    assert_eq!(third.stats.added, 0);
    assert_eq!(third.stats.reused, files.len() - 1);
    assert!(third
        .tags
        .iter()
        .any(|tag| tag.kind == TagKind::Def && tag.name == "added_by_edit"));
}

#[test]
fn deleted_files_are_removed_from_the_index() {
    let temp = fixture();
    let files = walk(temp.path());
    build_index(temp.path(), &files).expect("first build");

    fs::remove_file(temp.path().join("src/b3.rs")).expect("delete b3");
    let remaining = walk(temp.path());
    assert_eq!(remaining.len(), files.len() - 1);

    let build = build_index(temp.path(), &remaining).expect("second build");
    assert_eq!(build.stats.parsed, 0);
    assert_eq!(build.stats.reused, remaining.len());
    assert_eq!(build.stats.removed, 1);
    assert!(!build.index.files.contains_key(Path::new("src/b3.rs")));

    let persisted = load(temp.path())
        .expect("load index")
        .expect("index exists");
    assert_eq!(persisted, build.index);
}

#[test]
fn index_lives_under_dot_laudit_and_is_git_ignored() {
    let temp = fixture();
    let files = walk(temp.path());
    build_index(temp.path(), &files).expect("build index");

    assert!(index_path(temp.path()).starts_with(temp.path().join(INDEX_DIR)));
    assert!(index_path(temp.path()).is_file());

    let gitignore = fs::read_to_string(temp.path().join(GITIGNORE_FILE)).expect("read gitignore");
    assert!(
        gitignore.lines().any(|line| line.trim() == "index/"),
        "gitignore was: {gitignore:?}"
    );

    let init = Command::new("git")
        .arg("-C")
        .arg(temp.path())
        .args(["init", "--quiet"])
        .output()
        .expect("run git init");
    assert!(init.status.success(), "git init failed: {init:?}");

    let ignored = Command::new("git")
        .arg("-C")
        .arg(temp.path())
        .args(["check-ignore", ".laudit/index/index.json"])
        .output()
        .expect("run git check-ignore");
    assert!(
        ignored.status.success(),
        "index should be ignored: {}",
        String::from_utf8_lossy(&ignored.stderr)
    );
}

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use laudit_core::build::{build_offline, MODEL_FILE, SKELETON_FILE};
use laudit_core::config::Config;
use laudit_core::model::{CodebaseModel, Skeleton};
use laudit_core::recon::index::{index_path, GITIGNORE_FILE};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("laudit-build-it-{}-{id}", std::process::id()));
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

#[test]
fn offline_build_writes_valid_deterministic_artifacts() {
    let temp = fixture();
    let config = Config::load_from_offline(None, None).expect("offline config");

    let first = build_offline(temp.path(), &config).expect("first build");
    assert_eq!(first.files, 8);
    assert_eq!(first.symbols, 8);
    assert_eq!(first.communities, 2);
    assert_eq!(first.entrypoints, 1);
    assert_eq!(first.parsed, 8);
    assert_eq!(first.reused, 0);
    assert!(index_path(temp.path()).is_file());

    let skeleton_path = temp.path().join(SKELETON_FILE);
    let skeleton_bytes = fs::read(&skeleton_path).expect("read skeleton");
    let skeleton: Skeleton = serde_json::from_slice(&skeleton_bytes).expect("parse skeleton");
    skeleton.validate_schema_version().expect("schema version");
    assert_eq!(skeleton.tool_version, laudit_core::VERSION);
    assert!(!skeleton.files.is_empty());
    assert!(!skeleton.symbols.is_empty());
    assert!(!skeleton.communities.is_empty());
    assert_eq!(skeleton.entrypoints.len(), 1);
    assert_eq!(
        skeleton.ranked_files.first().map(String::as_str),
        Some("src/hub.rs")
    );
    assert!(!skeleton.ranked_symbols.is_empty());

    let model_path = temp.path().join(MODEL_FILE);
    let model: CodebaseModel =
        serde_json::from_slice(&fs::read(&model_path).expect("read model")).expect("parse model");
    model.validate_schema_version().expect("schema version");
    assert_eq!(model.tool_version, laudit_core::VERSION);
    assert!(model.subsystems.is_empty());
    assert!(model.chapters.is_empty());
    assert!(model.flows.is_empty());
    assert!(model.glossary.is_empty());

    let gitignore = fs::read_to_string(temp.path().join(GITIGNORE_FILE)).expect("read gitignore");
    assert!(gitignore.lines().any(|line| line.trim() == "index/"));

    let second = build_offline(temp.path(), &config).expect("second build");
    assert_eq!(second.parsed, 0);
    assert_eq!(second.reused, 8);
    let second_skeleton = fs::read(&skeleton_path).expect("read second skeleton");
    assert_eq!(
        skeleton_bytes, second_skeleton,
        "skeleton is not deterministic"
    );
}

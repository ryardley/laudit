//! Markdown export tests.
//!
//! These tests render committed fixture tours without any network access. The
//! golden snapshot pins the exact document shape, and the range test proves the
//! fenced code blocks contain the source lines the stops point at.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use laudit_core::build::{self, build_offline};
use laudit_core::config::Config;
use laudit_core::export::{render_markdown, MarkdownOptions};
use laudit_core::model::{
    CodebaseModel, Position, Skeleton, Stop, StopStatus, Tour, TourChapter, TourSource,
    SCHEMA_VERSION,
};
use laudit_core::plan::{self, PlanOptions};
use laudit_core::recon::{Community, Entrypoint, EntrypointKind, FileNode, SymbolNode};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("laudit-export-it-{}-{id}", std::process::id()));
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust-app")
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

fn symbol(id: &str, file: &str, name: &str, line: u32) -> SymbolNode {
    SymbolNode {
        id: id.to_owned(),
        file: PathBuf::from(file),
        name: name.to_owned(),
        line,
        rank: 1.0,
        community: 0,
    }
}

fn skeleton() -> Skeleton {
    Skeleton {
        schema_version: SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        repo_id: "fnv1a128:abc".to_owned(),
        head: None,
        dirty_hash: "fnv1a128:def".to_owned(),
        files: vec![
            FileNode {
                path: PathBuf::from("src/main.rs"),
                rank: 1.0,
                community: 0,
            },
            FileNode {
                path: PathBuf::from("src/hub.rs"),
                rank: 0.5,
                community: 0,
            },
            FileNode {
                path: PathBuf::from("src/util.rs"),
                rank: 0.5,
                community: 0,
            },
        ],
        symbols: vec![
            symbol("sym-main", "src/main.rs", "main", 4),
            symbol("sym-hub", "src/hub.rs", "hub", 3),
            symbol("sym-helper", "src/util.rs", "helper", 1),
        ],
        communities: vec![Community {
            id: 0,
            files: vec![
                PathBuf::from("src/main.rs"),
                PathBuf::from("src/hub.rs"),
                PathBuf::from("src/util.rs"),
            ],
            symbols: vec![
                "sym-main".to_owned(),
                "sym-hub".to_owned(),
                "sym-helper".to_owned(),
            ],
        }],
        entrypoints: vec![Entrypoint {
            file: PathBuf::from("src/main.rs"),
            line: 4,
            kind: EntrypointKind::RustMain,
            reason: "Rust binary crate root (main.rs)".to_owned(),
            score: 100,
        }],
        ranked_files: vec![
            "src/main.rs".to_owned(),
            "src/hub.rs".to_owned(),
            "src/util.rs".to_owned(),
        ],
        ranked_symbols: vec![
            "sym-main".to_owned(),
            "sym-hub".to_owned(),
            "sym-helper".to_owned(),
        ],
        provenance: BTreeMap::new(),
    }
}

fn model() -> CodebaseModel {
    build::offline_model(&skeleton())
}

fn stop(
    id: &str,
    file: &str,
    start_line: u32,
    end_line: u32,
    why_here: &str,
    explanation: &str,
    refs: Vec<&str>,
) -> Stop {
    Stop {
        id: id.to_owned(),
        file: file.to_owned(),
        start_line,
        end_line,
        symbol_id: format!("sym-{id}"),
        title: id.to_owned(),
        why_here: why_here.to_owned(),
        explanation: explanation.to_owned(),
        refs: refs.into_iter().map(str::to_owned).collect(),
        file_hash: String::new(),
        status: StopStatus::Explained,
    }
}

fn tour() -> Tour {
    Tour {
        schema_version: SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        tour_id: "tour-export".to_owned(),
        source: TourSource::codebase(),
        model: "test-model".to_owned(),
        prompt_version: "1".to_owned(),
        redaction_profile: "default".to_owned(),
        position: Position::default(),
        chapters: vec![
            TourChapter {
                id: "on-ramp".to_owned(),
                title: "On-ramp".to_owned(),
                stops: vec![stop(
                    "main",
                    "src/main.rs",
                    1,
                    4,
                    "Entrypoint: the tour starts here.",
                    "The program starts here.",
                    vec!["sym-hub", "sym-helper"],
                )],
            },
            TourChapter {
                id: "core".to_owned(),
                title: "Core".to_owned(),
                stops: vec![stop(
                    "helper",
                    "src/util.rs",
                    1,
                    3,
                    "Part of the Core subsystem.",
                    "The helper returns a constant.",
                    vec!["sym-main"],
                )],
            },
        ],
    }
}

#[test]
fn rust_app_tour_markdown_matches_golden() {
    let markdown = render_markdown(
        fixture_root(),
        &tour(),
        &model(),
        Some(&skeleton()),
        &MarkdownOptions::default(),
    )
    .expect("render markdown");

    insta::assert_snapshot!("rust_app_markdown", markdown);
}

#[test]
fn code_fences_contain_the_stop_ranges() {
    let markdown = render_markdown(
        fixture_root(),
        &tour(),
        &model(),
        Some(&skeleton()),
        &MarkdownOptions::default(),
    )
    .expect("render markdown");

    let main_source = fs::read_to_string(fixture_root().join("src/main.rs")).expect("read main");
    let main_range: String = main_source.lines().take(4).collect::<Vec<_>>().join("\n");
    assert!(
        markdown.contains(&format!("```rust\n{main_range}\n```")),
        "missing main range in:\n{markdown}"
    );

    let util_source = fs::read_to_string(fixture_root().join("src/util.rs")).expect("read util");
    let util_range: String = util_source.lines().take(3).collect::<Vec<_>>().join("\n");
    assert!(
        markdown.contains(&format!("```rust\n{util_range}\n```")),
        "missing util range in:\n{markdown}"
    );
}

#[test]
fn offline_tour_renders_without_network() {
    let temp = TempDir::new();
    let repo = temp.path().join("rust-app");
    copy_dir_all(&fixture_root(), &repo).expect("copy fixture");
    let config = Config::load_from_offline(None, None).expect("offline config");

    build_offline(&repo, &config).expect("offline build");
    let skeleton = plan::load_skeleton(&repo).expect("load skeleton");
    let model = build::offline_model(&skeleton);
    let report = plan::plan(&repo, &config, &skeleton, &model, &PlanOptions::default())
        .expect("plan offline tour");

    let markdown = render_markdown(
        &repo,
        &report.tour,
        &model,
        Some(&skeleton),
        &MarkdownOptions::default(),
    )
    .expect("render offline tour");

    assert!(markdown.starts_with("# rust-app\n"), "got:\n{markdown}");
    assert!(markdown.contains("## "), "got:\n{markdown}");
    assert!(markdown.contains("### "), "got:\n{markdown}");
    assert!(markdown.contains("```"), "got:\n{markdown}");
    assert!(markdown.contains("**Why here:**"), "got:\n{markdown}");
    assert!(!markdown.contains("Refs:"), "got:\n{markdown}");
}

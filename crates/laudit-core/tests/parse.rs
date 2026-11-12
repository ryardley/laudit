use std::collections::BTreeSet;
use std::path::PathBuf;

use laudit_core::recon::walk::{walk_with, WalkOptions};
use laudit_core::recon::{parse_file, parse_files, Tag, TagKind};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parse-tree")
}

fn tags_for(name: &str) -> Vec<Tag> {
    parse_file(fixture_root().join(name))
        .expect("read fixture")
        .expect("supported fixture")
}

fn names(tags: &[Tag], kind: TagKind) -> BTreeSet<String> {
    tags.iter()
        .filter(|tag| tag.kind == kind)
        .map(|tag| tag.name.clone())
        .collect()
}

fn assert_contains(tags: &[Tag], kind: TagKind, expected: &[&str]) {
    let actual = names(tags, kind);
    for name in expected {
        assert!(
            actual.contains(*name),
            "expected {kind:?} {name:?}, got {actual:?}"
        );
    }
}

#[test]
fn rust_fixture_yields_defs_and_refs() {
    let tags = tags_for("sample.rs");
    assert_contains(
        &tags,
        TagKind::Def,
        &[
            "Widget", "Mode", "Render", "render", "new", "MAX", "helper", "main",
        ],
    );
    assert_contains(
        &tags,
        TagKind::Ref,
        &[
            "std::collections::HashMap",
            "Widget",
            "helper",
            "new",
            "insert",
            "render",
        ],
    );
    assert!(tags
        .iter()
        .any(|tag| tag.kind == TagKind::Def && tag.name == "Widget" && tag.line == 3));
}

#[test]
fn typescript_fixture_yields_defs_and_refs() {
    let tags = tags_for("sample.ts");
    assert_contains(
        &tags,
        TagKind::Def,
        &[
            "Shape",
            "area",
            "Circle",
            "constructor",
            "makeCircle",
            "PI",
            "Radius",
        ],
    );
    assert_contains(
        &tags,
        TagKind::Ref,
        &["./helper", "helper", "Shape", "Circle"],
    );
}

#[test]
fn tsx_fixture_yields_defs_and_refs() {
    let tags = tags_for("sample.tsx");
    assert_contains(&tags, TagKind::Def, &["Props", "Button", "App"]);
    assert_contains(&tags, TagKind::Ref, &["react", "Props"]);
}

#[test]
fn javascript_fixture_yields_defs_and_refs() {
    let tags = tags_for("sample.js");
    assert_contains(
        &tags,
        TagKind::Def,
        &[
            "Counter",
            "constructor",
            "increment",
            "makeCounter",
            "MAX",
            "double",
        ],
    );
    assert_contains(&tags, TagKind::Ref, &["./helper.js", "helper", "Counter"]);
}

#[test]
fn bash_fixture_yields_defs_and_refs() {
    let tags = tags_for("sample.sh");
    assert_contains(&tags, TagKind::Def, &["greet", "NAME"]);
    assert_contains(&tags, TagKind::Ref, &["echo", "greet", "source"]);
}

#[test]
fn toml_fixture_yields_defs_and_refs() {
    let tags = tags_for("sample.toml");
    assert_contains(
        &tags,
        TagKind::Def,
        &[
            "package",
            "name",
            "version",
            "dependencies.serde",
            "package.metadata.docs",
            "features",
        ],
    );
    assert_contains(&tags, TagKind::Ref, &["dependencies"]);
}

#[test]
fn yaml_fixture_yields_defs_and_refs() {
    let tags = tags_for("sample.yaml");
    assert_contains(
        &tags,
        TagKind::Def,
        &["defaults", "adapter", "host", "development", "database"],
    );
    assert_contains(&tags, TagKind::Ref, &["defaults"]);
}

#[test]
fn json_fixture_yields_defs_and_refs() {
    let tags = tags_for("sample.json");
    assert_contains(
        &tags,
        TagKind::Def,
        &["name", "version", "main", "config", "$ref"],
    );
    assert_contains(&tags, TagKind::Ref, &["#/definitions/config"]);
}

#[test]
fn markdown_fixture_yields_defs_and_refs() {
    let tags = tags_for("sample.md");
    assert_contains(
        &tags,
        TagKind::Def,
        &["Project Title", "Usage", "reference"],
    );
    assert_contains(&tags, TagKind::Ref, &["https://example.com", "./docs.md"]);
}

#[test]
fn sql_fixture_yields_defs_and_refs() {
    let tags = tags_for("sample.sql");
    assert_contains(&tags, TagKind::Def, &["users", "active_users", "greet"]);
    assert_contains(&tags, TagKind::Ref, &["users", "active_users"]);
}

#[test]
fn unsupported_files_are_skipped_without_error() {
    let result = parse_file(fixture_root().join("unsupported.xyz")).expect("no error");
    assert!(result.is_none());
}

#[test]
fn broken_files_still_yield_partial_results() {
    let tags = tags_for("broken.rs");
    assert!(!tags.is_empty());
    assert_contains(&tags, TagKind::Def, &["good"]);
}

#[test]
fn parse_files_skips_unsupported_files() {
    let root = fixture_root();
    let files = walk_with(&root, &WalkOptions::default()).expect("walk fixture");
    let tags = parse_files(&root, &files).expect("parse fixture");

    assert!(!tags.is_empty());
    assert!(!tags
        .iter()
        .any(|tag| tag.file.to_string_lossy().ends_with("unsupported.xyz")));
}

#[test]
fn parsing_is_deterministic() {
    let first = tags_for("sample.rs");
    let second = tags_for("sample.rs");
    assert_eq!(first, second);
}

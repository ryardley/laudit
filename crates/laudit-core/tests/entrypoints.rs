use std::path::{Path, PathBuf};

use laudit_core::recon::entrypoints::{detect, Entrypoint, EntrypointKind};
use laudit_core::recon::walk::{walk_with, WalkOptions};

fn fixture_root(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/entrypoints")
        .join(name)
}

fn detect_fixture(name: &str) -> Vec<Entrypoint> {
    let root = fixture_root(name);
    let files = walk_with(&root, &WalkOptions::default()).expect("walk fixture");
    detect(&root, &files)
}

fn paths(entrypoints: &[Entrypoint]) -> Vec<String> {
    entrypoints
        .iter()
        .map(|entrypoint| entrypoint.file.to_string_lossy().replace('\\', "/"))
        .collect()
}

fn find<'a>(entrypoints: &'a [Entrypoint], file: &str) -> &'a Entrypoint {
    entrypoints
        .iter()
        .find(|entrypoint| entrypoint.file == Path::new(file))
        .unwrap_or_else(|| {
            panic!(
                "missing entrypoint for {file}; got {:?}",
                paths(entrypoints)
            )
        })
}

#[test]
fn rust_app_detects_main_and_bin_targets() {
    let entrypoints = detect_fixture("rust-app");

    let main = find(&entrypoints, "src/main.rs");
    assert_eq!(main.kind, EntrypointKind::RustMain);
    assert!(main.reason.contains("main.rs"), "reason: {}", main.reason);
    assert!(
        main.reason.contains("argument parser usage (clap)"),
        "reason: {}",
        main.reason
    );

    let tool = find(&entrypoints, "src/bin/tool.rs");
    assert_eq!(tool.kind, EntrypointKind::RustBin);
    assert!(
        tool.reason.contains("Cargo [[bin]] target tool"),
        "reason: {}",
        tool.reason
    );

    assert_eq!(entrypoints[0].file, PathBuf::from("src/main.rs"));
}

#[test]
fn ts_app_detects_package_main_bin_and_index() {
    let entrypoints = detect_fixture("ts-app");

    let index = find(&entrypoints, "src/index.ts");
    assert_eq!(index.kind, EntrypointKind::PackageMain);
    assert!(
        index.reason.contains("package.json main -> src/index.ts"),
        "reason: {}",
        index.reason
    );
    assert!(
        index.reason.contains("module index (index.ts)"),
        "reason: {}",
        index.reason
    );

    let cli = find(&entrypoints, "bin/cli.js");
    assert_eq!(cli.kind, EntrypointKind::PackageBin);
    assert!(
        cli.reason.contains("package.json bin ts-app -> bin/cli.js"),
        "reason: {}",
        cli.reason
    );
    assert!(
        cli.reason.contains("shebang script (node)"),
        "reason: {}",
        cli.reason
    );
    assert!(
        cli.reason.contains("argument parser usage (commander)"),
        "reason: {}",
        cli.reason
    );
}

#[test]
fn shebang_scripts_are_detected() {
    let entrypoints = detect_fixture("scripts");
    let script = find(&entrypoints, "scripts/deploy.sh");
    assert_eq!(script.kind, EntrypointKind::Shebang);
    assert!(
        script.reason.contains("shebang script (bash)"),
        "reason: {}",
        script.reason
    );
}

#[test]
fn http_routes_are_detected() {
    let entrypoints = detect_fixture("http-app");
    let server = find(&entrypoints, "src/server.ts");
    assert_eq!(server.kind, EntrypointKind::HttpRoute);
    assert!(
        server
            .reason
            .contains("HTTP route registration (GET /health)"),
        "reason: {}",
        server.reason
    );
}

#[test]
fn rust_http_routes_are_detected() {
    let entrypoints = detect_fixture("rust-http");
    let main = find(&entrypoints, "src/main.rs");
    assert_eq!(main.kind, EntrypointKind::RustMain);
    assert!(
        main.reason
            .contains("HTTP route registration (GET /health)"),
        "reason: {}",
        main.reason
    );
}

#[test]
fn library_only_fixture_has_no_entrypoints() {
    let entrypoints = detect_fixture("library-only");
    assert!(
        entrypoints.is_empty(),
        "expected no entrypoints, got {:?}",
        paths(&entrypoints)
    );
}

#[test]
fn results_are_ranked_and_deterministic() {
    let first = detect_fixture("rust-app");
    let second = detect_fixture("rust-app");
    assert_eq!(first, second);

    for pair in first.windows(2) {
        assert!(
            pair[0].score >= pair[1].score,
            "scores are not descending: {:?}",
            first
                .iter()
                .map(|entrypoint| (&entrypoint.file, entrypoint.score))
                .collect::<Vec<_>>()
        );
    }
}

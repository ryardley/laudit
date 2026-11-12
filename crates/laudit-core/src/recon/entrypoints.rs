//! Deterministic entrypoint detection.
//!
//! Entrypoints are the places a tour should start: the on-ramp (D4, D22). This
//! stage looks for the conventional ways a repository declares an executable
//! surface — Rust binary targets, `package.json` `main`/`bin`, module indexes,
//! shebang scripts, HTTP route registrations, and argument-parser usage — and
//! returns ranked candidates with human-readable reasons.
//!
//! Detection is intentionally conservative. A library-only repository should
//! produce no candidates, so signals are tied to concrete files and known
//! framework/library names rather than broad heuristics.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::walk::{Language, SourceFile};

/// Confidence scores used to rank entrypoint candidates.
const SCORE_RUST_MAIN: u32 = 100;
const SCORE_RUST_BIN: u32 = 95;
const SCORE_PACKAGE_MAIN: u32 = 90;
const SCORE_PACKAGE_BIN: u32 = 90;
const SCORE_SHEBANG: u32 = 80;
const SCORE_HTTP_ROUTE: u32 = 70;
const SCORE_INDEX_MODULE: u32 = 60;
const SCORE_ARG_PARSER: u32 = 50;

/// The kind of signal that identified an entrypoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntrypointKind {
    /// A Rust binary crate root (`main.rs` or `src/main.rs`).
    RustMain,
    /// A Rust binary target declared by Cargo or found under `src/bin`.
    RustBin,
    /// A TypeScript/JavaScript package `main` entry.
    PackageMain,
    /// A TypeScript/JavaScript package `bin` entry.
    PackageBin,
    /// A TypeScript/JavaScript module index (`index.ts`, `index.js`, ...).
    IndexModule,
    /// A script with a shebang interpreter.
    Shebang,
    /// An HTTP route registration.
    HttpRoute,
    /// Usage of an argument-parsing library.
    ArgParser,
}

/// A ranked entrypoint candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entrypoint {
    /// File containing the entrypoint, relative to the repository root.
    pub file: PathBuf,
    /// 1-based line of the strongest signal.
    pub line: u32,
    /// Strongest signal that identified the entrypoint.
    pub kind: EntrypointKind,
    /// Human-readable reasons, joined with `; `.
    pub reason: String,
    /// Ranking score; higher is a stronger entrypoint.
    pub score: u32,
}

/// Detect entrypoint candidates in `files`, resolving paths against `repo_root`.
///
/// The result is sorted by score descending, then by path and line ascending, so
/// repeated runs over the same repository produce identical output. Unreadable
/// files are skipped rather than aborting detection.
#[must_use]
pub fn detect(repo_root: impl AsRef<Path>, files: &[SourceFile]) -> Vec<Entrypoint> {
    let repo_root = repo_root.as_ref();
    let walked: BTreeSet<PathBuf> = files.iter().map(|file| file.path.clone()).collect();
    let mut candidates: BTreeMap<PathBuf, Candidate> = BTreeMap::new();

    for file in files {
        if file.language == Language::Rust {
            detect_rust_target(&mut candidates, &file.path);
        }
        detect_index_module(&mut candidates, &file.path, file.language);
    }

    for file in files {
        let Ok(bytes) = fs::read(repo_root.join(&file.path)) else {
            continue;
        };
        let source = String::from_utf8_lossy(&bytes);

        match (
            file.language,
            file.path.file_name().and_then(|name| name.to_str()),
        ) {
            (Language::Toml, Some("Cargo.toml")) => {
                detect_cargo_bins(&mut candidates, &file.path, &source, &walked);
            }
            (Language::Json, Some("package.json")) => {
                detect_package_json(&mut candidates, &file.path, &source);
            }
            _ => {}
        }

        if let Some(interpreter) = source.lines().next().and_then(shebang_interpreter) {
            add_candidate(
                &mut candidates,
                &file.path,
                1,
                EntrypointKind::Shebang,
                format!("shebang script ({interpreter})"),
                SCORE_SHEBANG,
            );
        }

        if let Some((line, reason)) = detect_http_route(&source) {
            add_candidate(
                &mut candidates,
                &file.path,
                line,
                EntrypointKind::HttpRoute,
                reason,
                SCORE_HTTP_ROUTE,
            );
        }

        if let Some((line, reason)) = detect_arg_parser(file.language, &source) {
            add_candidate(
                &mut candidates,
                &file.path,
                line,
                EntrypointKind::ArgParser,
                reason,
                SCORE_ARG_PARSER,
            );
        }
    }

    let mut entrypoints: Vec<Entrypoint> = candidates
        .into_iter()
        .map(|(file, candidate)| Entrypoint {
            file,
            line: candidate.line,
            kind: candidate.kind,
            reason: candidate.reasons.join("; "),
            score: candidate.score,
        })
        .collect();

    entrypoints.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    entrypoints
}

#[derive(Debug)]
struct Candidate {
    line: u32,
    kind: EntrypointKind,
    reasons: Vec<String>,
    score: u32,
}

fn add_candidate(
    candidates: &mut BTreeMap<PathBuf, Candidate>,
    file: &Path,
    line: u32,
    kind: EntrypointKind,
    reason: String,
    score: u32,
) {
    let candidate = candidates
        .entry(file.to_path_buf())
        .or_insert_with(|| Candidate {
            line,
            kind,
            reasons: Vec::new(),
            score,
        });

    if score > candidate.score {
        candidate.score = score;
        candidate.kind = kind;
        candidate.line = line;
    } else if score == candidate.score && line < candidate.line {
        candidate.line = line;
    }

    if !candidate.reasons.contains(&reason) {
        candidate.reasons.push(reason);
    }
}

fn detect_rust_target(candidates: &mut BTreeMap<PathBuf, Candidate>, path: &Path) {
    if is_rust_main_path(path) {
        add_candidate(
            candidates,
            path,
            1,
            EntrypointKind::RustMain,
            "Rust binary crate root (main.rs)".to_owned(),
            SCORE_RUST_MAIN,
        );
    } else if is_src_bin_path(path) {
        add_candidate(
            candidates,
            path,
            1,
            EntrypointKind::RustBin,
            "Rust binary target under src/bin".to_owned(),
            SCORE_RUST_BIN,
        );
    }
}

fn is_rust_main_path(path: &Path) -> bool {
    let components: Vec<&str> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect();
    matches!(components.as_slice(), ["main.rs"] | [.., "src", "main.rs"])
}

fn is_src_bin_path(path: &Path) -> bool {
    let components: Vec<&str> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect();
    components.windows(2).any(|window| window == ["src", "bin"])
}

fn detect_index_module(
    candidates: &mut BTreeMap<PathBuf, Candidate>,
    path: &Path,
    language: Language,
) {
    if !matches!(
        language,
        Language::TypeScript | Language::Tsx | Language::JavaScript
    ) {
        return;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    if matches!(
        name,
        "index.ts" | "index.tsx" | "index.js" | "index.mjs" | "index.cjs"
    ) {
        add_candidate(
            candidates,
            path,
            1,
            EntrypointKind::IndexModule,
            format!("module index ({name})"),
            SCORE_INDEX_MODULE,
        );
    }
}

fn detect_cargo_bins(
    candidates: &mut BTreeMap<PathBuf, Candidate>,
    manifest_path: &Path,
    source: &str,
    walked: &BTreeSet<PathBuf>,
) {
    let Ok(manifest) = toml::from_str::<toml::Value>(source) else {
        return;
    };
    let Some(bins) = manifest.get("bin").and_then(toml::Value::as_array) else {
        return;
    };
    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new(""));

    for bin in bins {
        let Some(name) = bin.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        let bin_path = match bin.get("path").and_then(toml::Value::as_str) {
            Some(path) => normalize_path(&manifest_dir.join(path)),
            None => infer_cargo_bin_path(manifest_dir, name, walked),
        };
        add_candidate(
            candidates,
            &bin_path,
            1,
            EntrypointKind::RustBin,
            format!("Cargo [[bin]] target {name}"),
            SCORE_RUST_BIN,
        );
    }
}

fn infer_cargo_bin_path(manifest_dir: &Path, name: &str, walked: &BTreeSet<PathBuf>) -> PathBuf {
    let candidates = [
        manifest_dir.join("src/bin").join(format!("{name}.rs")),
        manifest_dir.join("src/bin").join(name).join("main.rs"),
        manifest_dir.join("src/main.rs"),
    ];
    for candidate in &candidates {
        let normalized = normalize_path(candidate);
        if walked.contains(&normalized) {
            return normalized;
        }
    }
    normalize_path(&candidates[0])
}

fn detect_package_json(
    candidates: &mut BTreeMap<PathBuf, Candidate>,
    package_path: &Path,
    source: &str,
) {
    let Ok(package) = serde_json::from_str::<serde_json::Value>(source) else {
        return;
    };
    let package_dir = package_path.parent().unwrap_or_else(|| Path::new(""));

    if let Some(main) = package.get("main").and_then(serde_json::Value::as_str) {
        let path = normalize_path(&package_dir.join(main));
        add_candidate(
            candidates,
            &path,
            1,
            EntrypointKind::PackageMain,
            format!("package.json main -> {main}"),
            SCORE_PACKAGE_MAIN,
        );
    }

    match package.get("bin") {
        Some(serde_json::Value::String(path)) => {
            let resolved = normalize_path(&package_dir.join(path));
            add_candidate(
                candidates,
                &resolved,
                1,
                EntrypointKind::PackageBin,
                format!("package.json bin -> {path}"),
                SCORE_PACKAGE_BIN,
            );
        }
        Some(serde_json::Value::Object(bins)) => {
            for (name, path) in bins {
                let Some(path) = path.as_str() else {
                    continue;
                };
                let resolved = normalize_path(&package_dir.join(path));
                add_candidate(
                    candidates,
                    &resolved,
                    1,
                    EntrypointKind::PackageBin,
                    format!("package.json bin {name} -> {path}"),
                    SCORE_PACKAGE_BIN,
                );
            }
        }
        _ => {}
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn shebang_interpreter(line: &str) -> Option<&str> {
    let line = line.strip_prefix("#!")?.trim();
    if line.is_empty() {
        return None;
    }

    let mut parts = line.split_whitespace();
    let first = parts.next()?;
    let interpreter = if first.rsplit('/').next() == Some("env") {
        parts.find(|part| !part.starts_with('-'))?
    } else {
        first
    };
    let name = interpreter.rsplit('/').next().unwrap_or(interpreter);
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn detect_http_route(source: &str) -> Option<(u32, String)> {
    static JS_ROUTE: OnceLock<Regex> = OnceLock::new();
    static RUST_ROUTE: OnceLock<Regex> = OnceLock::new();
    static RUST_ATTRIBUTE: OnceLock<Regex> = OnceLock::new();
    static GO_ROUTE: OnceLock<Regex> = OnceLock::new();

    let js_route = JS_ROUTE.get_or_init(|| {
        Regex::new(
            r#"\b(?:app|router|server|fastify|api|routes?|r)\.(get|post|put|patch|delete|options|head|all|route)\s*\(\s*['"`]([^'"`]+)['"`]"#,
        )
        .expect("valid JavaScript route regex")
    });
    let rust_route = RUST_ROUTE.get_or_init(|| {
        Regex::new(
            r#"\.route\s*\(\s*"([^"]+)"\s*,\s*(get|post|put|patch|delete|options|head|any)\s*\("#,
        )
        .expect("valid Rust route regex")
    });
    let rust_attribute = RUST_ATTRIBUTE.get_or_init(|| {
        Regex::new(r#"#\[(get|post|put|patch|delete|options|head)\s*\(\s*"([^"]+)"\s*\)\]"#)
            .expect("valid Rust route attribute regex")
    });
    let go_route = GO_ROUTE.get_or_init(|| {
        Regex::new(r#"\b(?:http|mux|router|r)\.(?:HandleFunc|Handle)\s*\(\s*"([^"]+)""#)
            .expect("valid Go route regex")
    });

    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//")
            || trimmed.starts_with('*')
            || trimmed.starts_with("/*")
            || (trimmed.starts_with('#') && !trimmed.starts_with("#["))
        {
            continue;
        }

        if let Some(captures) = js_route.captures(line) {
            let method = captures
                .get(1)
                .map_or("", |m| m.as_str())
                .to_ascii_uppercase();
            let path = captures.get(2).map_or("", |m| m.as_str());
            return Some((
                index as u32 + 1,
                format!("HTTP route registration ({method} {path})"),
            ));
        }
        if let Some(captures) = rust_route.captures(line) {
            let path = captures.get(1).map_or("", |m| m.as_str());
            let method = captures
                .get(2)
                .map_or("", |m| m.as_str())
                .to_ascii_uppercase();
            return Some((
                index as u32 + 1,
                format!("HTTP route registration ({method} {path})"),
            ));
        }
        if let Some(captures) = rust_attribute.captures(line) {
            let method = captures
                .get(1)
                .map_or("", |m| m.as_str())
                .to_ascii_uppercase();
            let path = captures.get(2).map_or("", |m| m.as_str());
            return Some((
                index as u32 + 1,
                format!("HTTP route registration ({method} {path})"),
            ));
        }
        if let Some(captures) = go_route.captures(line) {
            let path = captures.get(1).map_or("", |m| m.as_str());
            return Some((
                index as u32 + 1,
                format!("HTTP route registration ({path})"),
            ));
        }
    }

    None
}

fn detect_arg_parser(language: Language, source: &str) -> Option<(u32, String)> {
    for (name, regex) in arg_parser_patterns(language) {
        if let Some(found) = regex.find(source) {
            let line = source[..found.start()]
                .bytes()
                .filter(|&byte| byte == b'\n')
                .count() as u32
                + 1;
            return Some((line, format!("argument parser usage ({name})")));
        }
    }
    None
}

fn arg_parser_patterns(language: Language) -> &'static [(&'static str, Regex)] {
    static RUST: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    static JS: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    static BASH: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();

    match language {
        Language::Rust => RUST
            .get_or_init(|| {
                compile_patterns(&[
                    ("clap", r"\bclap\b"),
                    ("clap", r"#\[derive\([^)]*Parser"),
                    ("clap", r"\bParser::parse\b"),
                    ("structopt", r"\bstructopt\b"),
                    ("argh", r"\bargh\b"),
                    ("pico-args", r"\bpico_args\b"),
                    ("lexopt", r"\blexopt\b"),
                    ("getopts", r"\bgetopts\b"),
                    ("std::env::args", r"\b(?:std::)?env::args\b"),
                ])
            })
            .as_slice(),
        Language::TypeScript | Language::Tsx | Language::JavaScript => JS
            .get_or_init(|| {
                compile_patterns(&[
                    ("commander", r"\bcommander\b"),
                    ("yargs", r"\byargs\b"),
                    ("minimist", r"\bminimist\b"),
                    (
                        "arg",
                        r#"\bfrom\s+['"]arg['"]|\brequire\s*\(\s*['"]arg['"]\s*\)"#,
                    ),
                    ("cac", r"\bcac\b"),
                    ("meow", r"\bmeow\b"),
                    ("process.argv", r"\bprocess\.argv\b"),
                ])
            })
            .as_slice(),
        Language::Bash => BASH
            .get_or_init(|| {
                compile_patterns(&[("getopts", r"\bgetopts\b"), ("getopt", r"\bgetopt\b")])
            })
            .as_slice(),
        _ => &[],
    }
}

fn compile_patterns(patterns: &[(&'static str, &'static str)]) -> Vec<(&'static str, Regex)> {
    patterns
        .iter()
        .filter_map(|(name, pattern)| Regex::new(pattern).ok().map(|regex| (*name, regex)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shebang_interpreter_handles_env_and_direct_paths() {
        assert_eq!(shebang_interpreter("#!/bin/bash"), Some("bash"));
        assert_eq!(shebang_interpreter("#!/usr/bin/env node"), Some("node"));
        assert_eq!(
            shebang_interpreter("#!/usr/bin/env -S deno run"),
            Some("deno")
        );
        assert_eq!(shebang_interpreter("not a shebang"), None);
        assert_eq!(shebang_interpreter("#!/usr/bin/env"), None);
    }

    #[test]
    fn http_route_detection_handles_common_frameworks() {
        assert_eq!(
            detect_http_route("app.get('/health', handler);").map(|(_, reason)| reason),
            Some("HTTP route registration (GET /health)".to_owned())
        );
        assert_eq!(
            detect_http_route("Router::new().route(\"/health\", get(health));")
                .map(|(_, reason)| reason),
            Some("HTTP route registration (GET /health)".to_owned())
        );
        assert_eq!(
            detect_http_route("#[get(\"/health\")]").map(|(_, reason)| reason),
            Some("HTTP route registration (GET /health)".to_owned())
        );
        assert_eq!(
            detect_http_route("http.HandleFunc(\"/health\", handler)").map(|(_, reason)| reason),
            Some("HTTP route registration (/health)".to_owned())
        );
        assert_eq!(detect_http_route("let value = map.get(\"key\");"), None);
        assert_eq!(detect_http_route("// app.get('/health', handler);"), None);
    }

    #[test]
    fn arg_parser_detection_handles_common_libraries() {
        assert_eq!(
            detect_arg_parser(Language::Rust, "use clap::Parser;").map(|(_, reason)| reason),
            Some("argument parser usage (clap)".to_owned())
        );
        assert_eq!(
            detect_arg_parser(
                Language::JavaScript,
                "const { program } = require('commander');"
            )
            .map(|(_, reason)| reason),
            Some("argument parser usage (commander)".to_owned())
        );
        assert_eq!(
            detect_arg_parser(Language::Bash, "while getopts \"ab\" opt; do")
                .map(|(_, reason)| reason),
            Some("argument parser usage (getopts)".to_owned())
        );
        assert_eq!(
            detect_arg_parser(
                Language::Rust,
                "pub fn add(a: i32, b: i32) -> i32 { a + b }"
            ),
            None
        );
    }

    #[test]
    fn normalize_path_removes_current_dir_components() {
        assert_eq!(
            normalize_path(Path::new("./src/./main.rs")),
            PathBuf::from("src/main.rs")
        );
    }

    #[test]
    fn rust_target_paths_work_inside_workspaces() {
        assert!(is_rust_main_path(Path::new("src/main.rs")));
        assert!(is_rust_main_path(Path::new("crates/app/src/main.rs")));
        assert!(!is_rust_main_path(Path::new("tests/main.rs")));
        assert!(is_src_bin_path(Path::new("src/bin/tool.rs")));
        assert!(is_src_bin_path(Path::new("crates/app/src/bin/tool.rs")));
        assert!(!is_src_bin_path(Path::new("src/lib.rs")));
    }
}

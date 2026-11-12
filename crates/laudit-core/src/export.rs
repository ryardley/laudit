//! Markdown export of a tour.
//!
//! Export turns a persisted tour artifact into a shareable markdown document
//! (D23). It reads only artifacts and source files — never the LLM — so it
//! works offline and can render tours whose stops are still pending. The
//! document is deterministic: no timestamps or other run-specific values are
//! written into the body, which keeps golden tests stable.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::build;
use crate::model::{CodebaseModel, Skeleton, Stop, Tour};
use crate::plan::{self, PlanError};
use crate::recon::Language;
use crate::status;

/// Fallback document title when the repository has no usable directory name.
pub const DEFAULT_TITLE: &str = "laudit tour";

/// Options controlling markdown rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkdownOptions {
    /// Document title. Defaults to the repository directory name.
    pub title: Option<String>,
}

/// A summary of a markdown export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportReport {
    /// Tour identifier that was rendered.
    pub tour_id: String,
    /// Path of the tour artifact.
    pub tour_path: PathBuf,
    /// Path the markdown was written to, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<PathBuf>,
    /// Number of chapters rendered.
    pub chapters: usize,
    /// Number of stops rendered.
    pub stops: usize,
    /// The rendered markdown.
    #[serde(skip)]
    pub markdown: String,
}

/// Render a tour as a markdown document.
///
/// The document title defaults to the repository directory name. Every stop is
/// rendered as a heading with its `file:line` range, a fenced code block of the
/// exact source range, the `why_here` line, the explanation, and a `Refs:` line
/// linking each referenced symbol to its definition.
pub fn render_markdown(
    repo_root: impl AsRef<Path>,
    tour: &Tour,
    model: &CodebaseModel,
    skeleton: Option<&Skeleton>,
    options: &MarkdownOptions,
) -> Result<String, ExportError> {
    let repo_root = repo_root.as_ref();
    let title = options
        .title
        .clone()
        .unwrap_or_else(|| default_title(repo_root, model));
    let symbols = symbol_index(tour, skeleton);

    let mut out = String::new();
    let _ = writeln!(out, "# {title}");

    for chapter in &tour.chapters {
        out.push('\n');
        let _ = writeln!(out, "## {}", chapter.title);

        for stop in &chapter.stops {
            out.push('\n');
            let stop_title = if stop.title.is_empty() {
                &stop.symbol_id
            } else {
                &stop.title
            };
            let _ = writeln!(
                out,
                "### {stop_title} — {}:{}-{}",
                stop.file, stop.start_line, stop.end_line
            );
            out.push('\n');

            let code = read_range(repo_root, stop)?;
            let language = Language::from_path(Path::new(&stop.file)).map_or("", Language::as_str);
            let fence = fence_for(&code);
            let _ = writeln!(out, "{fence}{language}");
            out.push_str(&code);
            if !code.ends_with('\n') {
                out.push('\n');
            }
            let _ = writeln!(out, "{fence}");

            if !stop.why_here.is_empty() {
                out.push('\n');
                let _ = writeln!(out, "**Why here:** {}", stop.why_here);
            }

            if !stop.explanation.is_empty() {
                out.push('\n');
                out.push_str(stop.explanation.trim_end());
                out.push('\n');
            }

            if !stop.refs.is_empty() {
                out.push('\n');
                let refs: Vec<String> = stop
                    .refs
                    .iter()
                    .map(|id| render_ref(id, &symbols))
                    .collect();
                let _ = writeln!(out, "Refs: {}", refs.join(", "));
            }
        }
    }

    Ok(out)
}

/// Load the latest (or named) tour and its artifacts, render markdown, and
/// optionally write it to `out`.
///
/// When `out` is `None` the markdown is returned in [`ExportReport::markdown`]
/// for the caller to print. No LLM or network access is involved.
pub fn export_markdown(
    repo_root: impl AsRef<Path>,
    tour_id: Option<&str>,
    out: Option<&Path>,
) -> Result<ExportReport, ExportError> {
    let repo_root = repo_root.as_ref();
    let (tour, tour_path) = load_tour(repo_root, tour_id)?;
    let skeleton = load_skeleton(repo_root)?;
    let model = load_model(repo_root, skeleton.as_ref())?;
    let markdown = render_markdown(
        repo_root,
        &tour,
        &model,
        skeleton.as_ref(),
        &MarkdownOptions::default(),
    )?;

    let output_path = match out {
        Some(path) => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).map_err(|source| ExportError::Io {
                        path: parent.to_path_buf(),
                        source,
                    })?;
                }
            }
            fs::write(path, &markdown).map_err(|source| ExportError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            Some(path.to_path_buf())
        }
        None => None,
    };

    let chapters = tour.chapters.len();
    let stops = tour
        .chapters
        .iter()
        .map(|chapter| chapter.stops.len())
        .sum();
    Ok(ExportReport {
        tour_id: tour.tour_id,
        tour_path,
        output_path,
        chapters,
        stops,
        markdown,
    })
}

fn load_tour(repo_root: &Path, tour_id: Option<&str>) -> Result<(Tour, PathBuf), ExportError> {
    let tour_id = match tour_id {
        Some(tour_id) => tour_id.to_owned(),
        None => status::latest_tour_id(repo_root)
            .map_err(|source| ExportError::Io {
                path: repo_root.join(plan::TOURS_DIR),
                source,
            })?
            .ok_or(ExportError::NoTour)?,
    };
    let tour = plan::load_tour(repo_root, &tour_id)?;
    let path = plan::tour_path(repo_root, &tour_id);
    Ok((tour, path))
}

fn load_skeleton(repo_root: &Path) -> Result<Option<Skeleton>, ExportError> {
    match plan::load_skeleton(repo_root) {
        Ok(skeleton) => Ok(Some(skeleton)),
        Err(PlanError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ExportError::Plan(error)),
    }
}

fn load_model(repo_root: &Path, skeleton: Option<&Skeleton>) -> Result<CodebaseModel, ExportError> {
    match plan::load_model(repo_root) {
        Ok(model) => Ok(model),
        Err(PlanError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            match skeleton {
                Some(skeleton) => Ok(build::offline_model(skeleton)),
                None => Err(ExportError::NoModel),
            }
        }
        Err(error) => Err(ExportError::Plan(error)),
    }
}

fn default_title(repo_root: &Path, model: &CodebaseModel) -> String {
    let canonical = fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    if let Some(name) = canonical.file_name().and_then(|name| name.to_str()) {
        if !name.is_empty() {
            return name.to_owned();
        }
    }
    if let Some(subsystem) = model.subsystems.first() {
        if !subsystem.title.is_empty() {
            return subsystem.title.clone();
        }
    }
    DEFAULT_TITLE.to_owned()
}

fn read_range(repo_root: &Path, stop: &Stop) -> Result<String, ExportError> {
    let path = repo_root.join(&stop.file);
    let bytes = fs::read(&path).map_err(|source| ExportError::Io {
        path: path.clone(),
        source,
    })?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = stop.start_line.max(1) as usize;
    let end = stop.end_line.max(stop.start_line) as usize;
    if start > lines.len() || end < start {
        return Ok(String::new());
    }
    let end = end.min(lines.len());
    let mut code = lines[start - 1..end].join("\n");
    code.push('\n');
    Ok(code)
}

struct SymbolRef {
    name: String,
    file: String,
    line: u32,
}

fn symbol_index(tour: &Tour, skeleton: Option<&Skeleton>) -> BTreeMap<String, SymbolRef> {
    let mut index = BTreeMap::new();
    if let Some(skeleton) = skeleton {
        for symbol in &skeleton.symbols {
            index.insert(
                symbol.id.clone(),
                SymbolRef {
                    name: symbol.name.clone(),
                    file: symbol.file.to_string_lossy().into_owned(),
                    line: symbol.line,
                },
            );
        }
    }
    for chapter in &tour.chapters {
        for stop in &chapter.stops {
            index
                .entry(stop.symbol_id.clone())
                .or_insert_with(|| SymbolRef {
                    name: if stop.title.is_empty() {
                        stop.symbol_id.clone()
                    } else {
                        stop.title.clone()
                    },
                    file: stop.file.clone(),
                    line: stop.start_line,
                });
        }
    }
    index
}

fn render_ref(id: &str, symbols: &BTreeMap<String, SymbolRef>) -> String {
    match symbols.get(id) {
        Some(symbol) => format!(
            "[{} ({}:{})]({}:{})",
            symbol.name, symbol.file, symbol.line, symbol.file, symbol.line
        ),
        None => id.to_owned(),
    }
}

fn fence_for(code: &str) -> String {
    let mut longest = 0;
    let mut current = 0;
    for character in code.chars() {
        if character == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

/// Errors produced while exporting a tour.
#[derive(Debug)]
pub enum ExportError {
    /// An artifact or source file could not be read or written.
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// A planning or artifact error.
    Plan(PlanError),
    /// No tour artifact exists to export.
    NoTour,
    /// Neither a model artifact nor a skeleton is available.
    NoModel,
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            Self::Plan(error) => write!(f, "{error}"),
            Self::NoTour => write!(
                f,
                "no tour artifact found; run `laudit build` or `laudit tour` first"
            ),
            Self::NoModel => write!(f, "no model artifact found; run `laudit build` first"),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Plan(error) => Some(error),
            Self::NoTour | Self::NoModel => None,
        }
    }
}

impl From<PlanError> for ExportError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Position, StopStatus, TourChapter, TourSource, SCHEMA_VERSION};
    use crate::recon::{Community, Entrypoint, EntrypointKind, FileNode, SymbolNode};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("laudit-export-test-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn write(&self, name: &str, contents: &str) {
            let path = self.path.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(path, contents).expect("write file");
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
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

    fn skeleton(symbols: Vec<SymbolNode>) -> Skeleton {
        Skeleton {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            repo_id: "fnv1a128:abc".to_owned(),
            head: None,
            dirty_hash: "fnv1a128:def".to_owned(),
            files: vec![FileNode {
                path: PathBuf::from("src/main.rs"),
                rank: 1.0,
                community: 0,
            }],
            symbols,
            communities: vec![Community {
                id: 0,
                files: vec![PathBuf::from("src/main.rs")],
                symbols: Vec::new(),
            }],
            entrypoints: vec![Entrypoint {
                file: PathBuf::from("src/main.rs"),
                line: 1,
                kind: EntrypointKind::RustMain,
                reason: "Rust binary crate root (main.rs)".to_owned(),
                score: 100,
            }],
            ranked_files: vec!["src/main.rs".to_owned()],
            ranked_symbols: Vec::new(),
            provenance: BTreeMap::new(),
        }
    }

    fn model() -> CodebaseModel {
        build::offline_model(&skeleton(Vec::new()))
    }

    fn stop(id: &str, file: &str, start: u32, end: u32, refs: Vec<&str>) -> Stop {
        Stop {
            id: id.to_owned(),
            file: file.to_owned(),
            start_line: start,
            end_line: end,
            symbol_id: format!("sym-{id}"),
            title: id.to_owned(),
            why_here: "It is part of the tour.".to_owned(),
            explanation: "The explanation.".to_owned(),
            refs: refs.into_iter().map(str::to_owned).collect(),
            file_hash: String::new(),
            status: StopStatus::Explained,
        }
    }

    fn tour(stops: Vec<Stop>) -> Tour {
        Tour {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            tour_id: "tour-test".to_owned(),
            source: TourSource::codebase(),
            model: "test-model".to_owned(),
            prompt_version: "1".to_owned(),
            redaction_profile: "default".to_owned(),
            position: Position::default(),
            chapters: vec![TourChapter {
                id: "on-ramp".to_owned(),
                title: "On-ramp".to_owned(),
                stops,
            }],
        }
    }

    #[test]
    fn renders_headings_code_why_and_refs() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {\n    greet();\n}\n");
        temp.write("src/lib.rs", "pub fn greet() {}\n");
        let skeleton = skeleton(vec![
            symbol("sym-main", "src/main.rs", "main", 1),
            symbol("sym-greet", "src/lib.rs", "greet", 1),
        ]);
        let tour = tour(vec![stop("main", "src/main.rs", 1, 3, vec!["sym-greet"])]);

        let markdown = render_markdown(
            temp.path(),
            &tour,
            &model(),
            Some(&skeleton),
            &MarkdownOptions::default(),
        )
        .expect("render");

        assert!(markdown.starts_with("# "));
        assert!(markdown.contains("## On-ramp"));
        assert!(markdown.contains("### main — src/main.rs:1-3"));
        assert!(markdown.contains("```rust\nfn main() {\n    greet();\n}\n```"));
        assert!(markdown.contains("**Why here:** It is part of the tour."));
        assert!(markdown.contains("The explanation."));
        assert!(markdown.contains("Refs: [greet (src/lib.rs:1)](src/lib.rs:1)"));
    }

    #[test]
    fn code_fence_uses_the_file_extension_language() {
        let temp = TempDir::new();
        temp.write("config/app.toml", "[server]\nport = 8080\n");
        let skeleton = skeleton(vec![symbol("sym-server", "config/app.toml", "server", 1)]);
        let tour = tour(vec![stop("server", "config/app.toml", 1, 2, Vec::new())]);

        let markdown = render_markdown(
            temp.path(),
            &tour,
            &model(),
            Some(&skeleton),
            &MarkdownOptions::default(),
        )
        .expect("render");

        assert!(markdown.contains("```toml\n[server]\nport = 8080\n```"));
    }

    #[test]
    fn empty_prose_and_refs_are_omitted() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let skeleton = skeleton(vec![symbol("sym-main", "src/main.rs", "main", 1)]);
        let mut stop = stop("main", "src/main.rs", 1, 1, Vec::new());
        stop.why_here.clear();
        stop.explanation.clear();
        let tour = tour(vec![stop]);

        let markdown = render_markdown(
            temp.path(),
            &tour,
            &model(),
            Some(&skeleton),
            &MarkdownOptions::default(),
        )
        .expect("render");

        assert!(!markdown.contains("**Why here:**"));
        assert!(!markdown.contains("Refs:"));
        assert!(markdown.contains("```rust\nfn main() {}\n```"));
    }

    #[test]
    fn explicit_title_overrides_the_repository_name() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let skeleton = skeleton(vec![symbol("sym-main", "src/main.rs", "main", 1)]);
        let tour = tour(vec![stop("main", "src/main.rs", 1, 1, Vec::new())]);

        let markdown = render_markdown(
            temp.path(),
            &tour,
            &model(),
            Some(&skeleton),
            &MarkdownOptions {
                title: Some("Custom title".to_owned()),
            },
        )
        .expect("render");

        assert!(markdown.starts_with("# Custom title\n"));
    }

    #[test]
    fn renders_without_a_skeleton_using_tour_stops_for_refs() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        temp.write("src/lib.rs", "pub fn greet() {}\n");
        let tour = tour(vec![
            stop("main", "src/main.rs", 1, 1, vec!["sym-greet"]),
            stop("greet", "src/lib.rs", 1, 1, Vec::new()),
        ]);

        let markdown = render_markdown(
            temp.path(),
            &tour,
            &model(),
            None,
            &MarkdownOptions::default(),
        )
        .expect("render");

        assert!(markdown.contains("### main — src/main.rs:1-1"));
        assert!(markdown.contains("Refs: [greet (src/lib.rs:1)](src/lib.rs:1)"));
    }

    #[test]
    fn missing_source_file_is_an_error() {
        let temp = TempDir::new();
        let skeleton = skeleton(vec![symbol("sym-main", "src/main.rs", "main", 1)]);
        let tour = tour(vec![stop("main", "src/main.rs", 1, 1, Vec::new())]);

        let error = render_markdown(
            temp.path(),
            &tour,
            &model(),
            Some(&skeleton),
            &MarkdownOptions::default(),
        )
        .expect_err("missing source");

        assert!(matches!(error, ExportError::Io { .. }));
    }

    #[test]
    fn export_writes_to_the_requested_path() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let skeleton = skeleton(vec![symbol("sym-main", "src/main.rs", "main", 1)]);
        let tour = tour(vec![stop("main", "src/main.rs", 1, 1, Vec::new())]);
        plan::write_tour(temp.path(), &tour).expect("write tour");
        build::write_skeleton(temp.path(), &skeleton).expect("write skeleton");
        build::write_model(temp.path(), &model()).expect("write model");

        let out = temp.path().join("out/tour.md");
        let report = export_markdown(temp.path(), None, Some(&out)).expect("export");

        assert_eq!(report.tour_id, "tour-test");
        assert_eq!(report.chapters, 1);
        assert_eq!(report.stops, 1);
        assert_eq!(report.output_path.as_deref(), Some(out.as_path()));
        assert_eq!(
            fs::read_to_string(&out).expect("read output"),
            report.markdown
        );
    }

    #[test]
    fn export_without_a_tour_is_an_error() {
        let temp = TempDir::new();
        let error = export_markdown(temp.path(), None, None).expect_err("no tour");
        assert!(matches!(error, ExportError::NoTour));
    }
}

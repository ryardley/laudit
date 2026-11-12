//! Non-interactive single-symbol explanation (`laudit explain`).
//!
//! The CLI resolves a user query against the recon skeleton, finds an existing
//! stop for the symbol or creates one, and narrates it with the same Stage 3
//! machinery as a full tour. No RPC server is involved (D23).

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::build;
use crate::cache;
use crate::config::Config;
use crate::llm::LlmClient;
use crate::model::{CodebaseModel, Skeleton, Stop, StopStatus, Tour, TourChapter};
use crate::narration::{self, NarrationError, NarrationSink};
use crate::plan::{self, PlanError};
use crate::recon::SymbolNode;
use crate::status;

/// A symbol resolved from a user query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedSymbol {
    /// Stable symbol identifier.
    pub id: String,
    /// Repository-relative file path.
    pub file: String,
    /// Symbol name.
    pub name: String,
    /// 1-based definition line.
    pub line: u32,
}

impl ResolvedSymbol {
    fn from_node(symbol: &SymbolNode) -> Self {
        Self {
            id: symbol.id.clone(),
            file: symbol.file.to_string_lossy().into_owned(),
            name: symbol.name.clone(),
            line: symbol.line,
        }
    }
}

/// Why a symbol query could not be resolved to exactly one symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// No symbol matched the query.
    NotFound {
        /// The original query.
        query: String,
    },
    /// More than one symbol matched the query.
    Ambiguous {
        /// The original query.
        query: String,
        /// Matching symbols, sorted by file and line.
        candidates: Vec<ResolvedSymbol>,
    },
    /// The query was malformed.
    Invalid {
        /// The original query.
        query: String,
        /// Why the query is invalid.
        reason: String,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { query } => write!(f, "no symbol matches `{query}`"),
            Self::Ambiguous { query, candidates } => {
                writeln!(f, "`{query}` is ambiguous; candidates:")?;
                for candidate in candidates {
                    writeln!(
                        f,
                        "  {} ({}:{})",
                        candidate.name, candidate.file, candidate.line
                    )?;
                }
                Ok(())
            }
            Self::Invalid { query, reason } => {
                write!(f, "invalid symbol query `{query}`: {reason}")
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// Resolve a symbol query against the recon skeleton.
///
/// Accepted forms are a bare name (`main`), `file:symbol`
/// (`src/main.rs:main`), and `file:line` (`src/main.rs:42`). A symbol ID
/// (`sym-…`) is also accepted. File paths may be full repository-relative
/// paths or suffixes such as `main.rs`.
pub fn resolve(skeleton: &Skeleton, query: &str) -> Result<ResolvedSymbol, ResolveError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(ResolveError::Invalid {
            query: query.to_owned(),
            reason: "query is empty".to_owned(),
        });
    }

    if let Some(symbol) = skeleton.symbols.iter().find(|symbol| symbol.id == query) {
        return Ok(ResolvedSymbol::from_node(symbol));
    }

    if let Some((file, rest)) = query.rsplit_once(':') {
        if file.is_empty() {
            return Err(ResolveError::Invalid {
                query: query.to_owned(),
                reason: "file part is empty".to_owned(),
            });
        }
        if rest.is_empty() {
            return Err(ResolveError::Invalid {
                query: query.to_owned(),
                reason: "symbol or line part is empty".to_owned(),
            });
        }
        let files = matching_files(skeleton, file);
        if files.is_empty() {
            return Err(ResolveError::NotFound {
                query: query.to_owned(),
            });
        }
        if let Ok(line) = rest.parse::<u32>() {
            return resolve_line(skeleton, &files, line, query);
        }
        return resolve_named(skeleton, &files, rest, query);
    }

    resolve_named(skeleton, &[], query, query)
}

fn matching_files(skeleton: &Skeleton, file: &str) -> Vec<PathBuf> {
    let target = Path::new(file);
    let mut files: Vec<PathBuf> = skeleton
        .files
        .iter()
        .map(|node| node.path.clone())
        .filter(|path| path == target || path.ends_with(target))
        .collect();
    files.sort();
    files.dedup();
    files
}

fn resolve_named(
    skeleton: &Skeleton,
    files: &[PathBuf],
    name: &str,
    query: &str,
) -> Result<ResolvedSymbol, ResolveError> {
    let mut candidates: Vec<&SymbolNode> = skeleton
        .symbols
        .iter()
        .filter(|symbol| symbol.name == name)
        .filter(|symbol| files.is_empty() || files.iter().any(|file| file == &symbol.file))
        .collect();
    candidates.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.id.cmp(&b.id))
    });
    match candidates.as_slice() {
        [] => Err(ResolveError::NotFound {
            query: query.to_owned(),
        }),
        [symbol] => Ok(ResolvedSymbol::from_node(symbol)),
        _ => Err(ResolveError::Ambiguous {
            query: query.to_owned(),
            candidates: candidates
                .iter()
                .map(|symbol| ResolvedSymbol::from_node(symbol))
                .collect(),
        }),
    }
}

fn resolve_line(
    skeleton: &Skeleton,
    files: &[PathBuf],
    line: u32,
    query: &str,
) -> Result<ResolvedSymbol, ResolveError> {
    let in_files = |symbol: &&SymbolNode| files.iter().any(|file| file == &symbol.file);
    let mut candidates: Vec<&SymbolNode> = skeleton
        .symbols
        .iter()
        .filter(in_files)
        .filter(|symbol| symbol.line == line)
        .collect();
    if candidates.is_empty() {
        let nearest = skeleton
            .symbols
            .iter()
            .filter(in_files)
            .filter(|symbol| symbol.line <= line)
            .map(|symbol| symbol.line)
            .max();
        if let Some(nearest) = nearest {
            candidates = skeleton
                .symbols
                .iter()
                .filter(in_files)
                .filter(|symbol| symbol.line == nearest)
                .collect();
        }
    }
    candidates.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.id.cmp(&b.id))
    });
    match candidates.as_slice() {
        [] => Err(ResolveError::NotFound {
            query: query.to_owned(),
        }),
        [symbol] => Ok(ResolvedSymbol::from_node(symbol)),
        _ => Err(ResolveError::Ambiguous {
            query: query.to_owned(),
            candidates: candidates
                .iter()
                .map(|symbol| ResolvedSymbol::from_node(symbol))
                .collect(),
        }),
    }
}

/// Load the skeleton artifact, running recon if it is missing.
pub fn load_skeleton(
    repo_root: impl AsRef<Path>,
    config: &Config,
) -> Result<Skeleton, ExplainError> {
    let repo_root = repo_root.as_ref();
    match plan::load_skeleton(repo_root) {
        Ok(skeleton) => Ok(skeleton),
        Err(PlanError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            let recon = build::recon(repo_root, config)?;
            build::write_skeleton(repo_root, &recon.skeleton)?;
            Ok(recon.skeleton)
        }
        Err(error) => Err(ExplainError::Plan(error)),
    }
}

/// Load the codebase model artifact.
pub fn load_model(repo_root: impl AsRef<Path>) -> Result<CodebaseModel, ExplainError> {
    plan::load_model(repo_root.as_ref()).map_err(ExplainError::Plan)
}

/// A single-symbol explanation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExplainReport {
    /// The resolved symbol.
    pub symbol: ResolvedSymbol,
    /// The stop that was explained.
    pub stop: Stop,
    /// Whether the explanation was reused from a tour artifact.
    pub cached: bool,
    /// Warnings produced while narrating.
    pub warnings: Vec<String>,
}

/// Explain one symbol, narrating it if necessary.
///
/// If the current tour already contains a stop for the symbol, that stop is
/// narrated in place and the tour is persisted so the explanation is cached.
/// Otherwise a temporary single-stop chapter is narrated without writing a new
/// tour artifact.
pub async fn explain(
    repo_root: impl AsRef<Path>,
    config: &Config,
    skeleton: &Skeleton,
    model: &CodebaseModel,
    symbol: &ResolvedSymbol,
    client: &LlmClient,
    sink: &mut dyn NarrationSink,
) -> Result<ExplainReport, ExplainError> {
    let repo_root = repo_root.as_ref();
    let node = skeleton
        .symbols
        .iter()
        .find(|node| node.id == symbol.id)
        .ok_or_else(|| {
            ExplainError::Resolve(ResolveError::NotFound {
                query: symbol.id.clone(),
            })
        })?;

    if let Some((mut tour, chapter_index, stop_index)) = find_stop(repo_root, &symbol.id)? {
        let chapter = tour.chapters[chapter_index].clone();
        let mut stop = chapter.stops[stop_index].clone();
        if stop.status == StopStatus::Explained && hash_matches(repo_root, &stop) {
            return Ok(ExplainReport {
                symbol: symbol.clone(),
                stop,
                cached: true,
                warnings: Vec::new(),
            });
        }
        let report = narration::narrate_stop(
            repo_root,
            config,
            narration::NarrationContext { skeleton, model },
            &chapter,
            &mut stop,
            client,
            sink,
        )
        .await?;
        tour.chapters[chapter_index].stops[stop_index] = stop.clone();
        tour.model = client.model().to_owned();
        tour.prompt_version = report.prompt_version.clone();
        plan::write_tour(repo_root, &tour)?;
        return Ok(ExplainReport {
            symbol: symbol.clone(),
            stop,
            cached: false,
            warnings: report.warnings,
        });
    }

    let mut stop = plan::stop_for_symbol(repo_root, skeleton, node)?;
    let chapter = TourChapter {
        id: "explain".to_owned(),
        title: "Explanation".to_owned(),
        stops: vec![stop.clone()],
    };
    let report = narration::narrate_stop(
        repo_root,
        config,
        narration::NarrationContext { skeleton, model },
        &chapter,
        &mut stop,
        client,
        sink,
    )
    .await?;
    Ok(ExplainReport {
        symbol: symbol.clone(),
        stop,
        cached: false,
        warnings: report.warnings,
    })
}

fn find_stop(
    repo_root: &Path,
    symbol_id: &str,
) -> Result<Option<(Tour, usize, usize)>, ExplainError> {
    let Some(tour_id) = status::latest_tour_id(repo_root)? else {
        return Ok(None);
    };
    let tour = plan::load_tour(repo_root, &tour_id)?;
    for (chapter_index, chapter) in tour.chapters.iter().enumerate() {
        for (stop_index, stop) in chapter.stops.iter().enumerate() {
            if stop.symbol_id == symbol_id {
                return Ok(Some((tour, chapter_index, stop_index)));
            }
        }
    }
    Ok(None)
}

fn hash_matches(repo_root: &Path, stop: &Stop) -> bool {
    if stop.file_hash.is_empty() {
        return true;
    }
    cache::file_hash(repo_root, &stop.file).is_ok_and(|hash| hash == stop.file_hash)
}

/// Errors produced while explaining a symbol.
#[derive(Debug)]
pub enum ExplainError {
    /// An artifact could not be read or written.
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// An artifact could not be serialized or parsed.
    Serialize(serde_json::Error),
    /// Recon failed while creating a missing skeleton.
    Build(build::BuildError),
    /// A planning or artifact error.
    Plan(PlanError),
    /// Narration failed.
    Narration(NarrationError),
    /// The symbol query could not be resolved.
    Resolve(ResolveError),
    /// The persisted skeleton is not usable.
    InvalidSkeleton(String),
    /// The persisted model is not usable.
    InvalidModel(String),
}

impl fmt::Display for ExplainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            Self::Serialize(error) => write!(f, "{error}"),
            Self::Build(error) => write!(f, "{error}"),
            Self::Plan(error) => write!(f, "{error}"),
            Self::Narration(error) => write!(f, "{error}"),
            Self::Resolve(error) => write!(f, "{error}"),
            Self::InvalidSkeleton(message) => write!(f, "invalid skeleton: {message}"),
            Self::InvalidModel(message) => write!(f, "invalid model: {message}"),
        }
    }
}

impl std::error::Error for ExplainError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Serialize(error) => Some(error),
            Self::Build(error) => Some(error),
            Self::Plan(error) => Some(error),
            Self::Narration(error) => Some(error),
            Self::Resolve(error) => Some(error),
            Self::InvalidSkeleton(_) | Self::InvalidModel(_) => None,
        }
    }
}

impl From<io::Error> for ExplainError {
    fn from(source: io::Error) -> Self {
        Self::Io {
            path: PathBuf::from("."),
            source,
        }
    }
}

impl From<serde_json::Error> for ExplainError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

impl From<build::BuildError> for ExplainError {
    fn from(error: build::BuildError) -> Self {
        Self::Build(error)
    }
}

impl From<PlanError> for ExplainError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}

impl From<NarrationError> for ExplainError {
    fn from(error: NarrationError) -> Self {
        Self::Narration(error)
    }
}

impl From<ResolveError> for ExplainError {
    fn from(error: ResolveError) -> Self {
        Self::Resolve(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SCHEMA_VERSION;
    use crate::recon::{Community, FileNode};
    use std::collections::BTreeMap;

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

    fn skeleton(symbols: Vec<SymbolNode>, files: Vec<&str>) -> Skeleton {
        Skeleton {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            repo_id: "fnv1a128:test".to_owned(),
            head: None,
            dirty_hash: "fnv1a128:test".to_owned(),
            files: files
                .iter()
                .map(|file| FileNode {
                    path: PathBuf::from(file),
                    rank: 1.0,
                    community: 0,
                })
                .collect(),
            symbols: symbols.clone(),
            communities: vec![Community {
                id: 0,
                files: files.iter().map(PathBuf::from).collect(),
                symbols: symbols.iter().map(|symbol| symbol.id.clone()).collect(),
            }],
            entrypoints: Vec::new(),
            ranked_files: files.iter().map(|file| (*file).to_owned()).collect(),
            ranked_symbols: symbols.iter().map(|symbol| symbol.id.clone()).collect(),
            provenance: BTreeMap::new(),
        }
    }

    #[test]
    fn bare_name_resolves_a_unique_symbol() {
        let skeleton = skeleton(
            vec![
                symbol("sym-main", "src/main.rs", "main", 1),
                symbol("sym-hub", "src/hub.rs", "hub", 1),
            ],
            vec!["src/main.rs", "src/hub.rs"],
        );
        let resolved = resolve(&skeleton, "main").expect("resolve");
        assert_eq!(resolved.id, "sym-main");
        assert_eq!(resolved.file, "src/main.rs");
        assert_eq!(resolved.line, 1);
    }

    #[test]
    fn file_and_symbol_resolves() {
        let skeleton = skeleton(
            vec![
                symbol("sym-a", "src/a.rs", "run", 1),
                symbol("sym-b", "src/b.rs", "run", 1),
            ],
            vec!["src/a.rs", "src/b.rs"],
        );
        let resolved = resolve(&skeleton, "src/b.rs:run").expect("resolve");
        assert_eq!(resolved.id, "sym-b");
        let resolved = resolve(&skeleton, "b.rs:run").expect("suffix resolve");
        assert_eq!(resolved.id, "sym-b");
    }

    #[test]
    fn file_and_line_resolves_the_enclosing_symbol() {
        let skeleton = skeleton(
            vec![
                symbol("sym-a", "src/a.rs", "first", 1),
                symbol("sym-b", "src/a.rs", "second", 10),
            ],
            vec!["src/a.rs"],
        );
        let resolved = resolve(&skeleton, "src/a.rs:12").expect("resolve");
        assert_eq!(resolved.id, "sym-b");
        let resolved = resolve(&skeleton, "src/a.rs:10").expect("exact resolve");
        assert_eq!(resolved.id, "sym-b");
    }

    #[test]
    fn ambiguous_names_list_candidates() {
        let skeleton = skeleton(
            vec![
                symbol("sym-a", "src/a.rs", "run", 1),
                symbol("sym-b", "src/b.rs", "run", 1),
            ],
            vec!["src/a.rs", "src/b.rs"],
        );
        let error = resolve(&skeleton, "run").expect_err("ambiguous");
        match error {
            ResolveError::Ambiguous { candidates, .. } => {
                assert_eq!(candidates.len(), 2);
                assert_eq!(candidates[0].file, "src/a.rs");
                assert_eq!(candidates[1].file, "src/b.rs");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn unknown_symbols_are_not_found() {
        let skeleton = skeleton(
            vec![symbol("sym-a", "src/a.rs", "run", 1)],
            vec!["src/a.rs"],
        );
        assert!(matches!(
            resolve(&skeleton, "missing"),
            Err(ResolveError::NotFound { .. })
        ));
        assert!(matches!(
            resolve(&skeleton, "src/a.rs:missing"),
            Err(ResolveError::NotFound { .. })
        ));
        assert!(matches!(
            resolve(&skeleton, "src/missing.rs:run"),
            Err(ResolveError::NotFound { .. })
        ));
    }

    #[test]
    fn symbol_ids_resolve_directly() {
        let skeleton = skeleton(
            vec![symbol("sym-a", "src/a.rs", "run", 1)],
            vec!["src/a.rs"],
        );
        let resolved = resolve(&skeleton, "sym-a").expect("resolve id");
        assert_eq!(resolved.id, "sym-a");
    }
}

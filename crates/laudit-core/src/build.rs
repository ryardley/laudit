//! Offline build pipeline.
//!
//! The offline build runs Stage 0 recon end to end and writes the deterministic
//! structural artifacts that later stages consume. It never calls an LLM and
//! never touches the network (D4, D20).

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::Config;
use crate::model::{CodebaseModel, Skeleton, SCHEMA_VERSION};
use crate::recon::index::index_path;
use crate::recon::walk::walk;
use crate::recon::{
    build_index, content_hash, detect_entrypoints, head as git_head, mine_files, Entrypoint, Graph,
    Index, IndexBuild, Provenance, WalkError,
};

/// Repository-relative path of the structural skeleton artifact.
pub const SKELETON_FILE: &str = ".laudit/skeleton.json";
/// Repository-relative path of the placeholder codebase model artifact.
pub const MODEL_FILE: &str = ".laudit/model.json";
/// Model identifier recorded in the offline placeholder model.
pub const OFFLINE_MODEL: &str = "offline";
/// Prompt version recorded in the offline placeholder model.
pub const OFFLINE_PROMPT_VERSION: &str = "0";
/// Deterministic timestamp recorded in the offline placeholder model.
pub const OFFLINE_GENERATED_AT: &str = "1970-01-01T00:00:00Z";

/// A summary of an offline build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildReport {
    /// Number of source files in the index.
    pub files: usize,
    /// Number of symbols in the graph.
    pub symbols: usize,
    /// Number of communities in the graph.
    pub communities: usize,
    /// Number of entrypoint candidates.
    pub entrypoints: usize,
    /// Number of files parsed during this build.
    pub parsed: usize,
    /// Number of files whose cached tags were reused.
    pub reused: usize,
    /// Path of the persisted index.
    pub index_path: PathBuf,
    /// Path of the skeleton artifact.
    pub skeleton_path: PathBuf,
    /// Path of the placeholder model artifact.
    pub model_path: PathBuf,
}

/// Errors produced by an offline build.
#[derive(Debug)]
pub enum BuildError {
    /// The repository walk failed.
    Walk(WalkError),
    /// An artifact could not be read or written.
    Io(io::Error),
    /// An artifact could not be serialized.
    Serialize(serde_json::Error),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Walk(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::Serialize(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Walk(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Serialize(error) => Some(error),
        }
    }
}

impl From<WalkError> for BuildError {
    fn from(error: WalkError) -> Self {
        Self::Walk(error)
    }
}

impl From<io::Error> for BuildError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for BuildError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

/// The result of running Stage 0 recon without writing artifacts.
#[derive(Debug, Clone, PartialEq)]
pub struct ReconReport {
    /// The deterministic structural skeleton.
    pub skeleton: Skeleton,
    /// The incremental index and graph built during recon.
    pub index_build: IndexBuild,
}

/// Run Stage 0 recon and return the skeleton and index without writing the
/// placeholder model.
///
/// This is the shared entry point for the offline build and for Stage 1
/// comprehension, which must be able to inspect an existing `model.json` before
/// deciding whether its cache is still valid.
pub fn recon(repo_root: impl AsRef<Path>, config: &Config) -> Result<ReconReport, BuildError> {
    let repo_root = repo_root.as_ref();
    let files = walk(repo_root, config)?;
    let index_build = build_index(repo_root, &files)?;
    let entrypoints = detect_entrypoints(repo_root, &files);
    let provenance = mine_files(repo_root, &files);

    let repo_id = repo_id(repo_root);
    let head = git_head(repo_root);
    let dirty_hash = dirty_hash(&index_build.index);

    let skeleton = skeleton(
        &index_build.graph,
        entrypoints,
        provenance,
        repo_id,
        head,
        dirty_hash,
    );

    Ok(ReconReport {
        skeleton,
        index_build,
    })
}

/// Write the structural skeleton artifact under `.laudit/`.
pub fn write_skeleton(
    repo_root: impl AsRef<Path>,
    skeleton: &Skeleton,
) -> Result<PathBuf, BuildError> {
    let path = repo_root.as_ref().join(SKELETON_FILE);
    write_json(&path, skeleton)?;
    Ok(path)
}

/// Run Stage 0 recon and write the offline artifacts under `.laudit/`.
///
/// The build walks the repository, updates the content-hash index, rebuilds the
/// graph, detects entrypoints, mines provenance, and writes a structural
/// skeleton plus a placeholder codebase model with empty prose fields. No LLM
/// is called.
pub fn build_offline(
    repo_root: impl AsRef<Path>,
    config: &Config,
) -> Result<BuildReport, BuildError> {
    let repo_root = repo_root.as_ref();
    let ReconReport {
        skeleton,
        index_build,
    } = recon(repo_root, config)?;
    let model = placeholder_model(
        skeleton.repo_id.clone(),
        skeleton.head.clone(),
        skeleton.dirty_hash.clone(),
    );

    let skeleton_path = write_skeleton(repo_root, &skeleton)?;
    let model_path = repo_root.join(MODEL_FILE);
    write_json(&model_path, &model)?;

    Ok(BuildReport {
        files: index_build.stats.total,
        symbols: index_build.graph.symbols.len(),
        communities: index_build.graph.communities.len(),
        entrypoints: skeleton.entrypoints.len(),
        parsed: index_build.stats.parsed,
        reused: index_build.stats.reused,
        index_path: index_path(repo_root),
        skeleton_path,
        model_path,
    })
}

fn skeleton(
    graph: &Graph,
    entrypoints: Vec<Entrypoint>,
    provenance: BTreeMap<PathBuf, Provenance>,
    repo_id: String,
    head: Option<String>,
    dirty_hash: String,
) -> Skeleton {
    let ranked_files = graph
        .ranked_files()
        .iter()
        .map(|file| file.path.to_string_lossy().into_owned())
        .collect();
    let ranked_symbols = graph
        .ranked_symbols()
        .iter()
        .map(|symbol| symbol.id.clone())
        .collect();

    Skeleton {
        schema_version: SCHEMA_VERSION,
        tool_version: crate::VERSION.to_owned(),
        repo_id,
        head,
        dirty_hash,
        files: graph.files.clone(),
        symbols: graph.symbols.clone(),
        communities: graph.communities.clone(),
        entrypoints,
        ranked_files,
        ranked_symbols,
        provenance,
    }
}

/// Build the placeholder codebase model for a skeleton.
///
/// This is the model written by an offline build. It is also used by offline
/// frontends that need a model to plan a tour but have not run comprehension.
#[must_use]
pub fn offline_model(skeleton: &Skeleton) -> CodebaseModel {
    placeholder_model(
        skeleton.repo_id.clone(),
        skeleton.head.clone(),
        skeleton.dirty_hash.clone(),
    )
}

fn placeholder_model(repo_id: String, head: Option<String>, dirty_hash: String) -> CodebaseModel {
    CodebaseModel {
        schema_version: SCHEMA_VERSION,
        tool_version: crate::VERSION.to_owned(),
        repo_id,
        head,
        dirty_hash,
        generator: "laudit".to_owned(),
        model: OFFLINE_MODEL.to_owned(),
        prompt_version: OFFLINE_PROMPT_VERSION.to_owned(),
        generated_at: OFFLINE_GENERATED_AT.to_owned(),
        subsystems: Vec::new(),
        chapters: Vec::new(),
        flows: Vec::new(),
        glossary: Vec::new(),
    }
}

fn repo_id(repo_root: &Path) -> String {
    let canonical = fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    content_hash(canonical.to_string_lossy().as_bytes())
}

fn dirty_hash(index: &Index) -> String {
    let mut input = String::new();
    for (path, entry) in &index.files {
        input.push_str(&path.to_string_lossy());
        input.push('\0');
        input.push_str(&entry.hash);
        input.push('\n');
    }
    content_hash(input.as_bytes())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), BuildError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

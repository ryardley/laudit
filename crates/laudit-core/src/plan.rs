//! Stage 2 planning: turn a codebase model into an ordered tour of anchored stops.
//!
//! Planning is deterministic (D4, D12). It derives chapters in the default order
//! — on-ramp, subsystems in dependency order, infrastructure, then configuration
//! — and anchors every stop to a real symbol ID and line range from recon
//! (D22, D24). Tests are skipped by default and opt-in. The result is written to
//! `.laudit/tours/<id>.json` with no prose yet; Stage 3 narration fills that in.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::build::{MODEL_FILE, SKELETON_FILE};
use crate::config::Config;
use crate::model::{
    CodebaseModel, Position, Skeleton, Stop, StopStatus, Tour, TourChapter, TourSource,
    SCHEMA_VERSION,
};
use crate::prompts::{self, NARRATION};
use crate::recon::{Entrypoint, SymbolNode};

/// Directory, relative to the repository root, holding tour artifacts.
pub const TOURS_DIR: &str = ".laudit/tours";
/// Redaction profile recorded in planned tours.
pub const DEFAULT_REDACTION_PROFILE: &str = "default";
/// Number of top-ranked symbols used as an on-ramp when no entrypoint symbol
/// can be resolved.
const ON_RAMP_FALLBACK_STOPS: usize = 3;

/// Options controlling a planning run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanOptions {
    /// Include test files in the tour.
    pub include_tests: bool,
    /// Include configuration files in the tour.
    pub include_config: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            include_tests: false,
            include_config: true,
        }
    }
}

/// A summary of a planning run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanReport {
    /// The planned tour.
    pub tour: Tour,
    /// Path of the persisted tour artifact.
    pub tour_path: PathBuf,
    /// Symbol IDs that were referenced but not found in the skeleton.
    pub dropped_ids: Vec<String>,
}

/// Plan a tour from an existing skeleton and codebase model.
///
/// Chapters are emitted in the default order: on-ramp, subsystems, infra, and
/// config. Tests are only included when [`PlanOptions::include_tests`] is set.
/// Every stop is validated against the real file before the artifact is
/// written; an invalid range is an error.
pub fn plan(
    repo_root: impl AsRef<Path>,
    config: &Config,
    skeleton: &Skeleton,
    model: &CodebaseModel,
    options: &PlanOptions,
) -> Result<PlanReport, PlanError> {
    let repo_root = repo_root.as_ref();
    let max_stops = config.budgets.max_stops_per_chapter;
    let mut builder = Builder::new(repo_root, skeleton, max_stops);
    let mut chapters = Vec::new();
    let mut dropped = BTreeSet::new();

    if let Some(chapter) = builder.on_ramp_chapter(skeleton)? {
        chapters.push(chapter);
    }
    chapters.extend(builder.subsystem_chapters(skeleton, model, &mut dropped)?);
    if let Some(chapter) = builder.infra_chapter(skeleton)? {
        chapters.push(chapter);
    }
    if options.include_config {
        if let Some(chapter) = builder.config_chapter(skeleton)? {
            chapters.push(chapter);
        }
    }
    if options.include_tests {
        if let Some(chapter) = builder.tests_chapter(skeleton)? {
            chapters.push(chapter);
        }
    }

    let tour = Tour {
        schema_version: SCHEMA_VERSION,
        tool_version: crate::VERSION.to_owned(),
        tour_id: tour_id(skeleton, options),
        source: TourSource::codebase(),
        model: model.model.clone(),
        prompt_version: narration_prompt_version(),
        redaction_profile: DEFAULT_REDACTION_PROFILE.to_owned(),
        position: Position::default(),
        chapters,
    };

    validate_tour(repo_root, &tour)?;
    let tour_path = write_tour(repo_root, &tour)?;

    Ok(PlanReport {
        tour,
        tour_path,
        dropped_ids: dropped.into_iter().collect(),
    })
}

/// Load the skeleton and model artifacts and plan a tour from them.
pub fn plan_from_artifacts(
    repo_root: impl AsRef<Path>,
    config: &Config,
    options: &PlanOptions,
) -> Result<PlanReport, PlanError> {
    let repo_root = repo_root.as_ref();
    let skeleton = load_skeleton(repo_root)?;
    let model = load_model(repo_root)?;
    plan(repo_root, config, &skeleton, &model, options)
}

/// Build a single stop anchored to `symbol` using the same range rules as a
/// planned tour.
///
/// This is used by `laudit explain`, which may need a stop for a symbol that is
/// not part of the current tour (for example a test symbol or a symbol outside
/// the configured budgets).
pub fn stop_for_symbol(
    repo_root: impl AsRef<Path>,
    skeleton: &Skeleton,
    symbol: &SymbolNode,
) -> Result<Stop, PlanError> {
    let builder = Builder::new(repo_root.as_ref(), skeleton, 0);
    builder.make_stop(symbol, "Requested symbol.")
}

/// Load a persisted tour artifact.
pub fn load_tour(repo_root: impl AsRef<Path>, tour_id: &str) -> Result<Tour, PlanError> {
    let path = tour_path(repo_root.as_ref(), tour_id);
    let bytes = fs::read(&path).map_err(|source| PlanError::Io {
        path: path.clone(),
        source,
    })?;
    let tour: Tour = serde_json::from_slice(&bytes)?;
    tour.validate_schema_version()
        .map_err(|error| PlanError::InvalidTour(error.to_string()))?;
    Ok(tour)
}

/// Persist a tour artifact under `.laudit/tours/`.
pub fn write_tour(repo_root: impl AsRef<Path>, tour: &Tour) -> Result<PathBuf, PlanError> {
    let path = tour_path(repo_root.as_ref(), &tour.tour_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| PlanError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let bytes = serde_json::to_vec_pretty(tour)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes).map_err(|source| PlanError::Io {
        path: temporary.clone(),
        source,
    })?;
    fs::rename(&temporary, &path).map_err(|source| PlanError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// The repository-relative path of a tour artifact.
#[must_use]
pub fn tour_path(repo_root: &Path, tour_id: &str) -> PathBuf {
    repo_root.join(TOURS_DIR).join(format!("{tour_id}.json"))
}

/// Validate that every stop in `tour` is anchored to a real file and range.
pub fn validate_tour(repo_root: impl AsRef<Path>, tour: &Tour) -> Result<(), PlanError> {
    let repo_root = repo_root.as_ref();
    for chapter in &tour.chapters {
        for stop in &chapter.stops {
            validate_stop(repo_root, stop)?;
        }
    }
    Ok(())
}

fn validate_stop(repo_root: &Path, stop: &Stop) -> Result<(), PlanError> {
    let path = repo_root.join(&stop.file);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(PlanError::InvalidRange {
                symbol_id: stop.symbol_id.clone(),
                file: stop.file.clone(),
                start_line: stop.start_line,
                end_line: stop.end_line,
                reason: "file not found".to_owned(),
            });
        }
        Err(source) => return Err(PlanError::Io { path, source }),
    };
    let text = String::from_utf8_lossy(&bytes);
    let line_count = u32::try_from(text.lines().count()).unwrap_or(u32::MAX);
    if stop.start_line == 0 || stop.end_line < stop.start_line || stop.end_line > line_count {
        return Err(PlanError::InvalidRange {
            symbol_id: stop.symbol_id.clone(),
            file: stop.file.clone(),
            start_line: stop.start_line,
            end_line: stop.end_line,
            reason: format!("file has {line_count} lines"),
        });
    }
    Ok(())
}

/// Load the persisted structural skeleton artifact.
pub fn load_skeleton(repo_root: &Path) -> Result<Skeleton, PlanError> {
    let path = repo_root.join(SKELETON_FILE);
    let bytes = fs::read(&path).map_err(|source| PlanError::Io {
        path: path.clone(),
        source,
    })?;
    let skeleton: Skeleton = serde_json::from_slice(&bytes)?;
    skeleton
        .validate_schema_version()
        .map_err(|error| PlanError::InvalidSkeleton(error.to_string()))?;
    Ok(skeleton)
}

/// Load the persisted codebase model artifact.
pub fn load_model(repo_root: &Path) -> Result<CodebaseModel, PlanError> {
    let path = repo_root.join(MODEL_FILE);
    let bytes = fs::read(&path).map_err(|source| PlanError::Io {
        path: path.clone(),
        source,
    })?;
    let model: CodebaseModel = serde_json::from_slice(&bytes)?;
    model
        .validate_schema_version()
        .map_err(|error| PlanError::InvalidModel(error.to_string()))?;
    Ok(model)
}

fn narration_prompt_version() -> String {
    match prompts::load(NARRATION) {
        Ok(prompt) => prompt.version.to_string(),
        Err(_) => prompts::version(NARRATION).map_or_else(|| "0".to_owned(), |v| v.to_string()),
    }
}

fn tour_id(skeleton: &Skeleton, options: &PlanOptions) -> String {
    let key = format!(
        "{}|{}|{}|{}|{}",
        skeleton.repo_id,
        skeleton.head.as_deref().unwrap_or(""),
        skeleton.dirty_hash,
        options.include_tests,
        options.include_config
    );
    let hash = crate::recon::content_hash(key.as_bytes());
    let hex = hash.strip_prefix("fnv1a128:").unwrap_or(&hash);
    let end = 16.min(hex.len());
    format!("tour-{}", &hex[..end])
}

struct Builder<'a> {
    max_stops: u32,
    symbols: BTreeMap<&'a str, &'a SymbolNode>,
    ranges: BTreeMap<&'a str, (u32, u32)>,
    line_counts: BTreeMap<PathBuf, u32>,
    file_hashes: BTreeMap<PathBuf, String>,
    seen: BTreeSet<String>,
}

impl<'a> Builder<'a> {
    fn new(repo_root: &'a Path, skeleton: &'a Skeleton, max_stops: u32) -> Self {
        let symbols: BTreeMap<&str, &SymbolNode> = skeleton
            .symbols
            .iter()
            .map(|symbol| (symbol.id.as_str(), symbol))
            .collect();

        let mut by_file: BTreeMap<&Path, Vec<&SymbolNode>> = BTreeMap::new();
        for symbol in &skeleton.symbols {
            by_file
                .entry(symbol.file.as_path())
                .or_default()
                .push(symbol);
        }

        let mut ranges = BTreeMap::new();
        let mut line_counts = BTreeMap::new();
        let mut file_hashes = BTreeMap::new();
        for (file, mut file_symbols) in by_file {
            file_symbols.sort_by(|a, b| {
                a.line
                    .cmp(&b.line)
                    .then_with(|| a.name.cmp(&b.name))
                    .then_with(|| a.id.cmp(&b.id))
            });
            if let Ok(bytes) = fs::read(repo_root.join(file)) {
                let text = String::from_utf8_lossy(&bytes);
                let count = u32::try_from(text.lines().count()).unwrap_or(u32::MAX);
                line_counts.insert(file.to_path_buf(), count);
                file_hashes.insert(file.to_path_buf(), crate::recon::content_hash(&bytes));
            }
            for (index, symbol) in file_symbols.iter().enumerate() {
                let start = symbol.line;
                let end = match file_symbols.get(index + 1) {
                    Some(next) if next.line > start => next.line - 1,
                    Some(_) => start,
                    None => line_counts.get(file).copied().unwrap_or(start),
                };
                let end = line_counts.get(file).map_or(end, |count| end.min(*count));
                ranges.insert(symbol.id.as_str(), (start, end));
            }
        }

        Self {
            max_stops,
            symbols,
            ranges,
            line_counts,
            file_hashes,
            seen: BTreeSet::new(),
        }
    }

    fn on_ramp_chapter(
        &mut self,
        skeleton: &'a Skeleton,
    ) -> Result<Option<TourChapter>, PlanError> {
        let mut stops = Vec::new();
        let mut entrypoints = skeleton.entrypoints.clone();
        entrypoints.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.file.cmp(&b.file))
                .then_with(|| a.line.cmp(&b.line))
        });

        for entrypoint in &entrypoints {
            if self.at_limit(stops.len()) {
                break;
            }
            if let Some(symbol) = symbol_for_entrypoint(skeleton, entrypoint) {
                if is_test_path(&symbol.file) {
                    continue;
                }
                self.push_symbol(&mut stops, symbol, "Entrypoint: the tour starts here.")?;
            }
        }

        if stops.is_empty() {
            for id in &skeleton.ranked_symbols {
                if stops.len() >= ON_RAMP_FALLBACK_STOPS || self.at_limit(stops.len()) {
                    break;
                }
                if let Some(symbol) = self.symbols.get(id.as_str()).copied() {
                    if is_test_path(&symbol.file) {
                        continue;
                    }
                    self.push_symbol(&mut stops, symbol, "A central symbol to orient the tour.")?;
                }
            }
        }

        Ok(chapter("on-ramp", "On-ramp", stops))
    }

    fn subsystem_chapters(
        &mut self,
        skeleton: &'a Skeleton,
        model: &CodebaseModel,
        dropped: &mut BTreeSet<String>,
    ) -> Result<Vec<TourChapter>, PlanError> {
        let subsystems = subsystem_views(skeleton, model);
        let mut chapters = Vec::new();

        for subsystem in subsystems {
            let mut ids = subsystem.symbol_ids.clone();
            if ids.is_empty() {
                for symbol in &skeleton.symbols {
                    if subsystem
                        .files
                        .iter()
                        .any(|file| Path::new(file) == symbol.file)
                    {
                        ids.push(symbol.id.clone());
                    }
                }
            }

            let mut symbols = Vec::new();
            for id in &ids {
                match self.symbols.get(id.as_str()).copied() {
                    Some(symbol) => symbols.push(symbol),
                    None => {
                        dropped.insert(id.clone());
                    }
                }
            }
            symbols.sort_by(|a, b| {
                b.rank
                    .partial_cmp(&a.rank)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.file.cmp(&b.file))
                    .then_with(|| a.line.cmp(&b.line))
                    .then_with(|| a.name.cmp(&b.name))
            });

            let mut stops = Vec::new();
            for symbol in symbols {
                if self.at_limit(stops.len()) {
                    break;
                }
                if is_test_path(&symbol.file) {
                    continue;
                }
                self.push_symbol(
                    &mut stops,
                    symbol,
                    &format!("Part of the {} subsystem.", subsystem.title),
                )?;
            }

            if let Some(chapter) = chapter(
                &format!("subsystem-{}", subsystem.id),
                &subsystem.title,
                stops,
            ) {
                chapters.push(chapter);
            }
        }

        Ok(chapters)
    }

    fn infra_chapter(&mut self, skeleton: &'a Skeleton) -> Result<Option<TourChapter>, PlanError> {
        let mut symbols: Vec<&SymbolNode> = skeleton
            .symbols
            .iter()
            .filter(|symbol| !is_test_path(&symbol.file) && is_infra_path(&symbol.file))
            .collect();
        symbols.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.name.cmp(&b.name))
        });

        let mut stops = Vec::new();
        for symbol in symbols {
            if self.at_limit(stops.len()) {
                break;
            }
            self.push_symbol(
                &mut stops,
                symbol,
                "Infrastructure that supports the codebase.",
            )?;
        }

        Ok(chapter("infra", "Infrastructure", stops))
    }

    fn config_chapter(&mut self, skeleton: &'a Skeleton) -> Result<Option<TourChapter>, PlanError> {
        let mut symbols: Vec<&SymbolNode> = skeleton
            .symbols
            .iter()
            .filter(|symbol| !is_test_path(&symbol.file) && is_config_path(&symbol.file))
            .collect();
        symbols.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.name.cmp(&b.name))
        });

        let mut stops = Vec::new();
        for symbol in symbols {
            if self.at_limit(stops.len()) {
                break;
            }
            self.push_symbol(
                &mut stops,
                symbol,
                "Configuration that shapes how the code runs.",
            )?;
        }

        Ok(chapter("config", "Configuration", stops))
    }

    fn tests_chapter(&mut self, skeleton: &'a Skeleton) -> Result<Option<TourChapter>, PlanError> {
        let mut symbols: Vec<&SymbolNode> = skeleton
            .symbols
            .iter()
            .filter(|symbol| is_test_path(&symbol.file))
            .collect();
        symbols.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.name.cmp(&b.name))
        });

        let mut stops = Vec::new();
        for symbol in symbols {
            if self.at_limit(stops.len()) {
                break;
            }
            self.push_symbol(&mut stops, symbol, "Test coverage for the codebase.")?;
        }

        Ok(chapter("tests", "Tests", stops))
    }

    fn push_symbol(
        &mut self,
        stops: &mut Vec<Stop>,
        symbol: &'a SymbolNode,
        why_here: &str,
    ) -> Result<(), PlanError> {
        if !self.seen.insert(symbol.id.clone()) {
            return Ok(());
        }
        let stop = self.make_stop(symbol, why_here)?;
        stops.push(stop);
        Ok(())
    }

    fn make_stop(&self, symbol: &SymbolNode, why_here: &str) -> Result<Stop, PlanError> {
        let Some(&line_count) = self.line_counts.get(&symbol.file) else {
            return Err(PlanError::InvalidRange {
                symbol_id: symbol.id.clone(),
                file: symbol.file.to_string_lossy().into_owned(),
                start_line: symbol.line,
                end_line: symbol.line,
                reason: "file not found or unreadable".to_owned(),
            });
        };
        let (start_line, end_line) = self
            .ranges
            .get(symbol.id.as_str())
            .copied()
            .unwrap_or((symbol.line, symbol.line));
        if start_line == 0 || end_line < start_line || end_line > line_count {
            return Err(PlanError::InvalidRange {
                symbol_id: symbol.id.clone(),
                file: symbol.file.to_string_lossy().into_owned(),
                start_line,
                end_line,
                reason: format!("file has {line_count} lines"),
            });
        }

        Ok(Stop {
            id: format!("stop-{}", symbol.id),
            file: symbol.file.to_string_lossy().into_owned(),
            start_line,
            end_line,
            symbol_id: symbol.id.clone(),
            title: symbol.name.clone(),
            why_here: why_here.to_owned(),
            explanation: String::new(),
            refs: Vec::new(),
            file_hash: self
                .file_hashes
                .get(&symbol.file)
                .cloned()
                .unwrap_or_default(),
            status: StopStatus::Pending,
        })
    }

    fn at_limit(&self, count: usize) -> bool {
        self.max_stops != 0 && count >= self.max_stops as usize
    }
}

struct SubsystemView {
    id: String,
    title: String,
    symbol_ids: Vec<String>,
    files: Vec<String>,
}

fn subsystem_views(skeleton: &Skeleton, model: &CodebaseModel) -> Vec<SubsystemView> {
    // Comprehension orders subsystems by dependency direction (see the
    // comprehension prompt), so planning preserves that order. When no model is
    // available, communities provide a deterministic fallback.
    if !model.subsystems.is_empty() {
        return model
            .subsystems
            .iter()
            .map(|subsystem| SubsystemView {
                id: subsystem.id.clone(),
                title: subsystem.title.clone(),
                symbol_ids: subsystem.symbol_ids.clone(),
                files: subsystem.files.clone(),
            })
            .collect();
    }

    if !skeleton.communities.is_empty() {
        return skeleton
            .communities
            .iter()
            .map(|community| SubsystemView {
                id: community.id.to_string(),
                title: format!("Subsystem {}", community.id),
                symbol_ids: community.symbols.clone(),
                files: community
                    .files
                    .iter()
                    .map(|file| file.to_string_lossy().into_owned())
                    .collect(),
            })
            .collect();
    }

    model
        .chapters
        .iter()
        .map(|chapter| SubsystemView {
            id: chapter.id.clone(),
            title: chapter.title.clone(),
            symbol_ids: chapter.stop_ids.clone(),
            files: Vec::new(),
        })
        .collect()
}

fn chapter(id: &str, title: &str, stops: Vec<Stop>) -> Option<TourChapter> {
    if stops.is_empty() {
        None
    } else {
        Some(TourChapter {
            id: id.to_owned(),
            title: title.to_owned(),
            stops,
        })
    }
}

fn symbol_for_entrypoint<'a>(
    skeleton: &'a Skeleton,
    entrypoint: &Entrypoint,
) -> Option<&'a SymbolNode> {
    let mut candidates: Vec<&SymbolNode> = skeleton
        .symbols
        .iter()
        .filter(|symbol| symbol.file == entrypoint.file)
        .collect();
    candidates.sort_by(|a, b| {
        a.line
            .cmp(&b.line)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    candidates
        .iter()
        .rev()
        .find(|symbol| symbol.line <= entrypoint.line)
        .copied()
        .or_else(|| candidates.first().copied())
}

fn is_test_path(path: &Path) -> bool {
    if path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .any(|component| {
            matches!(
                component.to_ascii_lowercase().as_str(),
                "test" | "tests" | "spec" | "specs" | "__tests__" | "benches"
            )
        })
    {
        return true;
    }

    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    matches!(name.as_str(), "test.rs" | "tests.rs")
        || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains("_spec.")
        || name.contains(".spec.")
}

fn is_config_path(path: &Path) -> bool {
    if is_infra_path(path) {
        return false;
    }
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "toml" | "yaml" | "yml" | "json"
    )
}

fn is_infra_path(path: &Path) -> bool {
    if path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .any(|component| {
            matches!(
                component.to_ascii_lowercase().as_str(),
                "scripts" | "ci" | ".github" | "deploy" | "docker" | "infra" | "ops" | "migrations"
            )
        })
    {
        return true;
    }

    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        if matches!(
            extension.to_ascii_lowercase().as_str(),
            "sh" | "bash" | "sql"
        ) {
            return true;
        }
    }

    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name.to_ascii_lowercase().as_str(),
        "dockerfile" | "makefile" | "justfile" | "build.rs" | "procfile"
    )
}

/// Errors produced by Stage 2 planning.
#[derive(Debug)]
pub enum PlanError {
    /// An artifact could not be read or written.
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// An artifact could not be serialized or parsed.
    Serialize(serde_json::Error),
    /// A stop was anchored to a file or range that does not exist.
    InvalidRange {
        /// Symbol the stop was anchored to.
        symbol_id: String,
        /// File containing the stop.
        file: String,
        /// First line of the stop.
        start_line: u32,
        /// Last line of the stop.
        end_line: u32,
        /// Why the range is invalid.
        reason: String,
    },
    /// The persisted skeleton is not usable.
    InvalidSkeleton(String),
    /// The persisted model is not usable.
    InvalidModel(String),
    /// The persisted tour is not usable.
    InvalidTour(String),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            Self::Serialize(error) => write!(f, "{error}"),
            Self::InvalidRange {
                symbol_id,
                file,
                start_line,
                end_line,
                reason,
            } => write!(
                f,
                "invalid stop range for {symbol_id} at {file}:{start_line}-{end_line}: {reason}"
            ),
            Self::InvalidSkeleton(message) => write!(f, "invalid skeleton: {message}"),
            Self::InvalidModel(message) => write!(f, "invalid model: {message}"),
            Self::InvalidTour(message) => write!(f, "invalid tour: {message}"),
        }
    }
}

impl std::error::Error for PlanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Serialize(error) => Some(error),
            Self::InvalidRange { .. }
            | Self::InvalidSkeleton(_)
            | Self::InvalidModel(_)
            | Self::InvalidTour(_) => None,
        }
    }
}

impl From<io::Error> for PlanError {
    fn from(source: io::Error) -> Self {
        Self::Io {
            path: PathBuf::from("."),
            source,
        }
    }
}

impl From<serde_json::Error> for PlanError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Subsystem, SCHEMA_VERSION};
    use crate::recon::{Community, EntrypointKind, FileNode};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("laudit-plan-test-{}-{id}", std::process::id()));
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

    fn config() -> Config {
        Config::load_from_offline(None, None).expect("offline config")
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

    fn skeleton(symbols: Vec<SymbolNode>, files: Vec<&str>) -> Skeleton {
        Skeleton {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            repo_id: "fnv1a128:abc".to_owned(),
            head: None,
            dirty_hash: "fnv1a128:def".to_owned(),
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
            entrypoints: vec![Entrypoint {
                file: PathBuf::from("src/main.rs"),
                line: 1,
                kind: EntrypointKind::RustMain,
                reason: "Rust binary crate root (main.rs)".to_owned(),
                score: 100,
            }],
            ranked_files: files.iter().map(|file| (*file).to_owned()).collect(),
            ranked_symbols: symbols.iter().map(|symbol| symbol.id.clone()).collect(),
            provenance: BTreeMap::new(),
        }
    }

    fn model(symbol_ids: Vec<String>) -> CodebaseModel {
        CodebaseModel {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            repo_id: "fnv1a128:abc".to_owned(),
            head: None,
            dirty_hash: "fnv1a128:def".to_owned(),
            generator: "laudit".to_owned(),
            model: "test-model".to_owned(),
            prompt_version: "1".to_owned(),
            generated_at: "2026-11-12T00:00:00Z".to_owned(),
            subsystems: vec![Subsystem {
                id: "core".to_owned(),
                title: "Core".to_owned(),
                summary: "Core subsystem.".to_owned(),
                symbol_ids,
                files: vec!["src/main.rs".to_owned()],
            }],
            chapters: Vec::new(),
            flows: Vec::new(),
            glossary: Vec::new(),
        }
    }

    #[test]
    fn stops_are_anchored_to_real_ranges() {
        let temp = TempDir::new();
        temp.write(
            "src/main.rs",
            "fn main() {\n    greet();\n}\n\nfn greet() {}\n",
        );
        let skeleton = skeleton(
            vec![
                symbol("sym-main", "src/main.rs", "main", 1),
                symbol("sym-greet", "src/main.rs", "greet", 5),
            ],
            vec!["src/main.rs"],
        );
        let model = model(vec!["sym-main".to_owned(), "sym-greet".to_owned()]);

        let report = plan(
            temp.path(),
            &config(),
            &skeleton,
            &model,
            &PlanOptions::default(),
        )
        .expect("plan");

        let stops: Vec<&Stop> = report
            .tour
            .chapters
            .iter()
            .flat_map(|chapter| chapter.stops.iter())
            .collect();
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0].file, "src/main.rs");
        assert_eq!(stops[0].start_line, 1);
        assert_eq!(stops[0].end_line, 4);
        assert_eq!(stops[1].start_line, 5);
        assert_eq!(stops[1].end_line, 5);
        assert!(report.tour_path.is_file());
    }

    #[test]
    fn tests_are_excluded_by_default_and_included_on_request() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        temp.write("tests/it.rs", "fn it_works() {}\n");
        let skeleton = skeleton(
            vec![
                symbol("sym-main", "src/main.rs", "main", 1),
                symbol("sym-test", "tests/it.rs", "it_works", 1),
            ],
            vec!["src/main.rs", "tests/it.rs"],
        );
        let model = model(vec!["sym-main".to_owned(), "sym-test".to_owned()]);

        let default = plan(
            temp.path(),
            &config(),
            &skeleton,
            &model,
            &PlanOptions::default(),
        )
        .expect("default plan");
        assert!(!default
            .tour
            .chapters
            .iter()
            .flat_map(|chapter| chapter.stops.iter())
            .any(|stop| stop.symbol_id == "sym-test"));

        let included = plan(
            temp.path(),
            &config(),
            &skeleton,
            &model,
            &PlanOptions {
                include_tests: true,
                include_config: true,
            },
        )
        .expect("plan with tests");
        assert!(included
            .tour
            .chapters
            .iter()
            .flat_map(|chapter| chapter.stops.iter())
            .any(|stop| stop.symbol_id == "sym-test"));
    }

    #[test]
    fn invalid_ranges_are_rejected() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let skeleton = skeleton(
            vec![symbol("sym-main", "src/main.rs", "main", 99)],
            vec!["src/main.rs"],
        );
        let model = model(vec!["sym-main".to_owned()]);

        let error = plan(
            temp.path(),
            &config(),
            &skeleton,
            &model,
            &PlanOptions::default(),
        )
        .expect_err("invalid range");
        assert!(matches!(error, PlanError::InvalidRange { .. }));
    }

    #[test]
    fn config_is_included_but_skippable() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        temp.write("config/app.toml", "[server]\nport = 8080\n");
        let skeleton = skeleton(
            vec![
                symbol("sym-main", "src/main.rs", "main", 1),
                symbol("sym-server", "config/app.toml", "server", 1),
            ],
            vec!["src/main.rs", "config/app.toml"],
        );
        let model = model(vec!["sym-main".to_owned()]);

        let included = plan(
            temp.path(),
            &config(),
            &skeleton,
            &model,
            &PlanOptions::default(),
        )
        .expect("plan");
        assert!(included
            .tour
            .chapters
            .iter()
            .any(|chapter| chapter.id == "config"));

        let skipped = plan(
            temp.path(),
            &config(),
            &skeleton,
            &model,
            &PlanOptions {
                include_tests: false,
                include_config: false,
            },
        )
        .expect("plan");
        assert!(!skipped
            .tour
            .chapters
            .iter()
            .any(|chapter| chapter.id == "config"));
    }

    #[test]
    fn budgets_limit_stops_per_chapter() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn a() {}\nfn b() {}\nfn c() {}\n");
        let skeleton = skeleton(
            vec![
                symbol("sym-a", "src/main.rs", "a", 1),
                symbol("sym-b", "src/main.rs", "b", 2),
                symbol("sym-c", "src/main.rs", "c", 3),
            ],
            vec!["src/main.rs"],
        );
        let model = model(vec![
            "sym-a".to_owned(),
            "sym-b".to_owned(),
            "sym-c".to_owned(),
        ]);
        let mut config = config();
        config.budgets.max_stops_per_chapter = 1;

        let report = plan(
            temp.path(),
            &config,
            &skeleton,
            &model,
            &PlanOptions::default(),
        )
        .expect("plan");
        for chapter in &report.tour.chapters {
            assert!(chapter.stops.len() <= 1);
        }
    }
}

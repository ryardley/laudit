//! Source adapters: build tours from changes rather than the whole codebase.
//!
//! Two adapters are provided:
//!
//! - [`SourceSpec::Uncommitted`] tours `git diff HEAD` plus untracked files.
//! - [`SourceSpec::Range`] tours a `base..head` commit range. The net diff is
//!   the default; `per_commit` expands the range into one chapter per commit.
//!
//! Both adapters produce ordered [`Stop`]s anchored to real files and line
//! ranges. A hunk that falls entirely inside a recon symbol range is anchored
//! to that symbol; other hunks become line-range stops. File order and hunk
//! order are preserved so the result is deterministic.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::build::{self, BuildError};
use crate::config::Config;
use crate::model::{
    CodebaseModel, Position, Skeleton, Stop, StopStatus, Tour, TourChapter, TourSource,
    SCHEMA_VERSION,
};
use crate::plan::{self, PlanError, DEFAULT_REDACTION_PROFILE};
use crate::recon::walk::DEFAULT_MAX_FILE_SIZE;
use crate::recon::{content_hash, SymbolNode};

/// A source of stops for a change-based tour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSpec {
    /// Uncommitted changes: `git diff HEAD` plus untracked files.
    Uncommitted,
    /// A commit range. The net diff is used unless `per_commit` is set.
    Range {
        /// Base revision (exclusive).
        base: String,
        /// Head revision (inclusive).
        head: String,
        /// Emit one chapter per commit instead of the net diff.
        per_commit: bool,
    },
}

impl SourceSpec {
    /// Parse a `base..head` range string.
    pub fn parse_range(value: &str, per_commit: bool) -> Result<Self, SourceError> {
        if value.contains("...") {
            return Err(SourceError::InvalidRange(value.to_owned()));
        }
        let (base, head) = value
            .split_once("..")
            .ok_or_else(|| SourceError::InvalidRange(value.to_owned()))?;
        if base.is_empty() || head.is_empty() || head.contains("..") {
            return Err(SourceError::InvalidRange(value.to_owned()));
        }
        Ok(Self::Range {
            base: base.to_owned(),
            head: head.to_owned(),
            per_commit,
        })
    }
}

/// A summary of a source-adapter planning run.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceReport {
    /// The planned tour.
    pub tour: Tour,
    /// Path of the persisted tour artifact.
    pub tour_path: PathBuf,
    /// Human-readable warnings produced while planning.
    pub warnings: Vec<String>,
}

/// Plan a change-based tour from an existing skeleton and codebase model.
pub fn plan(
    repo_root: impl AsRef<Path>,
    _config: &Config,
    skeleton: &Skeleton,
    model: &CodebaseModel,
    spec: &SourceSpec,
) -> Result<SourceReport, SourceError> {
    let repo_root = repo_root.as_ref();
    let mut warnings = Vec::new();

    let chapters = match spec {
        SourceSpec::Uncommitted => {
            let diffs = uncommitted_diffs(repo_root)?;
            let stops = stops_from_diffs(repo_root, skeleton, &diffs, "uncommitted", &mut warnings);
            chapter("uncommitted", "Uncommitted changes", stops)
                .into_iter()
                .collect()
        }
        SourceSpec::Range {
            base,
            head,
            per_commit: false,
        } => {
            let diffs = range_diffs(repo_root, base, head)?;
            let stops = stops_from_diffs(repo_root, skeleton, &diffs, "range", &mut warnings);
            chapter("range", &format!("Changes {base}..{head}"), stops)
                .into_iter()
                .collect()
        }
        SourceSpec::Range {
            base,
            head,
            per_commit: true,
        } => per_commit_chapters(repo_root, skeleton, base, head, &mut warnings)?,
    };

    if chapters.is_empty() {
        return Err(SourceError::NoChanges);
    }

    let tour = Tour {
        schema_version: SCHEMA_VERSION,
        tool_version: crate::VERSION.to_owned(),
        tour_id: tour_id(skeleton, spec),
        source: source_for(spec),
        model: model.model.clone(),
        prompt_version: plan::narration_prompt_version(),
        redaction_profile: DEFAULT_REDACTION_PROFILE.to_owned(),
        position: Position::default(),
        chapters,
    };

    plan::validate_tour(repo_root, &tour)?;
    let tour_path = plan::write_tour(repo_root, &tour)?;

    Ok(SourceReport {
        tour,
        tour_path,
        warnings,
    })
}

/// Load the skeleton and model artifacts and plan a change-based tour.
///
/// A missing skeleton is produced by running recon; a missing model falls back
/// to the offline placeholder model.
pub fn plan_from_artifacts(
    repo_root: impl AsRef<Path>,
    config: &Config,
    spec: &SourceSpec,
) -> Result<SourceReport, SourceError> {
    let repo_root = repo_root.as_ref();
    let skeleton = load_or_recon_skeleton(repo_root, config)?;
    let model = load_or_offline_model(repo_root, &skeleton)?;
    plan(repo_root, config, &skeleton, &model, spec)
}

fn load_or_recon_skeleton(repo_root: &Path, config: &Config) -> Result<Skeleton, SourceError> {
    match plan::load_skeleton(repo_root) {
        Ok(skeleton) => Ok(skeleton),
        Err(PlanError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            let recon = build::recon(repo_root, config)?;
            build::write_skeleton(repo_root, &recon.skeleton)?;
            Ok(recon.skeleton)
        }
        Err(error) => Err(SourceError::Plan(error)),
    }
}

fn load_or_offline_model(
    repo_root: &Path,
    skeleton: &Skeleton,
) -> Result<CodebaseModel, SourceError> {
    match plan::load_model(repo_root) {
        Ok(model) => Ok(model),
        Err(PlanError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            Ok(build::offline_model(skeleton))
        }
        Err(error) => Err(SourceError::Plan(error)),
    }
}

fn source_for(spec: &SourceSpec) -> TourSource {
    match spec {
        SourceSpec::Uncommitted => TourSource::uncommitted(),
        SourceSpec::Range { .. } => TourSource::range(),
    }
}

fn tour_id(skeleton: &Skeleton, spec: &SourceSpec) -> String {
    let key = match spec {
        SourceSpec::Uncommitted => format!(
            "uncommitted|{}|{}",
            skeleton.head.as_deref().unwrap_or(""),
            skeleton.dirty_hash
        ),
        SourceSpec::Range {
            base,
            head,
            per_commit,
        } => format!(
            "range|{base}|{head}|{per_commit}|{}|{}",
            skeleton.head.as_deref().unwrap_or(""),
            skeleton.dirty_hash
        ),
    };
    let hash = content_hash(key.as_bytes());
    let hex = hash.strip_prefix("fnv1a128:").unwrap_or(&hash);
    let end = 16.min(hex.len());
    format!("tour-{}", &hex[..end])
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

fn per_commit_chapters(
    repo_root: &Path,
    skeleton: &Skeleton,
    base: &str,
    head: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<TourChapter>, SourceError> {
    ensure_git_repo(repo_root)?;
    let range = format!("{base}..{head}");
    let log = run_git(
        repo_root,
        &[
            "log",
            "--reverse",
            "--no-color",
            "--format=%H%x00%s",
            &range,
        ],
    )?;

    let mut chapters = Vec::new();
    for line in log.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.splitn(2, '\0');
        let sha = fields.next().unwrap_or_default();
        let subject = fields.next().unwrap_or_default();
        if sha.is_empty() {
            continue;
        }

        let output = run_git(
            repo_root,
            &[
                "show",
                "--format=",
                "--no-color",
                "--no-ext-diff",
                "--unified=0",
                "--no-prefix",
                sha,
            ],
        )?;
        let diffs = parse_diff(&output);
        let stops = stops_from_diffs(repo_root, skeleton, &diffs, sha, warnings);
        let short = &sha[..sha.len().min(8)];
        if let Some(chapter) = chapter(
            &format!("commit-{short}"),
            &format!("{short} {subject}"),
            stops,
        ) {
            chapters.push(chapter);
        }
    }

    Ok(chapters)
}

fn uncommitted_diffs(repo_root: &Path) -> Result<Vec<FileDiff>, SourceError> {
    ensure_git_repo(repo_root)?;
    let mut diffs = Vec::new();

    if crate::recon::head(repo_root).is_some() {
        let output = run_git(
            repo_root,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--unified=0",
                "--no-prefix",
                "HEAD",
                "--",
            ],
        )?;
        diffs.extend(parse_diff(&output));
    } else {
        let unstaged = run_git(
            repo_root,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--unified=0",
                "--no-prefix",
                "--",
            ],
        )?;
        diffs.extend(parse_diff(&unstaged));
        let staged = run_git(
            repo_root,
            &[
                "diff",
                "--cached",
                "--no-color",
                "--no-ext-diff",
                "--unified=0",
                "--no-prefix",
                "--",
            ],
        )?;
        diffs.extend(parse_diff(&staged));
    }

    let untracked = run_git(repo_root, &["ls-files", "--others", "--exclude-standard"])?;
    let mut paths: Vec<&str> = untracked
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .collect();
    paths.sort_unstable();
    for path in paths {
        if should_skip_untracked(path) {
            continue;
        }
        if let Some(diff) = untracked_file_diff(repo_root, path) {
            diffs.push(diff);
        }
    }

    Ok(diffs)
}

fn range_diffs(repo_root: &Path, base: &str, head: &str) -> Result<Vec<FileDiff>, SourceError> {
    ensure_git_repo(repo_root)?;
    let range = format!("{base}..{head}");
    let output = run_git(
        repo_root,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--unified=0",
            "--no-prefix",
            &range,
            "--",
        ],
    )?;
    Ok(parse_diff(&output))
}

fn should_skip_untracked(path: &str) -> bool {
    let path = Path::new(path);
    if path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .any(|component| {
            matches!(
                component,
                ".laudit" | "node_modules" | "target" | "vendor" | "dist"
            )
        })
    {
        return true;
    }
    false
}

fn untracked_file_diff(repo_root: &Path, path: &str) -> Option<FileDiff> {
    let full = repo_root.join(path);
    let metadata = fs::metadata(&full).ok()?;
    if !metadata.is_file() || metadata.len() > DEFAULT_MAX_FILE_SIZE {
        return None;
    }
    let bytes = fs::read(&full).ok()?;
    if bytes.iter().take(8 * 1024).any(|&byte| byte == 0) {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    let line_count = u32::try_from(text.lines().count()).unwrap_or(u32::MAX);
    if line_count == 0 {
        return None;
    }
    Some(FileDiff {
        path: PathBuf::from(path),
        hunks: vec![Hunk {
            start: 1,
            end: line_count,
        }],
    })
}

fn stops_from_diffs(
    repo_root: &Path,
    skeleton: &Skeleton,
    diffs: &[FileDiff],
    scope: &str,
    warnings: &mut Vec<String>,
) -> Vec<Stop> {
    let symbols = SymbolRanges::new(repo_root, skeleton);
    let mut stops = Vec::new();
    let mut seen = BTreeSet::new();

    for diff in diffs {
        let file = diff.path.to_string_lossy().into_owned();
        let line_count = read_line_count(repo_root, &diff.path);
        if line_count == 0 {
            warnings.push(format!("skipped {file}: file is missing or empty"));
            continue;
        }
        let file_hash = read_file_hash(repo_root, &diff.path).unwrap_or_default();

        for hunk in &diff.hunks {
            let start = hunk.start.max(1);
            let end = hunk.end.max(start).min(line_count);
            if start > line_count || start > end {
                continue;
            }

            let symbol = symbols.enclosing(&diff.path, start, end);
            let symbol_id = symbol.map(|symbol| symbol.id.clone()).unwrap_or_default();
            let title = symbol.map_or_else(
                || format!("{file}:{start}-{end}"),
                |symbol| symbol.name.clone(),
            );
            let why_here = symbol.map_or_else(
                || "Changed lines outside a known symbol.".to_owned(),
                |symbol| format!("Changed lines in `{}`.", symbol.name),
            );
            let id = stop_id(scope, &file, start, end, &symbol_id);
            if !seen.insert(id.clone()) {
                continue;
            }

            stops.push(Stop {
                id,
                file: file.clone(),
                start_line: start,
                end_line: end,
                symbol_id,
                title,
                why_here,
                explanation: String::new(),
                refs: Vec::new(),
                file_hash: file_hash.clone(),
                status: StopStatus::Pending,
            });
        }
    }

    stops
}

fn stop_id(scope: &str, file: &str, start: u32, end: u32, symbol_id: &str) -> String {
    let key = format!("{scope}|{file}|{start}|{end}|{symbol_id}");
    let hash = content_hash(key.as_bytes());
    let hex = hash.strip_prefix("fnv1a128:").unwrap_or(&hash);
    let end = 16.min(hex.len());
    format!("stop-{}", &hex[..end])
}

fn read_line_count(repo_root: &Path, file: &Path) -> u32 {
    match fs::read(repo_root.join(file)) {
        Ok(bytes) => {
            u32::try_from(String::from_utf8_lossy(&bytes).lines().count()).unwrap_or(u32::MAX)
        }
        Err(_) => 0,
    }
}

fn read_file_hash(repo_root: &Path, file: &Path) -> Option<String> {
    fs::read(repo_root.join(file))
        .ok()
        .map(|bytes| content_hash(&bytes))
}

fn ensure_git_repo(repo_root: &Path) -> Result<(), SourceError> {
    match run_git(repo_root, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(output) if output.trim() == "true" => Ok(()),
        Ok(_) | Err(SourceError::Git { .. }) => {
            Err(SourceError::NotGitRepository(repo_root.to_path_buf()))
        }
        Err(error) => Err(error),
    }
}

fn run_git(repo_root: &Path, args: &[&str]) -> Result<String, SourceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["--no-pager", "-c", "core.quotepath=false"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .output()
        .map_err(|source| SourceError::Io {
            path: repo_root.to_path_buf(),
            source,
        })?;

    if !output.status.success() {
        return Err(SourceError::Git {
            command: format!("git {}", args.join(" ")),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileDiff {
    path: PathBuf,
    hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hunk {
    start: u32,
    end: u32,
}

fn parse_diff(output: &str) -> Vec<FileDiff> {
    let mut diffs = Vec::new();
    let mut current: Option<FileDiff> = None;
    let mut in_hunk = false;

    for line in output.lines() {
        if line.starts_with("diff --git ") {
            if let Some(diff) = current.take() {
                if !diff.hunks.is_empty() {
                    diffs.push(diff);
                }
            }
            in_hunk = false;
            continue;
        }

        if !in_hunk {
            if let Some(rest) = line.strip_prefix("+++ ") {
                let path = rest.trim();
                if path == "/dev/null" {
                    current = None;
                } else {
                    let path = path.strip_prefix("b/").unwrap_or(path);
                    let path = unquote_path(path);
                    current = Some(FileDiff {
                        path: PathBuf::from(path),
                        hunks: Vec::new(),
                    });
                }
                continue;
            }
        }

        if let Some(rest) = line.strip_prefix("@@ ") {
            if let Some(diff) = current.as_mut() {
                if let Some(hunk) = parse_hunk(rest) {
                    diff.hunks.push(hunk);
                }
            }
            in_hunk = true;
        }
    }

    if let Some(diff) = current.take() {
        if !diff.hunks.is_empty() {
            diffs.push(diff);
        }
    }

    diffs
}

fn parse_hunk(rest: &str) -> Option<Hunk> {
    let plus = rest
        .split_whitespace()
        .find(|part| part.starts_with('+'))?
        .trim_start_matches('+');
    let mut fields = plus.split(',');
    let start: u32 = fields.next()?.parse().ok()?;
    let count: u32 = fields.next().map_or(1, |value| value.parse().unwrap_or(1));
    let start = start.max(1);
    let end = if count == 0 {
        start
    } else {
        start.saturating_add(count - 1)
    };
    Some(Hunk { start, end })
}

fn unquote_path(path: &str) -> String {
    if !(path.starts_with('"') && path.ends_with('"') && path.len() >= 2) {
        return path.to_owned();
    }

    let inner = &path[1..path.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

struct SymbolRange {
    id: String,
    name: String,
    start: u32,
    end: u32,
}

struct SymbolRanges {
    by_file: BTreeMap<PathBuf, Vec<SymbolRange>>,
}

impl SymbolRanges {
    fn new(repo_root: &Path, skeleton: &Skeleton) -> Self {
        let mut by_file: BTreeMap<PathBuf, Vec<&SymbolNode>> = BTreeMap::new();
        for symbol in &skeleton.symbols {
            by_file.entry(symbol.file.clone()).or_default().push(symbol);
        }

        let mut ranges = BTreeMap::new();
        for (file, mut symbols) in by_file {
            symbols.sort_by(|a, b| {
                a.line
                    .cmp(&b.line)
                    .then_with(|| a.name.cmp(&b.name))
                    .then_with(|| a.id.cmp(&b.id))
            });
            let line_count = read_line_count(repo_root, &file);
            let mut file_ranges = Vec::new();
            for (index, symbol) in symbols.iter().enumerate() {
                let start = symbol.line;
                let end = match symbols.get(index + 1) {
                    Some(next) if next.line > start => next.line - 1,
                    Some(_) => start,
                    None => line_count,
                };
                let end = if line_count > 0 {
                    end.min(line_count)
                } else {
                    end
                };
                if start == 0 || end < start {
                    continue;
                }
                file_ranges.push(SymbolRange {
                    id: symbol.id.clone(),
                    name: symbol.name.clone(),
                    start,
                    end,
                });
            }
            ranges.insert(file, file_ranges);
        }

        Self { by_file: ranges }
    }

    fn enclosing(&self, file: &Path, start: u32, end: u32) -> Option<&SymbolRange> {
        self.by_file
            .get(file)?
            .iter()
            .filter(|range| range.start <= start && range.end >= end)
            .min_by_key(|range| range.end - range.start)
    }
}

/// Errors produced by a source adapter.
#[derive(Debug)]
pub enum SourceError {
    /// The repository is not a git work tree.
    NotGitRepository(PathBuf),
    /// A git command failed.
    Git {
        /// Command that failed.
        command: String,
        /// Git's error output.
        message: String,
    },
    /// A file could not be read.
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// The range string was not `base..head`.
    InvalidRange(String),
    /// There were no changes to tour.
    NoChanges,
    /// Recon or artifact writing failed.
    Build(BuildError),
    /// Planning or artifact writing failed.
    Plan(PlanError),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotGitRepository(path) => {
                write!(f, "not a git repository: {}", path.display())
            }
            Self::Git { command, message } => {
                write!(f, "{command} failed: {message}")
            }
            Self::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            Self::InvalidRange(value) => {
                write!(f, "invalid range {value:?}; expected <base>..<head>")
            }
            Self::NoChanges => write!(f, "no changes to tour"),
            Self::Build(error) => write!(f, "{error}"),
            Self::Plan(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Build(error) => Some(error),
            Self::Plan(error) => Some(error),
            Self::NotGitRepository(_)
            | Self::Git { .. }
            | Self::InvalidRange(_)
            | Self::NoChanges => None,
        }
    }
}

impl From<BuildError> for SourceError {
    fn from(error: BuildError) -> Self {
        Self::Build(error)
    }
}

impl From<PlanError> for SourceError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_range_strings() {
        assert_eq!(
            SourceSpec::parse_range("main..feature", false).expect("range"),
            SourceSpec::Range {
                base: "main".to_owned(),
                head: "feature".to_owned(),
                per_commit: false,
            }
        );
        assert!(SourceSpec::parse_range("main", false).is_err());
        assert!(SourceSpec::parse_range("..head", false).is_err());
        assert!(SourceSpec::parse_range("base..", false).is_err());
        assert!(SourceSpec::parse_range("base...head", false).is_err());
    }

    #[test]
    fn parses_hunk_headers() {
        assert_eq!(
            parse_hunk("-1,3 +1,4 @@ fn main()"),
            Some(Hunk { start: 1, end: 4 })
        );
        assert_eq!(parse_hunk("-0,0 +1,2 @@"), Some(Hunk { start: 1, end: 2 }));
        assert_eq!(parse_hunk("-5,2 +5,0 @@"), Some(Hunk { start: 5, end: 5 }));
        assert_eq!(parse_hunk("not a hunk"), None);
    }

    #[test]
    fn parses_file_diffs_and_ignores_content_that_looks_like_headers() {
        let output = "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,3 @@
 fn main() {}
+++ this is added content
@@ -10,1 +11,1 @@
-old
+new
";
        let diffs = parse_diff(output);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, PathBuf::from("src/lib.rs"));
        assert_eq!(
            diffs[0].hunks,
            vec![Hunk { start: 1, end: 3 }, Hunk { start: 11, end: 11 }]
        );
    }

    #[test]
    fn deleted_files_are_skipped() {
        let output = "\
diff --git a/src/old.rs b/src/old.rs
--- a/src/old.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-fn old() {}
";
        assert!(parse_diff(output).is_empty());
    }
}

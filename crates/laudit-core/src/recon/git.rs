//! Git provenance mining.
//!
//! Provenance explains why code exists (D9): how old a region is, who has
//! touched it, which commits changed it recently, and how much churn it has
//! seen. The miner shells out to the `git` CLI so the engine does not link
//! libgit2. Every operation degrades gracefully: a non-git directory, an
//! untracked file, or a missing `git` binary yields an empty [`Provenance`]
//! rather than an error.
//!
//! Output is bounded. At most [`MAX_RECENT_COMMITS`] subjects are retained,
//! each subject is truncated to [`MAX_SUBJECT_CHARS`] characters, and churn is
//! aggregated over at most [`MAX_CHURN_COMMITS`] commits. Blame is reduced to a
//! summary, so the struct never grows with file size.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::walk::SourceFile;

/// Maximum number of recent commit subjects retained per file or region.
pub const MAX_RECENT_COMMITS: usize = 5;
/// Maximum number of characters retained from a commit subject.
pub const MAX_SUBJECT_CHARS: usize = 120;
/// Maximum number of commits considered when computing churn.
pub const MAX_CHURN_COMMITS: usize = 100;

/// A 1-based, inclusive line range within a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    /// First line of the range, 1-based.
    pub start: u32,
    /// Last line of the range, 1-based and inclusive.
    pub end: u32,
}

impl LineRange {
    /// Create a line range.
    #[must_use]
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// Whether the range is non-empty and 1-based.
    #[must_use]
    pub fn is_valid(self) -> bool {
        self.start >= 1 && self.end >= self.start
    }
}

/// Bounded provenance for a file or a region within a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// File the provenance describes, relative to the repository root.
    pub file: PathBuf,
    /// Region within the file, or `None` for the whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<LineRange>,
    /// Blame summary for the file or region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blame: Option<BlameSummary>,
    /// Most recent commits touching the file or region, newest first.
    #[serde(default)]
    pub recent_commits: Vec<CommitSummary>,
    /// Aggregated churn for the file or region.
    #[serde(default)]
    pub churn: Churn,
}

impl Provenance {
    /// An empty provenance record, used when git data is unavailable.
    #[must_use]
    pub fn empty(file: impl Into<PathBuf>, region: Option<LineRange>) -> Self {
        Self {
            file: file.into(),
            region,
            blame: None,
            recent_commits: Vec::new(),
            churn: Churn::default(),
        }
    }

    /// Whether no provenance signals were collected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blame.is_none() && self.recent_commits.is_empty() && self.churn == Churn::default()
    }
}

/// Aggregate blame information for a file or region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlameSummary {
    /// Commit that last touched the oldest line.
    pub oldest_commit: String,
    /// Commit that last touched the newest line.
    pub newest_commit: String,
    /// Unix timestamp of the oldest line's commit.
    pub oldest_timestamp: i64,
    /// Unix timestamp of the newest line's commit.
    pub newest_timestamp: i64,
    /// Age of the oldest line in days, relative to the time of mining.
    pub oldest_age_days: u64,
    /// Age of the newest line in days, relative to the time of mining.
    pub newest_age_days: u64,
    /// Number of distinct authors (by email) that touched the region.
    pub author_count: usize,
    /// Number of blamed lines.
    pub line_count: u32,
}

/// A commit that touched a file or region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitSummary {
    /// Full commit SHA.
    pub sha: String,
    /// Commit subject, truncated to [`MAX_SUBJECT_CHARS`] characters.
    pub subject: String,
    /// Unix timestamp of the commit.
    pub timestamp: i64,
}

/// Aggregated line churn for a file or region.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Churn {
    /// Number of commits considered.
    pub commits: u32,
    /// Number of inserted lines.
    pub insertions: u32,
    /// Number of deleted lines.
    pub deletions: u32,
}

/// Mine provenance for a file or an optional region within it.
///
/// Returns an empty [`Provenance`] when `repo_root` is not a git work tree, the
/// file is untracked, the region is invalid, or git is unavailable.
#[must_use]
pub fn mine(
    repo_root: impl AsRef<Path>,
    file: impl AsRef<Path>,
    region: Option<LineRange>,
) -> Provenance {
    let repo_root = repo_root.as_ref();
    let file = relative_path(repo_root, file.as_ref());
    let mut provenance = Provenance::empty(file.clone(), region);

    if region.is_some_and(|region| !region.is_valid()) {
        return provenance;
    }
    if !is_git_repo(repo_root) {
        return provenance;
    }

    provenance.blame = blame(repo_root, &file, region);
    provenance.recent_commits = recent_commits(repo_root, &file, region);
    provenance.churn = churn(repo_root, &file, region);
    provenance
}

/// Return the current `HEAD` commit SHA, if `repo_root` is a git work tree.
///
/// Returns `None` for a non-git directory, an empty repository, or when git is
/// unavailable.
#[must_use]
pub fn head(repo_root: impl AsRef<Path>) -> Option<String> {
    let repo_root = repo_root.as_ref();
    if !is_git_repo(repo_root) {
        return None;
    }
    run_git(repo_root, &["rev-parse", "--verify", "HEAD"])
        .map(|output| output.trim().to_owned())
        .filter(|sha| !sha.is_empty())
}

/// Mine provenance for a whole file.
#[must_use]
pub fn mine_file(repo_root: impl AsRef<Path>, file: impl AsRef<Path>) -> Provenance {
    mine(repo_root, file, None)
}

/// Mine provenance for a 1-based, inclusive line region within a file.
#[must_use]
pub fn mine_region(
    repo_root: impl AsRef<Path>,
    file: impl AsRef<Path>,
    start_line: u32,
    end_line: u32,
) -> Provenance {
    mine(repo_root, file, Some(LineRange::new(start_line, end_line)))
}

/// Mine provenance for every file in `files`.
#[must_use]
pub fn mine_files(
    repo_root: impl AsRef<Path>,
    files: &[SourceFile],
) -> BTreeMap<PathBuf, Provenance> {
    let repo_root = repo_root.as_ref();
    files
        .iter()
        .map(|file| (file.path.clone(), mine_file(repo_root, &file.path)))
        .collect()
}

fn relative_path(repo_root: &Path, file: &Path) -> PathBuf {
    let file = if file.is_absolute() {
        file.strip_prefix(repo_root).unwrap_or(file)
    } else {
        file
    };

    let mut normalized = PathBuf::new();
    for component in file.components() {
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

fn is_git_repo(repo_root: &Path) -> bool {
    run_git(repo_root, &["rev-parse", "--is-inside-work-tree"])
        .is_some_and(|output| output.trim() == "true")
}

fn run_git(repo_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["--no-pager", "-c", "core.quotepath=false"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn blame(repo_root: &Path, file: &Path, region: Option<LineRange>) -> Option<BlameSummary> {
    let file = file.to_string_lossy();
    let range = region.map(|region| format!("-L{},{}", region.start, region.end));
    let mut args = vec!["blame", "--line-porcelain"];
    if let Some(range) = &range {
        args.push(range);
    }
    args.push("--");
    args.push(&file);

    let output = run_git(repo_root, &args)?;
    parse_blame(&output, now_seconds())
}

fn parse_blame(output: &str, now: i64) -> Option<BlameSummary> {
    let mut entries: Vec<BlameEntry> = Vec::new();
    let mut current: Option<BlameEntry> = None;

    for line in output.lines() {
        if is_blame_header(line) {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            let sha = line
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_start_matches('^')
                .to_owned();
            current = Some(BlameEntry {
                sha,
                author: String::new(),
                timestamp: 0,
            });
            continue;
        }

        let Some(entry) = current.as_mut() else {
            continue;
        };
        if let Some(author) = line.strip_prefix("author-mail ") {
            entry.author = author.trim().to_owned();
        } else if let Some(timestamp) = line.strip_prefix("author-time ") {
            entry.timestamp = timestamp.trim().parse().unwrap_or(0);
        }
    }
    if let Some(entry) = current.take() {
        entries.push(entry);
    }

    if entries.is_empty() {
        return None;
    }

    let mut authors = BTreeSet::new();
    let mut oldest: Option<&BlameEntry> = None;
    let mut newest: Option<&BlameEntry> = None;
    for entry in &entries {
        if !entry.author.is_empty() {
            authors.insert(entry.author.as_str());
        }
        if oldest.is_none_or(|oldest| entry.timestamp < oldest.timestamp) {
            oldest = Some(entry);
        }
        if newest.is_none_or(|newest| entry.timestamp > newest.timestamp) {
            newest = Some(entry);
        }
    }
    let oldest = oldest?;
    let newest = newest?;

    Some(BlameSummary {
        oldest_commit: oldest.sha.clone(),
        newest_commit: newest.sha.clone(),
        oldest_timestamp: oldest.timestamp,
        newest_timestamp: newest.timestamp,
        oldest_age_days: age_days(now, oldest.timestamp),
        newest_age_days: age_days(now, newest.timestamp),
        author_count: authors.len(),
        line_count: u32::try_from(entries.len()).unwrap_or(u32::MAX),
    })
}

fn is_blame_header(line: &str) -> bool {
    let mut parts = line.split_whitespace();
    let Some(sha) = parts.next() else {
        return false;
    };
    let sha = sha.trim_start_matches('^');
    if !is_sha(sha) {
        return false;
    }
    let Some(_original_line) = parts.next() else {
        return false;
    };
    let Some(_final_line) = parts.next() else {
        return false;
    };
    let Some(_line_count) = parts.next() else {
        return false;
    };
    parts.next().is_none()
}

fn recent_commits(repo_root: &Path, file: &Path, region: Option<LineRange>) -> Vec<CommitSummary> {
    let file = file.to_string_lossy();
    let mut args = vec!["log", "-z", "--no-color", "--format=%H%x00%ct%x00%s"];

    if let Some(region) = region {
        let range = format!("-L{},{}:{}", region.start, region.end, file);
        args.push(&range);
        args.push("--");
        let Some(output) = run_git(repo_root, &args) else {
            return Vec::new();
        };
        parse_log_headers(&output, MAX_RECENT_COMMITS)
    } else {
        args.push("--");
        args.push(&file);
        let Some(output) = run_git(repo_root, &args) else {
            return Vec::new();
        };
        parse_log_tokens(&output, MAX_RECENT_COMMITS)
    }
}

fn parse_log_tokens(output: &str, limit: usize) -> Vec<CommitSummary> {
    let mut commits = Vec::new();
    let mut tokens = output.split('\0');

    while let Some(sha) = tokens.next() {
        let sha = sha.trim_start_matches('\n');
        if sha.is_empty() {
            continue;
        }
        let Some(timestamp) = tokens.next() else {
            break;
        };
        let Some(subject) = tokens.next() else {
            break;
        };
        let Ok(timestamp) = timestamp.trim().parse::<i64>() else {
            continue;
        };
        commits.push(CommitSummary {
            sha: sha.to_owned(),
            subject: truncate_subject(subject),
            timestamp,
        });
        if commits.len() >= limit {
            break;
        }
    }

    commits
}

fn parse_log_headers(output: &str, limit: usize) -> Vec<CommitSummary> {
    let mut commits = Vec::new();

    for line in output.lines() {
        if !line.contains('\0') {
            continue;
        }
        let mut fields = line.split('\0');
        let sha = fields.next().unwrap_or_default().trim_start_matches('\n');
        let timestamp = fields
            .next()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .unwrap_or(0);
        let subject = fields.next().unwrap_or_default();
        commits.push(CommitSummary {
            sha: sha.to_owned(),
            subject: truncate_subject(subject),
            timestamp,
        });
        if commits.len() >= limit {
            break;
        }
    }

    commits
}

fn churn(repo_root: &Path, file: &Path, region: Option<LineRange>) -> Churn {
    let file = file.to_string_lossy();

    if let Some(region) = region {
        let range = format!("-L{},{}:{}", region.start, region.end, file);
        let mut args = vec!["log", "-z", "--no-color", "--format=%H%x00%ct%x00%s"];
        args.push(&range);
        args.push("--");
        let Some(output) = run_git(repo_root, &args) else {
            return Churn::default();
        };
        return parse_log_with_diffs(&output, MAX_CHURN_COMMITS).1;
    }

    let mut args = vec![
        "log",
        "-z",
        "--numstat",
        "--no-color",
        "--format=%H%x00%ct%x00%s",
        "--",
    ];
    args.push(&file);
    let Some(output) = run_git(repo_root, &args) else {
        return Churn::default();
    };
    parse_numstat(&output, MAX_CHURN_COMMITS)
}

fn parse_log_with_diffs(output: &str, limit: usize) -> (Vec<CommitSummary>, Churn) {
    let mut commits = Vec::new();
    let mut churn = Churn::default();
    let mut current: Option<CommitSummary> = None;
    let mut counting = false;

    for line in output.lines() {
        if line.contains('\0') {
            if let Some(commit) = current.take() {
                commits.push(commit);
            }
            if commits.len() >= limit {
                break;
            }

            let mut fields = line.split('\0');
            let sha = fields.next().unwrap_or_default().trim_start_matches('\n');
            let timestamp = fields
                .next()
                .and_then(|value| value.trim().parse::<i64>().ok())
                .unwrap_or(0);
            let subject = fields.next().unwrap_or_default();
            current = Some(CommitSummary {
                sha: sha.to_owned(),
                subject: truncate_subject(subject),
                timestamp,
            });
            churn.commits = churn.commits.saturating_add(1);
            counting = true;
            continue;
        }

        if !counting {
            continue;
        }
        if line.starts_with('+') && !line.starts_with("+++") {
            churn.insertions = churn.insertions.saturating_add(1);
        } else if line.starts_with('-') && !line.starts_with("---") {
            churn.deletions = churn.deletions.saturating_add(1);
        }
    }

    if let Some(commit) = current {
        if commits.len() < limit {
            commits.push(commit);
        }
    }

    (commits, churn)
}

fn parse_numstat(output: &str, limit: usize) -> Churn {
    let mut churn = Churn::default();
    let mut tokens = output.split('\0');

    while let Some(token) = tokens.next() {
        let token = token.trim_start_matches('\n');
        if token.is_empty() {
            continue;
        }

        if is_sha(token) {
            if churn.commits >= u32::try_from(limit).unwrap_or(u32::MAX) {
                break;
            }
            churn.commits = churn.commits.saturating_add(1);
            let _ = tokens.next();
            let _ = tokens.next();
            continue;
        }

        for line in token.lines() {
            let mut fields = line.split('\t');
            let Some(insertions) = fields.next() else {
                continue;
            };
            let Some(deletions) = fields.next() else {
                continue;
            };
            let Ok(insertions) = insertions.parse::<u32>() else {
                continue;
            };
            let Ok(deletions) = deletions.parse::<u32>() else {
                continue;
            };
            churn.insertions = churn.insertions.saturating_add(insertions);
            churn.deletions = churn.deletions.saturating_add(deletions);
        }
    }

    churn
}

fn is_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn truncate_subject(subject: &str) -> String {
    let subject = subject.trim();
    if subject.chars().count() <= MAX_SUBJECT_CHARS {
        return subject.to_owned();
    }

    let mut truncated: String = subject.chars().take(MAX_SUBJECT_CHARS - 1).collect();
    truncated.push('…');
    truncated
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

fn age_days(now: i64, timestamp: i64) -> u64 {
    let seconds = now.saturating_sub(timestamp).max(0);
    u64::try_from(seconds / 86_400).unwrap_or(u64::MAX)
}

struct BlameEntry {
    sha: String,
    author: String,
    timestamp: i64,
}

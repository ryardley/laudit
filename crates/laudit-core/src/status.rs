//! Tour status: report model/tour presence and per-status stop counts.
//!
//! `laudit status` and frontends use this to answer "where is the tour and how
//! much of it is done?" without loading the LLM or re-running recon. The report
//! is read-only: it never invalidates or rewrites artifacts.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::build::MODEL_FILE;
use crate::cache;
use crate::model::{Position, StopStatus, Tour};
use crate::plan::{self, PlanError};

/// A read-only summary of the artifacts in a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusReport {
    /// Whether `.laudit/model.json` exists.
    pub model_present: bool,
    /// Whether a tour artifact was found.
    pub tour_present: bool,
    /// Tour identifier, when a tour was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tour_id: Option<String>,
    /// Path of the tour artifact, when a tour was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tour_path: Option<PathBuf>,
    /// Saved position within the tour, when a tour was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// Number of chapters in the tour.
    pub chapters: usize,
    /// Number of stops in the tour.
    pub stops: usize,
    /// Stops that have been narrated.
    pub explained: usize,
    /// Stops that have not been narrated yet.
    pub pending: usize,
    /// Stops whose narration is in progress.
    pub streaming: usize,
    /// Stops that were skipped after a failure.
    pub skipped: usize,
    /// Stops that failed.
    pub failed: usize,
    /// Explained stops whose file content hash no longer matches.
    pub stale: usize,
}

impl StatusReport {
    fn empty(model_present: bool) -> Self {
        Self {
            model_present,
            tour_present: false,
            tour_id: None,
            tour_path: None,
            position: None,
            chapters: 0,
            stops: 0,
            explained: 0,
            pending: 0,
            streaming: 0,
            skipped: 0,
            failed: 0,
            stale: 0,
        }
    }
}

/// Report the state of the artifacts under `repo_root`.
///
/// When `tour_id` is `None`, the most recently modified tour artifact is
/// reported. A missing model or tour is not an error; the report simply records
/// that it is absent.
pub fn status(
    repo_root: impl AsRef<Path>,
    tour_id: Option<&str>,
) -> Result<StatusReport, StatusError> {
    let repo_root = repo_root.as_ref();
    let model_present = repo_root.join(MODEL_FILE).is_file();
    let tour_id = match tour_id {
        Some(tour_id) => Some(tour_id.to_owned()),
        None => latest_tour_id(repo_root)?,
    };
    let Some(tour_id) = tour_id else {
        return Ok(StatusReport::empty(model_present));
    };

    let tour = plan::load_tour(repo_root, &tour_id)?;
    Ok(report_for_tour(repo_root, &tour_id, &tour, model_present))
}

fn report_for_tour(
    repo_root: &Path,
    tour_id: &str,
    tour: &Tour,
    model_present: bool,
) -> StatusReport {
    let mut report = StatusReport {
        model_present,
        tour_present: true,
        tour_id: Some(tour_id.to_owned()),
        tour_path: Some(plan::tour_path(repo_root, tour_id)),
        position: Some(tour.position),
        chapters: tour.chapters.len(),
        ..StatusReport::empty(model_present)
    };

    for chapter in &tour.chapters {
        for stop in &chapter.stops {
            report.stops += 1;
            match stop.status {
                StopStatus::Pending => report.pending += 1,
                StopStatus::Streaming => report.streaming += 1,
                StopStatus::Explained => {
                    report.explained += 1;
                    if !hash_matches(repo_root, stop) {
                        report.stale += 1;
                    }
                }
                StopStatus::Skipped => report.skipped += 1,
                StopStatus::Failed => report.failed += 1,
            }
        }
    }

    report
}

fn hash_matches(repo_root: &Path, stop: &crate::model::Stop) -> bool {
    if stop.file_hash.is_empty() {
        return true;
    }
    match cache::file_hash(repo_root, &stop.file) {
        Ok(hash) => hash == stop.file_hash,
        Err(_) => false,
    }
}

/// Find the most recently modified tour artifact, if any.
pub fn latest_tour_id(repo_root: impl AsRef<Path>) -> io::Result<Option<String>> {
    let directory = repo_root.as_ref().join(plan::TOURS_DIR);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    let mut tours = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok();
        tours.push((modified, path));
    }

    tours.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    Ok(tours
        .first()
        .and_then(|(_, path)| path.file_stem())
        .and_then(|stem| stem.to_str())
        .map(str::to_owned))
}

/// Errors produced while reporting status.
#[derive(Debug)]
pub enum StatusError {
    /// The tours directory could not be read.
    Io(io::Error),
    /// A tour artifact could not be loaded.
    Plan(PlanError),
}

impl fmt::Display for StatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Plan(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for StatusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Plan(error) => Some(error),
        }
    }
}

impl From<io::Error> for StatusError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<PlanError> for StatusError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Position, Stop, StopStatus, Tour, TourChapter, TourSource, SCHEMA_VERSION};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("laudit-status-test-{}-{id}", std::process::id()));
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

    fn stop(id: &str, file: &str, status: StopStatus, file_hash: &str) -> Stop {
        Stop {
            id: id.to_owned(),
            file: file.to_owned(),
            start_line: 1,
            end_line: 1,
            symbol_id: format!("sym-{id}"),
            title: id.to_owned(),
            why_here: "why".to_owned(),
            explanation: String::new(),
            refs: Vec::new(),
            file_hash: file_hash.to_owned(),
            status,
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
            position: Position {
                chapter: 0,
                stop: 1,
            },
            chapters: vec![TourChapter {
                id: "ch-1".to_owned(),
                title: "Chapter".to_owned(),
                stops,
            }],
        }
    }

    #[test]
    fn missing_artifacts_report_absence() {
        let temp = TempDir::new();
        let report = status(temp.path(), None).expect("status");
        assert!(!report.model_present);
        assert!(!report.tour_present);
        assert_eq!(report.stops, 0);
    }

    #[test]
    fn status_counts_stops_by_state() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        temp.write("src/lib.rs", "fn helper() {}\n");
        let main_hash = cache::file_hash(temp.path(), "src/main.rs").expect("hash main");
        let lib_hash = cache::file_hash(temp.path(), "src/lib.rs").expect("hash lib");
        let tour = tour(vec![
            stop("stop-1", "src/main.rs", StopStatus::Explained, &main_hash),
            stop("stop-2", "src/lib.rs", StopStatus::Explained, &lib_hash),
            stop("stop-3", "src/main.rs", StopStatus::Pending, ""),
            stop("stop-4", "src/main.rs", StopStatus::Skipped, ""),
            stop("stop-5", "src/main.rs", StopStatus::Failed, ""),
        ]);
        plan::write_tour(temp.path(), &tour).expect("write tour");

        let report = status(temp.path(), Some("tour-test")).expect("status");

        assert!(report.tour_present);
        assert_eq!(report.tour_id.as_deref(), Some("tour-test"));
        assert_eq!(report.chapters, 1);
        assert_eq!(report.stops, 5);
        assert_eq!(report.explained, 2);
        assert_eq!(report.pending, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.stale, 0);
        assert_eq!(
            report.position,
            Some(Position {
                chapter: 0,
                stop: 1
            })
        );
    }

    #[test]
    fn status_counts_stale_explained_stops() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let tour = tour(vec![stop(
            "stop-1",
            "src/main.rs",
            StopStatus::Explained,
            "fnv1a128:stale",
        )]);
        plan::write_tour(temp.path(), &tour).expect("write tour");

        let report = status(temp.path(), Some("tour-test")).expect("status");

        assert_eq!(report.explained, 1);
        assert_eq!(report.stale, 1);
    }

    #[test]
    fn latest_tour_is_reported_when_no_id_is_given() {
        let temp = TempDir::new();
        let tour = tour(vec![stop("stop-1", "src/main.rs", StopStatus::Pending, "")]);
        plan::write_tour(temp.path(), &tour).expect("write tour");

        let report = status(temp.path(), None).expect("status");

        assert_eq!(report.tour_id.as_deref(), Some("tour-test"));
    }
}

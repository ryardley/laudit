//! Tour cache: per-stop content hashes, invalidation, and resume helpers.
//!
//! A tour is cheap to re-run because narration is cached in the tour artifact
//! itself: every explained stop records the content hash of the file it was
//! narrated from. On resume, a stop whose file hash still matches is skipped;
//! a stop whose file changed is invalidated (reset to [`StopStatus::Pending`])
//! and narrated again. Only the affected stops are invalidated, so editing one
//! file does not throw away the rest of the tour (D25).

use std::fs;
use std::io;
use std::path::Path;

use crate::model::{Position, StopStatus, Tour};
use crate::plan::{self, PlanError};
use crate::recon::content_hash;

/// Compute the content hash of a repository-relative file.
pub fn file_hash(repo_root: impl AsRef<Path>, file: &str) -> io::Result<String> {
    let bytes = fs::read(repo_root.as_ref().join(file))?;
    Ok(content_hash(&bytes))
}

/// A summary of a cache invalidation pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InvalidationReport {
    /// Stop IDs that were invalidated because their file changed.
    pub invalidated: Vec<String>,
}

impl InvalidationReport {
    /// Whether any stop was invalidated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.invalidated.is_empty()
    }
}

/// Invalidate explained stops whose file content hash no longer matches.
///
/// Stops that were left in [`StopStatus::Streaming`] by an interrupted run are
/// reset to [`StopStatus::Pending`] so they are narrated again. Stops that are
/// already pending, skipped, or failed are left alone; they are retried by the
/// next narration pass regardless.
pub fn invalidate_stale_stops(repo_root: impl AsRef<Path>, tour: &mut Tour) -> InvalidationReport {
    let repo_root = repo_root.as_ref();
    let mut report = InvalidationReport::default();

    for chapter in &mut tour.chapters {
        for stop in &mut chapter.stops {
            if stop.status == StopStatus::Streaming {
                stop.status = StopStatus::Pending;
            }
            if stop.status != StopStatus::Explained {
                continue;
            }

            let changed = match file_hash(repo_root, &stop.file) {
                Ok(hash) => !stop.file_hash.is_empty() && hash != stop.file_hash,
                Err(_) => true,
            };
            if changed {
                stop.status = StopStatus::Pending;
                stop.explanation.clear();
                stop.refs.clear();
                report.invalidated.push(stop.id.clone());
            }
        }
    }

    report
}

/// Persist a new position for a tour artifact.
///
/// Frontends call this as the user moves through the tour so a later resume
/// starts where they left off (D25).
pub fn set_position(
    repo_root: impl AsRef<Path>,
    tour_id: &str,
    position: Position,
) -> Result<Tour, PlanError> {
    let repo_root = repo_root.as_ref();
    let mut tour = plan::load_tour(repo_root, tour_id)?;
    tour.position = position;
    plan::write_tour(repo_root, &tour)?;
    Ok(tour)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Position, Stop, StopStatus, Tour, TourChapter, TourSource, SCHEMA_VERSION};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("laudit-cache-test-{}-{id}", std::process::id()));
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
            explanation: "explanation".to_owned(),
            refs: vec!["sym-ref".to_owned()],
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
            position: Position::default(),
            chapters: vec![TourChapter {
                id: "ch-1".to_owned(),
                title: "Chapter".to_owned(),
                stops,
            }],
        }
    }

    #[test]
    fn unchanged_files_keep_explained_stops() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let hash = file_hash(temp.path(), "src/main.rs").expect("hash");
        let mut tour = tour(vec![stop(
            "stop-1",
            "src/main.rs",
            StopStatus::Explained,
            &hash,
        )]);

        let report = invalidate_stale_stops(temp.path(), &mut tour);

        assert!(report.is_empty());
        assert_eq!(tour.chapters[0].stops[0].status, StopStatus::Explained);
        assert_eq!(tour.chapters[0].stops[0].explanation, "explanation");
    }

    #[test]
    fn changed_file_invalidates_only_that_stop() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        temp.write("src/lib.rs", "fn helper() {}\n");
        let main_hash = file_hash(temp.path(), "src/main.rs").expect("hash main");
        let lib_hash = file_hash(temp.path(), "src/lib.rs").expect("hash lib");
        let mut tour = tour(vec![
            stop("stop-1", "src/main.rs", StopStatus::Explained, &main_hash),
            stop("stop-2", "src/lib.rs", StopStatus::Explained, &lib_hash),
        ]);

        temp.write("src/lib.rs", "fn helper() {}\nfn extra() {}\n");
        let report = invalidate_stale_stops(temp.path(), &mut tour);

        assert_eq!(report.invalidated, vec!["stop-2"]);
        assert_eq!(tour.chapters[0].stops[0].status, StopStatus::Explained);
        assert_eq!(tour.chapters[0].stops[0].explanation, "explanation");
        assert_eq!(tour.chapters[0].stops[1].status, StopStatus::Pending);
        assert!(tour.chapters[0].stops[1].explanation.is_empty());
        assert!(tour.chapters[0].stops[1].refs.is_empty());
    }

    #[test]
    fn explained_stop_without_a_hash_is_kept() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let mut tour = tour(vec![stop(
            "stop-1",
            "src/main.rs",
            StopStatus::Explained,
            "",
        )]);

        let report = invalidate_stale_stops(temp.path(), &mut tour);

        assert!(report.is_empty());
        assert_eq!(tour.chapters[0].stops[0].status, StopStatus::Explained);
    }

    #[test]
    fn streaming_stops_are_reset_to_pending() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let hash = file_hash(temp.path(), "src/main.rs").expect("hash");
        let mut tour = tour(vec![stop(
            "stop-1",
            "src/main.rs",
            StopStatus::Streaming,
            &hash,
        )]);

        let report = invalidate_stale_stops(temp.path(), &mut tour);

        assert!(report.is_empty());
        assert_eq!(tour.chapters[0].stops[0].status, StopStatus::Pending);
    }

    #[test]
    fn missing_file_invalidates_explained_stop() {
        let temp = TempDir::new();
        let mut tour = tour(vec![stop(
            "stop-1",
            "src/missing.rs",
            StopStatus::Explained,
            "fnv1a128:old",
        )]);

        let report = invalidate_stale_stops(temp.path(), &mut tour);

        assert_eq!(report.invalidated, vec!["stop-1"]);
        assert_eq!(tour.chapters[0].stops[0].status, StopStatus::Pending);
    }

    #[test]
    fn set_position_persists_the_position() {
        let temp = TempDir::new();
        let tour = tour(vec![stop("stop-1", "src/main.rs", StopStatus::Pending, "")]);
        plan::write_tour(temp.path(), &tour).expect("write tour");

        let updated = set_position(
            temp.path(),
            "tour-test",
            Position {
                chapter: 0,
                stop: 1,
            },
        )
        .expect("set position");

        assert_eq!(updated.position.stop, 1);
        let loaded = plan::load_tour(temp.path(), "tour-test").expect("load tour");
        assert_eq!(loaded.position.stop, 1);
    }
}

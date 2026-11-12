//! Plain-text rendering for `laudit tour --print`.
//!
//! The renderer is deliberately simple and pipe-friendly: chapter headings,
//! stop titles, `file:line` ranges, and prose. It never requires a TTY and can
//! emit ANSI color escapes only when the caller asks for them (D23).

use std::fmt;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use crate::build::{self, MODEL_FILE};
use crate::config::Config;
use crate::explain::{self, ExplainError};
use crate::llm::LlmClient;
use crate::model::{CodebaseModel, Skeleton, StopStatus, Tour};
use crate::narration::{self, NarrationError, NarrationSink};
use crate::plan::{self, PlanError, PlanOptions};
use crate::status;

/// Options controlling plain-text rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintOptions {
    /// Emit ANSI color escapes.
    pub color: bool,
    /// Render only the structural skeleton (titles and ranges).
    pub offline: bool,
}

/// Render a tour as pipe-friendly plain text.
#[must_use]
pub fn render(tour: &Tour, options: PrintOptions) -> String {
    let mut out = String::new();
    for (chapter_index, chapter) in tour.chapters.iter().enumerate() {
        if chapter_index > 0 {
            out.push('\n');
        }
        let heading = format!("Chapter {}: {}", chapter_index + 1, chapter.title);
        if options.color {
            let _ = writeln!(out, "\x1b[1;36m{heading}\x1b[0m");
            let _ = writeln!(
                out,
                "\x1b[36m{}\x1b[0m",
                "=".repeat(heading.chars().count())
            );
        } else {
            let _ = writeln!(out, "{heading}");
            let _ = writeln!(out, "{}", "=".repeat(heading.chars().count()));
        }
        out.push('\n');

        for (stop_index, stop) in chapter.stops.iter().enumerate() {
            let title = if stop.title.is_empty() {
                &stop.symbol_id
            } else {
                &stop.title
            };
            let location = format!("{}:{}-{}", stop.file, stop.start_line, stop.end_line);
            if options.color {
                let _ = writeln!(out, "  \x1b[1m[{}] {}\x1b[0m", stop_index + 1, title);
                let _ = writeln!(out, "      \x1b[2m{location}\x1b[0m");
            } else {
                let _ = writeln!(out, "  [{}] {}", stop_index + 1, title);
                let _ = writeln!(out, "      {location}");
            }

            if !options.offline {
                if !stop.why_here.is_empty() {
                    let _ = writeln!(out, "      Why: {}", stop.why_here);
                }
                if !stop.explanation.is_empty() {
                    out.push('\n');
                    out.push_str(&stop.explanation);
                    if !stop.explanation.ends_with('\n') {
                        out.push('\n');
                    }
                }
                if stop.status == StopStatus::Skipped {
                    let _ = writeln!(out, "      (skipped)");
                }
            }
            out.push('\n');
        }
    }
    out
}

/// A tour prepared for printing.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedTour {
    /// The tour to render.
    pub tour: Tour,
    /// Path of the persisted tour artifact.
    pub tour_path: PathBuf,
    /// Number of stops narrated during this run.
    pub narrated: usize,
    /// Number of already-explained stops reused from the cache.
    pub cached: usize,
    /// Human-readable warnings from narration.
    pub warnings: Vec<String>,
}

/// Load or plan a tour and, unless `offline`, narrate pending stops.
pub async fn prepare(
    repo_root: impl AsRef<Path>,
    config: &Config,
    offline: bool,
    client: Option<&LlmClient>,
    sink: &mut dyn NarrationSink,
) -> Result<PreparedTour, PrintError> {
    let repo_root = repo_root.as_ref();
    let skeleton = explain::load_skeleton(repo_root, config)?;
    let model = load_model(repo_root, &skeleton, offline)?;

    let (mut tour, tour_path) = match status::latest_tour_id(repo_root)? {
        Some(tour_id) => {
            let tour = plan::load_tour(repo_root, &tour_id)?;
            let path = plan::tour_path(repo_root, &tour_id);
            (tour, path)
        }
        None => {
            let report = plan::plan(
                repo_root,
                config,
                &skeleton,
                &model,
                &PlanOptions::default(),
            )?;
            (report.tour, report.tour_path)
        }
    };

    if offline {
        return Ok(PreparedTour {
            tour,
            tour_path,
            narrated: 0,
            cached: 0,
            warnings: Vec::new(),
        });
    }

    let client = client.ok_or(PrintError::MissingClient)?;
    let report = narration::narrate(
        repo_root, config, &skeleton, &model, &mut tour, client, sink,
    )
    .await?;
    Ok(PreparedTour {
        tour: report.tour,
        tour_path: report.tour_path,
        narrated: report.narrated,
        cached: report.cached,
        warnings: report.warnings,
    })
}

/// Narrate a tour that was planned by a source adapter.
///
/// Unlike [`prepare`], the tour is supplied by the caller instead of being
/// loaded from the most recent artifact. A missing model falls back to the
/// offline placeholder so change tours work without a prior comprehension run.
pub async fn prepare_tour(
    repo_root: impl AsRef<Path>,
    config: &Config,
    offline: bool,
    client: Option<&LlmClient>,
    sink: &mut dyn NarrationSink,
    mut tour: Tour,
) -> Result<PreparedTour, PrintError> {
    let repo_root = repo_root.as_ref();
    let tour_path = plan::tour_path(repo_root, &tour.tour_id);

    if offline {
        return Ok(PreparedTour {
            tour,
            tour_path,
            narrated: 0,
            cached: 0,
            warnings: Vec::new(),
        });
    }

    let skeleton = explain::load_skeleton(repo_root, config)?;
    let model = match load_model(repo_root, &skeleton, false) {
        Ok(model) => model,
        Err(PrintError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            build::offline_model(&skeleton)
        }
        Err(error) => return Err(error),
    };
    let client = client.ok_or(PrintError::MissingClient)?;
    let report = narration::narrate(
        repo_root, config, &skeleton, &model, &mut tour, client, sink,
    )
    .await?;
    Ok(PreparedTour {
        tour: report.tour,
        tour_path: report.tour_path,
        narrated: report.narrated,
        cached: report.cached,
        warnings: report.warnings,
    })
}

fn load_model(
    repo_root: &Path,
    skeleton: &Skeleton,
    offline: bool,
) -> Result<CodebaseModel, PrintError> {
    let path = repo_root.join(MODEL_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let model: CodebaseModel = serde_json::from_slice(&bytes)?;
            model
                .validate_schema_version()
                .map_err(|error| PrintError::InvalidModel(error.to_string()))?;
            Ok(model)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && offline => {
            Ok(build::offline_model(skeleton))
        }
        Err(source) => Err(PrintError::Io { path, source }),
    }
}

/// Errors produced while preparing or rendering a tour.
#[derive(Debug)]
pub enum PrintError {
    /// An artifact could not be read or written.
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// An artifact could not be serialized or parsed.
    Serialize(serde_json::Error),
    /// A planning or artifact error.
    Plan(PlanError),
    /// Narration failed.
    Narration(NarrationError),
    /// Loading the skeleton or model failed.
    Explain(ExplainError),
    /// The persisted model is not usable.
    InvalidModel(String),
    /// A non-offline print was requested without an LLM client.
    MissingClient,
}

impl fmt::Display for PrintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            Self::Serialize(error) => write!(f, "{error}"),
            Self::Plan(error) => write!(f, "{error}"),
            Self::Narration(error) => write!(f, "{error}"),
            Self::Explain(error) => write!(f, "{error}"),
            Self::InvalidModel(message) => write!(f, "invalid model: {message}"),
            Self::MissingClient => write!(f, "an LLM client is required for narration"),
        }
    }
}

impl std::error::Error for PrintError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Serialize(error) => Some(error),
            Self::Plan(error) => Some(error),
            Self::Narration(error) => Some(error),
            Self::Explain(error) => Some(error),
            Self::InvalidModel(_) | Self::MissingClient => None,
        }
    }
}

impl From<io::Error> for PrintError {
    fn from(source: io::Error) -> Self {
        Self::Io {
            path: PathBuf::from("."),
            source,
        }
    }
}

impl From<serde_json::Error> for PrintError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

impl From<PlanError> for PrintError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}

impl From<NarrationError> for PrintError {
    fn from(error: NarrationError) -> Self {
        Self::Narration(error)
    }
}

impl From<ExplainError> for PrintError {
    fn from(error: ExplainError) -> Self {
        Self::Explain(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Position, Stop, TourChapter, TourSource, SCHEMA_VERSION};

    fn stop(title: &str, why: &str, body: &str) -> Stop {
        Stop {
            id: "stop-1".to_owned(),
            file: "src/main.rs".to_owned(),
            start_line: 1,
            end_line: 3,
            symbol_id: "sym-1".to_owned(),
            title: title.to_owned(),
            why_here: why.to_owned(),
            explanation: body.to_owned(),
            refs: Vec::new(),
            file_hash: String::new(),
            status: StopStatus::Explained,
        }
    }

    fn tour() -> Tour {
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
                stops: vec![stop("main", "It starts here.", "The body.")],
            }],
        }
    }

    #[test]
    fn renders_chapters_stops_and_prose() {
        let rendered = render(
            &tour(),
            PrintOptions {
                color: false,
                offline: false,
            },
        );
        assert!(rendered.contains("Chapter 1: On-ramp"));
        assert!(rendered.contains("[1] main"));
        assert!(rendered.contains("src/main.rs:1-3"));
        assert!(rendered.contains("Why: It starts here."));
        assert!(rendered.contains("The body."));
        assert!(!rendered.contains('\u{1b}'));
    }

    #[test]
    fn offline_rendering_omits_prose() {
        let rendered = render(
            &tour(),
            PrintOptions {
                color: false,
                offline: true,
            },
        );
        assert!(rendered.contains("Chapter 1: On-ramp"));
        assert!(rendered.contains("src/main.rs:1-3"));
        assert!(!rendered.contains("Why:"));
        assert!(!rendered.contains("The body."));
    }

    #[test]
    fn color_rendering_emits_ansi_escapes() {
        let rendered = render(
            &tour(),
            PrintOptions {
                color: true,
                offline: false,
            },
        );
        assert!(rendered.contains('\u{1b}'));
    }
}

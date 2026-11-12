//! Stage 3 narration: stream a per-stop explanation into the tour artifact.
//!
//! Narration is lazy and streamed (D10, D12). For each stop it assembles a
//! layered context — the global codebase model, the chapter, and a local
//! post-redaction bundle (code region, callers/callees, imports, provenance,
//! prose) — then asks the model for a front-matter envelope:
//!
//! ```text
//! TITLE: <one line>
//! WHY: <one line>
//! ---
//! <markdown body, streamed>
//! ```
//!
//! The body is streamed to a [`NarrationSink`] as it arrives. Symbol references
//! in the body are validated against the IDs provided in the context; unknown
//! references are dropped with a warning (D24). A stop that fails after one
//! retry is skipped and marked, never aborting the tour (D20). The finished
//! prose is persisted into `.laudit/tours/<id>.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use futures_util::StreamExt;
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};

use crate::build::{MODEL_FILE, SKELETON_FILE};
use crate::comprehension::REDACTIONS_FILE;
use crate::config::Config;
use crate::llm::{ChatMessage, ChatRequest, LlmClient};
use crate::model::{CodebaseModel, Skeleton, Stop, StopStatus, Tour, TourChapter};
use crate::plan::{self, PlanError};
use crate::prompts::{self, NARRATION};
use crate::recon::{load_index, Index, Provenance, SymbolNode};
use crate::redact::{RedactError, Redactor};

/// Maximum number of bytes of a stop's code region included in the prompt.
pub const MAX_CODE_BYTES: usize = 16 * 1024;
/// Maximum number of prose lines extracted from a stop's code region.
pub const MAX_PROSE_LINES: usize = 40;
/// Maximum number of bytes read from each documentation file.
pub const MAX_DOC_BYTES: usize = 2 * 1024;
/// Number of narration attempts per stop (the first try plus one retry).
pub const MAX_ATTEMPTS: usize = 2;

/// Receives streamed narration output.
///
/// Frontends implement this to render chunks as they arrive. The engine calls
/// [`NarrationSink::on_chunk`] for each body delta and
/// [`NarrationSink::on_stop`] once a stop has been persisted.
pub trait NarrationSink {
    /// A chunk of the markdown body for `stop_id`.
    fn on_chunk(&mut self, stop_id: &str, delta: &str);

    /// A stop finished (explained or skipped) and was persisted.
    fn on_stop(&mut self, _stop: &Stop) {}
}

/// A sink that discards all narration output.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopSink;

impl NarrationSink for NoopSink {
    fn on_chunk(&mut self, _stop_id: &str, _delta: &str) {}
}

/// A summary of a narration run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NarrationReport {
    /// The tour with prose filled in.
    pub tour: Tour,
    /// Path of the persisted tour artifact.
    pub tour_path: PathBuf,
    /// Number of stops that were explained.
    pub narrated: usize,
    /// Stops that were skipped, with the reason.
    pub skipped: Vec<SkippedStop>,
    /// Symbol references that were dropped because they were not provided.
    pub dropped_refs: Vec<DroppedRef>,
    /// Human-readable warnings for skipped stops and dropped references.
    pub warnings: Vec<String>,
}

/// A stop that could not be narrated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedStop {
    /// Stop identifier.
    pub stop_id: String,
    /// Why the stop was skipped.
    pub reason: String,
}

/// A symbol reference that was dropped from a stop's explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DroppedRef {
    /// Stop whose explanation contained the reference.
    pub stop_id: String,
    /// Unknown symbol ID.
    pub symbol_id: String,
}

/// Narrate every stop in `tour`, persisting prose into the tour artifact.
///
/// The tour is written after each stop so a frontend can observe progress and
/// resume later. Per-stop failures are recorded in the report and marked
/// [`StopStatus::Skipped`]; they never abort the run.
pub async fn narrate(
    repo_root: impl AsRef<Path>,
    config: &Config,
    skeleton: &Skeleton,
    model: &CodebaseModel,
    tour: &mut Tour,
    client: &LlmClient,
    sink: &mut dyn NarrationSink,
) -> Result<NarrationReport, NarrationError> {
    let repo_root = repo_root.as_ref();
    let index = load_index(repo_root).ok().flatten();
    let (prompt, prompt_version) = load_prompt();
    let mut redactor = Redactor::new(&config.redaction)?;
    let outline = tour_outline(tour);

    tour.prompt_version = prompt_version;
    tour.model = client.model().to_owned();
    let tour_path = plan::write_tour(repo_root, tour)?;

    let mut report = NarrationReport {
        tour: tour.clone(),
        tour_path,
        narrated: 0,
        skipped: Vec::new(),
        dropped_refs: Vec::new(),
        warnings: Vec::new(),
    };

    for chapter_index in 0..tour.chapters.len() {
        for stop_index in 0..tour.chapters[chapter_index].stops.len() {
            let chapter = tour.chapters[chapter_index].clone();
            let stop = chapter.stops[stop_index].clone();

            tour.chapters[chapter_index].stops[stop_index].status = StopStatus::Streaming;
            plan::write_tour(repo_root, tour)?;

            let outcome = {
                let mut narrator = Narrator {
                    repo_root,
                    skeleton,
                    model,
                    index: index.as_ref(),
                    outline: &outline,
                    prompt: &prompt,
                    client,
                    redactor: &mut redactor,
                    sink,
                };
                narrator.narrate_stop(&chapter, &stop).await
            };

            match outcome {
                Ok(narrated) => {
                    let stop_mut = &mut tour.chapters[chapter_index].stops[stop_index];
                    stop_mut.title = narrated.title;
                    stop_mut.why_here = narrated.why;
                    stop_mut.explanation = narrated.body;
                    stop_mut.refs = narrated.refs;
                    stop_mut.status = StopStatus::Explained;
                    report.narrated += 1;
                    report.dropped_refs.extend(narrated.dropped_refs);
                    report.warnings.extend(narrated.warnings);
                }
                Err(reason) => {
                    let stop_mut = &mut tour.chapters[chapter_index].stops[stop_index];
                    stop_mut.status = StopStatus::Skipped;
                    report.skipped.push(SkippedStop {
                        stop_id: stop.id.clone(),
                        reason: reason.clone(),
                    });
                    report
                        .warnings
                        .push(format!("skipped stop {}: {reason}", stop.id));
                }
            }

            plan::write_tour(repo_root, tour)?;
            sink.on_stop(&tour.chapters[chapter_index].stops[stop_index]);
        }
    }

    write_redactions(repo_root, &redactor)?;
    report.tour = tour.clone();
    Ok(report)
}

/// Load the skeleton, model, and tour artifacts and narrate the tour.
pub async fn narrate_from_artifacts(
    repo_root: impl AsRef<Path>,
    config: &Config,
    tour_id: &str,
    client: &LlmClient,
    sink: &mut dyn NarrationSink,
) -> Result<NarrationReport, NarrationError> {
    let repo_root = repo_root.as_ref();
    let skeleton = load_skeleton(repo_root)?;
    let model = load_model(repo_root)?;
    let mut tour = plan::load_tour(repo_root, tour_id)?;
    narrate(
        repo_root, config, &skeleton, &model, &mut tour, client, sink,
    )
    .await
}

struct Narrator<'a> {
    repo_root: &'a Path,
    skeleton: &'a Skeleton,
    model: &'a CodebaseModel,
    index: Option<&'a Index>,
    outline: &'a Value,
    prompt: &'a str,
    client: &'a LlmClient,
    redactor: &'a mut Redactor,
    sink: &'a mut dyn NarrationSink,
}

impl Narrator<'_> {
    async fn narrate_stop(
        &mut self,
        chapter: &TourChapter,
        stop: &Stop,
    ) -> Result<NarratedStop, String> {
        let context = self.build_context(chapter, stop)?;
        let valid_ids = context.valid_ids;
        let user = format!(
            "Explain the stop below. Use only the provided context and symbol IDs.\n\n{}",
            serde_json::to_string_pretty(&context.payload).map_err(|error| error.to_string())?
        );
        let prompt = self.prompt;
        let (system, _) = self.redactor.redact(prompt);
        let (user, _) = self.redactor.redact(&user);
        let messages = vec![ChatMessage::system(system), ChatMessage::user(user)];

        // Retry once on a schema or transport failure. A retry is only safe
        // before any body chunks have been emitted; once the frontend has seen
        // streamed prose we cannot retract it, so the stop is skipped instead.
        let mut last_error = String::from("narration failed");
        for _attempt in 0..MAX_ATTEMPTS {
            let request = ChatRequest::new(messages.clone());
            let mut stream = match self.client.chat_stream(&request).await {
                Ok(stream) => stream,
                Err(error) => {
                    last_error = error.to_string();
                    continue;
                }
            };

            let mut parser = FrontMatterStream::new();
            let mut emitted = false;
            let mut stream_error = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(chunk) => {
                        if let Some(delta) = parser.push(&chunk.delta) {
                            if !delta.is_empty() {
                                self.sink.on_chunk(&stop.id, &delta);
                                emitted = true;
                            }
                        }
                    }
                    Err(error) => {
                        stream_error = Some(error.to_string());
                        break;
                    }
                }
            }

            if let Some(error) = stream_error {
                last_error = error;
                if emitted {
                    break;
                }
                continue;
            }

            match parser.finish() {
                Ok(front) => {
                    let (refs, dropped_refs) = validate_refs(&front.body, &valid_ids, &stop.id);
                    let title = self.redactor.redact(&front.title).0;
                    let why = self.redactor.redact(&front.why).0;
                    let body = self.redactor.redact(&front.body).0;
                    let warnings = dropped_refs
                        .iter()
                        .map(|dropped| {
                            format!(
                                "dropped unknown symbol ref {} in stop {}",
                                dropped.symbol_id, dropped.stop_id
                            )
                        })
                        .collect();
                    return Ok(NarratedStop {
                        title,
                        why,
                        body,
                        refs,
                        dropped_refs,
                        warnings,
                    });
                }
                Err(error) => {
                    last_error = error;
                    if emitted {
                        break;
                    }
                    continue;
                }
            }
        }

        Err(last_error)
    }

    fn build_context(&mut self, chapter: &TourChapter, stop: &Stop) -> Result<Context, String> {
        let path = self.repo_root.join(&stop.file);
        let bytes =
            fs::read(&path).map_err(|error| format!("failed to read {}: {error}", stop.file))?;
        let text = String::from_utf8_lossy(&bytes);
        let region = extract_region(&text, stop.start_line, stop.end_line);
        let (region, _) = self
            .redactor
            .redact_with_origin(&region, &stop.file, stop.start_line);
        let prose = extract_prose(&region);
        let provenance = self
            .skeleton
            .provenance
            .get(Path::new(&stop.file))
            .cloned()
            .unwrap_or_else(|| Provenance::empty(&stop.file, None));
        let (callers, callees) = callers_and_callees(self.skeleton, self.index, &stop.symbol_id);
        let imports = file_edges(self.index, &stop.file, EdgeDirection::Outgoing);
        let dependents = file_edges(self.index, &stop.file, EdgeDirection::Incoming);
        let model_chapter = self.model.chapters.iter().find(|model_chapter| {
            model_chapter
                .stop_ids
                .iter()
                .any(|id| id == &stop.symbol_id)
        });
        let docs = read_docs(self.repo_root);

        let mut valid_ids = BTreeSet::new();
        for subsystem in &self.model.subsystems {
            valid_ids.extend(subsystem.symbol_ids.iter().cloned());
        }
        for model_chapter in &self.model.chapters {
            valid_ids.extend(model_chapter.stop_ids.iter().cloned());
        }
        for flow in &self.model.flows {
            valid_ids.extend(flow.steps.iter().cloned());
        }
        valid_ids.insert(stop.symbol_id.clone());
        for caller in &callers {
            valid_ids.insert(caller.id.clone());
        }
        for callee in &callees {
            valid_ids.insert(callee.id.clone());
        }

        let payload = json!({
            "stop": {
                "id": stop.id,
                "file": stop.file,
                "start_line": stop.start_line,
                "end_line": stop.end_line,
                "symbol_id": stop.symbol_id,
                "title": stop.title,
                "why_here": stop.why_here,
            },
            "chapter": {
                "id": chapter.id,
                "title": chapter.title,
                "stops": chapter.stops.iter().map(|stop| json!({
                    "id": stop.id,
                    "title": stop.title,
                    "file": stop.file,
                    "start_line": stop.start_line,
                    "end_line": stop.end_line,
                })).collect::<Vec<_>>(),
            },
            "model_chapter": model_chapter,
            "outline": self.outline,
            "codebase_model": self.model,
            "code": {
                "file": stop.file,
                "start_line": stop.start_line,
                "end_line": stop.end_line,
                "text": region,
            },
            "callers": callers,
            "callees": callees,
            "imports": imports,
            "dependents": dependents,
            "provenance": provenance,
            "prose": prose,
            "docs": docs,
            "valid_symbol_ids": valid_ids.iter().collect::<Vec<_>>(),
        });

        Ok(Context { payload, valid_ids })
    }
}

struct Context {
    payload: Value,
    valid_ids: BTreeSet<String>,
}

struct NarratedStop {
    title: String,
    why: String,
    body: String,
    refs: Vec<String>,
    dropped_refs: Vec<DroppedRef>,
    warnings: Vec<String>,
}

struct NarratedFrontMatter {
    title: String,
    why: String,
    body: String,
}

struct FrontMatterStream {
    full: String,
    body_start: Option<usize>,
    emitted: usize,
}

impl FrontMatterStream {
    fn new() -> Self {
        Self {
            full: String::new(),
            body_start: None,
            emitted: 0,
        }
    }

    fn push(&mut self, delta: &str) -> Option<String> {
        self.full.push_str(delta);
        if self.body_start.is_none() {
            if let Some((_, body_start)) = find_separator(&self.full) {
                self.body_start = Some(body_start);
            }
        }
        let body_start = self.body_start?;
        let start = self.emitted.max(body_start);
        if self.full.len() <= start {
            return None;
        }
        let output = self.full[start..].to_owned();
        self.emitted = self.full.len();
        Some(output)
    }

    fn finish(self) -> Result<NarratedFrontMatter, String> {
        let Some((front_end, body_start)) = find_separator(&self.full) else {
            return Err("narration response is missing the `---` separator".to_owned());
        };
        let front = &self.full[..front_end];
        let title = parse_field(front, "TITLE")
            .ok_or_else(|| "narration response is missing TITLE".to_owned())?;
        let why = parse_field(front, "WHY")
            .ok_or_else(|| "narration response is missing WHY".to_owned())?;
        let body = self.full[body_start..].to_owned();
        if body.trim().is_empty() {
            return Err("narration response has an empty body".to_owned());
        }
        Ok(NarratedFrontMatter { title, why, body })
    }
}

fn find_separator(text: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']).trim();
        if trimmed == "---" {
            return Some((offset, offset + line.len()));
        }
        offset += line.len();
    }
    None
}

fn parse_field(front: &str, name: &str) -> Option<String> {
    for line in front.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case(name) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn validate_refs(
    body: &str,
    valid: &BTreeSet<String>,
    stop_id: &str,
) -> (Vec<String>, Vec<DroppedRef>) {
    let mut refs = Vec::new();
    let mut dropped = Vec::new();
    let mut seen = BTreeSet::new();
    for id in extract_symbol_refs(body) {
        if !seen.insert(id.clone()) {
            continue;
        }
        if valid.contains(&id) {
            refs.push(id);
        } else {
            dropped.push(DroppedRef {
                stop_id: stop_id.to_owned(),
                symbol_id: id,
            });
        }
    }
    (refs, dropped)
}

fn extract_symbol_refs(text: &str) -> Vec<String> {
    static REF_RE: OnceLock<Regex> = OnceLock::new();
    let regex = REF_RE
        .get_or_init(|| Regex::new(r"\bsym-[A-Za-z0-9_-]+\b").expect("symbol ref regex compiles"));
    regex
        .find_iter(text)
        .map(|matched| matched.as_str().to_owned())
        .collect()
}

fn extract_region(text: &str, start_line: u32, end_line: u32) -> String {
    let start = start_line.max(1) as usize;
    let end = (end_line as usize).max(start);
    let mut region = String::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        if line_number < start {
            continue;
        }
        if line_number > end {
            break;
        }
        region.push_str(line);
        region.push('\n');
        if region.len() >= MAX_CODE_BYTES {
            break;
        }
    }
    truncate_chars(&region, MAX_CODE_BYTES)
}

fn extract_prose(region: &str) -> Vec<String> {
    region
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && (line.starts_with("//")
                    || line.starts_with('#')
                    || line.starts_with("/*")
                    || line.starts_with('*')
                    || line.contains("NOTE")
                    || line.contains("WHY")
                    || line.contains("TODO"))
        })
        .take(MAX_PROSE_LINES)
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Clone, Serialize)]
struct SymbolRef {
    id: String,
    name: String,
    file: String,
    line: u32,
}

fn callers_and_callees(
    skeleton: &Skeleton,
    index: Option<&Index>,
    symbol_id: &str,
) -> (Vec<SymbolRef>, Vec<SymbolRef>) {
    let Some(index) = index else {
        return (Vec::new(), Vec::new());
    };
    let by_id: BTreeMap<&str, &SymbolNode> = skeleton
        .symbols
        .iter()
        .map(|symbol| (symbol.id.as_str(), symbol))
        .collect();
    let mut callers = Vec::new();
    let mut callees = Vec::new();
    let mut seen_callers = BTreeSet::new();
    let mut seen_callees = BTreeSet::new();

    for edge in &index.symbol_edges {
        if edge.to == symbol_id && seen_callers.insert(edge.from.clone()) {
            if let Some(symbol) = by_id.get(edge.from.as_str()) {
                callers.push(symbol_ref(symbol));
            }
        }
        if edge.from == symbol_id && seen_callees.insert(edge.to.clone()) {
            if let Some(symbol) = by_id.get(edge.to.as_str()) {
                callees.push(symbol_ref(symbol));
            }
        }
    }

    callers.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.name.cmp(&b.name))
    });
    callees.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.name.cmp(&b.name))
    });
    (callers, callees)
}

fn symbol_ref(symbol: &SymbolNode) -> SymbolRef {
    SymbolRef {
        id: symbol.id.clone(),
        name: symbol.name.clone(),
        file: symbol.file.to_string_lossy().into_owned(),
        line: symbol.line,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgeDirection {
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone, Serialize)]
struct FileEdgeRef {
    file: String,
    weight: u32,
}

fn file_edges(index: Option<&Index>, file: &str, direction: EdgeDirection) -> Vec<FileEdgeRef> {
    let Some(index) = index else {
        return Vec::new();
    };
    let path = Path::new(file);
    let mut edges = Vec::new();
    let mut seen = BTreeSet::new();
    for edge in &index.file_edges {
        let (matches, other) = match direction {
            EdgeDirection::Outgoing => (edge.from.as_path() == path, &edge.to),
            EdgeDirection::Incoming => (edge.to.as_path() == path, &edge.from),
        };
        if matches {
            let other = other.to_string_lossy().into_owned();
            if seen.insert(other.clone()) {
                edges.push(FileEdgeRef {
                    file: other,
                    weight: edge.weight,
                });
            }
        }
    }
    edges.sort_by(|a, b| a.file.cmp(&b.file));
    edges
}

fn read_docs(repo_root: &Path) -> Vec<Value> {
    let mut docs = Vec::new();
    for name in ["README.md", "README", "ARCHITECTURE.md"] {
        let path = repo_root.join(name);
        if let Ok(text) = fs::read_to_string(&path) {
            docs.push(json!({
                "path": name,
                "content": truncate_chars(&text, MAX_DOC_BYTES),
            }));
        }
    }
    docs
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn tour_outline(tour: &Tour) -> Value {
    json!({
        "tour_id": tour.tour_id,
        "chapters": tour.chapters.iter().map(|chapter| json!({
            "id": chapter.id,
            "title": chapter.title,
            "stops": chapter.stops.iter().map(|stop| json!({
                "id": stop.id,
                "title": stop.title,
                "file": stop.file,
                "start_line": stop.start_line,
                "end_line": stop.end_line,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn load_prompt() -> (String, String) {
    match prompts::load(NARRATION) {
        Ok(prompt) => (prompt.contents, prompt.version.to_string()),
        Err(_) => {
            let embedded = prompts::embedded(NARRATION).expect("narration prompt is embedded");
            (embedded.contents.to_owned(), embedded.version.to_string())
        }
    }
}

fn load_skeleton(repo_root: &Path) -> Result<Skeleton, NarrationError> {
    let path = repo_root.join(SKELETON_FILE);
    let bytes = fs::read(&path).map_err(|source| NarrationError::Io {
        path: path.clone(),
        source,
    })?;
    let skeleton: Skeleton = serde_json::from_slice(&bytes)?;
    skeleton
        .validate_schema_version()
        .map_err(|error| NarrationError::InvalidSkeleton(error.to_string()))?;
    Ok(skeleton)
}

fn load_model(repo_root: &Path) -> Result<CodebaseModel, NarrationError> {
    let path = repo_root.join(MODEL_FILE);
    let bytes = fs::read(&path).map_err(|source| NarrationError::Io {
        path: path.clone(),
        source,
    })?;
    let model: CodebaseModel = serde_json::from_slice(&bytes)?;
    model
        .validate_schema_version()
        .map_err(|error| NarrationError::InvalidModel(error.to_string()))?;
    Ok(model)
}

fn write_redactions(repo_root: &Path, redactor: &Redactor) -> Result<(), NarrationError> {
    if redactor.placeholders().is_empty() {
        return Ok(());
    }
    let path = repo_root.join(REDACTIONS_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| NarrationError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let bytes = serde_json::to_vec_pretty(&redactor.redactions())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes).map_err(|source| NarrationError::Io {
        path: temporary.clone(),
        source,
    })?;
    fs::rename(&temporary, &path).map_err(|source| NarrationError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(())
}

/// Errors produced by Stage 3 narration.
#[derive(Debug)]
pub enum NarrationError {
    /// An artifact could not be read or written.
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// An artifact could not be serialized or parsed.
    Serialize(serde_json::Error),
    /// Redaction rules could not be compiled.
    Redact(RedactError),
    /// A tour artifact could not be loaded or written.
    Plan(PlanError),
    /// The persisted skeleton is not usable.
    InvalidSkeleton(String),
    /// The persisted model is not usable.
    InvalidModel(String),
}

impl fmt::Display for NarrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            Self::Serialize(error) => write!(f, "{error}"),
            Self::Redact(error) => write!(f, "{error}"),
            Self::Plan(error) => write!(f, "{error}"),
            Self::InvalidSkeleton(message) => write!(f, "invalid skeleton: {message}"),
            Self::InvalidModel(message) => write!(f, "invalid model: {message}"),
        }
    }
}

impl std::error::Error for NarrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Serialize(error) => Some(error),
            Self::Redact(error) => Some(error),
            Self::Plan(error) => Some(error),
            Self::InvalidSkeleton(_) | Self::InvalidModel(_) => None,
        }
    }
}

impl From<io::Error> for NarrationError {
    fn from(source: io::Error) -> Self {
        Self::Io {
            path: PathBuf::from("."),
            source,
        }
    }
}

impl From<serde_json::Error> for NarrationError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

impl From<RedactError> for NarrationError {
    fn from(error: RedactError) -> Self {
        Self::Redact(error)
    }
}

impl From<PlanError> for NarrationError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_is_parsed_and_body_streams_after_separator() {
        let mut stream = FrontMatterStream::new();
        assert_eq!(stream.push("TITLE: Entry\nWHY: It starts"), None);
        assert_eq!(
            stream.push(" here.\n---\nThe body "),
            Some("The body ".to_owned())
        );
        assert_eq!(stream.push("continues."), Some("continues.".to_owned()));
        let front = stream.finish().expect("front matter");
        assert_eq!(front.title, "Entry");
        assert_eq!(front.why, "It starts here.");
        assert_eq!(front.body, "The body continues.");
    }

    #[test]
    fn missing_separator_is_an_error() {
        let mut stream = FrontMatterStream::new();
        assert_eq!(stream.push("TITLE: Entry\nWHY: It starts here.\n"), None);
        assert!(stream.finish().is_err());
    }

    #[test]
    fn missing_title_is_an_error() {
        let mut stream = FrontMatterStream::new();
        let _ = stream.push("WHY: It starts here.\n---\nbody");
        assert!(stream.finish().is_err());
    }

    #[test]
    fn empty_body_is_an_error() {
        let mut stream = FrontMatterStream::new();
        let _ = stream.push("TITLE: Entry\nWHY: Why.\n---\n   \n");
        assert!(stream.finish().is_err());
    }

    #[test]
    fn separator_may_be_split_across_chunks() {
        let mut stream = FrontMatterStream::new();
        assert_eq!(stream.push("TITLE: Entry\nWHY: Why.\n--"), None);
        assert_eq!(stream.push("-\nbody"), Some("body".to_owned()));
        let front = stream.finish().expect("front matter");
        assert_eq!(front.body, "body");
    }

    #[test]
    fn refs_are_extracted_and_unknown_refs_are_dropped() {
        let mut valid = BTreeSet::new();
        valid.insert("sym-0123456789abcdef".to_owned());
        let body = "See `sym-0123456789abcdef` and `sym-unknown`.";
        let (refs, dropped) = validate_refs(body, &valid, "stop-1");
        assert_eq!(refs, vec!["sym-0123456789abcdef"]);
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].symbol_id, "sym-unknown");
        assert_eq!(dropped[0].stop_id, "stop-1");
    }

    #[test]
    fn region_extraction_is_bounded_and_one_based() {
        let text = "one\ntwo\nthree\nfour\n";
        assert_eq!(extract_region(text, 2, 3), "two\nthree\n");
        assert_eq!(extract_region(text, 0, 1), "one\n");
    }

    #[test]
    fn prose_extraction_keeps_comments_and_markers() {
        let region = "// A comment\nlet x = 1;\n// TODO: fix\n";
        let prose = extract_prose(region);
        assert_eq!(prose, vec!["// A comment", "// TODO: fix"]);
    }
}

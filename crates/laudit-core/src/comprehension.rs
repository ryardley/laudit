//! Stage 1 comprehension: turn the deterministic skeleton into a codebase model.
//!
//! Comprehension feeds the recon skeleton, repository docs, and entrypoints to
//! the LLM and asks for a [`CodebaseModel`] (subsystems, chapters, flows, and
//! glossary). The model is constrained to engine-provided symbol IDs and file
//! paths: anything the model invents is dropped before the artifact is written
//! (D12, D13, D24).
//!
//! Large repositories use hierarchical summarization: each community is
//! summarized on its own, then the summaries are synthesized into the final
//! model. The result is persisted as `.laudit/model.json` and cached by the
//! repository `HEAD` plus the working-tree dirty hash.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::build::{BuildError, MODEL_FILE, OFFLINE_MODEL};
use crate::config::Config;
use crate::llm::{ChatMessage, LlmClient, LlmError};
use crate::model::{
    CodebaseModel, Flow, GlossaryEntry, ModelChapter, Skeleton, Subsystem, SCHEMA_VERSION,
};
use crate::prompts::{self, COMPREHENSION};
use crate::recon::Community;
use crate::redact::{RedactError, Redactor};

/// Repository-relative path of the redaction placeholder artifact.
pub const REDACTIONS_FILE: &str = ".laudit/redactions.json";
/// Name of the structured-output schema sent to the provider.
pub const SCHEMA_NAME: &str = "codebase_model";
/// Maximum number of documentation files included in a prompt.
pub const MAX_DOCS: usize = 8;
/// Maximum number of bytes read from each documentation file.
pub const MAX_DOC_BYTES: usize = 8 * 1024;
/// Maximum number of ranked symbols added to a hierarchical synthesis prompt.
pub const MAX_SYNTHESIS_SYMBOLS: usize = 200;
/// Maximum number of ranked files added to a hierarchical synthesis prompt.
pub const MAX_SYNTHESIS_FILES: usize = 200;

/// Options controlling a comprehension run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ComprehensionOptions {
    /// Ignore a valid cached model and call the LLM again.
    pub force: bool,
    /// Use hierarchical summarization even when the input fits the budget.
    pub force_hierarchical: bool,
}

/// A summary of a comprehension run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComprehensionReport {
    /// The codebase model that was produced or loaded from cache.
    pub model: CodebaseModel,
    /// Path of the persisted model artifact.
    pub model_path: PathBuf,
    /// Whether the model was loaded from cache.
    pub cached: bool,
    /// Whether hierarchical summarization was used.
    pub hierarchical: bool,
    /// Symbol IDs the model referenced that were not provided by recon.
    pub dropped_ids: Vec<String>,
    /// File paths the model referenced that were not provided by recon.
    pub dropped_files: Vec<String>,
}

/// Run recon and comprehension, writing the skeleton and model artifacts.
pub async fn build(
    repo_root: impl AsRef<Path>,
    config: &Config,
    client: &LlmClient,
) -> Result<ComprehensionReport, ComprehensionError> {
    let repo_root = repo_root.as_ref();
    let recon = crate::build::recon(repo_root, config)?;
    crate::build::write_skeleton(repo_root, &recon.skeleton)?;
    comprehend(repo_root, config, &recon.skeleton, client).await
}

/// Run Stage 1 comprehension for an existing skeleton.
pub async fn comprehend(
    repo_root: impl AsRef<Path>,
    config: &Config,
    skeleton: &Skeleton,
    client: &LlmClient,
) -> Result<ComprehensionReport, ComprehensionError> {
    comprehend_with_options(
        repo_root,
        config,
        skeleton,
        client,
        &ComprehensionOptions::default(),
    )
    .await
}

/// Run Stage 1 comprehension with explicit options.
pub async fn comprehend_with_options(
    repo_root: impl AsRef<Path>,
    config: &Config,
    skeleton: &Skeleton,
    client: &LlmClient,
    options: &ComprehensionOptions,
) -> Result<ComprehensionReport, ComprehensionError> {
    let repo_root = repo_root.as_ref();
    let (prompt, prompt_version) = load_prompt();
    let model_path = repo_root.join(MODEL_FILE);

    if !options.force {
        if let Some(model) = load_cached_model(&model_path, skeleton, client, &prompt_version) {
            return Ok(ComprehensionReport {
                model,
                model_path,
                cached: true,
                hierarchical: false,
                dropped_ids: Vec::new(),
                dropped_files: Vec::new(),
            });
        }
    }

    let docs = gather_docs(repo_root);
    let mut redactor = Redactor::new(&config.redaction)?;

    let full_payload = input_payload(skeleton, &docs)?;
    let full_text = serde_json::to_string(&full_payload)?;
    let oversized = options.force_hierarchical
        || estimate_tokens(&full_text) > config.budgets.max_context_tokens as usize;

    let (response, valid, hierarchical) = if oversized {
        let (response, valid) =
            comprehend_hierarchical(skeleton, client, &mut redactor, &prompt, &docs).await?;
        (response, valid, true)
    } else {
        let response = call_model(
            client,
            &mut redactor,
            &prompt,
            "Produce the codebase model for this repository.",
            &full_payload,
        )
        .await?;
        (response, ValidIds::from_skeleton(skeleton), false)
    };

    let (mut response, dropped_ids, dropped_files) = validate_response(response, &valid);
    redact_response(&mut response, &mut redactor);

    let model = build_model(skeleton, client, &prompt_version, response);
    write_json(&model_path, &model)?;
    write_redactions(repo_root, &redactor)?;

    Ok(ComprehensionReport {
        model,
        model_path,
        cached: false,
        hierarchical,
        dropped_ids,
        dropped_files,
    })
}

/// Load the persisted skeleton artifact.
pub fn load_skeleton(repo_root: impl AsRef<Path>) -> Result<Skeleton, ComprehensionError> {
    let path = repo_root.as_ref().join(crate::build::SKELETON_FILE);
    let bytes = fs::read(&path)?;
    let skeleton: Skeleton = serde_json::from_slice(&bytes)?;
    skeleton
        .validate_schema_version()
        .map_err(|error| ComprehensionError::InvalidSkeleton(error.to_string()))?;
    Ok(skeleton)
}

#[derive(Debug, Clone, Deserialize)]
struct ModelResponse {
    #[serde(default)]
    subsystems: Vec<Subsystem>,
    #[serde(default)]
    chapters: Vec<ModelChapter>,
    #[serde(default)]
    flows: Vec<Flow>,
    #[serde(default)]
    glossary: Vec<GlossaryEntry>,
}

#[derive(Debug, Clone, Serialize)]
struct Doc {
    path: String,
    content: String,
}

#[derive(Debug, Clone, Default)]
struct ValidIds {
    symbols: BTreeSet<String>,
    files: BTreeSet<String>,
}

impl ValidIds {
    fn from_skeleton(skeleton: &Skeleton) -> Self {
        Self {
            symbols: skeleton
                .symbols
                .iter()
                .map(|symbol| symbol.id.clone())
                .collect(),
            files: skeleton
                .files
                .iter()
                .map(|file| file.path.to_string_lossy().into_owned())
                .collect(),
        }
    }
}

async fn comprehend_hierarchical(
    skeleton: &Skeleton,
    client: &LlmClient,
    redactor: &mut Redactor,
    prompt: &str,
    docs: &[Doc],
) -> Result<(ModelResponse, ValidIds), ComprehensionError> {
    let communities = if skeleton.communities.is_empty() {
        vec![Community {
            id: 0,
            files: skeleton
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect(),
            symbols: skeleton
                .symbols
                .iter()
                .map(|symbol| symbol.id.clone())
                .collect(),
        }]
    } else {
        skeleton.communities.clone()
    };

    let mut summaries = Vec::new();
    for community in &communities {
        if community.files.is_empty() && community.symbols.is_empty() {
            continue;
        }

        let filtered = filter_skeleton(skeleton, community);
        let payload = input_payload(&filtered, docs)?;
        let response = call_model(
            client,
            redactor,
            prompt,
            "This is a hierarchical comprehension pass. Summarize only the subsystem in the input. Return the same JSON object shape with a single subsystem entry.",
            &payload,
        )
        .await?;

        let valid = ValidIds::from_skeleton(&filtered);
        let (mut response, _, _) = validate_response(response, &valid);
        redact_response(&mut response, redactor);

        if response.subsystems.is_empty() {
            summaries.push(Subsystem {
                id: format!("sub-{}", community.id),
                title: format!("Subsystem {}", community.id),
                summary: String::new(),
                symbol_ids: community.symbols.clone(),
                files: community
                    .files
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect(),
            });
        } else {
            summaries.extend(response.subsystems);
        }
    }

    let mut seen_summary_ids = BTreeSet::new();
    summaries.retain(|summary| seen_summary_ids.insert(summary.id.clone()));

    let valid = synthesis_valid_ids(skeleton, &summaries);
    let payload = synthesis_payload(skeleton, &summaries, &valid, docs);
    let response = call_model(
        client,
        redactor,
        prompt,
        "This is a hierarchical synthesis pass. The input contains per-subsystem summaries and the only symbol IDs and file paths you may use. Produce the final codebase model JSON object.",
        &payload,
    )
    .await?;

    Ok((response, valid))
}

async fn call_model(
    client: &LlmClient,
    redactor: &mut Redactor,
    system_prompt: &str,
    user_prefix: &str,
    payload: &Value,
) -> Result<ModelResponse, ComprehensionError> {
    let payload = serde_json::to_string_pretty(payload)?;
    let user = format!("{user_prefix}\n\n{payload}");
    let (system, _) = redactor.redact(system_prompt);
    let (user, _) = redactor.redact(&user);
    let messages = vec![ChatMessage::system(system), ChatMessage::user(user)];
    let response = client
        .complete_json::<ModelResponse>(messages, SCHEMA_NAME, response_schema())
        .await?;
    Ok(response)
}

fn input_payload(skeleton: &Skeleton, docs: &[Doc]) -> Result<Value, ComprehensionError> {
    let mut value = serde_json::to_value(skeleton)?;
    if let Some(object) = value.as_object_mut() {
        object.insert("docs".to_owned(), serde_json::to_value(docs)?);
    }
    Ok(value)
}

fn synthesis_payload(
    skeleton: &Skeleton,
    summaries: &[Subsystem],
    valid: &ValidIds,
    docs: &[Doc],
) -> Value {
    json!({
        "repo_id": &skeleton.repo_id,
        "head": &skeleton.head,
        "dirty_hash": &skeleton.dirty_hash,
        "subsystem_summaries": summaries,
        "valid_symbol_ids": valid.symbols.iter().collect::<Vec<_>>(),
        "valid_files": valid.files.iter().collect::<Vec<_>>(),
        "entrypoints": &skeleton.entrypoints,
        "ranked_files": &skeleton.ranked_files,
        "ranked_symbols": &skeleton.ranked_symbols,
        "docs": docs,
    })
}

fn filter_skeleton(skeleton: &Skeleton, community: &Community) -> Skeleton {
    let files: BTreeSet<&Path> = community.files.iter().map(PathBuf::as_path).collect();
    let symbols: BTreeSet<&str> = community.symbols.iter().map(String::as_str).collect();

    Skeleton {
        schema_version: skeleton.schema_version,
        tool_version: skeleton.tool_version.clone(),
        repo_id: skeleton.repo_id.clone(),
        head: skeleton.head.clone(),
        dirty_hash: skeleton.dirty_hash.clone(),
        files: skeleton
            .files
            .iter()
            .filter(|file| files.contains(file.path.as_path()))
            .cloned()
            .collect(),
        symbols: skeleton
            .symbols
            .iter()
            .filter(|symbol| symbols.contains(symbol.id.as_str()))
            .cloned()
            .collect(),
        communities: vec![community.clone()],
        entrypoints: skeleton
            .entrypoints
            .iter()
            .filter(|entrypoint| files.contains(entrypoint.file.as_path()))
            .cloned()
            .collect(),
        ranked_files: skeleton
            .ranked_files
            .iter()
            .filter(|file| files.contains(Path::new(file.as_str())))
            .cloned()
            .collect(),
        ranked_symbols: skeleton
            .ranked_symbols
            .iter()
            .filter(|symbol| symbols.contains(symbol.as_str()))
            .cloned()
            .collect(),
        provenance: skeleton
            .provenance
            .iter()
            .filter(|(path, _)| files.contains(path.as_path()))
            .map(|(path, provenance)| (path.clone(), provenance.clone()))
            .collect(),
    }
}

fn synthesis_valid_ids(skeleton: &Skeleton, summaries: &[Subsystem]) -> ValidIds {
    let mut symbols = Vec::new();
    let mut seen_symbols = BTreeSet::new();
    let mut files = Vec::new();
    let mut seen_files = BTreeSet::new();

    for summary in summaries {
        for id in &summary.symbol_ids {
            if seen_symbols.insert(id.clone()) {
                symbols.push(id.clone());
            }
        }
        for file in &summary.files {
            if seen_files.insert(file.clone()) {
                files.push(file.clone());
            }
        }
    }

    for id in &skeleton.ranked_symbols {
        if symbols.len() >= MAX_SYNTHESIS_SYMBOLS {
            break;
        }
        if seen_symbols.insert(id.clone()) {
            symbols.push(id.clone());
        }
    }
    for file in &skeleton.ranked_files {
        if files.len() >= MAX_SYNTHESIS_FILES {
            break;
        }
        if seen_files.insert(file.clone()) {
            files.push(file.clone());
        }
    }

    ValidIds {
        symbols: symbols.into_iter().collect(),
        files: files.into_iter().collect(),
    }
}

fn validate_response(
    response: ModelResponse,
    valid: &ValidIds,
) -> (ModelResponse, Vec<String>, Vec<String>) {
    let mut dropped_ids = Vec::new();
    let mut dropped_files = Vec::new();
    let mut seen_dropped_ids = BTreeSet::new();
    let mut seen_dropped_files = BTreeSet::new();

    let subsystems = response
        .subsystems
        .into_iter()
        .filter_map(|mut subsystem| {
            subsystem.symbol_ids = filter_ids(
                subsystem.symbol_ids,
                &valid.symbols,
                &mut dropped_ids,
                &mut seen_dropped_ids,
            );
            subsystem.files = filter_ids(
                subsystem.files,
                &valid.files,
                &mut dropped_files,
                &mut seen_dropped_files,
            );
            if subsystem.symbol_ids.is_empty() && subsystem.files.is_empty() {
                None
            } else {
                Some(subsystem)
            }
        })
        .collect();

    let chapters = response
        .chapters
        .into_iter()
        .filter_map(|mut chapter| {
            chapter.stop_ids = filter_ids(
                chapter.stop_ids,
                &valid.symbols,
                &mut dropped_ids,
                &mut seen_dropped_ids,
            );
            if chapter.stop_ids.is_empty() {
                None
            } else {
                Some(chapter)
            }
        })
        .collect();

    let flows = response
        .flows
        .into_iter()
        .filter_map(|mut flow| {
            flow.steps = filter_ids(
                flow.steps,
                &valid.symbols,
                &mut dropped_ids,
                &mut seen_dropped_ids,
            );
            if flow.steps.is_empty() {
                None
            } else {
                Some(flow)
            }
        })
        .collect();

    (
        ModelResponse {
            subsystems,
            chapters,
            flows,
            glossary: response.glossary,
        },
        dropped_ids,
        dropped_files,
    )
}

fn filter_ids(
    ids: Vec<String>,
    valid: &BTreeSet<String>,
    dropped: &mut Vec<String>,
    seen_dropped: &mut BTreeSet<String>,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for id in ids {
        if valid.contains(&id) {
            if seen.insert(id.clone()) {
                result.push(id);
            }
        } else if seen_dropped.insert(id.clone()) {
            dropped.push(id);
        }
    }
    result
}

fn redact_response(response: &mut ModelResponse, redactor: &mut Redactor) {
    for subsystem in &mut response.subsystems {
        subsystem.title = redactor.redact(&subsystem.title).0;
        subsystem.summary = redactor.redact(&subsystem.summary).0;
    }
    for chapter in &mut response.chapters {
        chapter.title = redactor.redact(&chapter.title).0;
        chapter.intent = redactor.redact(&chapter.intent).0;
    }
    for flow in &mut response.flows {
        flow.title = redactor.redact(&flow.title).0;
    }
    for entry in &mut response.glossary {
        entry.term = redactor.redact(&entry.term).0;
        entry.definition = redactor.redact(&entry.definition).0;
    }
}

fn build_model(
    skeleton: &Skeleton,
    client: &LlmClient,
    prompt_version: &str,
    response: ModelResponse,
) -> CodebaseModel {
    CodebaseModel {
        schema_version: SCHEMA_VERSION,
        tool_version: crate::VERSION.to_owned(),
        repo_id: skeleton.repo_id.clone(),
        head: skeleton.head.clone(),
        dirty_hash: skeleton.dirty_hash.clone(),
        generator: "laudit".to_owned(),
        model: client.model().to_owned(),
        prompt_version: prompt_version.to_owned(),
        generated_at: rfc3339_now(),
        subsystems: response.subsystems,
        chapters: response.chapters,
        flows: response.flows,
        glossary: response.glossary,
    }
}

fn load_cached_model(
    path: &Path,
    skeleton: &Skeleton,
    client: &LlmClient,
    prompt_version: &str,
) -> Option<CodebaseModel> {
    let bytes = fs::read(path).ok()?;
    let model: CodebaseModel = serde_json::from_slice(&bytes).ok()?;
    if model.validate_schema_version().is_err() {
        return None;
    }
    if model.repo_id != skeleton.repo_id
        || model.head != skeleton.head
        || model.dirty_hash != skeleton.dirty_hash
        || model.generator != "laudit"
        || model.model != client.model()
        || model.prompt_version != prompt_version
        || model.model == OFFLINE_MODEL
    {
        return None;
    }
    Some(model)
}

fn load_prompt() -> (String, String) {
    match prompts::load(COMPREHENSION) {
        Ok(prompt) => (prompt.contents, prompt.version.to_string()),
        Err(_) => {
            let embedded =
                prompts::embedded(COMPREHENSION).expect("comprehension prompt is embedded");
            (embedded.contents.to_owned(), embedded.version.to_string())
        }
    }
}

fn gather_docs(repo_root: &Path) -> Vec<Doc> {
    let mut paths = Vec::new();
    for name in [
        "README.md",
        "README",
        "readme.md",
        "ARCHITECTURE.md",
        "CONTRIBUTING.md",
    ] {
        let path = repo_root.join(name);
        if path.is_file() {
            paths.push(path);
        }
    }

    let docs_dir = repo_root.join("docs");
    if let Ok(entries) = fs::read_dir(&docs_dir) {
        let mut doc_paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path.extension().and_then(|extension| extension.to_str()) == Some("md")
            })
            .collect();
        doc_paths.sort();
        paths.extend(doc_paths);
    }

    paths.sort();
    paths.dedup();
    paths.truncate(MAX_DOCS);

    paths
        .into_iter()
        .filter_map(|path| {
            let content = fs::read_to_string(&path).ok()?;
            let content = truncate_chars(&content, MAX_DOC_BYTES);
            let relative = path
                .strip_prefix(repo_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            Some(Doc {
                path: relative,
                content,
            })
        })
        .collect()
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

fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), ComprehensionError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn write_redactions(repo_root: &Path, redactor: &Redactor) -> Result<(), ComprehensionError> {
    if redactor.placeholders().is_empty() {
        return Ok(());
    }
    let path = repo_root.join(REDACTIONS_FILE);
    write_json(&path, &redactor.redactions())?;
    Ok(())
}

fn response_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "subsystems": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "title": { "type": "string" },
                        "summary": { "type": "string" },
                        "symbol_ids": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "files": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["id", "title", "summary", "symbol_ids", "files"],
                    "additionalProperties": false
                }
            },
            "chapters": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "title": { "type": "string" },
                        "intent": { "type": "string" },
                        "stop_ids": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["id", "title", "intent", "stop_ids"],
                    "additionalProperties": false
                }
            },
            "flows": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "title": { "type": "string" },
                        "steps": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["id", "title", "steps"],
                    "additionalProperties": false
                }
            },
            "glossary": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "term": { "type": "string" },
                        "definition": { "type": "string" }
                    },
                    "required": ["term", "definition"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["subsystems", "chapters", "flows", "glossary"],
        "additionalProperties": false
    })
}

fn rfc3339_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    format_timestamp(seconds)
}

fn format_timestamp(seconds: u64) -> String {
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

/// Errors produced by Stage 1 comprehension.
#[derive(Debug)]
pub enum ComprehensionError {
    /// Recon failed.
    Build(BuildError),
    /// An artifact could not be read or written.
    Io(io::Error),
    /// An artifact could not be serialized or parsed.
    Serialize(serde_json::Error),
    /// Redaction rules could not be compiled.
    Redact(RedactError),
    /// The LLM call failed.
    Llm(LlmError),
    /// The persisted skeleton is not usable.
    InvalidSkeleton(String),
}

impl fmt::Display for ComprehensionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::Serialize(error) => write!(f, "{error}"),
            Self::Redact(error) => write!(f, "{error}"),
            Self::Llm(error) => write!(f, "{error}"),
            Self::InvalidSkeleton(message) => write!(f, "invalid skeleton: {message}"),
        }
    }
}

impl std::error::Error for ComprehensionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Build(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Serialize(error) => Some(error),
            Self::Redact(error) => Some(error),
            Self::Llm(error) => Some(error),
            Self::InvalidSkeleton(_) => None,
        }
    }
}

impl From<BuildError> for ComprehensionError {
    fn from(error: BuildError) -> Self {
        Self::Build(error)
    }
}

impl From<io::Error> for ComprehensionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ComprehensionError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

impl From<RedactError> for ComprehensionError {
    fn from(error: RedactError) -> Self {
        Self::Redact(error)
    }
}

impl From<LlmError> for ComprehensionError {
    fn from(error: LlmError) -> Self {
        Self::Llm(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_rfc3339() {
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_timestamp(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn token_estimate_rounds_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 3), "hel…");
        assert_eq!(truncate_chars("héllo", 2), "h…");
    }
}

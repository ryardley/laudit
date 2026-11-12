//! Grounded Q&A over the codebase model (`laudit ask`).
//!
//! Retrieval is deterministic. Candidate symbols are scored by exact name
//! match, substring match, and a BM25-style term score over symbol names, file
//! paths, and nearby doc comments. The codebase model contributes a small boost
//! for symbols that belong to a matching subsystem, chapter, or flow. The top
//! candidates within a context budget are assembled into a redacted prompt and
//! streamed to the LLM.
//!
//! Answers are expected to cite `file:line`. Citations are validated against
//! the skeleton; unknown or out-of-range citations are reported as warnings.
//! When retrieval finds no evidence the engine answers "I don't know" without
//! calling the model.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use futures_util::StreamExt;
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::llm::{ChatMessage, ChatRequest, LlmClient, LlmError};
use crate::model::{CodebaseModel, Skeleton};
use crate::plan::{self, PlanError};
use crate::prompts::{self, ASK};
use crate::recon::{load_index, Index};
use crate::redact::{RedactError, Redactor};

/// Default number of retrieved symbols included in the prompt.
pub const DEFAULT_TOP_N: usize = 8;
/// Default maximum number of bytes of assembled context.
pub const DEFAULT_MAX_CONTEXT_BYTES: usize = 24 * 1024;
/// Maximum number of bytes of a single symbol's code region.
pub const MAX_CODE_BYTES: usize = 8 * 1024;
/// Maximum number of lines scanned for a symbol's doc comment.
pub const MAX_DOC_LINES: usize = 20;
/// Maximum number of bytes read from source files while building the corpus.
pub const MAX_CORPUS_BYTES: usize = 16 * 1024 * 1024;
/// Number of answer attempts (the first try plus one retry).
pub const MAX_ATTEMPTS: usize = 2;

/// Options controlling retrieval and context assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AskOptions {
    /// Maximum number of symbols to retrieve.
    pub top_n: usize,
    /// Maximum number of bytes of assembled context.
    pub max_context_bytes: usize,
}

impl Default for AskOptions {
    fn default() -> Self {
        Self {
            top_n: DEFAULT_TOP_N,
            max_context_bytes: DEFAULT_MAX_CONTEXT_BYTES,
        }
    }
}

/// A symbol selected as relevant to a question.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RetrievedSymbol {
    /// Stable symbol identifier.
    pub id: String,
    /// Symbol name.
    pub name: String,
    /// Repository-relative file path.
    pub file: String,
    /// 1-based definition line.
    pub line: u32,
    /// Retrieval score.
    pub score: f64,
    /// Nearby doc comment text, if any.
    pub doc: String,
}

/// A `file:line` citation found in an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Citation {
    /// Repository-relative file path.
    pub file: String,
    /// 1-based line number.
    pub line: u32,
}

/// The result of answering one question.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AskReport {
    /// The question that was asked.
    pub question: String,
    /// The grounded answer.
    pub answer: String,
    /// Citations that were validated against the skeleton.
    pub citations: Vec<Citation>,
    /// Symbols that were retrieved for the question.
    pub retrieved: Vec<RetrievedSymbol>,
    /// Human-readable warnings, including invalid citations.
    pub warnings: Vec<String>,
    /// Version of the ask prompt that was used.
    pub prompt_version: String,
}

/// Receives streamed answer chunks.
pub trait AskSink: Send {
    /// A chunk of the answer.
    fn on_chunk(&mut self, delta: &str);
}

/// A sink that discards all answer output.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopAskSink;

impl AskSink for NoopAskSink {
    fn on_chunk(&mut self, _delta: &str) {}
}

/// Retrieve the symbols most relevant to `question`.
///
/// Candidates are scored by exact name match, substring match, and a
/// BM25-style term score over symbol names, file paths, and nearby doc
/// comments. The codebase model boosts symbols in matching subsystems,
/// chapters, and flows. The index expands the top seeds with directly related
/// symbols. Results are sorted by score and truncated to `options.top_n`.
pub fn retrieve(
    repo_root: impl AsRef<Path>,
    skeleton: &Skeleton,
    model: &CodebaseModel,
    index: Option<&Index>,
    question: &str,
    options: &AskOptions,
) -> Vec<RetrievedSymbol> {
    let repo_root = repo_root.as_ref();
    let query = tokenize(question);
    if query.is_empty() || skeleton.symbols.is_empty() {
        return Vec::new();
    }

    let mut cache = DocCache::new(repo_root, MAX_CORPUS_BYTES);
    let mut docs = Vec::with_capacity(skeleton.symbols.len());
    let mut doc_texts = Vec::with_capacity(skeleton.symbols.len());
    for symbol in &skeleton.symbols {
        let text = cache.text(&symbol.file).unwrap_or_default();
        let doc = doc_for_symbol(&text, symbol.line);
        let mut tokens = tokenize(&symbol.name);
        tokens.extend(tokenize(&doc));
        tokens.extend(tokenize(&symbol.file.to_string_lossy()));
        docs.push(tokens);
        doc_texts.push(doc);
    }

    let bm25 = bm25_scores(&docs, &query);
    let boosts = model_boosts(model, &query);
    let question_lower = question.trim().to_lowercase();
    let mut scored: Vec<(f64, usize)> = Vec::new();
    for (index, symbol) in skeleton.symbols.iter().enumerate() {
        let mut score = bm25[index];
        let name_lower = symbol.name.to_lowercase();
        let file_lower = symbol.file.to_string_lossy().to_lowercase();
        for term in &query {
            if name_lower == *term {
                score += 12.0;
            } else if name_lower.contains(term) {
                score += 4.0;
            }
            if file_lower.contains(term) {
                score += 2.0;
            }
        }
        if name_lower == question_lower {
            score += 20.0;
        }
        score += boosts.get(&symbol.id).copied().unwrap_or(0.0);
        if score > 0.0 {
            scored.push((score, index));
        }
    }

    scored.sort_by(|a, b| compare_scores(skeleton, *a, *b));
    expand_with_index(skeleton, index, &mut scored);
    scored.sort_by(|a, b| compare_scores(skeleton, *a, *b));

    scored
        .into_iter()
        .take(options.top_n)
        .map(|(score, index)| {
            let symbol = &skeleton.symbols[index];
            RetrievedSymbol {
                id: symbol.id.clone(),
                name: symbol.name.clone(),
                file: symbol.file.to_string_lossy().into_owned(),
                line: symbol.line,
                score,
                doc: doc_texts[index].clone(),
            }
        })
        .collect()
}

fn compare_scores(skeleton: &Skeleton, a: (f64, usize), b: (f64, usize)) -> std::cmp::Ordering {
    b.0.partial_cmp(&a.0)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| skeleton.symbols[a.1].file.cmp(&skeleton.symbols[b.1].file))
        .then_with(|| skeleton.symbols[a.1].line.cmp(&skeleton.symbols[b.1].line))
        .then_with(|| skeleton.symbols[a.1].id.cmp(&skeleton.symbols[b.1].id))
}

fn expand_with_index(skeleton: &Skeleton, index: Option<&Index>, scored: &mut Vec<(f64, usize)>) {
    let Some(index) = index else {
        return;
    };
    let by_id: BTreeMap<&str, usize> = skeleton
        .symbols
        .iter()
        .enumerate()
        .map(|(position, symbol)| (symbol.id.as_str(), position))
        .collect();
    let seeds: Vec<usize> = scored.iter().take(3).map(|(_, index)| *index).collect();
    let mut extra: BTreeMap<usize, f64> = BTreeMap::new();
    for seed in seeds {
        let id = &skeleton.symbols[seed].id;
        for edge in &index.symbol_edges {
            if edge.from == *id {
                if let Some(&position) = by_id.get(edge.to.as_str()) {
                    *extra.entry(position).or_insert(0.0) += 0.5;
                }
            }
            if edge.to == *id {
                if let Some(&position) = by_id.get(edge.from.as_str()) {
                    *extra.entry(position).or_insert(0.0) += 0.5;
                }
            }
        }
    }
    for (position, bonus) in extra {
        if let Some((score, _)) = scored.iter_mut().find(|(_, index)| *index == position) {
            *score += bonus;
        } else {
            scored.push((bonus, position));
        }
    }
}

fn model_boosts(model: &CodebaseModel, query: &[String]) -> BTreeMap<String, f64> {
    let matches = |text: &str| {
        let lower = text.to_lowercase();
        query.iter().any(|term| lower.contains(term))
    };
    let mut boosts = BTreeMap::new();
    for subsystem in &model.subsystems {
        if matches(&subsystem.title) || matches(&subsystem.summary) {
            for id in &subsystem.symbol_ids {
                *boosts.entry(id.clone()).or_insert(0.0) += 3.0;
            }
        }
    }
    for chapter in &model.chapters {
        if matches(&chapter.title) || matches(&chapter.intent) {
            for id in &chapter.stop_ids {
                *boosts.entry(id.clone()).or_insert(0.0) += 2.0;
            }
        }
    }
    for flow in &model.flows {
        if matches(&flow.title) {
            for id in &flow.steps {
                *boosts.entry(id.clone()).or_insert(0.0) += 2.0;
            }
        }
    }
    boosts
}

fn bm25_scores(docs: &[Vec<String>], query: &[String]) -> Vec<f64> {
    let count = docs.len() as f64;
    if count == 0.0 {
        return Vec::new();
    }
    let average_length = docs.iter().map(|doc| doc.len() as f64).sum::<f64>() / count;
    let mut document_frequency: HashMap<&str, usize> = HashMap::new();
    for doc in docs {
        let unique: BTreeSet<&str> = doc.iter().map(String::as_str).collect();
        for term in unique {
            *document_frequency.entry(term).or_insert(0) += 1;
        }
    }
    let k1 = 1.2;
    let b = 0.75;
    docs.iter()
        .map(|doc| {
            let length = doc.len() as f64;
            let mut score = 0.0;
            for term in query {
                let frequency = doc.iter().filter(|token| *token == term).count() as f64;
                if frequency == 0.0 {
                    continue;
                }
                let frequency_in_documents =
                    *document_frequency.get(term.as_str()).unwrap_or(&0) as f64;
                let idf = (1.0
                    + (count - frequency_in_documents + 0.5) / (frequency_in_documents + 0.5))
                    .ln();
                let denominator = frequency + k1 * (1.0 - b + b * length / average_length.max(1.0));
                score += idf * (frequency * (k1 + 1.0)) / denominator;
            }
            score
        })
        .collect()
}

fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() || character == '_' {
            current.push(character);
        } else if !current.is_empty() {
            push_identifier_tokens(&mut tokens, &current);
            current.clear();
        }
    }
    if !current.is_empty() {
        push_identifier_tokens(&mut tokens, &current);
    }
    tokens
}

fn push_identifier_tokens(tokens: &mut Vec<String>, raw: &str) {
    let mut part = String::new();
    let mut previous_lower = false;
    for character in raw.chars() {
        if character == '_' || character == '-' {
            if !part.is_empty() {
                push_token(tokens, &part);
                part.clear();
            }
            previous_lower = false;
        } else if character.is_uppercase() && previous_lower {
            if !part.is_empty() {
                push_token(tokens, &part);
                part.clear();
            }
            part.push(character);
            previous_lower = false;
        } else {
            previous_lower = character.is_lowercase() || character.is_numeric();
            part.push(character);
        }
    }
    if !part.is_empty() {
        push_token(tokens, &part);
    }
}

fn push_token(tokens: &mut Vec<String>, raw: &str) {
    let token = raw.to_lowercase();
    if token.len() > 1 && !is_stopword(&token) {
        tokens.push(token);
    }
}

fn is_stopword(token: &str) -> bool {
    matches!(
        token,
        "the"
            | "a"
            | "an"
            | "is"
            | "are"
            | "was"
            | "were"
            | "what"
            | "which"
            | "who"
            | "how"
            | "why"
            | "where"
            | "when"
            | "does"
            | "do"
            | "did"
            | "to"
            | "of"
            | "in"
            | "on"
            | "for"
            | "and"
            | "or"
            | "not"
            | "this"
            | "that"
            | "these"
            | "those"
            | "it"
            | "its"
            | "with"
            | "from"
            | "by"
            | "as"
            | "at"
            | "be"
            | "been"
            | "being"
            | "can"
            | "could"
            | "should"
            | "would"
            | "will"
            | "i"
            | "you"
            | "we"
            | "they"
            | "he"
            | "she"
            | "them"
            | "our"
            | "your"
            | "my"
            | "me"
            | "us"
            | "about"
            | "into"
            | "over"
            | "under"
            | "between"
            | "through"
            | "during"
            | "before"
            | "after"
            | "above"
            | "below"
            | "up"
            | "down"
            | "out"
            | "off"
            | "then"
            | "than"
            | "so"
            | "if"
            | "but"
            | "because"
            | "while"
            | "there"
            | "here"
            | "all"
            | "any"
            | "both"
            | "each"
            | "few"
            | "more"
            | "most"
            | "other"
            | "some"
            | "such"
            | "no"
            | "nor"
            | "only"
            | "own"
            | "same"
            | "too"
            | "very"
            | "just"
            | "now"
    )
}

struct DocCache {
    repo_root: PathBuf,
    files: HashMap<PathBuf, Option<String>>,
    bytes_read: usize,
    max_bytes: usize,
}

impl DocCache {
    fn new(repo_root: &Path, max_bytes: usize) -> Self {
        Self {
            repo_root: repo_root.to_path_buf(),
            files: HashMap::new(),
            bytes_read: 0,
            max_bytes,
        }
    }

    fn text(&mut self, file: &Path) -> Option<String> {
        if let Some(text) = self.files.get(file) {
            return text.clone();
        }
        let text = if self.bytes_read >= self.max_bytes {
            None
        } else {
            match fs::read(self.repo_root.join(file)) {
                Ok(bytes) => {
                    self.bytes_read += bytes.len();
                    Some(String::from_utf8_lossy(&bytes).into_owned())
                }
                Err(_) => None,
            }
        };
        self.files.insert(file.to_path_buf(), text.clone());
        text
    }
}

fn doc_for_symbol(text: &str, line: u32) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let index = (line.saturating_sub(1) as usize).min(lines.len());
    let start = index.saturating_sub(MAX_DOC_LINES);
    let mut doc = Vec::new();
    for line in &lines[start..index] {
        let trimmed = line.trim();
        if is_comment(trimmed) {
            doc.push(trimmed);
        }
    }
    if doc.is_empty() {
        for line in lines.iter().take(MAX_DOC_LINES) {
            let trimmed = line.trim();
            if is_comment(trimmed) {
                doc.push(trimmed);
            }
        }
    }
    doc.join("\n")
}

fn is_comment(line: &str) -> bool {
    line.starts_with("//")
        || line.starts_with('#')
        || line.starts_with("/*")
        || line.starts_with('*')
        || line.starts_with("--")
        || line.starts_with(';')
}

fn build_context(
    repo_root: &Path,
    skeleton: &Skeleton,
    model: &CodebaseModel,
    retrieved: &[RetrievedSymbol],
    options: &AskOptions,
) -> Result<Value, AskError> {
    let mut items = Vec::new();
    let mut valid_ids = BTreeSet::new();
    let mut valid_citations = Vec::new();
    let mut used = 0usize;
    for symbol in retrieved {
        let Some(node) = skeleton.symbols.iter().find(|node| node.id == symbol.id) else {
            continue;
        };
        let stop = plan::stop_for_symbol(repo_root, skeleton, node).ok();
        let (start, end) = stop.map_or((symbol.line, symbol.line), |stop| {
            (stop.start_line, stop.end_line)
        });
        let text = fs::read_to_string(repo_root.join(&symbol.file)).unwrap_or_default();
        let region = extract_region(&text, start, end);
        let item = json!({
            "id": symbol.id,
            "name": symbol.name,
            "file": symbol.file,
            "start_line": start,
            "end_line": end,
            "score": symbol.score,
            "doc": symbol.doc,
            "code": region,
        });
        let size = serde_json::to_string(&item).map_or(0, |rendered| rendered.len());
        if used + size > options.max_context_bytes && !items.is_empty() {
            break;
        }
        used += size;
        valid_ids.insert(symbol.id.clone());
        valid_citations.push(format!("{}:{start}", symbol.file));
        items.push(item);
    }

    Ok(json!({
        "codebase_model": {
            "subsystems": model.subsystems,
            "chapters": model.chapters,
            "flows": model.flows,
            "glossary": model.glossary,
        },
        "symbols": items,
        "valid_symbol_ids": valid_ids.iter().collect::<Vec<_>>(),
        "valid_citations": valid_citations,
    }))
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

fn validate_citations(
    repo_root: &Path,
    skeleton: &Skeleton,
    answer: &str,
) -> (Vec<Citation>, Vec<String>) {
    static CITATION_RE: OnceLock<Regex> = OnceLock::new();
    let regex = CITATION_RE.get_or_init(|| {
        Regex::new(r"([A-Za-z0-9_./\\-]+\.[A-Za-z0-9]+):(\d+)").expect("citation regex compiles")
    });
    let mut citations = Vec::new();
    let mut warnings = Vec::new();
    let mut seen = BTreeSet::new();
    for captures in regex.captures_iter(answer) {
        let file = captures.get(1).map_or("", |matched| matched.as_str());
        let line = captures
            .get(2)
            .and_then(|matched| matched.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        if line == 0 {
            continue;
        }
        let Some(node) = skeleton
            .files
            .iter()
            .find(|node| node.path == Path::new(file) || node.path.ends_with(file))
        else {
            warnings.push(format!("answer cited unknown file {file}:{line}"));
            continue;
        };
        let path = repo_root.join(&node.path);
        let Ok(text) = fs::read_to_string(&path) else {
            warnings.push(format!("answer cited unreadable file {file}:{line}"));
            continue;
        };
        let line_count = u32::try_from(text.lines().count()).unwrap_or(u32::MAX);
        if line > line_count {
            warnings.push(format!("answer cited out-of-range line {file}:{line}"));
            continue;
        }
        let key = (node.path.to_string_lossy().into_owned(), line);
        if seen.insert(key.clone()) {
            citations.push(Citation { file: key.0, line });
        }
    }
    (citations, warnings)
}

fn answer_is_unknown(answer: &str) -> bool {
    let lower = answer.to_lowercase();
    [
        "don't know",
        "do not know",
        "not enough",
        "insufficient",
        "cannot answer",
        "can't answer",
        "no evidence",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
}

fn load_prompt() -> (String, String) {
    match prompts::load(ASK) {
        Ok(prompt) => (prompt.contents, prompt.version.to_string()),
        Err(_) => {
            let embedded = prompts::embedded(ASK).expect("ask prompt is embedded");
            (embedded.contents.to_owned(), embedded.version.to_string())
        }
    }
}

/// Answer `question` from the codebase model and retrieved code.
///
/// The assembled context is redacted before it is sent. The answer is streamed
/// to `sink` and validated for `file:line` citations. If retrieval finds no
/// evidence, a deterministic "I don't know" answer is returned without calling
/// the model.
pub async fn ask(
    repo_root: impl AsRef<Path>,
    config: &Config,
    skeleton: &Skeleton,
    model: &CodebaseModel,
    question: &str,
    client: &LlmClient,
    sink: &mut dyn AskSink,
) -> Result<AskReport, AskError> {
    let repo_root = repo_root.as_ref();
    let question = question.trim();
    if question.is_empty() {
        return Err(AskError::EmptyQuestion);
    }
    let original_question = question.to_owned();
    let index = load_index(repo_root).ok().flatten();
    let options = AskOptions::default();
    let retrieved = retrieve(
        repo_root,
        skeleton,
        model,
        index.as_ref(),
        question,
        &options,
    );
    let (prompt, prompt_version) = load_prompt();
    let mut redactor = Redactor::new(&config.redaction)?;

    if retrieved.is_empty() {
        let answer =
            "I don't know — the codebase model does not contain enough evidence to answer that."
                .to_owned();
        sink.on_chunk(&answer);
        return Ok(AskReport {
            question: original_question,
            answer,
            citations: Vec::new(),
            retrieved,
            warnings: vec!["no relevant symbols were found".to_owned()],
            prompt_version,
        });
    }

    let mut payload = build_context(repo_root, skeleton, model, &retrieved, &options)?;
    redactor.redact_value(&mut payload);
    let (question, _) = redactor.redact(question);
    let user = format!(
        "Question: {question}\n\nContext:\n{}",
        serde_json::to_string_pretty(&payload)?
    );
    let (system, _) = redactor.redact(&prompt);
    let (user, _) = redactor.redact(&user);
    let messages = vec![ChatMessage::system(system), ChatMessage::user(user)];

    let mut last_error: Option<LlmError> = None;
    for _attempt in 0..MAX_ATTEMPTS {
        let request = ChatRequest::new(messages.clone());
        let mut stream = match client.chat_stream(&request).await {
            Ok(stream) => stream,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };
        let mut answer = String::new();
        let mut emitted = false;
        let mut stream_error = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => {
                    if !chunk.delta.is_empty() {
                        let (delta, _) = redactor.redact(&chunk.delta);
                        sink.on_chunk(&delta);
                        answer.push_str(&delta);
                        emitted = true;
                    }
                }
                Err(error) => {
                    stream_error = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = stream_error {
            last_error = Some(error);
            if emitted {
                break;
            }
            continue;
        }
        if answer.trim().is_empty() {
            last_error = Some(LlmError::InvalidResponse(
                "ask response was empty".to_owned(),
            ));
            if emitted {
                break;
            }
            continue;
        }
        let (mut answer, _) = redactor.redact(&answer);
        let (citations, mut warnings) = validate_citations(repo_root, skeleton, &answer);
        if citations.is_empty() && !answer_is_unknown(&answer) {
            let sources: Vec<String> = retrieved
                .iter()
                .map(|symbol| format!("{}:{}", symbol.file, symbol.line))
                .collect();
            if !sources.is_empty() {
                answer.push_str("\n\nSources:\n");
                for source in &sources {
                    answer.push_str(&format!("- {source}\n"));
                }
                warnings
                    .push("answer had no valid citations; appended retrieved sources".to_owned());
            }
        }
        return Ok(AskReport {
            question: original_question,
            answer,
            citations,
            retrieved,
            warnings,
            prompt_version,
        });
    }

    Err(AskError::Llm(last_error.unwrap_or_else(|| {
        LlmError::InvalidResponse("ask failed".to_owned())
    })))
}

/// Load the skeleton and model artifacts and answer `question`.
pub async fn ask_from_artifacts(
    repo_root: impl AsRef<Path>,
    config: &Config,
    question: &str,
    client: &LlmClient,
    sink: &mut dyn AskSink,
) -> Result<AskReport, AskError> {
    let repo_root = repo_root.as_ref();
    let skeleton = plan::load_skeleton(repo_root)?;
    let model = plan::load_model(repo_root)?;
    ask(repo_root, config, &skeleton, &model, question, client, sink).await
}

/// Errors produced while answering a question.
#[derive(Debug)]
pub enum AskError {
    /// The question was empty.
    EmptyQuestion,
    /// The prompt could not be serialized.
    Serialize(serde_json::Error),
    /// Redaction rules could not be compiled.
    Redact(RedactError),
    /// An artifact could not be loaded.
    Plan(PlanError),
    /// The LLM call failed after retries.
    Llm(LlmError),
}

impl fmt::Display for AskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyQuestion => write!(f, "question is empty"),
            Self::Serialize(error) => write!(f, "{error}"),
            Self::Redact(error) => write!(f, "{error}"),
            Self::Plan(error) => write!(f, "{error}"),
            Self::Llm(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for AskError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) => Some(error),
            Self::Redact(error) => Some(error),
            Self::Plan(error) => Some(error),
            Self::Llm(error) => Some(error),
            Self::EmptyQuestion => None,
        }
    }
}

impl From<serde_json::Error> for AskError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

impl From<RedactError> for AskError {
    fn from(error: RedactError) -> Self {
        Self::Redact(error)
    }
}

impl From<PlanError> for AskError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}

impl From<LlmError> for AskError {
    fn from(error: LlmError) -> Self {
        Self::Llm(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CodebaseModel, SCHEMA_VERSION};
    use crate::recon::{Community, FileNode, SymbolNode};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("laudit-ask-test-{}-{id}", std::process::id()));
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

    fn model() -> CodebaseModel {
        CodebaseModel {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            repo_id: "fnv1a128:test".to_owned(),
            head: None,
            dirty_hash: "fnv1a128:test".to_owned(),
            generator: "laudit".to_owned(),
            model: "test-model".to_owned(),
            prompt_version: "1".to_owned(),
            generated_at: "2026-11-12T00:00:00Z".to_owned(),
            subsystems: Vec::new(),
            chapters: Vec::new(),
            flows: Vec::new(),
            glossary: Vec::new(),
        }
    }

    #[test]
    fn tokenize_splits_identifiers_and_drops_stopwords() {
        assert_eq!(tokenize("parse_file"), vec!["parse", "file"]);
        assert_eq!(tokenize("parseFile"), vec!["parse", "file"]);
        assert_eq!(tokenize("where is the parser"), vec!["parser"]);
    }

    #[test]
    fn exact_name_match_ranks_first() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        temp.write("src/parser.rs", "fn parse_file() {}\n");
        let skeleton = skeleton(
            vec![
                symbol("sym-main", "src/main.rs", "main", 1),
                symbol("sym-parse", "src/parser.rs", "parse_file", 1),
            ],
            vec!["src/main.rs", "src/parser.rs"],
        );
        let retrieved = retrieve(
            temp.path(),
            &skeleton,
            &model(),
            None,
            "parse_file",
            &AskOptions::default(),
        );
        assert_eq!(retrieved[0].id, "sym-parse");
    }

    #[test]
    fn doc_comment_terms_are_retrieved() {
        let temp = TempDir::new();
        temp.write(
            "src/parser.rs",
            "// Parses the configuration file into a tree.\nfn build() {}\n",
        );
        temp.write("src/main.rs", "fn main() {}\n");
        let skeleton = skeleton(
            vec![
                symbol("sym-build", "src/parser.rs", "build", 2),
                symbol("sym-main", "src/main.rs", "main", 1),
            ],
            vec!["src/parser.rs", "src/main.rs"],
        );
        let retrieved = retrieve(
            temp.path(),
            &skeleton,
            &model(),
            None,
            "configuration tree",
            &AskOptions::default(),
        );
        assert_eq!(retrieved[0].id, "sym-build");
    }

    #[test]
    fn citations_are_validated_against_the_skeleton() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let skeleton = skeleton(
            vec![symbol("sym-main", "src/main.rs", "main", 1)],
            vec!["src/main.rs"],
        );
        let (citations, warnings) = validate_citations(
            temp.path(),
            &skeleton,
            "See src/main.rs:1 and src/missing.rs:4 and src/main.rs:99.",
        );
        assert_eq!(
            citations,
            vec![Citation {
                file: "src/main.rs".to_owned(),
                line: 1,
            }]
        );
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn empty_retrieval_returns_no_symbols() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let skeleton = skeleton(
            vec![symbol("sym-main", "src/main.rs", "main", 1)],
            vec!["src/main.rs"],
        );
        let retrieved = retrieve(
            temp.path(),
            &skeleton,
            &model(),
            None,
            "quantum chromodynamics",
            &AskOptions::default(),
        );
        assert!(retrieved.is_empty());
    }
}

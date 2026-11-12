//! MCP tools backed by `laudit-core` handlers.
//!
//! Each tool validates its arguments, calls the same engine entry points the
//! CLI and RPC server use, and redacts the structured result before returning
//! it. No engine logic is duplicated here.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use laudit_core::build;
use laudit_core::comprehension;
use laudit_core::config::Config;
use laudit_core::explain;
use laudit_core::llm::LlmClient;
use laudit_core::model::{Stop, Tour};
use laudit_core::narration;
use laudit_core::plan;
use laudit_core::redact::Redactor;
use laudit_core::status;

/// Names of the tools exposed by this server.
pub const TOOL_NAMES: &[&str] = &["build_tour", "explain", "outline", "status"];

/// A tool call failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// The requested tool does not exist.
    Unknown(String),
    /// The tool ran but failed.
    Failed(String),
}

/// Describe the available tools for `tools/list`.
#[must_use]
pub fn list() -> Value {
    json!({
        "tools": [
            {
                "name": "build_tour",
                "description": "Build a guided tour for a repository and return its outline.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo": {
                            "type": "string",
                            "description": "Repository root path."
                        },
                        "offline": {
                            "type": "boolean",
                            "description": "Run recon and planning only; no LLM calls."
                        }
                    },
                    "required": ["repo"],
                    "additionalProperties": false
                }
            },
            {
                "name": "explain",
                "description": "Explain one symbol using the codebase model and tour context.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo": {
                            "type": "string",
                            "description": "Repository root path."
                        },
                        "symbol": {
                            "type": "string",
                            "description": "Symbol name, file:symbol, file:line, or symbol id."
                        }
                    },
                    "required": ["repo", "symbol"],
                    "additionalProperties": false
                }
            },
            {
                "name": "outline",
                "description": "Return the outline of the most recent tour.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo": {
                            "type": "string",
                            "description": "Repository root path."
                        }
                    },
                    "required": ["repo"],
                    "additionalProperties": false
                }
            },
            {
                "name": "status",
                "description": "Report model/tour presence and per-status stop counts.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo": {
                            "type": "string",
                            "description": "Repository root path."
                        }
                    },
                    "required": ["repo"],
                    "additionalProperties": false
                }
            }
        ]
    })
}

/// Call a tool by name with JSON arguments.
pub async fn call(name: &str, arguments: &Value) -> Result<Value, ToolError> {
    match name {
        "build_tour" => build_tour(arguments).await,
        "explain" => explain(arguments).await,
        "outline" => outline(arguments),
        "status" => status(arguments),
        other => Err(ToolError::Unknown(other.to_owned())),
    }
}

async fn build_tour(arguments: &Value) -> Result<Value, ToolError> {
    let repo = repo_arg(arguments)?;
    let offline = arguments
        .get("offline")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match build_tour_inner(&repo, offline).await {
        Ok(mut value) => {
            redact_value(&repo, &mut value);
            Ok(value)
        }
        Err(message) => Err(ToolError::Failed(redact_text(&repo, &message))),
    }
}

async fn build_tour_inner(repo: &Path, offline: bool) -> Result<Value, String> {
    let options = plan::PlanOptions::default();
    let tour = if offline {
        let config = Config::load_offline(repo).map_err(|error| error.to_string())?;
        build::build_offline(repo, &config).map_err(|error| error.to_string())?;
        plan::plan_from_artifacts(repo, &config, &options)
            .map_err(|error| error.to_string())?
            .tour
    } else {
        let config = Config::load(repo).map_err(|error| error.to_string())?;
        let client = LlmClient::from_config(&config).map_err(|error| error.to_string())?;
        comprehension::build(repo, &config, &client)
            .await
            .map_err(|error| error.to_string())?;
        let tour_id = plan::plan_from_artifacts(repo, &config, &options)
            .map_err(|error| error.to_string())?
            .tour
            .tour_id;
        let mut sink = narration::NoopSink;
        narration::narrate_from_artifacts(repo, &config, &tour_id, &client, &mut sink)
            .await
            .map_err(|error| error.to_string())?
            .tour
    };

    let tour_id = tour.tour_id.clone();
    let outline = outline_value(&tour);
    Ok(json!({
        "tour_id": tour_id,
        "outline": outline,
    }))
}

async fn explain(arguments: &Value) -> Result<Value, ToolError> {
    let repo = repo_arg(arguments)?;
    let symbol = arguments
        .get("symbol")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Failed("symbol is required".to_owned()))?;
    match explain_inner(&repo, symbol).await {
        Ok(mut value) => {
            redact_value(&repo, &mut value);
            Ok(value)
        }
        Err(message) => Err(ToolError::Failed(redact_text(&repo, &message))),
    }
}

async fn explain_inner(repo: &Path, symbol: &str) -> Result<Value, String> {
    let config = match Config::load(repo) {
        Ok(config) => config,
        Err(_) => return explain_offline(repo, symbol),
    };
    let client = match LlmClient::from_config(&config) {
        Ok(client) => client,
        Err(_) => return explain_offline(repo, symbol),
    };

    let skeleton = explain::load_skeleton(repo, &config).map_err(|error| error.to_string())?;
    let model = explain::load_model(repo).unwrap_or_else(|_| build::offline_model(&skeleton));
    let resolved = explain::resolve(&skeleton, symbol).map_err(|error| error.to_string())?;
    let mut sink = narration::NoopSink;
    let report = explain::explain(
        repo, &config, &skeleton, &model, &resolved, &client, &mut sink,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(stop_value(&resolved, &report.stop))
}

fn explain_offline(repo: &Path, symbol: &str) -> Result<Value, String> {
    let config = Config::load_offline(repo).map_err(|error| error.to_string())?;
    let skeleton = explain::load_skeleton(repo, &config).map_err(|error| error.to_string())?;
    let resolved = explain::resolve(&skeleton, symbol).map_err(|error| error.to_string())?;
    let node = skeleton
        .symbols
        .iter()
        .find(|node| node.id == resolved.id)
        .ok_or_else(|| "symbol not found".to_owned())?;
    let stop = plan::stop_for_symbol(repo, &skeleton, node).map_err(|error| error.to_string())?;
    Ok(stop_value(&resolved, &stop))
}

fn outline(arguments: &Value) -> Result<Value, ToolError> {
    let repo = repo_arg(arguments)?;
    match outline_inner(&repo) {
        Ok(mut value) => {
            redact_value(&repo, &mut value);
            Ok(value)
        }
        Err(message) => Err(ToolError::Failed(redact_text(&repo, &message))),
    }
}

fn outline_inner(repo: &Path) -> Result<Value, String> {
    let tour_id = status::latest_tour_id(repo)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "no tour found".to_owned())?;
    let tour = plan::load_tour(repo, &tour_id).map_err(|error| error.to_string())?;
    Ok(json!({
        "tour_id": tour_id,
        "chapters": outline_chapters(&tour),
    }))
}

fn status(arguments: &Value) -> Result<Value, ToolError> {
    let repo = repo_arg(arguments)?;
    match status_inner(&repo) {
        Ok(mut value) => {
            redact_value(&repo, &mut value);
            Ok(value)
        }
        Err(message) => Err(ToolError::Failed(redact_text(&repo, &message))),
    }
}

fn status_inner(repo: &Path) -> Result<Value, String> {
    let report = status::status(repo, None).map_err(|error| error.to_string())?;
    Ok(json!({
        "model_present": report.model_present,
        "tour_present": report.tour_present,
        "tour_id": report.tour_id,
        "position": report.position,
        "counts": {
            "chapters": report.chapters,
            "stops": report.stops,
            "explained": report.explained,
            "pending": report.pending,
            "streaming": report.streaming,
            "skipped": report.skipped,
            "failed": report.failed,
            "stale": report.stale,
        },
    }))
}

fn repo_arg(arguments: &Value) -> Result<PathBuf, ToolError> {
    let repo = arguments
        .get("repo")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Failed("repo is required".to_owned()))?;
    let path = PathBuf::from(repo);
    if !path.is_dir() {
        return Err(ToolError::Failed(format!(
            "repo is not a directory: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn stop_value(symbol: &explain::ResolvedSymbol, stop: &Stop) -> Value {
    json!({
        "symbol": symbol.id,
        "file": stop.file,
        "start_line": stop.start_line,
        "end_line": stop.end_line,
        "title": stop.title,
        "why_here": stop.why_here,
        "explanation": stop.explanation,
        "refs": stop.refs,
        "status": stop.status,
    })
}

fn outline_value(tour: &Tour) -> Value {
    json!({
        "tour_id": tour.tour_id,
        "chapters": outline_chapters(tour),
    })
}

fn outline_chapters(tour: &Tour) -> Value {
    Value::Array(
        tour.chapters
            .iter()
            .map(|chapter| {
                json!({
                    "id": chapter.id,
                    "title": chapter.title,
                    "stops": chapter.stops.iter().map(|stop| json!({
                        "id": stop.id,
                        "title": stop.title,
                        "why_here": stop.why_here,
                        "file": stop.file,
                        "start_line": stop.start_line,
                        "end_line": stop.end_line,
                        "explanation": stop.explanation,
                        "status": stop.status,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

fn redact_value(repo: &Path, value: &mut Value) {
    let redaction = Config::load_offline(repo)
        .map(|config| config.redaction)
        .unwrap_or_default();
    if let Ok(mut redactor) = Redactor::new(&redaction) {
        redactor.redact_value(value);
    }
}

fn redact_text(repo: &Path, text: &str) -> String {
    let redaction = Config::load_offline(repo)
        .map(|config| config.redaction)
        .unwrap_or_default();
    Redactor::new(&redaction)
        .map_or_else(|_| text.to_owned(), |mut redactor| redactor.redact(text).0)
}

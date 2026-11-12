//! Versioned artifact schemas.
//!
//! These types are the durable interface between the engine, frontends, and
//! future tools. They serialize to the JSON shapes documented in
//! `docs/artifacts.md` and every artifact carries a `schema_version` and a
//! `tool_version` (D16).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Current artifact schema version.
///
/// Bump this only on breaking shape changes. Readers should reject artifacts
/// whose `schema_version` they do not understand.
pub const SCHEMA_VERSION: u32 = 1;

/// Error returned when an artifact declares an unsupported schema version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaVersionError {
    /// The version found in the artifact.
    pub found: u32,
    /// The version this build understands.
    pub expected: u32,
}

impl fmt::Display for SchemaVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported artifact schema_version {} (this build supports {})",
            self.found, self.expected
        )
    }
}

impl std::error::Error for SchemaVersionError {}

/// Check that `version` is a schema version this build understands.
pub fn check_schema_version(version: u32) -> Result<(), SchemaVersionError> {
    if version == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(SchemaVersionError {
            found: version,
            expected: SCHEMA_VERSION,
        })
    }
}

/// The whole-codebase model produced by Stage 1 comprehension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodebaseModel {
    /// Artifact schema version.
    pub schema_version: u32,
    /// Engine version that produced the artifact.
    pub tool_version: String,
    /// Stable identifier for the repository path.
    pub repo_id: String,
    /// Git commit the model was built from, if any.
    #[serde(default)]
    pub head: Option<String>,
    /// Hash of the working-tree state.
    pub dirty_hash: String,
    /// Name of the generator that produced the artifact.
    pub generator: String,
    /// Model identifier used for comprehension.
    pub model: String,
    /// Version of the prompt used for comprehension.
    pub prompt_version: String,
    /// RFC 3339 timestamp of generation.
    pub generated_at: String,
    /// Subsystems discovered in the codebase.
    pub subsystems: Vec<Subsystem>,
    /// Chapters that structure the tour.
    pub chapters: Vec<ModelChapter>,
    /// Cross-cutting flows through the codebase.
    pub flows: Vec<Flow>,
    /// Terms worth defining for a newcomer.
    pub glossary: Vec<GlossaryEntry>,
}

impl CodebaseModel {
    /// Reject an artifact whose schema version this build does not understand.
    pub fn validate_schema_version(&self) -> Result<(), SchemaVersionError> {
        check_schema_version(self.schema_version)
    }
}

/// A coherent part of the codebase, such as a layer or module group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subsystem {
    /// Stable subsystem identifier.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Short summary of the subsystem's role.
    pub summary: String,
    /// Symbol IDs that belong to the subsystem.
    pub symbol_ids: Vec<String>,
    /// Files that belong to the subsystem.
    pub files: Vec<String>,
}

/// A chapter in the codebase model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelChapter {
    /// Stable chapter identifier.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// What the chapter is meant to teach.
    pub intent: String,
    /// Stops that make up the chapter.
    pub stop_ids: Vec<String>,
}

/// A cross-cutting flow through the codebase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flow {
    /// Stable flow identifier.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Ordered symbol IDs that participate in the flow.
    pub steps: Vec<String>,
}

/// A glossary entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlossaryEntry {
    /// Term being defined.
    pub term: String,
    /// Definition of the term.
    pub definition: String,
}

/// A tour produced by Stage 2 planning and filled in by Stage 3 narration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tour {
    /// Artifact schema version.
    pub schema_version: u32,
    /// Engine version that produced the artifact.
    pub tool_version: String,
    /// Stable tour identifier.
    pub tour_id: String,
    /// What the tour was built from.
    pub source: TourSource,
    /// Model identifier used for narration.
    pub model: String,
    /// Version of the prompt used for narration.
    pub prompt_version: String,
    /// Redaction profile applied to the tour.
    pub redaction_profile: String,
    /// Current position within the tour.
    #[serde(default)]
    pub position: Position,
    /// Chapters and their stops.
    pub chapters: Vec<TourChapter>,
}

impl Tour {
    /// Reject an artifact whose schema version this build does not understand.
    pub fn validate_schema_version(&self) -> Result<(), SchemaVersionError> {
        check_schema_version(self.schema_version)
    }
}

/// The source a tour was built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TourSource {
    /// Source kind, for example `codebase`.
    pub kind: String,
}

impl TourSource {
    /// A whole-codebase tour source.
    #[must_use]
    pub fn codebase() -> Self {
        Self {
            kind: "codebase".to_owned(),
        }
    }
}

/// Current position within a tour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    /// Index of the current chapter.
    pub chapter: u32,
    /// Index of the current stop within the chapter.
    pub stop: u32,
}

/// A chapter in a tour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TourChapter {
    /// Stable chapter identifier.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Ordered stops in the chapter.
    pub stops: Vec<Stop>,
}

/// A single anchored stop in a tour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stop {
    /// Stable stop identifier.
    pub id: String,
    /// File containing the stop.
    pub file: String,
    /// First line of the stop, 1-based.
    pub start_line: u32,
    /// Last line of the stop, 1-based and inclusive.
    pub end_line: u32,
    /// Symbol the stop is anchored to.
    pub symbol_id: String,
    /// Human-readable title.
    pub title: String,
    /// Why this stop is part of the tour.
    pub why_here: String,
    /// Markdown explanation of the stop.
    pub explanation: String,
    /// Symbol IDs referenced by the explanation.
    #[serde(default)]
    pub refs: Vec<String>,
    /// Narration status of the stop.
    #[serde(default)]
    pub status: StopStatus,
}

/// Narration status of a stop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StopStatus {
    /// Not yet narrated.
    #[default]
    Pending,
    /// Narration is in progress.
    Streaming,
    /// Narration completed successfully.
    Explained,
    /// Narration was skipped.
    Skipped,
    /// Narration failed.
    Failed,
}

/// The placeholder map produced by redaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Redactions {
    /// Artifact schema version.
    pub schema_version: u32,
    /// Engine version that produced the artifact.
    pub tool_version: String,
    /// Placeholder to origin mapping. Never contains secret values.
    pub placeholders: BTreeMap<String, Placeholder>,
}

impl Redactions {
    /// Reject an artifact whose schema version this build does not understand.
    pub fn validate_schema_version(&self) -> Result<(), SchemaVersionError> {
        check_schema_version(self.schema_version)
    }
}

/// Where a redaction placeholder came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placeholder {
    /// Redaction rule that matched.
    pub rule: String,
    /// File containing the redacted value.
    pub file: String,
    /// Line containing the redacted value.
    pub line: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn codebase_model() -> CodebaseModel {
        CodebaseModel {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            repo_id: "sha256:abc".to_owned(),
            head: Some("deadbeef".to_owned()),
            dirty_hash: "sha256:def".to_owned(),
            generator: "laudit".to_owned(),
            model: "gpt-4o-mini".to_owned(),
            prompt_version: "1".to_owned(),
            generated_at: "2026-11-12T00:00:00Z".to_owned(),
            subsystems: vec![Subsystem {
                id: "sub-1".to_owned(),
                title: "HTTP layer".to_owned(),
                summary: "Handles requests.".to_owned(),
                symbol_ids: vec!["sym-1".to_owned()],
                files: vec!["src/http.rs".to_owned()],
            }],
            chapters: vec![ModelChapter {
                id: "ch-1".to_owned(),
                title: "On-ramp".to_owned(),
                intent: "Get oriented.".to_owned(),
                stop_ids: vec!["stop-1".to_owned()],
            }],
            flows: vec![Flow {
                id: "flow-1".to_owned(),
                title: "Request lifecycle".to_owned(),
                steps: vec!["sym-1".to_owned(), "sym-2".to_owned()],
            }],
            glossary: vec![GlossaryEntry {
                term: "PCZT".to_owned(),
                definition: "Partially created Zcash transaction.".to_owned(),
            }],
        }
    }

    fn tour() -> Tour {
        Tour {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            tour_id: "tour-1".to_owned(),
            source: TourSource::codebase(),
            model: "gpt-4o-mini".to_owned(),
            prompt_version: "1".to_owned(),
            redaction_profile: "default".to_owned(),
            position: Position {
                chapter: 0,
                stop: 0,
            },
            chapters: vec![TourChapter {
                id: "ch-1".to_owned(),
                title: "On-ramp".to_owned(),
                stops: vec![Stop {
                    id: "stop-1".to_owned(),
                    file: "src/main.rs".to_owned(),
                    start_line: 1,
                    end_line: 42,
                    symbol_id: "sym-1".to_owned(),
                    title: "Program entry".to_owned(),
                    why_here: "Start here.".to_owned(),
                    explanation: "The program starts here.".to_owned(),
                    refs: vec!["sym-2".to_owned()],
                    status: StopStatus::Explained,
                }],
            }],
        }
    }

    fn redactions() -> Redactions {
        let mut placeholders = BTreeMap::new();
        placeholders.insert(
            "«aws_key#1»".to_owned(),
            Placeholder {
                rule: "aws-access-key".to_owned(),
                file: "src/config.rs".to_owned(),
                line: 12,
            },
        );
        Redactions {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            placeholders,
        }
    }

    fn round_trip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("serialize");
        let decoded: T = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&decoded, value);
    }

    #[test]
    fn codebase_model_round_trips() {
        round_trip(&codebase_model());
    }

    #[test]
    fn tour_round_trips() {
        round_trip(&tour());
    }

    #[test]
    fn redactions_round_trip() {
        round_trip(&redactions());
    }

    #[test]
    fn codebase_model_matches_documented_shape() {
        let value = serde_json::to_value(codebase_model()).expect("serialize");
        let object = value.as_object().expect("object");
        for key in [
            "schema_version",
            "tool_version",
            "repo_id",
            "head",
            "dirty_hash",
            "generator",
            "model",
            "prompt_version",
            "generated_at",
            "subsystems",
            "chapters",
            "flows",
            "glossary",
        ] {
            assert!(object.contains_key(key), "missing {key}");
        }
        assert_eq!(value["subsystems"][0]["id"], json!("sub-1"));
        assert_eq!(value["chapters"][0]["stop_ids"][0], json!("stop-1"));
        assert_eq!(value["flows"][0]["steps"][1], json!("sym-2"));
        assert_eq!(value["glossary"][0]["term"], json!("PCZT"));
    }

    #[test]
    fn tour_matches_documented_shape() {
        let value = serde_json::to_value(tour()).expect("serialize");
        let object = value.as_object().expect("object");
        for key in [
            "schema_version",
            "tool_version",
            "tour_id",
            "source",
            "model",
            "prompt_version",
            "redaction_profile",
            "position",
            "chapters",
        ] {
            assert!(object.contains_key(key), "missing {key}");
        }
        assert_eq!(value["source"]["kind"], json!("codebase"));
        assert_eq!(value["position"]["chapter"], json!(0));
        assert_eq!(value["position"]["stop"], json!(0));
        let stop = &value["chapters"][0]["stops"][0];
        for key in [
            "id",
            "file",
            "start_line",
            "end_line",
            "symbol_id",
            "title",
            "why_here",
            "explanation",
            "refs",
            "status",
        ] {
            assert!(stop.get(key).is_some(), "missing stop.{key}");
        }
        assert_eq!(stop["status"], json!("explained"));
    }

    #[test]
    fn redactions_match_documented_shape() {
        let value = serde_json::to_value(redactions()).expect("serialize");
        assert_eq!(value["schema_version"], json!(1));
        assert_eq!(value["tool_version"], json!("0.1.0"));
        assert_eq!(
            value["placeholders"]["«aws_key#1»"],
            json!({ "rule": "aws-access-key", "file": "src/config.rs", "line": 12 })
        );
    }

    #[test]
    fn stop_status_serializes_lowercase() {
        for (status, expected) in [
            (StopStatus::Pending, "pending"),
            (StopStatus::Streaming, "streaming"),
            (StopStatus::Explained, "explained"),
            (StopStatus::Skipped, "skipped"),
            (StopStatus::Failed, "failed"),
        ] {
            assert_eq!(
                serde_json::to_value(status).expect("serialize"),
                json!(expected)
            );
        }
    }

    #[test]
    fn head_may_be_null() {
        let mut model = codebase_model();
        model.head = None;
        let value = serde_json::to_value(&model).expect("serialize");
        assert!(value["head"].is_null());
        round_trip(&model);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let mut value = serde_json::to_value(tour()).expect("serialize");
        value["unexpected"] = json!(true);
        assert!(serde_json::from_value::<Tour>(value).is_err());
    }

    #[test]
    fn unknown_schema_versions_are_rejected() {
        let mut model = codebase_model();
        model.schema_version = SCHEMA_VERSION + 1;
        let error = model
            .validate_schema_version()
            .expect_err("unknown version");
        assert_eq!(error.found, SCHEMA_VERSION + 1);
        assert_eq!(error.expected, SCHEMA_VERSION);
        assert!(error.to_string().contains("schema_version"));
    }

    #[test]
    fn every_artifact_carries_versions() {
        assert_eq!(codebase_model().schema_version, SCHEMA_VERSION);
        assert_eq!(tour().schema_version, SCHEMA_VERSION);
        assert_eq!(redactions().schema_version, SCHEMA_VERSION);
        assert!(!codebase_model().tool_version.is_empty());
        assert!(!tour().tool_version.is_empty());
        assert!(!redactions().tool_version.is_empty());
    }

    #[test]
    fn documented_json_deserializes() {
        let value: Value = json!({
            "schema_version": 1,
            "tool_version": "0.1.0",
            "tour_id": "tour-1",
            "source": { "kind": "codebase" },
            "model": "gpt-4o-mini",
            "prompt_version": "1",
            "redaction_profile": "default",
            "position": { "chapter": 0, "stop": 0 },
            "chapters": [{
                "id": "ch-1",
                "title": "On-ramp",
                "stops": [{
                    "id": "stop-1",
                    "file": "src/main.rs",
                    "start_line": 1,
                    "end_line": 42,
                    "symbol_id": "sym-1",
                    "title": "Program entry",
                    "why_here": "…",
                    "explanation": "…markdown…",
                    "refs": ["sym-1"],
                    "status": "explained"
                }]
            }]
        });
        let tour: Tour = serde_json::from_value(value).expect("deserialize tour");
        assert_eq!(tour.chapters[0].stops[0].status, StopStatus::Explained);
    }
}

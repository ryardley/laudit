//! Layered secret redaction (D11).
//!
//! Redaction is always on. Every payload that leaves the process — LLM
//! prompts, log lines, cached artifacts — must pass through a [`Redactor`]
//! first. The engine combines:
//!
//! 1. a gitleaks-derived regex ruleset,
//! 2. key-name-aware redaction for env/toml/yaml/json,
//! 3. session-stable placeholders, and
//! 4. user-supplied extra rules and an allowlist.
//!
//! A [`Redactor`] is a session. The same secret always maps to the same
//! placeholder for the lifetime of the redactor, so a model can correlate
//! usages without ever seeing the secret.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use regex::Regex;
use serde_json::Value;

use crate::config::Redaction;
use crate::model::{Placeholder, Redactions, SCHEMA_VERSION};

/// A rule shipped with the engine.
struct RuleDef {
    name: &'static str,
    pattern: &'static str,
    secret_group: usize,
    key_group: Option<usize>,
}

/// Gitleaks-derived rules plus key-name-aware rules.
///
/// The patterns are intentionally conservative: over-redaction is preferable
/// to leaking a secret, and the allowlist exists to un-redact false positives.
/// Key-name-aware rules come first so that a secret in a key/value pair keeps
/// its key name instead of being swallowed by a broader token rule.
const DEFAULT_RULES: &[RuleDef] = &[
    RuleDef {
        name: "env_value",
        pattern: r#"(?m)^[ \t]*(?:export[ \t]+)?([A-Za-z_][A-Za-z0-9_]*)[ \t]*=[ \t]*("[^"\n]*"|'[^'\n]*'|[^#\s][^\n]*?)[ \t]*(?:#.*)?$"#,
        secret_group: 2,
        key_group: Some(1),
    },
    RuleDef {
        name: "toml_value",
        pattern: r#"(?m)^[ \t]*("?[A-Za-z0-9_.-]+"?)[ \t]*=[ \t]*("[^"\n]*"|'[^'\n]*'|[^#\s][^\n]*?)[ \t]*(?:#.*)?$"#,
        secret_group: 2,
        key_group: Some(1),
    },
    RuleDef {
        name: "yaml_value",
        pattern: r#"(?m)^[ \t]*("?[A-Za-z0-9_.-]+"?)[ \t]*:[ \t]*("[^"\n]*"|'[^'\n]*'|[^#\s][^\n]*?)[ \t]*(?:#.*)?$"#,
        secret_group: 2,
        key_group: Some(1),
    },
    RuleDef {
        name: "json_value",
        pattern: r#""([^"\n]+)"[ \t]*:[ \t]*"([^"\n]*)""#,
        secret_group: 2,
        key_group: Some(1),
    },
    RuleDef {
        name: "aws_key",
        pattern: r"\b(?:AKIA|ASIA|ABIA|ACCA|AGPA|AIDA|AIPA|ANPA|ANVA|APKA|AROA|ASCA)[A-Z0-9]{16}\b",
        secret_group: 0,
        key_group: None,
    },
    RuleDef {
        name: "aws_secret_key",
        pattern: r#"(?i)aws(?:.{0,20})?['"][0-9a-zA-Z/+]{40}['"]"#,
        secret_group: 0,
        key_group: None,
    },
    RuleDef {
        name: "gcp_api_key",
        pattern: r"\bAIza[0-9A-Za-z\-_]{35}\b",
        secret_group: 0,
        key_group: None,
    },
    RuleDef {
        name: "github_token",
        pattern: r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{82})\b",
        secret_group: 0,
        key_group: None,
    },
    RuleDef {
        name: "slack_token",
        pattern: r"\bxox[baprs]-[0-9A-Za-z-]{10,}\b",
        secret_group: 0,
        key_group: None,
    },
    RuleDef {
        name: "stripe_key",
        pattern: r"\b(?:sk|rk)_(?:live|test)_[0-9A-Za-z]{24,}\b",
        secret_group: 0,
        key_group: None,
    },
    RuleDef {
        name: "jwt",
        pattern: r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
        secret_group: 0,
        key_group: None,
    },
    RuleDef {
        name: "pem_private_key",
        pattern: r"(?s)-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP )?PRIVATE KEY-----.*?-----END (?:RSA |EC |DSA |OPENSSH |PGP )?PRIVATE KEY-----",
        secret_group: 0,
        key_group: None,
    },
    RuleDef {
        name: "authorization_header",
        pattern: r#"(?i)(["']?\bauthorization["']?\s*:\s*["']?)([^"'\r\n]+)"#,
        secret_group: 2,
        key_group: None,
    },
    RuleDef {
        name: "bearer_token",
        pattern: r"(?i)(\bbearer\s+)([A-Za-z0-9._~+/=-]{16,})",
        secret_group: 2,
        key_group: None,
    },
    RuleDef {
        name: "url_credentials",
        pattern: r"(?i)(\b[a-z][a-z0-9+.-]*://)([^/\s:@]+:[^/\s@]+@)([^\s]+)",
        secret_group: 2,
        key_group: None,
    },
];

struct CompiledRule {
    name: String,
    regex: Regex,
    secret_group: usize,
    key_group: Option<usize>,
}

struct Candidate {
    start: usize,
    end: usize,
    secret: String,
    rule: String,
}

/// A redaction session.
///
/// Keep one redactor for the lifetime of a process (or a tour) so that the
/// same secret always maps to the same placeholder. The placeholder map never
/// contains secret values.
pub struct Redactor {
    rules: Vec<CompiledRule>,
    allowlist: Vec<Regex>,
    placeholders: BTreeMap<String, Placeholder>,
    by_secret: HashMap<String, String>,
    counters: HashMap<String, u32>,
}

impl fmt::Debug for Redactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Redactor")
            .field("rules", &self.rules.len())
            .field("allowlist", &self.allowlist.len())
            .field("placeholders", &self.placeholders.len())
            .finish_non_exhaustive()
    }
}

impl Redactor {
    /// Build a redactor from the configured extra rules and allowlist.
    pub fn new(config: &Redaction) -> Result<Self, RedactError> {
        let mut rules = Vec::with_capacity(DEFAULT_RULES.len() + config.extra_rules.len());
        for def in DEFAULT_RULES {
            let regex = Regex::new(def.pattern).map_err(|source| RedactError::InvalidRule {
                rule: def.name.to_owned(),
                source,
            })?;
            rules.push(CompiledRule {
                name: def.name.to_owned(),
                regex,
                secret_group: def.secret_group,
                key_group: def.key_group,
            });
        }
        for (index, pattern) in config.extra_rules.iter().enumerate() {
            let name = format!("extra_rule_{}", index + 1);
            let regex = Regex::new(pattern).map_err(|source| RedactError::InvalidRule {
                rule: name.clone(),
                source,
            })?;
            rules.push(CompiledRule {
                name,
                regex,
                secret_group: 0,
                key_group: None,
            });
        }
        let allowlist = config
            .allowlist
            .iter()
            .map(|pattern| {
                Regex::new(pattern).map_err(|source| RedactError::InvalidAllowlist {
                    pattern: pattern.clone(),
                    source,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            rules,
            allowlist,
            placeholders: BTreeMap::new(),
            by_secret: HashMap::new(),
            counters: HashMap::new(),
        })
    }

    /// Redact `text`, returning the redacted text and the placeholders used.
    ///
    /// The returned map contains only the placeholders that occur in `text`.
    /// Use [`Redactor::placeholders`] for the full session map.
    pub fn redact(&mut self, text: &str) -> (String, BTreeMap<String, Placeholder>) {
        self.redact_with_origin(text, "", 0)
    }

    /// Redact `text` and record the file and starting line for each match.
    ///
    /// `line` is the 1-based line number of the first line of `text`. Pass `0`
    /// when the text has no meaningful source location.
    pub fn redact_with_origin(
        &mut self,
        text: &str,
        file: &str,
        line: u32,
    ) -> (String, BTreeMap<String, Placeholder>) {
        let mut candidates = Vec::new();
        for rule in &self.rules {
            for captures in rule.regex.captures_iter(text) {
                let Some(secret_match) = captures.get(rule.secret_group) else {
                    continue;
                };
                let secret = secret_match.as_str();
                if secret.is_empty() || should_skip(secret) {
                    continue;
                }
                if let Some(key_group) = rule.key_group {
                    let Some(key_match) = captures.get(key_group) else {
                        continue;
                    };
                    if !is_secret_key(key_match.as_str()) {
                        continue;
                    }
                }
                let full = captures.get(0).map_or(secret, |matched| matched.as_str());
                if self.is_allowlisted(secret, full) {
                    continue;
                }
                candidates.push(Candidate {
                    start: secret_match.start(),
                    end: secret_match.end(),
                    secret: secret.to_owned(),
                    rule: rule.name.clone(),
                });
            }
        }

        candidates.sort_by(|a, b| {
            a.start
                .cmp(&b.start)
                .then_with(|| b.end.cmp(&a.end))
                .then_with(|| a.rule.cmp(&b.rule))
        });

        let mut selected = Vec::new();
        let mut last_end = 0;
        for candidate in candidates {
            if candidate.start >= last_end {
                last_end = candidate.end;
                selected.push(candidate);
            }
        }

        let mut output = String::with_capacity(text.len());
        let mut used = BTreeMap::new();
        let mut last = 0;
        for candidate in selected {
            output.push_str(&text[last..candidate.start]);
            let line_number = if line == 0 {
                0
            } else {
                line + text[..candidate.start].matches('\n').count() as u32
            };
            let placeholder =
                self.placeholder_for(&candidate.secret, &candidate.rule, file, line_number);
            if let Some(origin) = self.placeholders.get(&placeholder) {
                used.insert(placeholder.clone(), origin.clone());
            }
            output.push_str(&placeholder);
            last = candidate.end;
        }
        output.push_str(&text[last..]);
        (output, used)
    }

    /// The full placeholder map for this session.
    #[must_use]
    pub fn placeholders(&self) -> &BTreeMap<String, Placeholder> {
        &self.placeholders
    }

    /// Recursively redact every string in a JSON value.
    ///
    /// Redacting the structured payload before serializing it avoids the
    /// escaped-newline blind spot: a secret on its own line inside a JSON
    /// string is still preceded by a real newline, so line-anchored and
    /// word-boundary rules match. Object values whose key looks secret-bearing
    /// are redacted as a whole, preserving the key-name-aware layer.
    pub fn redact_value(&mut self, value: &mut Value) {
        match value {
            Value::String(text) => {
                *text = self.redact(text).0;
            }
            Value::Array(items) => {
                for item in items {
                    self.redact_value(item);
                }
            }
            Value::Object(map) => {
                for (key, item) in map.iter_mut() {
                    if is_secret_key(key) {
                        if let Value::String(text) = item {
                            if !should_skip(text) {
                                *text = self.placeholder_for(text, "key_value", "", 0);
                                continue;
                            }
                        }
                    }
                    self.redact_value(item);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    /// Build the `redactions.json` artifact for this session.
    #[must_use]
    pub fn redactions(&self) -> Redactions {
        Redactions {
            schema_version: SCHEMA_VERSION,
            tool_version: crate::VERSION.to_owned(),
            placeholders: self.placeholders.clone(),
        }
    }

    /// Consume the redactor and build the `redactions.json` artifact.
    #[must_use]
    pub fn into_redactions(self) -> Redactions {
        Redactions {
            schema_version: SCHEMA_VERSION,
            tool_version: crate::VERSION.to_owned(),
            placeholders: self.placeholders,
        }
    }

    fn placeholder_for(&mut self, secret: &str, rule: &str, file: &str, line: u32) -> String {
        if let Some(existing) = self.by_secret.get(secret) {
            return existing.clone();
        }
        let counter = self.counters.entry(rule.to_owned()).or_insert(0);
        *counter += 1;
        let placeholder = format!("«{rule}#{counter}»");
        self.by_secret
            .insert(secret.to_owned(), placeholder.clone());
        self.placeholders.insert(
            placeholder.clone(),
            Placeholder {
                rule: rule.to_owned(),
                file: file.to_owned(),
                line,
            },
        );
        placeholder
    }

    fn is_allowlisted(&self, secret: &str, full: &str) -> bool {
        let unquoted = secret.trim_matches(|c| c == '"' || c == '\'');
        self.allowlist
            .iter()
            .any(|regex| regex.is_match(secret) || regex.is_match(full) || regex.is_match(unquoted))
    }
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new(&Redaction::default()).expect("default redaction rules compile")
    }
}

/// Redact `text` with a fresh session.
///
/// For session-stable placeholders across multiple payloads, keep a
/// [`Redactor`] and call [`Redactor::redact`] instead.
#[must_use]
pub fn redact(text: &str) -> (String, BTreeMap<String, Placeholder>) {
    Redactor::default().redact(text)
}

/// Redact `text` with the supplied configuration in a fresh session.
pub fn redact_with_config(
    text: &str,
    config: &Redaction,
) -> Result<(String, BTreeMap<String, Placeholder>), RedactError> {
    let mut redactor = Redactor::new(config)?;
    Ok(redactor.redact(text))
}

/// Redact a remote URL for an error message.
///
/// The whole URL is replaced with a placeholder so endpoints and any
/// credentials in them never appear in logs. Use this before logging errors
/// that may contain a URL.
#[must_use]
pub fn redact_url(_url: &str) -> String {
    "«url»".to_owned()
}

fn should_skip(secret: &str) -> bool {
    let trimmed = secret.trim();
    trimmed.is_empty()
        || (trimmed.contains('«') && trimmed.contains('»'))
        || trimmed.starts_with('$')
        || trimmed.starts_with("%(")
}

fn is_secret_key(key: &str) -> bool {
    let key = key
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_ascii_uppercase();
    key.contains("KEY")
        || key.contains("SECRET")
        || key.contains("TOKEN")
        || key.contains("PASSWORD")
        || key.contains("PASSWD")
        || key.contains("DSN")
        || key.contains("PRIVATE")
        || key.ends_with("_URL")
}

/// Errors produced while building a redactor.
#[derive(Debug)]
pub enum RedactError {
    /// A redaction rule was not a valid regex.
    InvalidRule {
        /// Rule name.
        rule: String,
        /// Underlying regex error.
        source: regex::Error,
    },
    /// An allowlist pattern was not a valid regex.
    InvalidAllowlist {
        /// The invalid pattern.
        pattern: String,
        /// Underlying regex error.
        source: regex::Error,
    },
}

impl fmt::Display for RedactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRule { rule, source } => {
                write!(f, "invalid redaction rule `{rule}`: {source}")
            }
            Self::InvalidAllowlist { pattern, source } => {
                write!(
                    f,
                    "invalid redaction allowlist pattern `{pattern}`: {source}"
                )
            }
        }
    }
}

impl std::error::Error for RedactError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidRule { source, .. } | Self::InvalidAllowlist { source, .. } => {
                Some(source)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AWS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";
    const GITHUB_TOKEN: &str = "ghp_1234567890abcdefghijklmnopqrstuvwxyz";
    const JWT: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
    const PEM: &str =
        "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA...\n-----END RSA PRIVATE KEY-----";

    #[test]
    fn redacts_aws_key() {
        let mut redactor = Redactor::default();
        let (redacted, map) = redactor.redact(&format!("key {AWS_KEY} here"));
        assert!(!redacted.contains(AWS_KEY));
        assert!(redacted.contains("«aws_key#1»"));
        assert_eq!(map["«aws_key#1»"].rule, "aws_key");
    }

    #[test]
    fn redacts_github_token() {
        let mut redactor = Redactor::default();
        let (redacted, _) = redactor.redact(&format!("token {GITHUB_TOKEN}"));
        assert!(!redacted.contains(GITHUB_TOKEN));
        assert!(redacted.contains("«github_token#1»"));
    }

    #[test]
    fn redacts_jwt() {
        let mut redactor = Redactor::default();
        let (redacted, _) = redactor.redact(&format!("jwt {JWT}"));
        assert!(!redacted.contains(JWT));
        assert!(redacted.contains("«jwt#1»"));
    }

    #[test]
    fn redacts_pem_private_key() {
        let mut redactor = Redactor::default();
        let (redacted, _) = redactor.redact(PEM);
        assert!(!redacted.contains("BEGIN RSA PRIVATE KEY"));
        assert!(!redacted.contains("MIIEowIBAAKCAQEA"));
        assert!(redacted.contains("«pem_private_key#1»"));
    }

    #[test]
    fn redacts_env_values_and_keeps_key_names() {
        let mut redactor = Redactor::default();
        let text = "DATABASE_PASSWORD=hunter2\nAPI_TOKEN=abc123\nNAME=laudit\n";
        let (redacted, _) = redactor.redact(text);
        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains("abc123"));
        assert!(redacted.contains("DATABASE_PASSWORD=«env_value#1»"));
        assert!(redacted.contains("API_TOKEN=«env_value#2»"));
        assert!(redacted.contains("NAME=laudit"));
    }

    #[test]
    fn redacts_values_with_spaces_and_ignores_comments() {
        let mut redactor = Redactor::default();
        let text = "PASSWORD=correct horse battery staple # comment\nEMPTY= # comment\n";
        let (redacted, _) = redactor.redact(text);
        assert!(!redacted.contains("correct horse battery staple"));
        assert!(redacted.contains("PASSWORD=«env_value#1» # comment"));
        assert!(redacted.contains("EMPTY= # comment"));
    }

    #[test]
    fn redacts_toml_yaml_and_json_values() {
        let mut redactor = Redactor::default();
        let text = "[provider]\napi_key = \"toml-secret\"\nname = \"laudit\"\n\npassword: yaml-secret\nname: laudit\n\n{\"token\": \"json-secret\", \"name\": \"laudit\"}\n";
        let (redacted, _) = redactor.redact(text);
        assert!(!redacted.contains("toml-secret"));
        assert!(!redacted.contains("yaml-secret"));
        assert!(!redacted.contains("json-secret"));
        assert!(redacted.contains("api_key = «env_value#1»"));
        assert!(redacted.contains("password: «yaml_value#1»"));
        assert!(redacted.contains("\"token\": \"«json_value#1»\""));
        assert!(redacted.contains("name = \"laudit\""));
        assert!(redacted.contains("name: laudit"));
        assert!(redacted.contains("\"name\": \"laudit\""));
    }

    #[test]
    fn placeholders_are_stable_within_a_session() {
        let mut redactor = Redactor::default();
        let (first, _) = redactor.redact(AWS_KEY);
        let (second, _) = redactor.redact(AWS_KEY);
        assert!(first.contains("«aws_key#1»"));
        assert!(second.contains("«aws_key#1»"));

        let (third, _) = redactor.redact("AKIAIOSFODNN7EXAMPLF");
        assert!(third.contains("«aws_key#2»"));
    }

    #[test]
    fn allowlist_un_redacts_false_positives() {
        let config = Redaction {
            extra_rules: Vec::new(),
            allowlist: vec![r"^AKIAIOSFODNN7EXAMPLE$".to_owned()],
        };
        let mut redactor = Redactor::new(&config).expect("redactor");
        let (redacted, map) = redactor.redact(AWS_KEY);
        assert!(redacted.contains(AWS_KEY));
        assert!(map.is_empty());
    }

    #[test]
    fn extra_rules_are_applied() {
        let config = Redaction {
            extra_rules: vec![r"custom-[0-9]{4}".to_owned()],
            allowlist: Vec::new(),
        };
        let mut redactor = Redactor::new(&config).expect("redactor");
        let (redacted, _) = redactor.redact("value custom-1234 value");
        assert!(!redacted.contains("custom-1234"));
        assert!(redacted.contains("«extra_rule_1#1»"));
    }

    #[test]
    fn authorization_headers_are_redacted() {
        let mut redactor = Redactor::default();
        let (redacted, _) = redactor.redact("Authorization: Bearer abcdefghijklmnopqrstuvwxyz");
        assert!(!redacted.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(redacted.contains("Authorization: «authorization_header#1»"));
    }

    #[test]
    fn json_authorization_headers_are_redacted() {
        let mut redactor = Redactor::default();
        let (redacted, _) = redactor.redact(r#"{"Authorization": "Basic dXNlcjpwYXNz"}"#);
        assert!(!redacted.contains("dXNlcjpwYXNz"));
        assert!(redacted.contains("«authorization_header#1»"));
    }

    #[test]
    fn redact_url_hides_remote_endpoints() {
        assert_eq!(redact_url("https://api.openai.com/v1"), "«url»");
        assert_eq!(redact_url("https://user:pass@example.com/path"), "«url»");
    }

    #[test]
    fn url_credentials_are_redacted() {
        let mut redactor = Redactor::default();
        let (redacted, _) = redactor.redact("error: https://user:pass@example.com/path");
        assert!(!redacted.contains("user:pass"));
        assert!(redacted.contains("«url_credentials#1»"));
    }

    #[test]
    fn non_secret_keys_are_left_alone() {
        let mut redactor = Redactor::default();
        let text = "name = \"laudit\"\nversion = \"0.1.0\"\n";
        let (redacted, map) = redactor.redact(text);
        assert_eq!(redacted, text);
        assert!(map.is_empty());
    }

    #[test]
    fn redactions_artifact_never_contains_secret_values() {
        let mut redactor = Redactor::default();
        let _ = redactor.redact(&format!("{AWS_KEY} {GITHUB_TOKEN}"));
        let artifact = redactor.into_redactions();
        let json = serde_json::to_string(&artifact).expect("serialize redactions");
        assert!(!json.contains(AWS_KEY));
        assert!(!json.contains(GITHUB_TOKEN));
        assert!(json.contains("«aws_key#1»"));
        assert!(json.contains("«github_token#1»"));
    }

    #[test]
    fn debug_output_does_not_leak_secrets() {
        let mut redactor = Redactor::default();
        let _ = redactor.redact(AWS_KEY);
        let debug = format!("{redactor:?}");
        assert!(!debug.contains(AWS_KEY));
        assert!(debug.contains("Redactor"));
    }

    #[test]
    fn redact_with_origin_records_file_and_line() {
        let mut redactor = Redactor::default();
        let text = format!("line one\nkey {AWS_KEY}\nline three");
        let (_, map) = redactor.redact_with_origin(&text, "src/config.rs", 10);
        let placeholder = map.keys().next().expect("placeholder");
        assert_eq!(map[placeholder].file, "src/config.rs");
        assert_eq!(map[placeholder].line, 11);
    }

    #[test]
    fn redact_value_catches_secrets_on_their_own_lines() {
        let mut redactor = Redactor::default();
        let mut value = serde_json::json!({
            "docs": [{
                "path": "docs/security.md",
                "content": format!("Intro\n\n{JWT}\n\nDATABASE_PASSWORD=hunter2\n")
            }]
        });

        redactor.redact_value(&mut value);

        let json = serde_json::to_string(&value).expect("serialize");
        assert!(!json.contains(JWT));
        assert!(!json.contains("hunter2"));
        assert!(json.contains("«jwt#1»"));
        assert!(json.contains("«env_value#1»"));
    }

    #[test]
    fn redact_value_uses_secret_bearing_keys() {
        let mut redactor = Redactor::default();
        let mut value = serde_json::json!({
            "github_token": "opaque-value",
            "name": "laudit"
        });

        redactor.redact_value(&mut value);

        assert_eq!(value["github_token"], "«key_value#1»");
        assert_eq!(value["name"], "laudit");
    }

    #[test]
    fn invalid_extra_rule_is_reported() {
        let config = Redaction {
            extra_rules: vec!["(".to_owned()],
            allowlist: Vec::new(),
        };
        let error = match Redactor::new(&config) {
            Ok(_) => panic!("invalid rule should fail"),
            Err(error) => error,
        };
        assert!(matches!(error, RedactError::InvalidRule { .. }));
        assert!(error.to_string().contains("extra_rule_1"));
    }

    #[test]
    fn invalid_allowlist_is_reported() {
        let config = Redaction {
            extra_rules: Vec::new(),
            allowlist: vec!["[".to_owned()],
        };
        let error = match Redactor::new(&config) {
            Ok(_) => panic!("invalid allowlist should fail"),
            Err(error) => error,
        };
        assert!(matches!(error, RedactError::InvalidAllowlist { .. }));
        assert!(error.to_string().contains("allowlist"));
    }
}

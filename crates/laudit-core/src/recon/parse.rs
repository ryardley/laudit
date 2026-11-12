//! Tree-sitter parsing and definition/reference extraction.
//!
//! The parser turns source text into a flat list of [`Tag`]s. Definitions are
//! the symbols a file introduces (functions, types, constants, keys, headings,
//! and so on); references are the symbols it mentions (calls, imports, type
//! usages, links, table reads). The graph stage consumes these tags to build
//! the structural skeleton (D4, D7).
//!
//! Parsing is deliberately forgiving: unsupported files are skipped, and a
//! syntactically broken file still yields whatever tags tree-sitter could
//! recover from its error-recovering parse.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language as TsLanguage, Parser, Query, QueryCursor, Tree};

use super::walk::{Language, SourceFile};

/// Whether a tag introduces a symbol or mentions one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagKind {
    /// A definition introduced by the file.
    Def,
    /// A reference to a symbol defined elsewhere.
    Ref,
}

/// A definition or reference extracted from a source file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Tag {
    /// File containing the tag, relative to the repository root when parsed
    /// through [`parse_files`].
    pub file: PathBuf,
    /// 1-based line number where the tag's name starts.
    pub line: u32,
    /// Symbol name.
    pub name: String,
    /// Whether this is a definition or a reference.
    pub kind: TagKind,
}

/// Parse a single source string.
///
/// The `file` is recorded verbatim on every tag. Unsupported languages and
/// query failures produce an empty list rather than an error.
#[must_use]
pub fn parse_source(file: impl Into<PathBuf>, language: Language, source: &str) -> Vec<Tag> {
    let file = file.into();
    let Some(queries) = queries().get(&language).and_then(Option::as_ref) else {
        return Vec::new();
    };

    let mut tags = Vec::new();

    if language == Language::Markdown {
        if let Some(query) = &queries.defs {
            if let Some(tree) = parse_tree(&queries.defs_language, source) {
                collect_tags(
                    &file,
                    language,
                    source,
                    &tree,
                    query,
                    TagKind::Def,
                    &mut tags,
                );
            }
        }
        if let Some(query) = &queries.refs {
            if let Some(tree) = parse_tree(&queries.refs_language, source) {
                collect_tags(
                    &file,
                    language,
                    source,
                    &tree,
                    query,
                    TagKind::Ref,
                    &mut tags,
                );
            }
        }
    } else if let Some(tree) = parse_tree(&queries.defs_language, source) {
        if let Some(query) = &queries.defs {
            collect_tags(
                &file,
                language,
                source,
                &tree,
                query,
                TagKind::Def,
                &mut tags,
            );
        }
        if let Some(query) = &queries.refs {
            collect_tags(
                &file,
                language,
                source,
                &tree,
                query,
                TagKind::Ref,
                &mut tags,
            );
        }
    }

    tags.sort();
    tags.dedup();
    tags
}

/// Parse a file, detecting its language from the extension.
///
/// Returns `Ok(None)` for unsupported files. Files that are not valid UTF-8 are
/// parsed lossily so a stray byte does not abort recon.
pub fn parse_file(path: impl AsRef<Path>) -> io::Result<Option<Vec<Tag>>> {
    let path = path.as_ref();
    let Some(language) = Language::from_path(path) else {
        return Ok(None);
    };
    let bytes = fs::read(path)?;
    let source = String::from_utf8_lossy(&bytes);
    Ok(Some(parse_source(path, language, &source)))
}

/// Parse every file in `files`, resolving paths against `repo_root`.
///
/// Unreadable files are skipped so one bad file cannot abort recon. The result
/// is sorted and deduplicated, making it deterministic across runs.
pub fn parse_files(repo_root: impl AsRef<Path>, files: &[SourceFile]) -> io::Result<Vec<Tag>> {
    let repo_root = repo_root.as_ref();
    let mut tags = Vec::new();

    for file in files {
        let path = repo_root.join(&file.path);
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let source = String::from_utf8_lossy(&bytes);
        tags.extend(parse_source(&file.path, file.language, &source));
    }

    tags.sort();
    tags.dedup();
    Ok(tags)
}

struct LanguageQueries {
    defs: Option<Query>,
    refs: Option<Query>,
    defs_language: TsLanguage,
    refs_language: TsLanguage,
}

fn queries() -> &'static BTreeMap<Language, Option<LanguageQueries>> {
    static QUERIES: OnceLock<BTreeMap<Language, Option<LanguageQueries>>> = OnceLock::new();
    QUERIES.get_or_init(|| {
        let mut map = BTreeMap::new();
        for language in [
            Language::Rust,
            Language::TypeScript,
            Language::Tsx,
            Language::JavaScript,
            Language::Bash,
            Language::Toml,
            Language::Yaml,
            Language::Json,
            Language::Markdown,
            Language::Sql,
        ] {
            map.insert(language, build_queries(language));
        }
        map
    })
}

fn build_queries(language: Language) -> Option<LanguageQueries> {
    let (defs_source, refs_source) = query_sources(language);
    let defs_language = ts_language(language);
    let refs_language = if language == Language::Markdown {
        tree_sitter_md::INLINE_LANGUAGE.into()
    } else {
        defs_language.clone()
    };

    Some(LanguageQueries {
        defs: Query::new(&defs_language, defs_source).ok(),
        refs: Query::new(&refs_language, refs_source).ok(),
        defs_language,
        refs_language,
    })
}

fn ts_language(language: Language) -> TsLanguage {
    match language {
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Language::Bash => tree_sitter_bash::LANGUAGE.into(),
        Language::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
        Language::Yaml => tree_sitter_yaml::LANGUAGE.into(),
        Language::Json => tree_sitter_json::LANGUAGE.into(),
        Language::Markdown => tree_sitter_md::LANGUAGE.into(),
        Language::Sql => tree_sitter_sequel::LANGUAGE.into(),
    }
}

fn parse_tree(language: &TsLanguage, source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    parser.parse(source, None)
}

fn collect_tags(
    file: &Path,
    language: Language,
    source: &str,
    tree: &Tree,
    query: &Query,
    kind: TagKind,
    tags: &mut Vec<Tag>,
) {
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    let capture_names = query.capture_names();

    while let Some(query_match) = matches.next() {
        for capture in query_match.captures {
            if capture_names[capture.index as usize] != "name" {
                continue;
            }
            let Ok(text) = capture.node.utf8_text(source.as_bytes()) else {
                continue;
            };
            let Some(name) = normalize_name(language, kind, text) else {
                continue;
            };
            tags.push(Tag {
                file: file.to_path_buf(),
                line: capture.node.start_position().row as u32 + 1,
                name,
                kind,
            });
        }
    }
}

fn normalize_name(language: Language, kind: TagKind, raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let name = match language {
        Language::Toml => {
            let unquoted = strip_quotes(trimmed);
            if kind == TagKind::Ref {
                unquoted
                    .split('.')
                    .next()
                    .unwrap_or(unquoted.as_str())
                    .to_owned()
            } else {
                unquoted
            }
        }
        Language::Yaml => {
            let unquoted = strip_quotes(trimmed);
            if unquoted == "<<" {
                return None;
            }
            unquoted
        }
        Language::Sql => strip_quotes(trimmed),
        Language::Markdown => trimmed
            .trim_start_matches('<')
            .trim_end_matches('>')
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim()
            .to_owned(),
        Language::Rust
        | Language::TypeScript
        | Language::Tsx
        | Language::JavaScript
        | Language::Bash
        | Language::Json => trimmed.to_owned(),
    };

    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn strip_quotes(value: &str) -> String {
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(value);
    let value = value
        .strip_prefix('\'')
        .and_then(|inner| inner.strip_suffix('\''))
        .unwrap_or(value);
    let value = value
        .strip_prefix('`')
        .and_then(|inner| inner.strip_suffix('`'))
        .unwrap_or(value);
    let value = value
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(value);
    value.to_owned()
}

fn query_sources(language: Language) -> (&'static str, &'static str) {
    match language {
        Language::Rust => (RUST_DEFS, RUST_REFS),
        Language::TypeScript | Language::Tsx => (TYPESCRIPT_DEFS, TYPESCRIPT_REFS),
        Language::JavaScript => (JAVASCRIPT_DEFS, JAVASCRIPT_REFS),
        Language::Bash => (BASH_DEFS, BASH_REFS),
        Language::Toml => (TOML_DEFS, TOML_REFS),
        Language::Yaml => (YAML_DEFS, YAML_REFS),
        Language::Json => (JSON_DEFS, JSON_REFS),
        Language::Markdown => (MARKDOWN_DEFS, MARKDOWN_REFS),
        Language::Sql => (SQL_DEFS, SQL_REFS),
    }
}

const RUST_DEFS: &str = r#"
(function_item name: (identifier) @name) @definition.function
(function_signature_item name: (identifier) @name) @definition.function
(struct_item name: (type_identifier) @name) @definition.class
(enum_item name: (type_identifier) @name) @definition.class
(union_item name: (type_identifier) @name) @definition.class
(trait_item name: (type_identifier) @name) @definition.interface
(type_item name: (type_identifier) @name) @definition.class
(const_item name: (identifier) @name) @definition.constant
(static_item name: (identifier) @name) @definition.constant
(mod_item name: (identifier) @name) @definition.module
(macro_definition name: (identifier) @name) @definition.macro
(declaration_list (function_item name: (identifier) @name) @definition.method)
"#;

const RUST_REFS: &str = r#"
(call_expression function: (identifier) @name) @reference.call
(call_expression function: (field_expression field: (field_identifier) @name)) @reference.call
(call_expression function: (scoped_identifier name: (identifier) @name)) @reference.call
(macro_invocation macro: (identifier) @name) @reference.call
(use_declaration argument: (_) @name) @reference.import
(impl_item trait: (type_identifier) @name) @reference.implementation
(impl_item type: (type_identifier) @name) @reference.implementation
(parameter type: (type_identifier) @name) @reference.type
(field_declaration type: (type_identifier) @name) @reference.type
(let_declaration type: (type_identifier) @name) @reference.type
(function_item return_type: (type_identifier) @name) @reference.type
"#;

const TYPESCRIPT_DEFS: &str = r#"
(function_declaration name: (identifier) @name) @definition.function
(generator_function_declaration name: (identifier) @name) @definition.function
(function_signature name: (identifier) @name) @definition.function
(class_declaration name: (type_identifier) @name) @definition.class
(abstract_class_declaration name: (type_identifier) @name) @definition.class
(interface_declaration name: (type_identifier) @name) @definition.interface
(type_alias_declaration name: (type_identifier) @name) @definition.class
(enum_declaration name: (identifier) @name) @definition.class
(method_definition name: (property_identifier) @name) @definition.method
(method_signature name: (property_identifier) @name) @definition.method
(abstract_method_signature name: (property_identifier) @name) @definition.method
(public_field_definition name: (property_identifier) @name) @definition.constant
(lexical_declaration (variable_declarator name: (identifier) @name)) @definition.constant
(variable_declaration (variable_declarator name: (identifier) @name)) @definition.constant
(lexical_declaration (variable_declarator name: (identifier) @name value: [(arrow_function) (function_expression)])) @definition.function
(variable_declaration (variable_declarator name: (identifier) @name value: [(arrow_function) (function_expression)])) @definition.function
(module name: (identifier) @name) @definition.module
"#;

const TYPESCRIPT_REFS: &str = r#"
(call_expression function: (identifier) @name) @reference.call
(call_expression function: (member_expression property: (property_identifier) @name)) @reference.call
(new_expression constructor: (identifier) @name) @reference.class
(import_statement source: (string (string_fragment) @name)) @reference.import
(import_specifier name: (identifier) @name) @reference.import
(type_annotation (type_identifier) @name) @reference.type
(extends_clause value: (identifier) @name) @reference.type
(implements_clause (type_identifier) @name) @reference.type
"#;

const JAVASCRIPT_DEFS: &str = r#"
(function_declaration name: (identifier) @name) @definition.function
(generator_function_declaration name: (identifier) @name) @definition.function
(class_declaration name: (identifier) @name) @definition.class
(method_definition name: (property_identifier) @name) @definition.method
(lexical_declaration (variable_declarator name: (identifier) @name)) @definition.constant
(variable_declaration (variable_declarator name: (identifier) @name)) @definition.constant
(lexical_declaration (variable_declarator name: (identifier) @name value: [(arrow_function) (function_expression)])) @definition.function
(variable_declaration (variable_declarator name: (identifier) @name value: [(arrow_function) (function_expression)])) @definition.function
(assignment_expression left: (identifier) @name right: [(arrow_function) (function_expression)]) @definition.function
(pair key: (property_identifier) @name value: [(arrow_function) (function_expression)]) @definition.function
"#;

const JAVASCRIPT_REFS: &str = r#"
(call_expression function: (identifier) @name) @reference.call
(call_expression function: (member_expression property: (property_identifier) @name)) @reference.call
(new_expression constructor: (identifier) @name) @reference.class
(import_statement source: (string (string_fragment) @name)) @reference.import
(import_specifier name: (identifier) @name) @reference.import
"#;

const BASH_DEFS: &str = r#"
(function_definition name: (word) @name) @definition.function
(variable_assignment name: (variable_name) @name) @definition.constant
"#;

const BASH_REFS: &str = r#"
(command name: (command_name) @name) @reference.call
"#;

const TOML_DEFS: &str = r#"
(table (bare_key) @name) @definition.class
(table (quoted_key) @name) @definition.class
(table (dotted_key) @name) @definition.class
(table_array_element (bare_key) @name) @definition.class
(table_array_element (quoted_key) @name) @definition.class
(table_array_element (dotted_key) @name) @definition.class
(pair (bare_key) @name) @definition.constant
(pair (quoted_key) @name) @definition.constant
(pair (dotted_key) @name) @definition.constant
"#;

const TOML_REFS: &str = r#"
(pair (dotted_key) @name) @reference.import
"#;

const YAML_DEFS: &str = r#"
(block_mapping_pair key: (flow_node (plain_scalar (string_scalar) @name))) @definition.constant
(block_mapping_pair key: (flow_node (double_quote_scalar) @name)) @definition.constant
(block_mapping_pair key: (flow_node (single_quote_scalar) @name)) @definition.constant
(flow_pair key: (flow_node (plain_scalar (string_scalar) @name))) @definition.constant
(flow_pair key: (flow_node (double_quote_scalar) @name)) @definition.constant
(flow_pair key: (flow_node (single_quote_scalar) @name)) @definition.constant
(anchor (anchor_name) @name) @definition.constant
"#;

const YAML_REFS: &str = r#"
(alias (alias_name) @name) @reference.call
"#;

const JSON_DEFS: &str = r#"
(pair key: (string (string_content) @name)) @definition.constant
"#;

const JSON_REFS: &str = r#"
((pair key: (string (string_content) @key) value: (string (string_content) @name)) @reference.call
 (#eq? @key "$ref"))
"#;

const MARKDOWN_DEFS: &str = r#"
((atx_heading heading_content: (inline) @name) @definition.section)
((setext_heading heading_content: (paragraph) @name) @definition.section)
((link_reference_definition (link_label) @name) @definition.constant)
"#;

const MARKDOWN_REFS: &str = r#"
((link_destination) @name @reference.call)
((link_label) @name @reference.call)
"#;

const SQL_DEFS: &str = r#"
(create_table (object_reference name: (identifier) @name)) @definition.class
(create_view (object_reference name: (identifier) @name)) @definition.class
(create_materialized_view (object_reference name: (identifier) @name)) @definition.class
(create_index (object_reference name: (identifier) @name)) @definition.class
(create_schema (identifier) @name) @definition.class
(create_type name: (identifier) @name) @definition.class
(create_sequence (object_reference name: (identifier) @name)) @definition.class
(create_function (object_reference name: (identifier) @name)) @definition.function
(create_procedure (object_reference name: (identifier) @name)) @definition.function
(create_trigger (identifier) @name) @definition.function
(create_trigger (object_reference name: (identifier) @name)) @definition.function
"#;

const SQL_REFS: &str = r#"
(from (object_reference name: (identifier) @name)) @reference.call
(from (relation (object_reference name: (identifier) @name))) @reference.call
(join (relation (object_reference name: (identifier) @name))) @reference.call
(update (relation (object_reference name: (identifier) @name))) @reference.call
(insert name: (identifier) @name) @reference.call
(invocation (object_reference name: (identifier) @name)) @reference.call
"#;

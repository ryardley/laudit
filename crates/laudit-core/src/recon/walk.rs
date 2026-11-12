//! Deterministic source-file enumeration.
//!
//! The walker honors `.gitignore` files and configured excludes, skips vendored
//! directories, binary files, and files above a size cap, and returns a stable
//! sorted list of source files with their detected language.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use ignore::overrides::{Override, OverrideBuilder};
use ignore::{DirEntry, WalkBuilder};

use crate::config::Config;

/// Default maximum size of a file considered for parsing, in bytes.
pub const DEFAULT_MAX_FILE_SIZE: u64 = 1024 * 1024;

/// Directories that are always skipped, regardless of ignore rules.
pub const VENDORED_DIRS: &[&str] = &["node_modules", "target", "vendor", "dist"];

/// Number of bytes inspected when deciding whether a file is binary.
const BINARY_SNIFF_LEN: usize = 8 * 1024;

/// A language understood by the bundled tree-sitter grammars (D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Language {
    /// Rust.
    Rust,
    /// TypeScript.
    TypeScript,
    /// TypeScript with JSX.
    Tsx,
    /// JavaScript.
    JavaScript,
    /// Bash.
    Bash,
    /// TOML.
    Toml,
    /// YAML.
    Yaml,
    /// JSON.
    Json,
    /// Markdown.
    Markdown,
    /// SQL.
    Sql,
}

impl Language {
    /// Detect a language from a file path's extension.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        Self::from_extension(extension)
    }

    /// Detect a language from a file extension, case-insensitively.
    #[must_use]
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "rs" => Some(Self::Rust),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "sh" | "bash" => Some(Self::Bash),
            "toml" => Some(Self::Toml),
            "yaml" | "yml" => Some(Self::Yaml),
            "json" => Some(Self::Json),
            "md" | "markdown" => Some(Self::Markdown),
            "sql" => Some(Self::Sql),
            _ => None,
        }
    }

    /// The canonical name used in configuration and artifacts.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::JavaScript => "javascript",
            Self::Bash => "bash",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
            Self::Json => "json",
            Self::Markdown => "markdown",
            Self::Sql => "sql",
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A source file discovered by the walker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// Path relative to the repository root.
    pub path: PathBuf,
    /// Language detected from the file extension.
    pub language: Language,
    /// File size in bytes.
    pub size: u64,
}

/// Options controlling a repository walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkOptions {
    /// Roots to scan, relative to the repository root unless absolute.
    pub roots: Vec<PathBuf>,
    /// Additional gitignore-style exclude patterns.
    pub excludes: Vec<String>,
    /// Languages to include. An empty list includes every supported language.
    pub languages: Vec<String>,
    /// Maximum file size in bytes. Larger files are skipped.
    pub max_file_size: u64,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            roots: vec![PathBuf::from(".")],
            excludes: Vec::new(),
            languages: Vec::new(),
            max_file_size: DEFAULT_MAX_FILE_SIZE,
        }
    }
}

impl WalkOptions {
    /// Build walk options from resolved engine configuration.
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        Self {
            roots: config.roots.iter().map(PathBuf::from).collect(),
            excludes: config.excludes.clone(),
            languages: config.languages.clone(),
            max_file_size: DEFAULT_MAX_FILE_SIZE,
        }
    }
}

/// Errors produced while walking a repository.
#[derive(Debug)]
pub enum WalkError {
    /// A configured root does not exist.
    MissingRoot {
        /// The missing root.
        path: PathBuf,
    },
    /// A configured exclude pattern was not a valid glob.
    InvalidExclude {
        /// The offending pattern.
        pattern: String,
        /// Underlying ignore error.
        source: ignore::Error,
    },
}

impl fmt::Display for WalkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRoot { path } => {
                write!(f, "walk root does not exist: {}", path.display())
            }
            Self::InvalidExclude { pattern, source } => {
                write!(f, "invalid exclude pattern {pattern:?}: {source}")
            }
        }
    }
}

impl std::error::Error for WalkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingRoot { .. } => None,
            Self::InvalidExclude { source, .. } => Some(source),
        }
    }
}

/// Walk `repo_root` using the roots, excludes, and languages from `config`.
pub fn walk(repo_root: impl AsRef<Path>, config: &Config) -> Result<Vec<SourceFile>, WalkError> {
    walk_with(repo_root, &WalkOptions::from_config(config))
}

/// Walk `repo_root` with explicit options.
///
/// The returned files are sorted by their repository-relative path, so the
/// result is deterministic regardless of filesystem iteration order.
pub fn walk_with(
    repo_root: impl AsRef<Path>,
    options: &WalkOptions,
) -> Result<Vec<SourceFile>, WalkError> {
    let repo_root = repo_root.as_ref();
    let mut files = BTreeMap::new();

    for root in &options.roots {
        let root_path = if root.is_absolute() {
            root.clone()
        } else {
            repo_root.join(root)
        };
        if !root_path.exists() {
            return Err(WalkError::MissingRoot { path: root_path });
        }

        let overrides = build_overrides(repo_root, &options.excludes)?;
        let mut builder = WalkBuilder::new(&root_path);
        builder
            .hidden(true)
            .parents(false)
            .require_git(false)
            .git_global(false)
            .git_ignore(true)
            .git_exclude(true)
            .follow_links(false)
            .overrides(overrides);
        builder.filter_entry(is_not_vendored);

        for result in builder.build() {
            let Ok(entry) = result else {
                continue;
            };
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }

            let path = entry.path();
            let Some(language) = Language::from_path(path) else {
                continue;
            };
            if !language_enabled(language, &options.languages) {
                continue;
            }

            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > options.max_file_size {
                continue;
            }
            if is_binary(path).unwrap_or(true) {
                continue;
            }

            let relative = path.strip_prefix(repo_root).unwrap_or(path).to_path_buf();
            files.entry(relative.clone()).or_insert(SourceFile {
                path: relative,
                language,
                size: metadata.len(),
            });
        }
    }

    Ok(files.into_values().collect())
}

fn build_overrides(repo_root: &Path, excludes: &[String]) -> Result<Override, WalkError> {
    let mut builder = OverrideBuilder::new(repo_root);
    for exclude in excludes {
        let exclude = exclude.trim();
        if exclude.is_empty() {
            continue;
        }
        let pattern = if exclude.starts_with('!') {
            exclude.to_owned()
        } else {
            format!("!{exclude}")
        };
        builder
            .add(&pattern)
            .map_err(|source| WalkError::InvalidExclude {
                pattern: exclude.to_owned(),
                source,
            })?;
    }
    builder.build().map_err(|source| WalkError::InvalidExclude {
        pattern: String::new(),
        source,
    })
}

fn is_not_vendored(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    if !entry
        .file_type()
        .is_some_and(|file_type| file_type.is_dir())
    {
        return true;
    }
    entry
        .file_name()
        .to_str()
        .map_or(true, |name| !VENDORED_DIRS.contains(&name))
}

fn language_enabled(language: Language, configured: &[String]) -> bool {
    if configured.is_empty() {
        return true;
    }
    configured.iter().any(|name| {
        let name = name.trim();
        name.eq_ignore_ascii_case(language.as_str())
            || (language == Language::Tsx && name.eq_ignore_ascii_case("typescript"))
    })
}

fn is_binary(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut buffer = [0_u8; BINARY_SNIFF_LEN];
    let read = file.read(&mut buffer)?;
    Ok(buffer[..read].contains(&0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("laudit-walk-test-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn committed_fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/walk-tree")
    }

    fn copy_dir_all(source: &Path, destination: &Path) -> io::Result<()> {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let target = destination.join(entry.file_name());
            if file_type.is_dir() {
                copy_dir_all(&entry.path(), &target)?;
            } else if file_type.is_file() {
                fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }

    fn fixture_root() -> TempDir {
        let temp = TempDir::new();
        copy_dir_all(&committed_fixture(), temp.path()).expect("copy fixture");
        fs::create_dir_all(temp.path().join("src/ignored-dir")).expect("create ignored dir");
        fs::write(temp.path().join("src/ignored.rs"), "pub fn ignored() {}\n")
            .expect("write ignored file");
        fs::write(
            temp.path().join("src/ignored-dir/hidden.rs"),
            "pub fn hidden() {}\n",
        )
        .expect("write ignored dir file");
        fs::write(temp.path().join("src/notes.log"), "ignored log\n").expect("write ignored log");
        fs::write(temp.path().join("src/binary.rs"), b"fn main() {}\0\x01\x02")
            .expect("write binary file");
        temp
    }

    fn fixture_options() -> WalkOptions {
        WalkOptions {
            roots: vec![PathBuf::from(".")],
            excludes: Vec::new(),
            languages: Vec::new(),
            max_file_size: 1024,
        }
    }

    fn paths(files: &[SourceFile]) -> Vec<String> {
        files
            .iter()
            .map(|file| file.path.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn fixture_walk_returns_expected_files_in_sorted_order() {
        let fixture = fixture_root();
        let files = walk_with(fixture.path(), &fixture_options()).expect("walk fixture");

        assert_eq!(
            paths(&files),
            vec![
                "assets/readme.md",
                "config/app.toml",
                "data/query.sql",
                "scripts/run.sh",
                "src/lib.rs",
                "src/main.rs",
                "web/app.js",
                "web/app.jsx",
                "web/app.ts",
                "web/component.tsx",
            ]
        );

        let languages: Vec<Language> = files.iter().map(|file| file.language).collect();
        assert_eq!(
            languages,
            vec![
                Language::Markdown,
                Language::Toml,
                Language::Sql,
                Language::Bash,
                Language::Rust,
                Language::Rust,
                Language::JavaScript,
                Language::JavaScript,
                Language::TypeScript,
                Language::Tsx,
            ]
        );
    }

    #[test]
    fn ignored_vendored_binary_and_huge_files_are_excluded() {
        let fixture = fixture_root();
        let files = walk_with(fixture.path(), &fixture_options()).expect("walk fixture");
        let paths = paths(&files);

        for excluded in [
            "src/ignored.rs",
            "src/ignored-dir/hidden.rs",
            "src/notes.log",
            "src/binary.rs",
            "node_modules/pkg/index.js",
            "target/debug/artifact.rs",
            "vendor/lib.rs",
            "dist/bundle.js",
            "assets/logo.bin",
            "src/large.rs",
            "src/unsupported.xyz",
            "src/noext",
        ] {
            assert!(
                !paths.iter().any(|path| path == excluded),
                "{excluded} should have been excluded, got {paths:?}"
            );
        }
    }

    #[test]
    fn output_order_is_deterministic() {
        let fixture = fixture_root();
        let first = walk_with(fixture.path(), &fixture_options()).expect("first walk");
        let second = walk_with(fixture.path(), &fixture_options()).expect("second walk");
        assert_eq!(first, second);
    }

    #[test]
    fn configured_excludes_are_honored() {
        let fixture = fixture_root();
        let mut options = fixture_options();
        options.excludes = vec!["web/".to_owned(), "*.md".to_owned()];

        let files = walk_with(fixture.path(), &options).expect("walk fixture");
        let paths = paths(&files);

        assert!(!paths.iter().any(|path| path.starts_with("web/")));
        assert!(!paths.iter().any(|path| path.ends_with(".md")));
        assert!(paths.iter().any(|path| path == "src/main.rs"));
    }

    #[test]
    fn walk_uses_config_excludes() {
        let fixture = fixture_root();
        let config_path = fixture.path().join("laudit-config.conf");
        fs::write(
            &config_path,
            "excludes = [\"web/\"]\n\n[provider]\nmodel = \"test-model\"\n",
        )
        .expect("write config");
        let config = Config::load_from(Some(&config_path), None).expect("load config");

        let files = walk(fixture.path(), &config).expect("walk fixture");
        let paths = paths(&files);

        assert!(!paths.iter().any(|path| path.starts_with("web/")));
        assert!(paths.iter().any(|path| path == "src/main.rs"));
    }

    #[test]
    fn language_filter_limits_results() {
        let fixture = fixture_root();
        let mut options = fixture_options();
        options.languages = vec!["rust".to_owned()];

        let files = walk_with(fixture.path(), &options).expect("walk fixture");
        assert_eq!(paths(&files), vec!["src/lib.rs", "src/main.rs"]);
    }

    #[test]
    fn typescript_filter_includes_tsx() {
        let fixture = fixture_root();
        let mut options = fixture_options();
        options.languages = vec!["typescript".to_owned()];

        let files = walk_with(fixture.path(), &options).expect("walk fixture");
        assert_eq!(paths(&files), vec!["web/app.ts", "web/component.tsx"]);
    }

    #[test]
    fn missing_root_is_an_error() {
        let fixture = fixture_root();
        let mut options = fixture_options();
        options.roots = vec![PathBuf::from("does-not-exist")];

        let error = walk_with(fixture.path(), &options).expect_err("missing root");
        assert!(matches!(error, WalkError::MissingRoot { .. }));
    }

    #[test]
    fn detects_languages_case_insensitively() {
        assert_eq!(Language::from_extension("RS"), Some(Language::Rust));
        assert_eq!(Language::from_extension("Ts"), Some(Language::TypeScript));
        assert_eq!(Language::from_extension("YML"), Some(Language::Yaml));
        assert_eq!(Language::from_extension("unknown"), None);
    }
}

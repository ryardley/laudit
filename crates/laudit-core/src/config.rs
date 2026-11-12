//! Global and per-repository configuration loading, plus API key resolution.
//!
//! Configuration is read from two TOML files:
//!
//! - global: `~/.config/laudit/config.toml` (or `$XDG_CONFIG_HOME/laudit/config.toml`)
//! - per-repo: `<repo>/.laudit/config.toml`
//!
//! Per-repo values override global values for the same key. API keys are
//! resolved from the environment first, then the global config, then the repo
//! config (see D17). Local endpoints do not require a key.

use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Environment variable consulted when `provider.api_key_env` is not set.
pub const API_KEY_ENV: &str = "LAUDIT_API_KEY";

/// Default OpenAI-compatible endpoint.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Languages understood by the bundled tree-sitter grammars.
pub const DEFAULT_LANGUAGES: &[&str] = &[
    "rust",
    "typescript",
    "javascript",
    "bash",
    "toml",
    "yaml",
    "json",
    "markdown",
    "sql",
];

/// Default roots when none are configured.
pub const DEFAULT_ROOTS: &[&str] = &["."];

/// Default maximum number of stops in a chapter.
pub const DEFAULT_MAX_STOPS_PER_CHAPTER: u32 = 12;

/// Default maximum context size, in tokens.
pub const DEFAULT_MAX_CONTEXT_TOKENS: u32 = 12_000;

/// Resolved provider settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// OpenAI-compatible base URL.
    pub base_url: String,
    /// Model identifier. Required; there is no hidden default.
    pub model: String,
    /// Optional environment variable that holds the API key.
    pub api_key_env: Option<String>,
}

/// Resolved redaction settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redaction {
    /// Additional regex rules supplied by the user.
    pub extra_rules: Vec<String>,
    /// Regexes that un-redact known false positives.
    pub allowlist: Vec<String>,
}

/// Resolved budget settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Budgets {
    /// Maximum number of stops in a chapter.
    pub max_stops_per_chapter: u32,
    /// Maximum context size, in tokens.
    pub max_context_tokens: u32,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            max_stops_per_chapter: DEFAULT_MAX_STOPS_PER_CHAPTER,
            max_context_tokens: DEFAULT_MAX_CONTEXT_TOKENS,
        }
    }
}

/// Fully resolved engine configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Provider settings.
    pub provider: Provider,
    /// Repository roots to scan.
    pub roots: Vec<String>,
    /// Languages to include.
    pub languages: Vec<String>,
    /// Additional path excludes.
    pub excludes: Vec<String>,
    /// Redaction settings.
    pub redaction: Redaction,
    /// Budget settings.
    pub budgets: Budgets,
    global_api_key: Option<String>,
    repo_api_key: Option<String>,
}

impl Config {
    /// Load the global config and the per-repo config for `repo_root`.
    ///
    /// Missing files are treated as empty. A missing `provider.model` is an
    /// error.
    pub fn load(repo_root: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let global = global_config_path();
        let repo = repo_config_path(repo_root);
        Self::load_from(global.as_deref(), Some(repo.as_path()))
    }

    /// Load configuration for the current working directory.
    pub fn load_current() -> Result<Self, ConfigError> {
        let cwd = env::current_dir().map_err(|source| ConfigError::Read {
            path: PathBuf::from("."),
            source,
        })?;
        Self::load(cwd)
    }

    /// Load configuration for recon-only work, where a provider model is not
    /// required.
    ///
    /// This is the offline build path: roots, languages, excludes, redaction,
    /// and budgets are still honored, but a missing `provider.model` is not an
    /// error. The resolved model is the placeholder `offline`.
    pub fn load_offline(repo_root: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let global = global_config_path();
        let repo = repo_config_path(repo_root);
        Self::load_from_offline(global.as_deref(), Some(repo.as_path()))
    }

    /// Load configuration from explicit paths without requiring a model.
    ///
    /// `None` means "no file at this layer". A path that does not exist is
    /// also treated as an empty layer.
    pub fn load_from_offline(
        global: Option<&Path>,
        repo: Option<&Path>,
    ) -> Result<Self, ConfigError> {
        let global = match global {
            Some(path) => FileConfig::read(path)?,
            None => FileConfig::default(),
        };
        let repo = match repo {
            Some(path) => FileConfig::read(path)?,
            None => FileConfig::default(),
        };
        Self::from_layers_with_model(global, repo, false)
    }

    /// Load configuration from explicit paths.
    ///
    /// `None` means "no file at this layer". A path that does not exist is
    /// also treated as an empty layer.
    pub fn load_from(global: Option<&Path>, repo: Option<&Path>) -> Result<Self, ConfigError> {
        let global = match global {
            Some(path) => FileConfig::read(path)?,
            None => FileConfig::default(),
        };
        let repo = match repo {
            Some(path) => FileConfig::read(path)?,
            None => FileConfig::default(),
        };
        Self::from_layers(global, repo)
    }

    fn from_layers(global: FileConfig, repo: FileConfig) -> Result<Self, ConfigError> {
        Self::from_layers_with_model(global, repo, true)
    }

    fn from_layers_with_model(
        global: FileConfig,
        repo: FileConfig,
        require_model: bool,
    ) -> Result<Self, ConfigError> {
        let FileConfig {
            provider: global_provider,
            roots: global_roots,
            languages: global_languages,
            excludes: global_excludes,
            redaction: global_redaction,
            budgets: global_budgets,
        } = global;
        let FileConfig {
            provider: repo_provider,
            roots: repo_roots,
            languages: repo_languages,
            excludes: repo_excludes,
            redaction: repo_redaction,
            budgets: repo_budgets,
        } = repo;

        let global_provider = global_provider.unwrap_or_default();
        let repo_provider = repo_provider.unwrap_or_default();

        let base_url = non_empty(repo_provider.base_url)
            .or_else(|| non_empty(global_provider.base_url))
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let model = non_empty(repo_provider.model)
            .or_else(|| non_empty(global_provider.model))
            .or_else(|| (!require_model).then(|| "offline".to_owned()))
            .ok_or(ConfigError::MissingModel)?;
        let api_key_env =
            non_empty(repo_provider.api_key_env).or_else(|| non_empty(global_provider.api_key_env));

        let global_api_key = non_empty(global_provider.api_key);
        let repo_api_key = non_empty(repo_provider.api_key);

        let roots = repo_roots.or(global_roots).unwrap_or_else(|| {
            DEFAULT_ROOTS
                .iter()
                .map(|root| (*root).to_owned())
                .collect()
        });
        let languages = repo_languages.or(global_languages).unwrap_or_else(|| {
            DEFAULT_LANGUAGES
                .iter()
                .map(|language| (*language).to_owned())
                .collect()
        });
        let excludes = repo_excludes.or(global_excludes).unwrap_or_default();

        let global_redaction = global_redaction.unwrap_or_default();
        let repo_redaction = repo_redaction.unwrap_or_default();
        let redaction = Redaction {
            extra_rules: repo_redaction
                .extra_rules
                .or(global_redaction.extra_rules)
                .unwrap_or_default(),
            allowlist: repo_redaction
                .allowlist
                .or(global_redaction.allowlist)
                .unwrap_or_default(),
        };

        let global_budgets = global_budgets.unwrap_or_default();
        let repo_budgets = repo_budgets.unwrap_or_default();
        let budgets = Budgets {
            max_stops_per_chapter: repo_budgets
                .max_stops_per_chapter
                .or(global_budgets.max_stops_per_chapter)
                .unwrap_or(DEFAULT_MAX_STOPS_PER_CHAPTER),
            max_context_tokens: repo_budgets
                .max_context_tokens
                .or(global_budgets.max_context_tokens)
                .unwrap_or(DEFAULT_MAX_CONTEXT_TOKENS),
        };

        Ok(Self {
            provider: Provider {
                base_url,
                model,
                api_key_env,
            },
            roots,
            languages,
            excludes,
            redaction,
            budgets,
            global_api_key,
            repo_api_key,
        })
    }

    /// The configured model identifier.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.provider.model
    }

    /// The configured OpenAI-compatible base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.provider.base_url
    }

    /// Resolve the API key, if any.
    ///
    /// Order, first match wins:
    ///
    /// 1. the environment variable named by `provider.api_key_env`
    /// 2. `LAUDIT_API_KEY`
    /// 3. the global config
    /// 4. the per-repo config
    #[must_use]
    pub fn api_key(&self) -> Option<String> {
        if let Some(name) = &self.provider.api_key_env {
            if let Some(value) = env_value(name) {
                return Some(value);
            }
        }
        if let Some(value) = env_value(API_KEY_ENV) {
            return Some(value);
        }
        self.global_api_key
            .clone()
            .or_else(|| self.repo_api_key.clone())
    }

    /// Resolve the API key, allowing local endpoints to have none.
    ///
    /// Returns `Ok(None)` for a local endpoint with no key, and an error for a
    /// remote endpoint with no key.
    pub fn resolve_api_key(&self) -> Result<Option<String>, ConfigError> {
        match self.api_key() {
            Some(key) => Ok(Some(key)),
            None if self.is_local_endpoint() => Ok(None),
            None => Err(ConfigError::MissingApiKey {
                base_url: self.provider.base_url.clone(),
            }),
        }
    }

    /// Validate that the configuration is usable.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.provider.model.trim().is_empty() {
            return Err(ConfigError::MissingModel);
        }
        self.resolve_api_key().map(|_| ())
    }

    /// Whether the configured endpoint is local and therefore needs no key.
    #[must_use]
    pub fn is_local_endpoint(&self) -> bool {
        is_local_url(&self.provider.base_url)
    }

    /// Alias for [`Config::is_local_endpoint`].
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.is_local_endpoint()
    }
}

/// The global config directory, if a home directory can be determined.
///
/// This is `$XDG_CONFIG_HOME/laudit` when `XDG_CONFIG_HOME` is set, otherwise
/// `$HOME/.config/laudit`.
#[must_use]
pub fn global_config_dir() -> Option<PathBuf> {
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(xdg).join("laudit"));
    }
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".config").join("laudit"))
}

/// The global config path, if a home directory can be determined.
#[must_use]
pub fn global_config_path() -> Option<PathBuf> {
    global_config_dir().map(|dir| dir.join("config.toml"))
}

/// The per-repo config path for `repo_root`.
#[must_use]
pub fn repo_config_path(repo_root: impl AsRef<Path>) -> PathBuf {
    repo_root.as_ref().join(".laudit").join("config.toml")
}

/// Whether `url` points at a local endpoint.
#[must_use]
pub fn is_local_url(url: &str) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host == "0.0.0.0"
}

fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if let Some(stripped) = authority.strip_prefix('[') {
        return stripped.split_once(']').map(|(host, _)| host);
    }
    Some(authority.split(':').next().unwrap_or(authority))
}

fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[derive(Debug, Clone, Default, Deserialize)]
struct FileConfig {
    provider: Option<ProviderFile>,
    roots: Option<Vec<String>>,
    languages: Option<Vec<String>>,
    excludes: Option<Vec<String>>,
    redaction: Option<RedactionFile>,
    budgets: Option<BudgetsFile>,
}

impl FileConfig {
    fn read(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(contents) => toml::from_str(&contents).map_err(|source| ConfigError::Parse {
                path: path.to_path_buf(),
                source,
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProviderFile {
    base_url: Option<String>,
    model: Option<String>,
    api_key_env: Option<String>,
    api_key: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RedactionFile {
    extra_rules: Option<Vec<String>>,
    allowlist: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct BudgetsFile {
    max_stops_per_chapter: Option<u32>,
    max_context_tokens: Option<u32>,
}

/// Errors produced while loading or validating configuration.
#[derive(Debug)]
pub enum ConfigError {
    /// A config file could not be read.
    Read {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// A config file was not valid TOML.
    Parse {
        /// Path that failed.
        path: PathBuf,
        /// Underlying parse error.
        source: toml::de::Error,
    },
    /// `provider.model` was not configured.
    MissingModel,
    /// A remote endpoint was configured without an API key.
    MissingApiKey {
        /// The endpoint that needs a key.
        base_url: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "failed to read config {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(f, "failed to parse config {}: {source}", path.display())
            }
            Self::MissingModel => write!(
                f,
                "provider.model is required; set it in ~/.config/laudit/config.toml or .laudit/config.toml"
            ),
            Self::MissingApiKey { base_url } => write!(
                f,
                "no API key found for {base_url}; set {API_KEY_ENV} or provider.api_key_env, or use a local endpoint"
            ),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::MissingModel | Self::MissingApiKey { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                env::temp_dir().join(format!("laudit-config-test-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.path.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent dir");
            }
            fs::write(&path, contents).expect("write config");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct EnvGuard {
        name: String,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(name: &str, value: &str) -> Self {
            let previous = env::var_os(name);
            env::set_var(name, value);
            Self {
                name: name.to_owned(),
                previous,
            }
        }

        fn remove(name: &str) -> Self {
            let previous = env::var_os(name);
            env::remove_var(name);
            Self {
                name: name.to_owned(),
                previous,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => env::set_var(&self.name, value),
                None => env::remove_var(&self.name),
            }
        }
    }

    fn config_with(global: &str, repo: &str) -> Config {
        let dir = TempDir::new();
        let global = dir.write("global.toml", global);
        let repo = dir.write("repo.toml", repo);
        Config::load_from(Some(&global), Some(&repo)).expect("load config")
    }

    #[test]
    fn missing_model_is_a_clear_error() {
        let dir = TempDir::new();
        let global = dir.write(
            "global.toml",
            "[provider]\nbase_url = \"http://localhost:11434/v1\"\n",
        );
        let error = Config::load_from(Some(&global), None).expect_err("missing model");
        assert!(matches!(error, ConfigError::MissingModel));
        assert!(error.to_string().contains("model"));
    }

    #[test]
    fn offline_loading_does_not_require_a_model() {
        let dir = TempDir::new();
        let global = dir.write(
            "global.toml",
            "roots = [\"src\"]\n\n[provider]\nbase_url = \"http://localhost:11434/v1\"\n",
        );
        let config = Config::load_from_offline(Some(&global), None).expect("load offline config");
        assert_eq!(config.model(), "offline");
        assert_eq!(config.roots, vec!["src"]);
    }

    #[test]
    fn repo_overrides_global_roots_and_languages() {
        let config = config_with(
            "roots = [\"global-root\"]\nlanguages = [\"rust\"]\n\n[provider]\nmodel = \"global-model\"\n",
            "roots = [\"repo-root\"]\nlanguages = [\"typescript\"]\n\n[provider]\nmodel = \"repo-model\"\n",
        );
        assert_eq!(config.roots, vec!["repo-root"]);
        assert_eq!(config.languages, vec!["typescript"]);
        assert_eq!(config.model(), "repo-model");
    }

    #[test]
    fn global_values_are_used_when_repo_omits_them() {
        let config = config_with(
            "roots = [\"global-root\"]\nlanguages = [\"rust\"]\nexcludes = [\"vendor/\"]\n\n[provider]\nmodel = \"global-model\"\n",
            "[provider]\nmodel = \"repo-model\"\n",
        );
        assert_eq!(config.roots, vec!["global-root"]);
        assert_eq!(config.languages, vec!["rust"]);
        assert_eq!(config.excludes, vec!["vendor/"]);
        assert_eq!(config.model(), "repo-model");
    }

    #[test]
    fn api_key_prefers_environment_then_global_then_repo() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _laudit = EnvGuard::remove(API_KEY_ENV);
        let _custom = EnvGuard::remove("LAUDIT_TEST_KEY");

        let global = "[provider]\nmodel = \"m\"\napi_key = \"global-key\"\napi_key_env = \"LAUDIT_TEST_KEY\"\n";
        let repo = "[provider]\napi_key = \"repo-key\"\n";

        let config = config_with(global, repo);
        assert_eq!(config.api_key().as_deref(), Some("global-key"));

        let _custom = EnvGuard::set("LAUDIT_TEST_KEY", "custom-env-key");
        let config = config_with(global, repo);
        assert_eq!(config.api_key().as_deref(), Some("custom-env-key"));

        let _laudit = EnvGuard::set(API_KEY_ENV, "laudit-env-key");
        let config = config_with(global, repo);
        assert_eq!(config.api_key().as_deref(), Some("custom-env-key"));
    }

    #[test]
    fn laudit_api_key_env_is_used_when_named_env_is_absent() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _laudit = EnvGuard::remove(API_KEY_ENV);
        let _custom = EnvGuard::remove("LAUDIT_TEST_KEY");

        let global = "[provider]\nmodel = \"m\"\napi_key = \"global-key\"\napi_key_env = \"LAUDIT_TEST_KEY\"\n";
        let repo = "[provider]\napi_key = \"repo-key\"\n";

        let _laudit = EnvGuard::set(API_KEY_ENV, "laudit-env-key");
        let config = config_with(global, repo);
        assert_eq!(config.api_key().as_deref(), Some("laudit-env-key"));
    }

    #[test]
    fn repo_key_is_used_when_global_has_none() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _laudit = EnvGuard::remove(API_KEY_ENV);

        let config = config_with(
            "[provider]\nmodel = \"m\"\n",
            "[provider]\napi_key = \"repo-key\"\n",
        );
        assert_eq!(config.api_key().as_deref(), Some("repo-key"));
    }

    #[test]
    fn local_endpoint_requires_no_key() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _laudit = EnvGuard::remove(API_KEY_ENV);

        let dir = TempDir::new();
        let global = dir.write(
            "global.toml",
            "[provider]\nbase_url = \"http://localhost:11434/v1\"\nmodel = \"llama3\"\n",
        );
        let config = Config::load_from(Some(&global), None).expect("load config");

        assert!(config.is_local_endpoint());
        assert!(config.is_local());
        assert_eq!(config.api_key(), None);
        assert_eq!(config.resolve_api_key().expect("local key"), None);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn remote_endpoint_without_key_is_an_error() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _laudit = EnvGuard::remove(API_KEY_ENV);

        let dir = TempDir::new();
        let global = dir.write(
            "global.toml",
            "[provider]\nbase_url = \"https://api.example.com/v1\"\nmodel = \"m\"\n",
        );
        let config = Config::load_from(Some(&global), None).expect("load config");

        assert!(!config.is_local_endpoint());
        assert!(matches!(
            config.resolve_api_key(),
            Err(ConfigError::MissingApiKey { .. })
        ));
        assert!(config.validate().is_err());
    }

    #[test]
    fn detects_local_endpoints() {
        for url in [
            "http://localhost",
            "http://localhost:11434/v1",
            "http://127.0.0.1:8080",
            "https://[::1]:443/v1",
            "localhost:11434",
        ] {
            assert!(is_local_url(url), "{url} should be local");
        }
        for url in [
            "https://api.openai.com/v1",
            "http://example.com",
            "http://localhost.example.com",
        ] {
            assert!(!is_local_url(url), "{url} should not be local");
        }
    }

    #[test]
    fn defaults_are_applied() {
        let dir = TempDir::new();
        let global = dir.write("global.toml", "[provider]\nmodel = \"m\"\n");
        let config = Config::load_from(Some(&global), None).expect("load config");

        assert_eq!(config.roots, vec!["."]);
        assert_eq!(
            config.languages,
            DEFAULT_LANGUAGES
                .iter()
                .map(|language| (*language).to_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(config.budgets.max_stops_per_chapter, 12);
        assert_eq!(config.budgets.max_context_tokens, 12_000);
        assert_eq!(config.base_url(), DEFAULT_BASE_URL);
    }

    #[test]
    fn redaction_and_budgets_override_per_field() {
        let config = config_with(
            "[provider]\nmodel = \"m\"\n\n[redaction]\nextra_rules = [\"global-rule\"]\nallowlist = [\"global-allow\"]\n\n[budgets]\nmax_stops_per_chapter = 5\nmax_context_tokens = 1000\n",
            "[redaction]\nextra_rules = [\"repo-rule\"]\n\n[budgets]\nmax_context_tokens = 2000\n",
        );
        assert_eq!(config.redaction.extra_rules, vec!["repo-rule"]);
        assert_eq!(config.redaction.allowlist, vec!["global-allow"]);
        assert_eq!(config.budgets.max_stops_per_chapter, 5);
        assert_eq!(config.budgets.max_context_tokens, 2000);
    }

    #[test]
    fn invalid_toml_reports_the_path() {
        let dir = TempDir::new();
        let global = dir.write("global.toml", "not = [valid");
        let error = Config::load_from(Some(&global), None).expect_err("invalid toml");
        assert!(matches!(error, ConfigError::Parse { .. }));
        assert!(error.to_string().contains("global.toml"));
    }

    #[test]
    fn missing_files_are_treated_as_empty_layers() {
        let dir = TempDir::new();
        let global = dir.path.join("missing-global.toml");
        let repo = dir.path.join("missing-repo.toml");
        let error = Config::load_from(Some(&global), Some(&repo)).expect_err("missing model");
        assert!(matches!(error, ConfigError::MissingModel));
    }

    #[test]
    fn load_reads_repo_config_from_dot_laudit() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _laudit = EnvGuard::remove(API_KEY_ENV);
        let _xdg = EnvGuard::remove("XDG_CONFIG_HOME");

        let dir = TempDir::new();
        let repo = dir.write(
            ".laudit/config.toml",
            "[provider]\nmodel = \"repo-model\"\n",
        );
        let _home = EnvGuard::set("HOME", dir.path.to_str().expect("utf-8 temp path"));

        let config = Config::load(&dir.path).expect("load config");
        assert_eq!(config.model(), "repo-model");
        assert_eq!(repo, repo_config_path(&dir.path));
    }
}

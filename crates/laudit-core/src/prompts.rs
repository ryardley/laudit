//! Embedded prompts, first-run installation, and versioning (D18).
//!
//! Default prompts are compiled into the binary. On first run they are
//! installed to `~/.config/laudit/prompts/` together with a manifest that
//! records the installed version of each prompt. At runtime prompts are loaded
//! from that directory so users can edit them.
//!
//! When a newer prompt ships, the engine warns and writes the new version
//! alongside the installed file (`<name>.md.new`) instead of overwriting the
//! user's edits.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;

/// Prompt used by Stage 1 comprehension.
pub const COMPREHENSION: &str = "comprehension";
/// Prompt used by Stage 3 narration.
pub const NARRATION: &str = "narration";
/// Prompt used to repair malformed structured output.
pub const REPAIR: &str = "repair";
/// Prompt used by grounded Q&A.
pub const ASK: &str = "ask";

/// Name of the manifest that records installed prompt versions.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Schema version of the prompt manifest.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// A prompt compiled into the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedPrompt {
    /// Stable prompt name, also the file stem.
    pub name: &'static str,
    /// Version of this shipped prompt.
    pub version: u32,
    /// Default prompt contents.
    pub contents: &'static str,
}

/// All prompts shipped with this build.
pub const EMBEDDED_PROMPTS: &[EmbeddedPrompt] = &[
    EmbeddedPrompt {
        name: COMPREHENSION,
        version: 1,
        contents: include_str!("../prompts/comprehension.md"),
    },
    EmbeddedPrompt {
        name: NARRATION,
        version: 1,
        contents: include_str!("../prompts/narration.md"),
    },
    EmbeddedPrompt {
        name: REPAIR,
        version: 1,
        contents: include_str!("../prompts/repair.md"),
    },
    EmbeddedPrompt {
        name: ASK,
        version: 1,
        contents: include_str!("../prompts/ask.md"),
    },
];

/// Look up a shipped prompt by name.
#[must_use]
pub fn embedded(name: &str) -> Option<&'static EmbeddedPrompt> {
    EMBEDDED_PROMPTS.iter().find(|prompt| prompt.name == name)
}

/// Iterate over the names of all shipped prompts.
pub fn embedded_names() -> impl Iterator<Item = &'static str> {
    EMBEDDED_PROMPTS.iter().map(|prompt| prompt.name)
}

/// The shipped version of `name`, if it exists.
#[must_use]
pub fn version(name: &str) -> Option<u32> {
    embedded(name).map(|prompt| prompt.version)
}

/// The prompts directory under the global config directory.
#[must_use]
pub fn prompts_dir() -> Option<PathBuf> {
    config::global_config_dir().map(|dir| dir.join("prompts"))
}

/// The prompts directory for an explicit config home (for example `$HOME/.config`).
#[must_use]
pub fn prompts_dir_in(config_home: &Path) -> PathBuf {
    config_home.join("laudit").join("prompts")
}

/// Where a loaded prompt came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSource {
    /// Loaded from the user's prompts directory.
    Installed,
    /// Fell back to the prompt compiled into the binary.
    Embedded,
}

/// A prompt loaded for use at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedPrompt {
    /// Stable prompt name.
    pub name: String,
    /// Version recorded for the installed prompt.
    pub version: u32,
    /// Prompt contents.
    pub contents: String,
    /// Installed path, or `None` when the embedded fallback was used.
    pub path: Option<PathBuf>,
    /// Where the contents came from.
    pub source: PromptSource,
}

/// A warning that a shipped prompt is newer than the installed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptWarning {
    /// Prompt name.
    pub name: String,
    /// Version currently installed.
    pub installed_version: u32,
    /// Version shipped with this build.
    pub shipped_version: u32,
    /// Path of the non-destructive update file.
    pub update_path: PathBuf,
}

impl fmt::Display for PromptWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "prompt `{}` has a newer shipped version (installed {}, shipped {}); \
             wrote the update to {} (your installed prompt was left unchanged)",
            self.name,
            self.installed_version,
            self.shipped_version,
            self.update_path.display()
        )
    }
}

/// Result of ensuring prompts are installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    /// Prompts directory that was used.
    pub dir: PathBuf,
    /// Prompts that were newly installed.
    pub installed: Vec<String>,
    /// Warnings about newer shipped prompts.
    pub warnings: Vec<PromptWarning>,
}

/// Summary of one prompt for `laudit prompts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptInfo {
    /// Stable prompt name.
    pub name: String,
    /// Installed version.
    pub version: u32,
    /// Version shipped with this build.
    pub shipped_version: u32,
    /// Expected installed path.
    pub path: PathBuf,
    /// Whether the prompt file exists.
    pub installed: bool,
    /// Whether a newer shipped version is available.
    pub outdated: bool,
}

/// Install any missing prompts and warn about newer shipped versions.
pub fn ensure_installed() -> Result<InstallReport, PromptError> {
    let dir = prompts_dir().ok_or(PromptError::NoConfigDir)?;
    ensure_installed_at(&dir)
}

/// Install any missing prompts under an explicit config home.
pub fn ensure_installed_in(config_home: &Path) -> Result<InstallReport, PromptError> {
    ensure_installed_at(&prompts_dir_in(config_home))
}

/// Install any missing prompts into `dir`.
///
/// Existing prompt files are never overwritten. If a shipped prompt is newer
/// than the installed version, the new contents are written to
/// `<name>.md.new` and a warning is returned.
pub fn ensure_installed_at(dir: &Path) -> Result<InstallReport, PromptError> {
    fs::create_dir_all(dir).map_err(|source| PromptError::Write {
        path: dir.to_path_buf(),
        source,
    })?;

    let manifest_path = dir.join(MANIFEST_FILE);
    let mut manifest = read_manifest(&manifest_path)?;
    let mut installed = Vec::new();
    let mut warnings = Vec::new();
    let mut changed = false;

    for prompt in EMBEDDED_PROMPTS {
        let path = dir.join(format!("{}.md", prompt.name));
        let installed_version = manifest.prompts.get(prompt.name).copied();

        if !path.exists() {
            write_file(&path, prompt.contents)?;
            manifest
                .prompts
                .insert(prompt.name.to_owned(), prompt.version);
            installed.push(prompt.name.to_owned());
            changed = true;
        } else if let Some(installed_version) = installed_version {
            if prompt.version > installed_version {
                let update_path = dir.join(format!("{}.md.new", prompt.name));
                write_file(&update_path, prompt.contents)?;
                warnings.push(PromptWarning {
                    name: prompt.name.to_owned(),
                    installed_version,
                    shipped_version: prompt.version,
                    update_path,
                });
            }
        } else {
            manifest
                .prompts
                .insert(prompt.name.to_owned(), prompt.version);
            changed = true;
        }
    }

    if changed || !manifest_path.exists() {
        write_manifest(&manifest_path, &manifest)?;
    }

    Ok(InstallReport {
        dir: dir.to_path_buf(),
        installed,
        warnings,
    })
}

/// Load a prompt from the user's prompts directory, falling back to the
/// embedded default when the file is missing.
pub fn load(name: &str) -> Result<LoadedPrompt, PromptError> {
    let dir = prompts_dir().ok_or(PromptError::NoConfigDir)?;
    load_from_dir(&dir, name)
}

/// Load a prompt from an explicit config home.
pub fn load_from(config_home: &Path, name: &str) -> Result<LoadedPrompt, PromptError> {
    load_from_dir(&prompts_dir_in(config_home), name)
}

/// Load a prompt from `dir`, falling back to the embedded default.
pub fn load_from_dir(dir: &Path, name: &str) -> Result<LoadedPrompt, PromptError> {
    let embedded = embedded(name).ok_or_else(|| PromptError::UnknownPrompt {
        name: name.to_owned(),
    })?;
    let path = dir.join(format!("{name}.md"));

    match fs::read_to_string(&path) {
        Ok(contents) => {
            let manifest = read_manifest(&dir.join(MANIFEST_FILE))?;
            let version = manifest
                .prompts
                .get(name)
                .copied()
                .unwrap_or(embedded.version);
            Ok(LoadedPrompt {
                name: name.to_owned(),
                version,
                contents,
                path: Some(path),
                source: PromptSource::Installed,
            })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(LoadedPrompt {
            name: name.to_owned(),
            version: embedded.version,
            contents: embedded.contents.to_owned(),
            path: None,
            source: PromptSource::Embedded,
        }),
        Err(source) => Err(PromptError::Read { path, source }),
    }
}

/// The installed version of `name` as a string, suitable for `prompt_version`
/// in artifacts.
pub fn prompt_version(name: &str) -> Result<String, PromptError> {
    Ok(load(name)?.version.to_string())
}

/// The installed version of `name` for an explicit config home, as a string.
pub fn prompt_version_from(config_home: &Path, name: &str) -> Result<String, PromptError> {
    Ok(load_from(config_home, name)?.version.to_string())
}

/// List all shipped prompts and their installation state.
pub fn list() -> Result<Vec<PromptInfo>, PromptError> {
    let dir = prompts_dir().ok_or(PromptError::NoConfigDir)?;
    list_from_dir(&dir)
}

/// List all shipped prompts for an explicit config home.
pub fn list_from(config_home: &Path) -> Result<Vec<PromptInfo>, PromptError> {
    list_from_dir(&prompts_dir_in(config_home))
}

/// List all shipped prompts for `dir`.
pub fn list_from_dir(dir: &Path) -> Result<Vec<PromptInfo>, PromptError> {
    let manifest = read_manifest(&dir.join(MANIFEST_FILE))?;
    Ok(EMBEDDED_PROMPTS
        .iter()
        .map(|prompt| {
            let path = dir.join(format!("{}.md", prompt.name));
            let installed = path.exists();
            let version = manifest
                .prompts
                .get(prompt.name)
                .copied()
                .unwrap_or(prompt.version);
            PromptInfo {
                name: prompt.name.to_owned(),
                version,
                shipped_version: prompt.version,
                path,
                installed,
                outdated: prompt.version > version,
            }
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    prompts: BTreeMap<String, u32>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            prompts: BTreeMap::new(),
        }
    }
}

fn read_manifest(path: &Path) -> Result<Manifest, PromptError> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let manifest: Manifest =
                serde_json::from_str(&contents).map_err(|source| PromptError::Parse {
                    path: path.to_path_buf(),
                    source,
                })?;
            if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
                return Err(PromptError::UnsupportedManifest {
                    path: path.to_path_buf(),
                    found: manifest.schema_version,
                    expected: MANIFEST_SCHEMA_VERSION,
                });
            }
            Ok(manifest)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Manifest::default()),
        Err(source) => Err(PromptError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_manifest(path: &Path, manifest: &Manifest) -> Result<(), PromptError> {
    let contents =
        serde_json::to_string_pretty(manifest).map_err(|source| PromptError::Serialize {
            path: path.to_path_buf(),
            source,
        })?;
    write_file(path, &format!("{contents}\n"))
}

fn write_file(path: &Path, contents: &str) -> Result<(), PromptError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| PromptError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, contents).map_err(|source| PromptError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// Errors produced while installing or loading prompts.
#[derive(Debug)]
pub enum PromptError {
    /// No home/config directory could be determined.
    NoConfigDir,
    /// The requested prompt is not shipped with this build.
    UnknownPrompt {
        /// Requested prompt name.
        name: String,
    },
    /// A prompt or manifest could not be read.
    Read {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// A prompt or manifest could not be written.
    Write {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// The manifest was not valid JSON.
    Parse {
        /// Path that failed.
        path: PathBuf,
        /// Underlying parse error.
        source: serde_json::Error,
    },
    /// The manifest could not be serialized.
    Serialize {
        /// Path that failed.
        path: PathBuf,
        /// Underlying serialization error.
        source: serde_json::Error,
    },
    /// The manifest declares an unsupported schema version.
    UnsupportedManifest {
        /// Path that failed.
        path: PathBuf,
        /// Version found in the manifest.
        found: u32,
        /// Version this build understands.
        expected: u32,
    },
}

impl fmt::Display for PromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoConfigDir => write!(
                f,
                "could not determine a config directory; set HOME or XDG_CONFIG_HOME"
            ),
            Self::UnknownPrompt { name } => write!(f, "unknown prompt `{name}`"),
            Self::Read { path, source } => {
                write!(f, "failed to read prompt file {}: {source}", path.display())
            }
            Self::Write { path, source } => {
                write!(f, "failed to write prompt file {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(f, "failed to parse prompt manifest {}: {source}", path.display())
            }
            Self::Serialize { path, source } => {
                write!(f, "failed to serialize prompt manifest {}: {source}", path.display())
            }
            Self::UnsupportedManifest {
                path,
                found,
                expected,
            } => write!(
                f,
                "unsupported prompt manifest schema_version {found} in {} (this build supports {expected})",
                path.display()
            ),
        }
    }
}

impl std::error::Error for PromptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Write { source, .. } => Some(source),
            Self::Parse { source, .. } | Self::Serialize { source, .. } => Some(source),
            Self::NoConfigDir | Self::UnknownPrompt { .. } | Self::UnsupportedManifest { .. } => {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                env::temp_dir().join(format!("laudit-prompts-test-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn manifest_path(dir: &Path) -> PathBuf {
        dir.join(MANIFEST_FILE)
    }

    fn read_manifest_json(dir: &Path) -> Manifest {
        let contents = fs::read_to_string(manifest_path(dir)).expect("read manifest");
        serde_json::from_str(&contents).expect("parse manifest")
    }

    fn write_manifest_json(dir: &Path, manifest: &Manifest) {
        let contents = serde_json::to_string_pretty(manifest).expect("serialize manifest");
        fs::write(manifest_path(dir), contents).expect("write manifest");
    }

    #[test]
    fn first_run_installs_every_prompt_and_manifest() {
        let temp = TempDir::new();
        let report = ensure_installed_in(&temp.path).expect("install prompts");

        assert_eq!(report.dir, prompts_dir_in(&temp.path));
        assert!(report.warnings.is_empty());
        assert_eq!(report.installed.len(), EMBEDDED_PROMPTS.len());

        for prompt in EMBEDDED_PROMPTS {
            let path = report.dir.join(format!("{}.md", prompt.name));
            assert!(path.is_file(), "missing {}", path.display());
            assert_eq!(
                fs::read_to_string(&path).expect("read prompt"),
                prompt.contents
            );
        }

        let manifest = read_manifest_json(&report.dir);
        assert_eq!(manifest.schema_version, MANIFEST_SCHEMA_VERSION);
        for prompt in EMBEDDED_PROMPTS {
            assert_eq!(manifest.prompts.get(prompt.name), Some(&prompt.version));
        }
    }

    #[test]
    fn second_run_is_idempotent() {
        let temp = TempDir::new();
        ensure_installed_in(&temp.path).expect("first install");
        let report = ensure_installed_in(&temp.path).expect("second install");

        assert!(report.installed.is_empty());
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn edits_in_the_prompts_directory_are_loaded() {
        let temp = TempDir::new();
        let report = ensure_installed_in(&temp.path).expect("install prompts");
        let path = report.dir.join("comprehension.md");
        fs::write(&path, "custom comprehension prompt\n").expect("edit prompt");

        let loaded = load_from(&temp.path, COMPREHENSION).expect("load prompt");
        assert_eq!(loaded.contents, "custom comprehension prompt\n");
        assert_eq!(loaded.source, PromptSource::Installed);
        assert_eq!(loaded.path.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn missing_prompt_falls_back_to_embedded() {
        let temp = TempDir::new();
        let loaded = load_from(&temp.path, NARRATION).expect("load prompt");

        assert_eq!(loaded.source, PromptSource::Embedded);
        assert_eq!(loaded.path, None);
        assert_eq!(loaded.version, version(NARRATION).expect("shipped version"));
        assert_eq!(
            loaded.contents,
            embedded(NARRATION).expect("embedded prompt").contents
        );
    }

    #[test]
    fn newer_shipped_version_warns_and_writes_a_new_file() {
        let temp = TempDir::new();
        let report = ensure_installed_in(&temp.path).expect("install prompts");
        let original = fs::read_to_string(report.dir.join("comprehension.md")).expect("read");

        let mut manifest = read_manifest_json(&report.dir);
        manifest.prompts.insert(COMPREHENSION.to_owned(), 0);
        write_manifest_json(&report.dir, &manifest);

        let report = ensure_installed_in(&temp.path).expect("reinstall prompts");
        assert_eq!(report.warnings.len(), 1);
        let warning = &report.warnings[0];
        assert_eq!(warning.name, COMPREHENSION);
        assert_eq!(warning.installed_version, 0);
        assert_eq!(
            warning.shipped_version,
            version(COMPREHENSION).expect("shipped version")
        );
        assert!(warning.update_path.is_file());
        assert!(warning.to_string().contains("newer shipped version"));

        assert_eq!(
            fs::read_to_string(report.dir.join("comprehension.md")).expect("read original"),
            original
        );
        assert_eq!(
            fs::read_to_string(&warning.update_path).expect("read update"),
            embedded(COMPREHENSION).expect("embedded prompt").contents
        );
    }

    #[test]
    fn list_reports_versions_and_outdated_state() {
        let temp = TempDir::new();
        let report = ensure_installed_in(&temp.path).expect("install prompts");

        let mut manifest = read_manifest_json(&report.dir);
        manifest.prompts.insert(REPAIR.to_owned(), 0);
        write_manifest_json(&report.dir, &manifest);

        let prompts = list_from(&temp.path).expect("list prompts");
        assert_eq!(prompts.len(), EMBEDDED_PROMPTS.len());

        let repair = prompts
            .iter()
            .find(|prompt| prompt.name == REPAIR)
            .expect("repair prompt");
        assert!(repair.installed);
        assert!(repair.outdated);
        assert_eq!(repair.version, 0);
        assert_eq!(
            repair.shipped_version,
            version(REPAIR).expect("shipped version")
        );

        let narration = prompts
            .iter()
            .find(|prompt| prompt.name == NARRATION)
            .expect("narration prompt");
        assert!(narration.installed);
        assert!(!narration.outdated);
    }

    #[test]
    fn prompt_version_matches_the_installed_manifest() {
        let temp = TempDir::new();
        let report = ensure_installed_in(&temp.path).expect("install prompts");

        let mut manifest = read_manifest_json(&report.dir);
        manifest.prompts.insert(NARRATION.to_owned(), 7);
        write_manifest_json(&report.dir, &manifest);

        assert_eq!(
            prompt_version_from(&temp.path, NARRATION).expect("prompt version"),
            "7"
        );
    }

    #[test]
    fn unknown_prompt_is_an_error() {
        let temp = TempDir::new();
        let error = load_from(&temp.path, "nope").expect_err("unknown prompt");
        assert!(matches!(error, PromptError::UnknownPrompt { .. }));
        assert!(error.to_string().contains("nope"));
    }

    #[test]
    fn existing_files_without_a_manifest_are_preserved() {
        let temp = TempDir::new();
        let dir = prompts_dir_in(&temp.path);
        fs::create_dir_all(&dir).expect("create prompts dir");
        fs::write(dir.join("comprehension.md"), "user prompt\n").expect("write prompt");

        let report = ensure_installed_at(&dir).expect("install prompts");
        assert!(report.installed.contains(&NARRATION.to_owned()));
        assert!(!report.installed.contains(&COMPREHENSION.to_owned()));
        assert_eq!(
            fs::read_to_string(dir.join("comprehension.md")).expect("read prompt"),
            "user prompt\n"
        );
        assert!(manifest_path(&dir).is_file());
    }

    #[test]
    fn unsupported_manifest_version_is_rejected() {
        let temp = TempDir::new();
        let dir = prompts_dir_in(&temp.path);
        fs::create_dir_all(&dir).expect("create prompts dir");
        fs::write(manifest_path(&dir), r#"{"schema_version":99,"prompts":{}}"#)
            .expect("write manifest");

        let error = ensure_installed_at(&dir).expect_err("unsupported manifest");
        assert!(matches!(
            error,
            PromptError::UnsupportedManifest { found: 99, .. }
        ));
    }
}

//! Content-hash incremental index.
//!
//! Recon is re-run often, but most files do not change between runs. This
//! module persists the parse result for every source file under
//! `.laudit/index/` and reuses it when the file's content hash is unchanged
//! (D19). A build therefore re-parses only added or modified files, drops
//! entries for deleted files, and rebuilds the graph from the combined tag
//! set.
//!
//! Tree-sitter trees are not serializable, so cross-process incremental
//! parsing is not practical. The content-hash index is the incremental
//! mechanism: unchanged files are not parsed at all, and changed files are
//! parsed from scratch. The index is machine-local and regenerable; its format
//! is internal and may change without a schema-version bump (see
//! `docs/artifacts.md`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::graph::{FileEdge, Graph, SymbolEdge};
use super::parse::{parse_source, Tag};
use super::walk::{Language, SourceFile};

/// Directory, relative to the repository root, holding the machine-local index.
pub const INDEX_DIR: &str = ".laudit/index";
/// File name of the persisted index inside [`INDEX_DIR`].
pub const INDEX_FILE: &str = "index.json";
/// Repository-relative path of the shipped `.laudit/.gitignore`.
pub const GITIGNORE_FILE: &str = ".laudit/.gitignore";

/// The persisted parse result for one source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Path relative to the repository root.
    pub path: PathBuf,
    /// Stable content hash of the file bytes.
    pub hash: String,
    /// Language used to parse the file.
    pub language: Language,
    /// Definitions and references extracted from the file.
    pub tags: Vec<Tag>,
}

/// The persisted incremental index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    /// Per-file entries, keyed and sorted by repository-relative path.
    pub files: BTreeMap<PathBuf, FileEntry>,
    /// Directed file edges derived from the combined tag set.
    #[serde(default)]
    pub file_edges: Vec<FileEdge>,
    /// Directed symbol edges derived from the combined tag set.
    #[serde(default)]
    pub symbol_edges: Vec<SymbolEdge>,
}

impl Index {
    /// Load the index for `repo_root`, if one exists and is readable.
    pub fn load(repo_root: impl AsRef<Path>) -> io::Result<Option<Self>> {
        load(repo_root)
    }

    /// Persist this index under `repo_root`.
    pub fn save(&self, repo_root: impl AsRef<Path>) -> io::Result<()> {
        save(repo_root, self)
    }

    /// Build the index for `files`, reusing unchanged entries.
    pub fn build(repo_root: impl AsRef<Path>, files: &[SourceFile]) -> io::Result<IndexBuild> {
        build(repo_root, files)
    }

    /// All tags in the index, sorted and deduplicated.
    #[must_use]
    pub fn tags(&self) -> Vec<Tag> {
        let mut tags: Vec<Tag> = self
            .files
            .values()
            .flat_map(|entry| entry.tags.iter().cloned())
            .collect();
        tags.sort();
        tags.dedup();
        tags
    }
}

/// Counters describing what an index build did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuildStats {
    /// Number of files in the resulting index.
    pub total: usize,
    /// Number of files parsed during this build.
    pub parsed: usize,
    /// Number of files whose cached tags were reused.
    pub reused: usize,
    /// Number of files that were not in the previous index.
    pub added: usize,
    /// Number of files whose content hash changed.
    pub changed: usize,
    /// Number of previous entries dropped because the file is gone.
    pub removed: usize,
}

/// The result of an index build.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexBuild {
    /// The persisted index.
    pub index: Index,
    /// Combined tags from every indexed file.
    pub tags: Vec<Tag>,
    /// Graph rebuilt from the combined tags.
    pub graph: Graph,
    /// Counters describing the work performed.
    pub stats: BuildStats,
}

/// Build the incremental index for `files`, resolving paths against `repo_root`.
///
/// Files whose content hash and language match the previous index are reused
/// without parsing. Added or changed files are parsed, and entries for files
/// that are no longer present are dropped. The resulting index and graph edges
/// are written under `.laudit/index/`, and `.laudit/.gitignore` is created or
/// updated to ignore the index directory.
///
/// Unreadable files are skipped so one bad file cannot abort recon.
pub fn build(repo_root: impl AsRef<Path>, files: &[SourceFile]) -> io::Result<IndexBuild> {
    let repo_root = repo_root.as_ref();
    let previous = load(repo_root)?.unwrap_or_default();
    let mut entries: BTreeMap<PathBuf, FileEntry> = BTreeMap::new();
    let mut stats = BuildStats::default();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();

    for file in files {
        if !seen.insert(file.path.clone()) {
            continue;
        }

        let path = repo_root.join(&file.path);
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let hash = content_hash(&bytes);
        let cached = previous.files.get(&file.path);

        let entry = match cached {
            Some(entry) if entry.hash == hash && entry.language == file.language => {
                stats.reused += 1;
                entry.clone()
            }
            Some(_) => {
                stats.parsed += 1;
                stats.changed += 1;
                parse_entry(&file.path, file.language, hash, &bytes)
            }
            None => {
                stats.parsed += 1;
                stats.added += 1;
                parse_entry(&file.path, file.language, hash, &bytes)
            }
        };

        entries.insert(file.path.clone(), entry);
    }

    stats.total = entries.len();
    stats.removed = previous
        .files
        .keys()
        .filter(|path| !entries.contains_key(*path))
        .count();

    let mut tags: Vec<Tag> = entries
        .values()
        .flat_map(|entry| entry.tags.iter().cloned())
        .collect();
    tags.sort();
    tags.dedup();

    let graph = Graph::build(&tags);
    let index = Index {
        files: entries,
        file_edges: graph.file_edges.clone(),
        symbol_edges: graph.symbol_edges.clone(),
    };

    ensure_gitignore(repo_root)?;
    save(repo_root, &index)?;

    Ok(IndexBuild {
        index,
        tags,
        graph,
        stats,
    })
}

/// Alias for [`build`].
pub fn build_index(repo_root: impl AsRef<Path>, files: &[SourceFile]) -> io::Result<IndexBuild> {
    build(repo_root, files)
}

/// Load the persisted index for `repo_root`.
///
/// A missing or corrupt index is treated as absent so a bad cache never blocks
/// a rebuild.
pub fn load(repo_root: impl AsRef<Path>) -> io::Result<Option<Index>> {
    let path = index_path(repo_root);
    match fs::read(&path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Persist `index` under `repo_root`, creating the index directory if needed.
pub fn save(repo_root: impl AsRef<Path>, index: &Index) -> io::Result<()> {
    let directory = index_dir(repo_root);
    fs::create_dir_all(&directory)?;

    let path = directory.join(INDEX_FILE);
    let temporary = directory.join(format!("{INDEX_FILE}.tmp"));
    let bytes = serde_json::to_vec_pretty(index).map_err(io::Error::other)?;
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, &path)
}

/// Ensure `.laudit/.gitignore` exists and ignores the index directory.
///
/// Existing content is preserved; `index/` is appended only when it is not
/// already ignored.
pub fn ensure_gitignore(repo_root: impl AsRef<Path>) -> io::Result<()> {
    let directory = repo_root.as_ref().join(".laudit");
    fs::create_dir_all(&directory)?;

    let path = directory.join(".gitignore");
    let existing = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };

    if existing.lines().any(is_index_ignore) {
        return Ok(());
    }

    let mut contents = existing;
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str("index/\n");
    fs::write(&path, contents)
}

/// The repository-relative directory holding the index.
#[must_use]
pub fn index_dir(repo_root: impl AsRef<Path>) -> PathBuf {
    repo_root.as_ref().join(INDEX_DIR)
}

/// The repository-relative path of the persisted index file.
#[must_use]
pub fn index_path(repo_root: impl AsRef<Path>) -> PathBuf {
    index_dir(repo_root).join(INDEX_FILE)
}

fn parse_entry(path: &Path, language: Language, hash: String, bytes: &[u8]) -> FileEntry {
    let source = String::from_utf8_lossy(bytes);
    FileEntry {
        path: path.to_path_buf(),
        hash,
        language,
        tags: parse_source(path, language, &source),
    }
}

fn is_index_ignore(line: &str) -> bool {
    matches!(
        line.trim(),
        "index/" | "/index/" | ".laudit/index/" | "/.laudit/index/"
    )
}

/// A stable 128-bit FNV-1a content hash.
///
/// The index is a cache, so a fast, deterministic hash is sufficient. The
/// 128-bit width makes accidental collisions negligible.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> String {
    const OFFSET_BASIS: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

    let mut hash = OFFSET_BASIS;
    for &byte in bytes {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("fnv1a128:{hash:032x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("laudit-index-test-{}-{id}", std::process::id()));
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

    fn source_file(path: &str, language: Language) -> SourceFile {
        SourceFile {
            path: PathBuf::from(path),
            language,
            size: 0,
        }
    }

    #[test]
    fn content_hash_is_stable_and_sensitive_to_content() {
        assert_eq!(content_hash(b"hello"), content_hash(b"hello"));
        assert_ne!(content_hash(b"hello"), content_hash(b"hello!"));
        assert!(content_hash(b"hello").starts_with("fnv1a128:"));
    }

    #[test]
    fn gitignore_is_created_and_idempotent() {
        let temp = TempDir::new();
        ensure_gitignore(temp.path()).expect("create gitignore");
        let path = temp.path().join(GITIGNORE_FILE);
        assert_eq!(fs::read_to_string(&path).expect("read"), "index/\n");

        ensure_gitignore(temp.path()).expect("update gitignore");
        assert_eq!(fs::read_to_string(&path).expect("read"), "index/\n");
    }

    #[test]
    fn gitignore_preserves_existing_entries() {
        let temp = TempDir::new();
        fs::create_dir_all(temp.path().join(".laudit")).expect("create .laudit");
        fs::write(temp.path().join(GITIGNORE_FILE), "model.json").expect("write gitignore");

        ensure_gitignore(temp.path()).expect("update gitignore");
        assert_eq!(
            fs::read_to_string(temp.path().join(GITIGNORE_FILE)).expect("read"),
            "model.json\nindex/\n"
        );
    }

    #[test]
    fn corrupt_index_is_treated_as_absent() {
        let temp = TempDir::new();
        fs::create_dir_all(index_dir(temp.path())).expect("create index dir");
        fs::write(index_path(temp.path()), b"not json").expect("write corrupt index");

        assert_eq!(load(temp.path()).expect("load"), None);
    }

    #[test]
    fn build_reuses_unchanged_files_and_drops_deleted_ones() {
        let temp = TempDir::new();
        fs::write(temp.path().join("a.rs"), "pub fn a() {}\n").expect("write a");
        fs::write(temp.path().join("b.rs"), "pub fn b() {}\n").expect("write b");
        let files = vec![
            source_file("a.rs", Language::Rust),
            source_file("b.rs", Language::Rust),
        ];

        let first = build(temp.path(), &files).expect("first build");
        assert_eq!(first.stats.parsed, 2);
        assert_eq!(first.stats.reused, 0);
        assert_eq!(first.stats.added, 2);
        assert_eq!(first.stats.removed, 0);

        let second = build(temp.path(), &files).expect("second build");
        assert_eq!(second.stats.parsed, 0);
        assert_eq!(second.stats.reused, 2);
        assert_eq!(second.stats.removed, 0);

        fs::write(temp.path().join("a.rs"), "pub fn a() {}\npub fn c() {}\n").expect("edit a");
        let third = build(temp.path(), &files).expect("third build");
        assert_eq!(third.stats.parsed, 1);
        assert_eq!(third.stats.changed, 1);
        assert_eq!(third.stats.reused, 1);

        fs::remove_file(temp.path().join("b.rs")).expect("delete b");
        let remaining = vec![source_file("a.rs", Language::Rust)];
        let fourth = build(temp.path(), &remaining).expect("fourth build");
        assert_eq!(fourth.stats.parsed, 0);
        assert_eq!(fourth.stats.reused, 1);
        assert_eq!(fourth.stats.removed, 1);
        assert!(!fourth.index.files.contains_key(Path::new("b.rs")));
    }
}

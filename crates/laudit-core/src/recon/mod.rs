//! Deterministic Stage 0 recon.
//!
//! Recon turns a repository into a structural skeleton without calling an LLM
//! (D4). The first step is [`walk`], which enumerates candidate source files;
//! [`parse`] then extracts definitions and references from each file; [`graph`]
//! turns those tags into a ranked file/symbol graph with communities;
//! [`entrypoints`] identifies where a tour should start; and [`git`] mines
//! provenance signals that explain why code exists.

pub mod entrypoints;
pub mod git;
pub mod graph;
pub mod parse;
pub mod walk;

pub use entrypoints::{detect as detect_entrypoints, Entrypoint, EntrypointKind};
pub use git::{
    mine as mine_provenance, mine_file, mine_files, mine_region, BlameSummary, Churn,
    CommitSummary, LineRange, Provenance,
};
pub use graph::{Community, FileEdge, FileNode, Graph, SymbolEdge, SymbolNode};
pub use parse::{parse_file, parse_files, parse_source, Tag, TagKind};
pub use walk::{Language, SourceFile, WalkError, WalkOptions};

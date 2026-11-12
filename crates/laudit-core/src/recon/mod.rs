//! Deterministic Stage 0 recon.
//!
//! Recon turns a repository into a structural skeleton without calling an LLM
//! (D4). The first step is [`walk`], which enumerates candidate source files;
//! [`parse`] then extracts definitions and references from each file.

pub mod parse;
pub mod walk;

pub use parse::{parse_file, parse_files, parse_source, Tag, TagKind};
pub use walk::{Language, SourceFile, WalkError, WalkOptions};

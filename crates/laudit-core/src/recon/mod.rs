//! Deterministic Stage 0 recon.
//!
//! Recon turns a repository into a structural skeleton without calling an LLM
//! (D4). The first step is [`walk`], which enumerates candidate source files.

pub mod walk;

pub use walk::{Language, SourceFile, WalkError, WalkOptions};

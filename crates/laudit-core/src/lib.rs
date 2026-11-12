//! Editor-agnostic engine for `laudit` guided codebase tours.

pub mod build;
pub mod cache;
pub mod comprehension;
pub mod config;
pub mod llm;
pub mod model;
pub mod narration;
pub mod plan;
pub mod prompts;
pub mod recon;
pub mod redact;
pub mod rpc;
pub mod status;

/// The crate version, stamped into artifacts as `tool_version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns the engine version.
#[must_use]
pub fn version() -> &'static str {
    VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_crate_metadata() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert!(!version().is_empty());
    }
}

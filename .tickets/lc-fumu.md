---
id: lc-fumu
status: open
deps: []
links: []
created: 2026-11-12T08:57:02Z
type: epic
priority: 1
tags: [foundation]
---
# Foundation: workspace, schemas, config, CLI, RPC skeleton

Stand up the Cargo workspace and the editor-agnostic engine skeleton so every later ticket has a home. This covers the crate layout, artifact schemas, config loading, CLI subcommand stubs, and the JSON-RPC stdio server skeleton.

## Design

Create a Cargo workspace at repo root with crates/laudit-core (lib) and crates/laudit-cli (bin). Add rustfmt.toml and clippy config. Define serde schemas for artifacts. Implement config loading and CLI stubs. Implement a tokio-based JSON-RPC 2.0 stdio server skeleton.

## Acceptance Criteria

cargo build succeeds; cargo test passes; laudit --help lists subcommands; laudit serve responds to a ping over stdio.


## References

See `docs/architecture.md` (workspace layout, invariants) and `docs/decisions.md` D5, D6, D16, D17.

---
id: lc-ycp2
status: closed
deps: []
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-fumu
tags: [foundation]
---
# Scaffold Cargo workspace (laudit-core + laudit-cli)

Create the Rust workspace that hosts the engine. This is the first ticket; everything else builds on it.

## Design

At repo root create Cargo.toml [workspace] with members crates/laudit-core and crates/laudit-cli. laudit-core is a lib crate; laudit-cli is a bin named laudit depending on laudit-core. Add rustfmt.toml (edition 2021, max_width 100) and a clippy lint baseline. Add a trivial unit test in laudit-core and a main that prints version. Do not add business logic yet.

## Acceptance Criteria

cargo build --workspace succeeds; cargo test --workspace passes; cargo run -p laudit-cli -- --version prints a version; cargo fmt --check and cargo clippy -- -D warnings pass.


## Notes

**2026-11-12T09:02:40Z**

Workspace will gain two more members: laudit-tui (added by lc-9hxm) and laudit-mcp (added by lc-4gp9). Keep the workspace members list extensible.

## References

See `docs/architecture.md` (workspace layout, invariants) and `docs/decisions.md` D5, D6.

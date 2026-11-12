---
id: lc-32ar
status: open
deps: []
links: []
created: 2026-11-12T09:02:36Z
type: epic
priority: 1
tags: [cli, frontend, export, tui]
---
# Terminal & export frontends (editor-free usage)

Make laudit fully usable without Neovim: a terminal tour, shareable exports, and non-interactive/CI access, all over the existing engine. The engine is already editor-agnostic; these are additional frontends and CLI subcommands that reuse laudit-core.

## Design

New frontends live in laudit-cli and two new workspace crates (laudit-tui, laudit-mcp). All reuse the recon/comprehension/plan/narration pipeline and the .laudit/ artifacts. No changes to the engine's core contracts.

## Acceptance Criteria

A user can tour, export, and query a repo from a plain terminal or CI with no editor installed.


## References

See `docs/frontends.md` and `docs/decisions.md` D23.

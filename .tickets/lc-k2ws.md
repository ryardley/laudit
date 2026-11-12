---
id: lc-k2ws
status: open
deps: []
links: []
created: 2026-11-12T08:57:02Z
type: epic
priority: 1
tags: [recon]
---
# Recon: deterministic codebase graph and index

Build the deterministic Stage 0 recon pipeline: walk the repo, parse with tree-sitter, extract defs/refs, build the graph, rank with PageRank, detect communities and entrypoints, mine git provenance, and persist an incremental index. This is the structural spine the LLM later narrates.

## Design

All work lives in laudit-core::recon. No LLM calls. Outputs feed the offline build path and the comprehension stage.

## Acceptance Criteria

laudit build --offline on a fixture repo emits .laudit/index/ and a skeleton artifact deterministically; re-running after a one-file edit only re-parses that file.


## References

See `docs/architecture.md` (Stage 0 recon) and `docs/decisions.md` D4, D7, D19.

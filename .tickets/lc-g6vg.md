---
id: lc-g6vg
status: open
deps: [lc-57oh, lc-s7w8, lc-ftdp, lc-k97t]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-xlp6
tags: [llm, comprehension]
---
# Stage 1 comprehension: codebase model

Produce a whole-codebase understanding that grounds every later explanation.

## Design

In laudit-core::comprehension feed the recon skeleton + docs + entrypoints to the LLM and emit CodebaseModel{subsystems, chapters, flows, glossary}. Constrain output to engine-provided symbol IDs. If oversized, do hierarchical summarization (per-subsystem -> synthesize). Redact before send. Persist codebase-model.json. Cache keyed by HEAD+dirty hash.

## Acceptance Criteria

Mock-LLM run emits a valid codebase-model.json; oversized fixture triggers hierarchical path; unknown IDs are dropped; cache invalidates on HEAD change.


## References

See `docs/architecture.md` (Stage 1) and `docs/decisions.md` D12, D13.

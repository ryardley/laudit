---
id: lc-57oh
status: closed
deps: [lc-berl, lc-hl1z, lc-u1cz, lc-lu0l]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-k2ws
tags: [recon, cli]
---
# Offline build: deterministic artifacts without LLM

Make laudit build --offline produce useful structural artifacts with no network, proving recon works end to end.

## Design

Wire recon (walk->parse->graph->entrypoints->provenance->index) into laudit build --offline. Emit a skeleton artifact (files, symbols, communities, entrypoints, ranked order) and a placeholder codebase-model.json with empty prose fields. No LLM calls. Print a summary.

## Acceptance Criteria

laudit build --offline on a fixture repo writes .laudit/index/ and a valid skeleton artifact; runs with no network; exit code 0.


## References

See `docs/architecture.md` (Stage 0) and `docs/decisions.md` D4, D20.

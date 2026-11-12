---
id: lc-berl
status: open
deps: [lc-fylw]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-k2ws
tags: [recon, graph]
---
# Symbol/file graph, PageRank, and communities

Turn defs/refs into a ranked graph that orders the tour by importance and groups subsystems.

## Design

In laudit-core::recon::graph build a directed file graph (referencer->definer, weighted by reference count) and a symbol-level view. Run PageRank (petgraph or hand-rolled power iteration) for centrality. Run Louvain community detection to produce subsystems. Expose ranked files/symbols and community membership. Unit-test on a small fixture with a known hub.

## Acceptance Criteria

Fixture graph ranks the expected hub highest; communities group related files; results are deterministic across runs.


## References

See `docs/architecture.md` (Stage 0) and `docs/decisions.md` D4.

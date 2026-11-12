---
id: lc-lz7a
status: open
deps: [lc-1yrq, lc-s7w8, lc-k97t]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-nh9h
tags: [llm, narration]
---
# Stage 3 narration with streamed front-matter

Generate the per-stop explanation that the UI streams.

## Design

In laudit-core::narration, for each stop build context = global codebase model + chapter context + local post-redaction bundle (code region, callers/callees, imports, provenance, prose). Request front-matter (TITLE:/WHY:) then --- then markdown body; stream body chunks. Validate refs against provided IDs (drop+warn+log). On schema/failure: one retry then skip+warn, mark stop status. Persist explanation into tour.json.

## Acceptance Criteria

Mock-LLM run streams chunks and fills tour.json; unknown refs dropped with warning; a failing stop is skipped and marked; redaction applied to the prompt.


## References

See `docs/architecture.md` (Stage 3), `docs/config.md` (narration format), and `docs/decisions.md` D9, D10, D24.

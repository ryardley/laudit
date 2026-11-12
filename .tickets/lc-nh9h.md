---
id: lc-nh9h
status: open
deps: []
links: []
created: 2026-11-12T08:57:02Z
type: epic
priority: 1
tags: [narration]
---
# Narration: streamed per-stop explanations, cache, resume

Implement Stage 3 narration: per-stop explanations streamed to the frontend, with ref validation, failure handling, caching, and resume.

## Design

Front-matter (TITLE/WHY) + streamed markdown body. Validate refs against provided IDs (drop+warn+log). Skip+warn on per-stop failure. Persist tour.json position for resume; invalidate on file change.

## Acceptance Criteria

Mock-LLM run streams explanation chunks; a failing stop is skipped and marked; resume restores position after restart.


## References

See `docs/architecture.md` (Stage 3), `docs/protocol.md`, and `docs/decisions.md` D9, D10, D15, D24, D25.

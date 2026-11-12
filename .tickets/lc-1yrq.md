---
id: lc-1yrq
status: closed
deps: [lc-g6vg]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-xlp6
tags: [llm, plan]
---
# Stage 2 plan: chapters and anchored stops

Turn the codebase model into an ordered, real-range tour plan.

## Design

In laudit-core::plan derive chapters (on-ramp -> subsystems by dependency direction -> infra -> config; tests opt-in) and stops anchored to real symbol IDs/ranges from recon. Default skips tests; config included but skippable. Emit tour.json with stops (no prose yet). Validate every stop range exists. Respect budgets (max stops/chapter).

## Acceptance Criteria

Mock-LLM run emits tour.json with valid file:line ranges; tests excluded by default and includable via flag; invalid ranges are rejected.


## References

See `docs/architecture.md` (Stage 2) and `docs/decisions.md` D22, D24.

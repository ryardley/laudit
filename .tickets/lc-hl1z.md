---
id: lc-hl1z
status: open
deps: [lc-berl]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-k2ws
tags: [recon]
---
# Entrypoint detection

Identify likely entrypoints so the tour can start at the on-ramp.

## Design

In laudit-core::recon::entrypoints detect: Rust main.rs / [[bin]] / src/main.rs; TS/JS package.json main/bin and index.ts; shebang scripts; HTTP route registrations; arg-parser usage. Return ranked entrypoint candidates with reasons. Unit-test against fixtures.

## Acceptance Criteria

Fixtures detect the expected entrypoints with reasons; no false positives on library-only fixtures.


## References

See `docs/architecture.md` (Stage 0) and `docs/decisions.md` D4, D22.

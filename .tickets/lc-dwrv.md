---
id: lc-dwrv
status: open
deps: [lc-57oh, lc-lz7a]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-yiw2
tags: [testing]
---
# Fixture repos, mock LLM server, golden artifact tests

Provide deterministic integration coverage of the engine.

## Design

Add tests/fixtures repos (small Rust and TS projects, plus a git fixture). Add a mock LLM HTTP server (wiremock/httpmock) returning canned structured responses. Add insta golden tests for codebase-model.json and tour.json. Cover redaction-before-send.

## Acceptance Criteria

cargo test runs offline; golden snapshots stable; mock server asserts redaction; fixtures cover Rust and TS.


## References

See `docs/testing.md` and `docs/decisions.md` D21.

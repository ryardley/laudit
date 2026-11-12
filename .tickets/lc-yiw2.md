---
id: lc-yiw2
status: open
deps: []
links: []
created: 2026-11-12T08:57:02Z
type: epic
priority: 2
tags: [ci, docs]
---
# Quality: tests, CI, Linux release, docs

Hardening and delivery: fixture repos, mock LLM server, golden artifact tests, CI, Linux release binaries, and documentation.

## Design

Add tests/fixtures repos, a mock LLM HTTP server, insta golden tests, CI workflow (fmt/clippy/test/nvim headless/cargo-deny), a Linux release workflow, build.lua fallback, and docs.

## Acceptance Criteria

CI is green on a clean checkout; a tagged release produces Linux x86_64/aarch64 binaries with checksums; README documents install and config.


## References

See `docs/testing.md`, `docs/release.md`, and `docs/decisions.md` D8, D21.

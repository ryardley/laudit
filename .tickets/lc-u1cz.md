---
id: lc-u1cz
status: closed
deps: [lc-fylw]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-k2ws
tags: [recon, index]
---
# Content-hash incremental index

Persist parse results so re-runs only re-parse changed files.

## Design

In laudit-core::recon::index store per-file content hash + extracted tags + graph edges under .laudit/index/. On build, hash files, reuse unchanged entries, re-parse only changed/added, drop deleted. Use tree-sitter incremental parsing where practical. Add a .laudit/.gitignore ignoring index/.

## Acceptance Criteria

Second build after a one-file edit re-parses only that file (assert via counters); deleted files are removed; index/ is git-ignored.


## References

See `docs/architecture.md` (Stage 0) and `docs/decisions.md` D19.

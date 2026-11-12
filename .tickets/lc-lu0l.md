---
id: lc-lu0l
status: open
deps: [lc-ntlg]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-k2ws
tags: [recon, git]
---
# Git provenance mining

Collect provenance signals that explain why code exists.

## Design

In laudit-core::recon::git use git2 or the git CLI to gather per-region blame summary (age, author count), recent commit subjects touching a file/region, and churn. Handle non-git dirs gracefully. Expose a bounded Provenance struct per file/region.

## Acceptance Criteria

Fixture git repo yields blame age and recent subjects; non-git dir returns empty provenance without error; output is bounded in size.


## References

See `docs/architecture.md` (Stage 0) and `docs/decisions.md` D9.

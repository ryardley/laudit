---
id: lc-ntlg
status: open
deps: [lc-ycp2]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-k2ws
tags: [recon]
---
# Repo walker with ignore/exclude rules

Enumerate candidate source files deterministically, honoring ignore rules and skipping non-source content.

## Design

In laudit-core::recon::walk use the ignore crate to respect .gitignore plus config excludes. Skip binary files, vendored dirs (node_modules, target, vendor, dist), and files over a size cap. Return a stable sorted list of paths with detected language by extension. Unit-test with a fixture tree.

## Acceptance Criteria

Fixture walk returns expected files; ignored/vendored/binary/huge files excluded; output order is deterministic.


## References

See `docs/architecture.md` (Stage 0) and `docs/decisions.md` D4, D7.

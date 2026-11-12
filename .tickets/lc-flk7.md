---
id: lc-flk7
status: open
deps: [lc-n5h7]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-yiw2
tags: [release]
---
# Linux release workflow and build.lua fallback

Ship installable binaries and a source fallback.

## Design

Add a release workflow (cargo-dist or cross) producing Linux x86_64/aarch64 (gnu+musl) binaries with checksums on tag. Add build.lua that runs cargo build --release as a fallback. Document binary resolution and optional auto-download.

## Acceptance Criteria

A tagged release produces the four Linux artifacts with checksums; build.lua builds locally; docs explain install.


## References

See `docs/release.md` and `docs/decisions.md` D8.

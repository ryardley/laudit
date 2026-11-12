---
id: lc-q4hn
status: closed
deps: [lc-berl, lc-1yrq]
links: []
created: 2026-11-12T09:02:36Z
type: task
priority: 2
parent: lc-k2ws
tags: [recon, git, adapters]
---
# Source adapters: uncommitted and commit range

Enable change-based tours (uncommitted changes and commit ranges), which also unblocks CI PR tours. This is the deferred adapter work from the original design.

## Design

In laudit-core::source implement adapters that produce ordered TourStops from (a) git diff HEAD plus untracked files, and (b) a base..head commit range. Default to the net diff; add --per-commit to expand per commit. Map hunks to symbol IDs where possible, falling back to line ranges. Wire 'laudit tour --uncommitted' and 'laudit tour --range <base..head>'.

## Acceptance Criteria

On a fixture repo, uncommitted and range modes produce valid stops; net-diff is the default; --per-commit expands correctly; hunks map to symbols when possible.


## References

See `docs/architecture.md` (source adapters) and `docs/decisions.md` D2, D3.

## Additional context

Uncommitted adapter: `git diff HEAD` for tracked changes plus `git ls-files --others --exclude-standard` for untracked files. Range adapter: `git diff <base>..<head>` (net diff) by default; `--per-commit` walks each commit and emits a chapter per commit. Map each hunk to the enclosing symbol from the recon index when the hunk falls inside a symbol range; otherwise emit a line-range stop. Preserve file order and hunk order for determinism.

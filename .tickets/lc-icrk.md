---
id: lc-icrk
status: open
deps: [lc-57oh]
links: []
created: 2026-11-12T09:02:36Z
type: task
priority: 2
parent: lc-32ar
tags: [ci, cli]
---
# Offline --check build for CI

Use recon as a fast structural build/verification step in CI without any model calls.

## Design

Add 'laudit build --offline --check --quiet'. --check exits non-zero if artifacts are stale or regeneration would differ from what is on disk; --quiet suppresses progress. Reuse the existing offline build path. No network.

## Acceptance Criteria

CI-ready exit codes (0 fresh, non-zero stale); runs with no network; --quiet produces no progress output.


## References

See `docs/frontends.md` (CLI) and `docs/decisions.md` D20, D23.

## Additional context

"Stale" means: regenerating the offline artifacts from the current working tree would produce different bytes than what is on disk (compare content hashes of the generated artifacts). `--check` must not write artifacts. Exit 0 when fresh, non-zero when stale or missing. `--quiet` suppresses all non-error output.

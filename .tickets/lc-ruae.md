---
id: lc-ruae
status: open
deps: [lc-q4hn, lc-taeh]
links: []
created: 2026-11-12T09:02:36Z
type: task
priority: 3
parent: lc-yiw2
tags: [ci]
---
# PR change tour in CI

Generate a tour of a pull request's changes for review, using the change-based adapters.

## Design

Add a CI-friendly path: 'laudit tour --range origin/main..HEAD --print' and '--md' to produce a change tour suitable for a PR comment or artifact. Document a GitHub Actions example. Depends on the source adapters and markdown export.

## Acceptance Criteria

A CI job on a fixture PR produces a change tour in markdown; output is stable and redacted; documented workflow works.


## References

See `docs/frontends.md` and `docs/decisions.md` D23.

## Additional context

Example workflow step: `laudit tour --range origin/${{ github.base_ref }}..HEAD --md --out tour.md`, then post `tour.md` as a PR comment (or upload as an artifact). Must work with `--offline` for structure-only when no API key is configured. Document the required secret (`LAUDIT_API_KEY`) and the fallback behavior.

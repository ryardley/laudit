---
id: lc-taeh
status: open
deps: [lc-1yrq, lc-lz7a]
links: []
created: 2026-11-12T09:02:36Z
type: task
priority: 1
parent: lc-32ar
tags: [export]
---
# laudit export --md

Render a tour as a shareable markdown document so a walkthrough becomes a deliverable (README, PR, Notion).

## Design

Add laudit-core::export with a markdown renderer. Read tour.json + model.json and emit: document title, chapter headings, per-stop heading with file:line, a fenced code block of the stop range, why_here and explanation, and refs as links. Support --out <path> or stdout. Must work from artifacts alone (no LLM) so it can render offline skeletons too.

## Acceptance Criteria

Golden test on a fixture tour produces the expected markdown; code fences contain the correct ranges; rendering works with no network.


## References

See `docs/frontends.md` (Exports) and `docs/artifacts.md`.

## Additional context

Markdown structure: `# <tour title>`, then `## <chapter title>`, then `### <stop title> — <file>:<start>-<end>`, a fenced code block with the language from the file extension, then `**Why here:** <why_here>`, then the explanation markdown, then a `Refs:` line linking each ref as `<symbol> (<file>:<line>)`. Deterministic ordering; no timestamps in the body so golden tests are stable.

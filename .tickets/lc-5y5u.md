---
id: lc-5y5u
status: open
deps: [lc-n5h7, lc-lz7a, lc-1yrq]
links: []
created: 2026-11-12T09:02:36Z
type: task
priority: 1
parent: lc-32ar
tags: [cli, frontend]
---
# laudit explain <symbol> and tour --print

Provide minimal non-interactive access to the tour and narration without an editor. This is the cheapest editor-free win and validates the engine end to end from a shell.

## Design

Extend laudit-cli. 'laudit explain <symbol>' resolves the symbol in the index, finds/creates its stop, runs narration, and prints title, why_here, and body to stdout. 'laudit tour --print' prints all chapters and stops (chapter headings, file:line, prose) to stdout, pipe-friendly, with --color auto|always|never. Reuse laudit-core narration; no RPC. Respect --offline (print structural skeleton only).

## Acceptance Criteria

'laudit explain <sym>' prints a grounded explanation; 'laudit tour --print | less' works; exit codes are correct; --offline prints without network.


## References

See `docs/frontends.md` (CLI) and `docs/decisions.md` D23.

## Additional context

Symbol resolution: accept a bare name, `file:symbol`, or `file:line`. Resolve against the recon index; if ambiguous, list candidates and exit non-zero. `tour --print` must not require a TTY and must emit plain text when `--color never` or when stdout is not a TTY. `--offline` prints the structural skeleton (titles + ranges) with no prose.

---
id: lc-n5h7
status: closed
deps: [lc-ycp2]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-fumu
tags: [cli]
---
# CLI subcommand skeleton

Provide the CLI surface so the engine is usable standalone and testable without an editor.

## Design

In laudit-cli implement subcommands: build, serve, explain, outline, status, export, prompts, clear. Wire argument parsing (clap). build supports --offline, --repo <path>, --json. serve starts the RPC server. Stubs may return not-implemented but must parse args and exit non-zero with a clear message. --version and --help work.

## Acceptance Criteria

laudit --help lists all subcommands; each subcommand parses its documented flags; unknown flags error; --version prints version.


## Notes

**2026-11-12T09:02:40Z**

Extend the CLI surface with editor-free frontends: subcommands explain, tour, export, ask, mcp; flags --print, --md, --html, --offline, --check, --range, --uncommitted. See epic lc-32ar (Terminal & export frontends).

## References

See `docs/frontends.md` (CLI) and `docs/decisions.md` D23.

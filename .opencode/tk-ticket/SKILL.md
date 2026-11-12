---
name: tk-tickets
description: Work with the tk git-native ticket system (tickets in .tickets/). Use when the user asks to create, list, start, close, reopen, or manage tickets/tasks/epics, add dependencies, or check what is ready/blocked — or mentions tk, tickets, ticket IDs (e.g. pw-xxxx), backlog, or .tickets.
---

# Working with tk tickets

This project tracks work with **tk** (github.com/wedow/ticket): a git-native
ticket system. Tickets are markdown files with YAML frontmatter in `.tickets/`
at the repo root. No daemon, no database — the files are the source of truth
and should be committed.

## Finding tk

`tk` is installed via devenv, so it is on `PATH` inside the devenv shell
(`devenv shell` or direnv). If `tk` is not on `PATH`, run it through devenv:

```sh
devenv shell -c "tk <command>"
```

or locate the binary in the nix store:

```sh
find /nix/store -path '*tk*/bin/tk' -type f | head -1
```

## Ticket file format

Each ticket is `.tickets/<id>.md`. The YAML frontmatter is the machine state;
the body is free-form markdown.

```markdown
---
id: pw-4hz9
status: open            # open | in_progress | closed
deps: []                # ids this ticket depends on
links: []               # symmetric links
created: 2026-11-06T15:44:51Z
type: task              # bug | feature | task | epic | chore
priority: 1             # 0-4, 0 = highest
parent: pw-mn26         # optional: parent epic/ticket
tags: [scanner]         # optional
---
# Title

Description (what and why).

## Design

How to do it: files, functions, approach.

## Acceptance Criteria

Verifiable checks that define done.

## References

Links to specs/docs/code.
```

IDs are `<prefix>-<suffix>` (e.g. `pw-4hz9`); the prefix comes from the repo.
Partial IDs work everywhere (`tk show 4hz` matches `pw-4hz9`).

## Commands

| Command | Purpose |
|---|---|
| `tk create "<title>" [options]` | Create a ticket, prints the ID. Options: `-d/--description`, `--design`, `--acceptance`, `-t/--type` (bug\|feature\|task\|epic\|chore), `-p/--priority` (0-4), `-a/--assignee`, `--parent <id>`, `--tags a,b`, `--external-ref` |
| `tk start <id>` | Set status to `in_progress` |
| `tk close <id>` | Set status to `closed` |
| `tk reopen <id>` | Set status to `open` |
| `tk status <id> <status>` | Set status explicitly |
| `tk dep <id> <dep-id>` | Add dependency: `<id>` depends on `<dep-id>` |
| `tk undep <id> <dep-id>` | Remove a dependency |
| `tk dep tree [--full] <id>` | Show the dependency tree |
| `tk dep cycle` | Find dependency cycles in open tickets |
| `tk link <id> <id> [...]` | Symmetric link |
| `tk unlink <id> <target>` | Remove a link |
| `tk ready` | Open/in-progress tickets with deps resolved (do these next) |
| `tk blocked` | Open/in-progress tickets with unresolved deps |
| `tk closed [--limit=N]` | Recently closed tickets |
| `tk show <id>` | Display a ticket (includes children for epics) |
| `tk add-note <id> [text]` | Append a timestamped note (or pipe via stdin) |
| `tk help` | Full command reference |

`ls`/`list`/`query` are extras plugins and may not be bundled; use `ready`,
`blocked`, `closed`, `show`, or read `.tickets/` directly.

## Workflow

1. **Idea occurs** -> `tk create "<title>" -d "<context>"` (optionally
   `--parent <epic>` to attach to a phase, `--tags`, `-p` priority).
2. **About to start it** -> it appears in `tk ready` once its dependencies are
   resolved.
3. **Doing it** -> `tk start <id>`.
4. **Finished** -> `tk close <id>`; if it surprised you, drop a bullet in
   `docs/notes.md`.

To add a phase: create an epic (`-t epic`), create its tickets with
`--parent <epic>`, and wire up `tk dep` edges. `tk ready` then lists the
entry points.

## Adding context to tickets

Tickets should carry enough context to be actionable without re-deriving the
design. When creating or enriching a ticket, include:

- **Description** — what and why, with background.
- **Design** — how: files, functions, exact approach, wire formats.
- **Acceptance Criteria** — concrete, verifiable checks.
- **References** — the relevant spec/docs/code paths (e.g.
  `docs/app/spec.md`, `scanner/src/decrypt.ts:213`).

You can edit the `.tickets/<id>.md` file directly (preserve the frontmatter
exactly) or use `tk add-note <id>` for timestamped notes.

## Tips

- Always run `tk` from the repo root (it searches parent dirs for `.tickets/`).
- `TICKETS_DIR` overrides the tickets directory if needed.
- Keep the frontmatter valid: `deps`/`links` are YAML arrays; `status` is one
  of `open|in_progress|closed`.
- Commit `.tickets/` with the rest of the work — it is git-native.


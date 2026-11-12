---
id: lc-pyh2
status: closed
deps: [lc-7xd3]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-ej4t
tags: [nvim, testing]
---
# Headless-nvim tests with fake engine

Make the plugin testable without network or a real model.

## Design

Add a fake engine script speaking the JSON-RPC protocol with canned responses/notifications. Add headless nvim tests (plenary or busted) covering setup, health, tour start, advance, return, resume, outline. Wire a make/test target.

## Acceptance Criteria

Tests run headless and pass; no network used; fake engine covers success and error paths.


## References

See `docs/testing.md` (headless Neovim) and `docs/decisions.md` D21.

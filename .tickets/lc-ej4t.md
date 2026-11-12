---
id: lc-ej4t
status: open
deps: []
links: []
created: 2026-11-12T08:57:02Z
type: epic
priority: 1
tags: [nvim]
---
# Neovim plugin: thin frontend over the engine

Build the Neovim plugin as a thin UI frontend that drives the laudit binary over JSON-RPC. Real buffers, persistent explanation split, extmark highlights, auto-follow, free exploration with return-to-stop, commands and keymaps.

## Design

Lua under lua/laudit/. setup{} holds only UI options. Binary resolution config->PATH->optional auto-download. jobstart + vim.json RPC client. Headless tests with a fake engine.

## Acceptance Criteria

Headless nvim can start a tour against a fake engine, advance stops, return to current stop, and resume.


## References

See `docs/frontends.md` (Neovim), `docs/protocol.md`, and `docs/decisions.md` D6, D14, D15.

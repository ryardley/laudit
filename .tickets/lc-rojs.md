---
id: lc-rojs
status: open
deps: [lc-n5h7]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-ej4t
tags: [nvim]
---
# Neovim plugin scaffold and binary resolution

Create the plugin skeleton and locate the engine binary.

## Design

Create lua/laudit/{init,config,health}.lua and plugin/laudit.lua. setup{} holds only UI options (split side/size, keymaps enable, binary path). Binary resolution: config path -> vim.fn.exepath('laudit') -> optional auto-download (opt-in). :Laudit health reports binary/version/engine status. No tour logic yet.

## Acceptance Criteria

require('laudit').setup{} works; :Laudit health reports found/missing binary; config path overrides PATH.


## References

See `docs/frontends.md` (Neovim) and `docs/decisions.md` D6, D8.

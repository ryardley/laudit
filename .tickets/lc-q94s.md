---
id: lc-q94s
status: open
deps: [lc-rojs, lc-1i5z]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-ej4t
tags: [nvim, rpc]
---
# Neovim JSON-RPC client over jobstart

Connect the plugin to laudit serve.

## Design

In lua/laudit/rpc.lua start the engine with jobstart using newline-delimited JSON, encode/decode with vim.json, correlate request ids, dispatch notifications to handlers, and handle process exit/restart. Provide request(method, params, cb) and on(notification, handler).

## Acceptance Criteria

Headless test starts a fake engine, sends ping, receives response; notifications dispatch; engine crash triggers bounded restart.


## References

See `docs/protocol.md` and `docs/decisions.md` D6, D15.

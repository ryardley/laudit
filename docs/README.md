# laudit design docs

Authoritative design for the `laudit` guided-codebase-tour tool. Tickets in
`.tickets/` reference these documents; where a ticket and a document disagree,
the document wins.

| Document | Contents |
|---|---|
| [architecture.md](architecture.md) | Engine/frontend split, workspace layout, pipeline stages, module map, firm invariants, non-goals. |
| [decisions.md](decisions.md) | Numbered decision log (D1–D25) with rationale. |
| [artifacts.md](artifacts.md) | `.laudit/` layout and JSON schemas (`model.json`, `tours/<id>.json`, `redactions.json`). |
| [protocol.md](protocol.md) | JSON-RPC 2.0 methods/notifications/errors and MCP tools. |
| [config.md](config.md) | Config files, key resolution, redaction, prompts. |
| [frontends.md](frontends.md) | CLI commands/flags, TUI layout/keys, Neovim commands/keymaps/UX, exports. |
| [testing.md](testing.md) | Test layers, fixtures, mock LLM, golden artifacts, headless Neovim, CI. |
| [release.md](release.md) | Linux targets, tooling, binary resolution, source fallback. |

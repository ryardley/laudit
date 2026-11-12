---
id: lc-4gp9
status: closed
deps: [lc-lz7a, lc-1yrq]
links: []
created: 2026-11-12T09:02:36Z
type: task
priority: 2
parent: lc-32ar
tags: [mcp, protocol]
---
# laudit mcp MCP server

Expose the engine to AI agents (Claude Code, Cursor, etc.) so they can consume the codebase model and tours.

## Design

Add a new workspace crate laudit-mcp implementing MCP over stdio: initialize handshake, capabilities, tools/list, tools/call. Expose tools build_tour, explain, outline, status. Reuse laudit-core handlers; apply redaction to any returned text. Keep the transport separate from the engine.

## Acceptance Criteria

An MCP client harness lists the tools and successfully calls explain; protocol handshake completes; no secrets in responses.


## References

See `docs/protocol.md` (MCP) and `docs/decisions.md` D23.

## Additional context

MCP transport is stdio with JSON-RPC 2.0 messages (MCP is JSON-RPC based). Implement the `initialize` handshake and advertise the `tools` capability. Tool input/output schemas are in `docs/protocol.md`. Reuse `laudit-core` handlers; do not duplicate engine logic. Redact all returned text. Keep `laudit-mcp` a separate crate so the engine has no MCP dependency.

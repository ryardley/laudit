---
id: lc-1i5z
status: closed
deps: [lc-ycp2]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-fumu
tags: [rpc]
---
# JSON-RPC 2.0 stdio server skeleton

Implement the editor-agnostic RPC transport that frontends use to drive the engine.

## Design

In laudit-core::rpc implement newline-delimited JSON-RPC 2.0 over stdin/stdout using tokio. Support request/response and server->client notifications. Register methods: ping, status. Include request id correlation, error objects, and a clean shutdown on EOF. laudit serve runs it. Keep method dispatch extensible for later build_tour/explain_stop/cancel.

## Acceptance Criteria

An integration test spawns laudit serve, sends ping, receives a valid JSON-RPC response; malformed input yields a JSON-RPC error; EOF shuts down cleanly.


## References

See `docs/protocol.md` (JSON-RPC framing, methods, error codes, lifecycle) and `docs/decisions.md` D6, D15.

## Additional context

Framing is newline-delimited JSON (one object per line). Logs MUST go to stderr; stdout is the protocol channel. Use the standard JSON-RPC error codes plus the application range documented in `docs/protocol.md`. The dispatcher must be table-driven so `build_tour`, `explain_stop`, `cancel`, and `get_outline` can be registered later without restructuring. Shut down cleanly on stdin EOF.

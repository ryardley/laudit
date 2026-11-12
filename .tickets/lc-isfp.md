---
id: lc-isfp
status: closed
deps: [lc-1i5z, lc-lz7a]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-nh9h
tags: [rpc, narration]
---
# RPC methods for tour lifecycle

Expose the tour pipeline to frontends over JSON-RPC.

## Design

Add methods build_tour, explain_stop, cancel, get_outline, status to laudit serve. Emit notifications progress, comprehension_done, stop_ready, explanation_chunk, error. Key work by tour_id/stop_id; support cancellation. Keep the server non-blocking (tokio).

## Acceptance Criteria

Integration test drives build_tour and receives progress + stop_ready; cancel stops work; explanation_chunk streams; errors are JSON-RPC errors.


## References

See `docs/protocol.md` and `docs/decisions.md` D15.

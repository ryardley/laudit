---
id: lc-0k4a
status: closed
deps: [lc-lz7a]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-nh9h
tags: [narration, cache]
---
# Tour cache, invalidation, and resume

Make tours resumable and cheap to re-run.

## Design

Persist tour.json position and per-stop status. On resume, restore position and skip already-explained stops. Invalidate affected stops when their file content hash changes; keep the rest. Store only redacted text. Add laudit status to report tour state.

## Acceptance Criteria

Resume after restart restores position; editing one stop's file invalidates only that stop; status reports counts.


## References

See `docs/artifacts.md` (tour.json) and `docs/decisions.md` D25.

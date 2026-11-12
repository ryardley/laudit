---
id: lc-y03c
status: open
deps: [lc-g6vg]
links: []
created: 2026-11-12T09:02:36Z
type: task
priority: 3
parent: lc-32ar
tags: [llm, cli]
---
# laudit ask <question>

Grounded natural-language Q&A over the codebase model, for terminal users who want to ask rather than tour.

## Design

Add laudit-core::ask: retrieve relevant context from model.json + index (symbol/file name match plus a BM25-style score), assemble a grounded post-redaction context, call the LLM, and stream an answer that cites file:line. Provide a REPL when no question argument is given. Reuse the narration prompt path where possible.

## Acceptance Criteria

Answers cite real file:line refs; unknown answers say so instead of inventing; redaction applied; REPL works.


## References

See `docs/frontends.md` (CLI) and `docs/decisions.md` D23.

## Additional context

Retrieval: score candidate symbols/files by exact name match, then substring, then a BM25-style term score over symbol names and doc comments. Take the top-N within the context budget. The answer must cite `file:line` for every claim and must say it does not know when evidence is insufficient. Apply redaction to the assembled context before the call.

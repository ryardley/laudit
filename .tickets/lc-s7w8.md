---
id: lc-s7w8
status: closed
deps: [lc-60sv]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-xlp6
tags: [llm]
---
# OpenAI-compatible LLM client

Provide the single provider client used by comprehension and narration.

## Design

In laudit-core::llm implement an OpenAI-compatible chat completions client (reqwest + tokio). Config: base_url, model (required), api_key from resolution order. Support structured output via response_format json_schema where available, else JSON-only + repair. Bounded exponential backoff on 429/5xx/timeouts. Support streaming responses. Never log keys; redact URLs in errors.

## Acceptance Criteria

Integration test against a mock server covers success, 429 retry, timeout, and streaming; missing model errors clearly; no key required for localhost.


## References

See `docs/config.md` (provider, key resolution) and `docs/decisions.md` D1, D17.

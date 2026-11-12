---
id: lc-bbh9
status: open
deps: []
links: []
created: 2026-11-12T08:57:02Z
type: epic
priority: 1
tags: [security]
---
# Redaction: always-on secret scrubbing

Implement the layered secret-redaction engine that runs in Rust on every payload leaving the process (LLM prompts, logs, cached artifacts). Secrets must never be sent, logged, or persisted.

## Design

Embed a gitleaks-derived ruleset, add key-name-aware redaction for env/toml/yaml/json, session-stable placeholders, and a user allowlist. Apply before send/cache/log.

## Acceptance Criteria

Unit tests cover AWS/GitHub/JWT/PEM/env-value cases; a payload with a secret is redacted before the mock LLM server receives it; logs contain no secret.


## References

See `docs/config.md` (Redaction) and `docs/decisions.md` D11.

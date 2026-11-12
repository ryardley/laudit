---
id: lc-k97t
status: closed
deps: [lc-ycp2]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-bbh9
tags: [security]
---
# Layered secret redaction engine

Guarantee secrets never leave the process via prompts, logs, or cached artifacts.

## Design

In laudit-core::redact embed a gitleaks-derived ruleset (regex crate) plus key-name-aware redaction for env/toml/yaml/json (keys matching *KEY|SECRET|TOKEN|PASSWORD|PASSWD|DSN|PRIVATE|*_URL). Replace matches with session-stable placeholders (e.g. «aws_key#1»). Support config extra_rules and allowlist. Provide redact(text)->(redacted, map). Apply before LLM send, before logging, and before caching. Never log keys or Authorization headers.

## Acceptance Criteria

Unit tests cover AWS/GitHub/JWT/PEM/env-value redaction; placeholders are stable within a session; allowlist un-redacts false positives; a mock-LLM integration test asserts the secret never reaches the server.


## References

See `docs/config.md` (Redaction) and `docs/decisions.md` D11.

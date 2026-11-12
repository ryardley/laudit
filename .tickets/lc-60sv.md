---
id: lc-60sv
status: open
deps: [lc-ycp2]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-fumu
tags: [config]
---
# Config loading and API key resolution

Load engine configuration from global and per-repo files and resolve credentials with a clear precedence.

## Design

In laudit-core::config load ~/.config/laudit/config.toml (global) and <repo>/.laudit/config.toml (repo). Fields: provider{base_url, model, api_key_env}, roots[], languages[], excludes[], redaction{extra_rules[], allowlist[]}, budgets{}. Key resolution order: env (api_key_env or LAUDIT_API_KEY) -> user config -> repo config; env wins. model is required; error clearly if missing. Support local endpoints with no key.

## Acceptance Criteria

Unit tests cover precedence and missing-model error; a repo config overrides global for roots/languages; no key required when base_url is localhost.


## References

See `docs/config.md` and `docs/decisions.md` D17.

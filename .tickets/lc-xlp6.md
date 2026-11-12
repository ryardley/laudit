---
id: lc-xlp6
status: open
deps: []
links: []
created: 2026-11-12T08:57:02Z
type: epic
priority: 1
tags: [llm]
---
# LLM: client, prompts, comprehension, plan

Implement the OpenAI-compatible LLM client, prompt management, Stage 1 comprehension (codebase model), and Stage 2 plan (chapters and anchored stops).

## Design

Client supports base_url/model/api_key_env, structured output, retry/backoff. Prompts embedded and installed to ~/.config/laudit/prompts on first run. Comprehension emits codebase-model.json; plan emits tour.json stops anchored to real symbol IDs.

## Acceptance Criteria

With a mock LLM, laudit build produces a valid codebase-model.json and tour.json; unknown symbol refs are dropped with a warning.


## References

See `docs/architecture.md` (Stages 1-2), `docs/config.md` (prompts), and `docs/decisions.md` D1, D12, D17, D18.

---
id: lc-ftdp
status: closed
deps: [lc-ycp2]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-xlp6
tags: [llm, prompts]
---
# Prompt management and first-run install

Ship versioned prompts that users can inspect and tune.

## Design

Embed default prompts in the binary (comprehension, narration, repair). On first run install them to ~/.config/laudit/prompts/ with a version stamp. Load from there at runtime. If a shipped prompt version is newer, warn and offer update (non-destructive). laudit prompts lists/prints prompts. Record prompt_version in artifacts.

## Acceptance Criteria

First run creates ~/.config/laudit/prompts/; edits there are used; laudit prompts prints them; prompt_version appears in artifacts; newer shipped version triggers a warning.


## References

See `docs/config.md` (Prompts) and `docs/decisions.md` D18.

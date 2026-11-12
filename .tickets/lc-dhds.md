---
id: lc-dhds
status: closed
deps: [lc-ycp2]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-fumu
tags: [schemas]
---
# Define artifact schemas (serde) with version headers

Define the stable, versioned JSON artifact types that are the durable interface between engine, frontends, and future tools.

## Design

In laudit-core::model define serde structs: CodebaseModel {schema_version, tool_version, repo_id, head, dirty_hash, subsystems[], chapters[], flows[], glossary[], generator, model, prompt_version, generated_at}; Tour {schema_version, tool_version, tour_id, chapters[]->stops[], position, redaction_profile, model, prompt_version}; Stop {id, file, start_line, end_line, symbol_id, title, why_here, explanation, status}; Redactions {schema_version, placeholders{}}. Use serde with deny_unknown_fields where sensible. Add round-trip tests.

## Acceptance Criteria

Structs serialize/deserialize to JSON matching the documented shape; round-trip tests pass; schema_version and tool_version present on every artifact.


## References

See `docs/artifacts.md` (schemas) and `docs/decisions.md` D16, D24.

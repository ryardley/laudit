---
id: lc-fylw
status: closed
deps: [lc-ntlg]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-k2ws
tags: [recon, grammar]
---
# Tree-sitter parsing and def/ref extraction

Parse supported languages and extract symbol definitions and references to build the graph vocabulary.

## Design

Bundle tree-sitter grammar crates for Rust, TypeScript, TSX, JavaScript, bash, toml, yaml, json, markdown, sql. In laudit-core::recon::parse implement per-language queries to extract definitions (fn/struct/class/const/function/method) and references (call sites, imports, type usages). Produce Tag{file,line,name,kind:def|ref}. Degrade gracefully for unsupported files. Add fixtures per language.

## Acceptance Criteria

Fixtures for each language yield expected defs/refs; unsupported files are skipped without error; parsing a syntactically broken file still yields a partial result.


## References

See `docs/architecture.md` (Stage 0) and `docs/decisions.md` D7.

# laudit architecture

`laudit` is a guided codebase tour: it walks a developer through a repository,
highlighting real code regions and explaining **what the code does** and **why it
is there**, grounded in a whole-codebase understanding.

This document is the authoritative architecture. Every ticket must conform to it.
If a ticket appears to contradict this document, the document wins and the ticket
should be corrected.

## Core principle: editor-agnostic engine, thin frontends

The Rust engine is the product. Neovim is *one* frontend. The engine must never
depend on Neovim, and no business logic may live in the Lua plugin.

```
                 +-------------------------------+
                 |        laudit-core (lib)      |
                 |  recon -> comprehension ->    |
                 |  plan -> narration            |
                 |  redaction, config, artifacts |
                 +---------------+---------------+
                                 |
              +------------------+-------------------+
              |                  |                   |
        laudit-cli          laudit-tui          laudit-mcp
        (bin: laudit)       (ratatui)           (MCP stdio)
              |                  |                   |
     +--------+--------+         |                   |
     |        |        |         |                   |
  terminal  JSON-RPC  exports  terminal          AI agents
  (print)   (serve)   (md/html)  tour
              |
        Neovim plugin (Lua, thin UI)
```

## Workspace layout

Cargo workspace at the repository root.

| Crate | Kind | Responsibility |
|---|---|---|
| `crates/laudit-core` | lib | All engine logic: recon, graph, redaction, config, artifacts, LLM client, comprehension, plan, narration, cache, explain, ask, print, export, status, RPC server. |
| `crates/laudit-cli` | bin `laudit` | CLI surface: `build`, `serve`, `explain`, `ask`, `tour`, `export`, `mcp`, `status`, `prompts` (plus reserved `outline`, `clear`). |
| `crates/laudit-tui` | bin/lib | Interactive terminal tour (ratatui + crossterm). Added by the TUI ticket. |
| `crates/laudit-mcp` | bin/lib | MCP stdio server exposing engine tools to AI agents. Added by the MCP ticket. |

The Neovim plugin lives in `lua/laudit/` + `plugin/laudit.lua` and is pure Lua.

## Pipeline

The tour is produced in four stages. Stages 0 and 2 are deterministic; stages 1
and 3 call the LLM.

### Stage 0 — Recon (deterministic, no LLM)

`laudit-core::recon`

1. **walk** — enumerate source files, honoring `.gitignore` + config excludes;
   skip binaries, vendored dirs, and oversized files.
2. **parse** — tree-sitter parse; extract definitions and references (`Tag`).
3. **graph** — build a directed file/symbol graph; PageRank centrality; Louvain
   communities (subsystems).
4. **entrypoints** — detect likely entrypoints with reasons.
5. **git** — mine provenance (blame age, recent commit subjects, churn).
6. **index** — persist a content-hash incremental index under `.laudit/index/`.

Output: a structural skeleton (files, symbols, communities, entrypoints, ranked
order, provenance), persisted as `.laudit/skeleton.json`. This is the spine the
LLM narrates. `laudit build --offline` writes the skeleton plus a placeholder
`.laudit/model.json` and never calls the LLM.

### Stage 1 — Comprehension (LLM, cached)

`laudit-core::comprehension`

Feed the skeleton + docs + entrypoints to the LLM; emit a **codebase model**
(subsystems, chapters, flows, glossary). Output is constrained to engine-provided
symbol IDs. Oversized repos use hierarchical summarization (per-subsystem →
synthesize). Persisted as `.laudit/model.json`, cached by `HEAD` + dirty hash.

### Stage 2 — Plan (deterministic)

`laudit-core::plan`

Turn the codebase model into ordered chapters and stops anchored to real symbol
IDs/ranges. Default order: **on-ramp → subsystems (by dependency direction) →
infra → config**. Tests are skipped by default (opt-in). Emits `.laudit/tours/<id>.json`
with stops but no prose yet.

### Stage 3 — Narration (LLM, lazy, streamed)

`laudit-core::narration`

For each stop, assemble context = global codebase model + chapter context + local
post-redaction bundle (code region, callers/callees, imports, provenance, prose).
Request front-matter (`TITLE:` / `WHY:`) then `---` then a markdown body, streamed.
Validate refs against provided IDs (drop + warn + log). On failure: one retry, then
skip + warn and mark the stop. Persist prose into the tour artifact.

## Module map (`laudit-core`)

| Module | Responsibility |
|---|---|
| `model` | Versioned artifact structs (serde). |
| `config` | Global + repo config loading, key resolution. |
| `redact` | Layered secret redaction. |
| `recon::walk` | File enumeration. |
| `recon::parse` | Tree-sitter parsing, def/ref extraction. |
| `recon::graph` | Graph, PageRank, communities. |
| `recon::entrypoints` | Entrypoint detection. |
| `recon::git` | Provenance mining. |
| `recon::index` | Content-hash incremental index. |
| `build` | Offline Stage 0 build and `--check` freshness verification. |
| `llm` | OpenAI-compatible client (structured output, streaming, retry). |
| `prompts` | Embedded prompts, first-run install, versioning. |
| `comprehension` | Stage 1. |
| `plan` | Stage 2. |
| `narration` | Stage 3. |
| `cache` | Per-stop content hashes, invalidation, resume position. |
| `explain` | Single-symbol grounded explanation. |
| `ask` | Grounded Q&A: deterministic retrieval, redacted context, streamed answer with `file:line` citations. |
| `print` | Pipe-friendly tour rendering. |
| `export` | Markdown and HTML rendering. |
| `status` | Read-only model/tour presence and stop counts. |
| `source` | Change-based source adapters (uncommitted, commit range). |
| `rpc` | JSON-RPC 2.0 stdio server. |

Grounded Q&A (`ask`) retrieves relevant symbols from the skeleton, model, and
index, assembles a redacted context, and streams an answer that cites
`file:line`. When retrieval finds no evidence it says so instead of guessing.

## Artifacts

All durable state lives under `.laudit/` and is versioned JSON stamped with
`schema_version` and `tool_version` (D16). The layout is:

```
.laudit/
  .gitignore          # ignores index/
  config.toml         # per-repo config
  skeleton.json       # Stage 0 structure
  model.json          # Stage 1 codebase model
  tours/<id>.json     # Stage 2 plan + Stage 3 prose + position
  redactions.json     # placeholder map (never contains secrets)
  index/              # machine-local parse/graph cache
```

`skeleton.json`, `model.json`, `tours/`, and `redactions.json` are committable;
`index/` is machine-local and git-ignored. See `docs/artifacts.md` for schemas
and invalidation rules.

## RPC

`laudit serve` exposes the engine over newline-delimited JSON-RPC 2.0 on stdio.
The Neovim plugin and other frontends drive it with `ping`, `status`,
`build_tour`, `resume_tour`, `explain_stop`, `get_outline`, `set_position`, and
`cancel`, and receive `progress`, `comprehension_done`, `stop_ready`,
`explanation_chunk`, and `error` notifications. Logs go to stderr; stdout is the
protocol channel. `laudit mcp` exposes the same handlers as MCP tools. See
`docs/protocol.md` for the full method and error tables.

## Firm invariants

These are non-negotiable and apply to every ticket:

1. **No Neovim dependency in the engine.** The engine builds and runs without an
   editor. Lua is UI only.
2. **All network I/O happens in Rust.** Lua never performs HTTP.
3. **Redaction is always on.** Every payload leaving the process (LLM prompts,
   logs, cached artifacts) is redacted first. Secrets are never sent, logged, or
   persisted.
4. **Artifacts are versioned JSON** in `.laudit/`, stamped with `schema_version`
   and `tool_version`. They are the durable interface for frontends and future
   tools.
5. **Stops are anchored to real symbol IDs/ranges.** The LLM may not invent
   locations; unknown refs are dropped with a warning.
6. **Recon is deterministic; the LLM only orders and writes prose.** No LLM call
   is required to produce structural artifacts (`--offline`).
7. **Async runtime is tokio.** Parsing and narration are sequential initially;
   concurrency is added only behind seams. **No rayon.**
8. **One provider, OpenAI-compatible.** `base_url` + `model` (required) +
   `api_key` resolution. Provider profiles are a later addition.
9. **One active tour per engine instance.** New tours cancel/replace.
10. **The engine is CLI-first.** Every capability is reachable from the CLI; the
    RPC server and other frontends are layers over the same core.

## Non-goals (v1)

- No embeddings or vector store; the graph is deterministic.
- No tests chapter by default (opt-in).
- No non-Linux release binaries.
- No provider profiles, no multi-provider routing.

## References

- Decisions and rationale: `docs/decisions.md`
- Artifact schemas: `docs/artifacts.md`
- RPC + MCP protocol: `docs/protocol.md`
- Config, redaction, prompts: `docs/config.md`
- CLI/TUI/Neovim surfaces: `docs/frontends.md`
- Testing: `docs/testing.md`
- Release: `docs/release.md`

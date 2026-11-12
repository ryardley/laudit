# laudit artifacts

Artifacts are the durable, versioned interface between the engine, frontends,
and future tools. They live in the repository under `.laudit/` (D16).

## Directory layout

```
.laudit/
  .gitignore          # ignores index/ by default
  config.toml         # per-repo engine config (see docs/config.md)
  skeleton.json       # deterministic Stage 0 structure — committable
  model.json          # codebase model (Stage 1) — committable
  tours/<id>.json     # tour plan + prose + position — committable
  redactions.json     # placeholder map — committable
  index/              # machine-local parse/graph cache — git-ignored
```

The engine creates `.laudit/.gitignore` and appends `index/` when it is not
already ignored. Existing content is preserved, so the file may contain more
than:

```
index/
```

Whether to commit `skeleton.json`, `model.json`, `tours/`, and
`redactions.json` is the team's choice (D16). The engine never requires them to
be committed. Committing them lets teammates and CI reuse the tour and lets
`laudit build --offline --check` verify freshness.

## Versioning rules

- Every artifact carries `schema_version` (integer) and `tool_version` (string).
- `schema_version` increments on breaking shape changes.
- Readers must reject unknown `schema_version` with a clear error.
- Artifacts store **redacted** text only (D11).

## `skeleton.json` — structural skeleton (Stage 0)

The deterministic recon output. It contains no prose and no LLM output, so it is
produced by `laudit build --offline` and is the spine that Stage 1 narrates.

```jsonc
{
  "schema_version": 1,
  "tool_version": "0.1.0",
  "repo_id": "fnv1a128:…",        // stable id for the repo path
  "head": "git-sha-or-null",
  "dirty_hash": "fnv1a128:…",     // hash of working-tree state
  "files": [
    { "path": "src/main.rs", "rank": 0.18, "community": 0 }
  ],
  "symbols": [
    { "id": "sym-…", "file": "src/main.rs", "name": "main",
      "line": 1, "rank": 0.18, "community": 0 }
  ],
  "communities": [
    { "id": 0, "files": ["src/main.rs"], "symbols": ["sym-…"] }
  ],
  "entrypoints": [
    { "file": "src/main.rs", "line": 1, "kind": "rust-main",
      "reason": "Rust binary crate root (main.rs)", "score": 100 }
  ],
  "ranked_files": ["src/main.rs"],
  "ranked_symbols": ["sym-…"],
  "provenance": {
    "src/main.rs": {
      "file": "src/main.rs",
      "recent_commits": [],
      "churn": { "commits": 0, "insertions": 0, "deletions": 0 }
    }
  }
}
```

`files` and `symbols` are sorted deterministically; `ranked_files` and
`ranked_symbols` are ordered by PageRank descending. `provenance` is keyed by
repository-relative path and may include `region`, `blame`, and `recent_commits`
when git data is available.

## `model.json` — codebase model (Stage 1)

```jsonc
{
  "schema_version": 1,
  "tool_version": "0.1.0",
  "repo_id": "fnv1a128:…",        // stable id for the repo path
  "head": "git-sha-or-null",
  "dirty_hash": "fnv1a128:…",     // hash of working-tree state
  "generator": "laudit",
  "model": "gpt-4o-mini",         // model id used
  "prompt_version": "1",
  "generated_at": "2026-11-12T00:00:00Z",
  "subsystems": [
    { "id": "sub-1", "title": "HTTP layer", "summary": "…",
      "symbol_ids": ["sym-…"], "files": ["src/http.rs"] }
  ],
  "chapters": [
    { "id": "ch-1", "title": "On-ramp", "intent": "…",
      "stop_ids": ["stop-…"] }
  ],
  "flows": [
    { "id": "flow-1", "title": "Request lifecycle",
      "steps": ["sym-…", "sym-…"] }
  ],
  "glossary": [
    { "term": "PCZT", "definition": "…" }
  ]
}
```

## `tours/<id>.json` — tour (Stages 2–3)

```jsonc
{
  "schema_version": 1,
  "tool_version": "0.1.0",
  "tour_id": "tour-…",
  "source": { "kind": "codebase" },   // "codebase" | "uncommitted" | "range"
  "model": "gpt-4o-mini",
  "prompt_version": "1",
  "redaction_profile": "default",
  "position": { "chapter": 0, "stop": 0 },
  "chapters": [
    {
      "id": "ch-1",
      "title": "On-ramp",
      "stops": [
        {
          "id": "stop-…",
          "file": "src/main.rs",
          "start_line": 1,
          "end_line": 42,
          "symbol_id": "sym-…",
          "title": "Program entry",
          "why_here": "…",
          "explanation": "…markdown…",
          "refs": ["sym-…"],
          "file_hash": "fnv1a128:…", // content hash when narrated; used to invalidate
          "status": "explained"   // pending | streaming | explained | skipped | failed
        }
      ]
    }
  ]
}
```

### Resume and invalidation

`position` and each stop's `status` are persisted so a tour can resume after a
restart (D25). On resume, stops with `status: "explained"` are reused without
calling the LLM. Each explained stop records the `file_hash` of the file it was
narrated from; if the file's content hash changes, only that stop is reset to
`pending` (its explanation and refs are cleared) and narrated again. Stops left
in `streaming` by an interrupted run are reset to `pending`.

## `redactions.json` — placeholder map

```jsonc
{
  "schema_version": 1,
  "tool_version": "0.1.0",
  "placeholders": {
    "«aws_key#1»": { "rule": "aws_key", "file": "src/config.rs", "line": 12 }
  }
}
```

Placeholders are session-stable (D11). The map never contains the secret value.

## `index/` — machine-local cache

Opaque to frontends. Contains per-file content hashes, extracted tags, and graph
edges. Regenerable; git-ignored. Format is internal and may change without a
`schema_version` bump.

## `config.toml`

See `docs/config.md` for the full schema.

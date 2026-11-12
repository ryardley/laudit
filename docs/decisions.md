# laudit decision log

Firm decisions made during design. Each is numbered so tickets can cite it
(e.g. "see D11"). Changing a decision requires updating this file and the
affected tickets.

## D1 — Explanations come from an LLM via an OpenAI-compatible provider
"Explain what this does and why it is here" is semantic and needs a model.
We use an OpenAI-compatible chat-completions API with configurable `base_url`,
`model`, and `api_key`, so OpenAI, OpenRouter, Ollama, llama.cpp, vLLM, and
LocalAI all work. No provider-specific SDKs.

## D2 — v1 scope is whole-codebase tours
Uncommitted-change and commit-range tours are deferred to the source-adapter
ticket (D3). The `TourStop` abstraction is kept so they slot in later.

## D3 — A tour is an ordered list of `TourStop`s produced by a source adapter
`TourStop = {file, start_line, end_line, symbol_id, title, why_here, explanation,
status}`. A source adapter yields ordered stops. v1 ships the whole-codebase
adapter; uncommitted and rev-range adapters come later.

## D4 — Map-then-detail, with a deterministic map
The structural map is built deterministically (tree-sitter defs/refs → graph →
PageRank → Louvain communities → entrypoints). The LLM does comprehension,
ordering, and prose — not structure discovery. This keeps the spine cheap,
reproducible, and offline-capable.

## D5 — Rust engine, thin Lua frontend
Most code is Rust. The Neovim plugin is a thin UI shell. See D6 for transport.

## D6 — Rust runs as a subprocess over JSON-RPC 2.0 (newline-delimited) on stdio
Not in-process FFI (fragile marshalling, crash risk), not nvim-native msgpack
(only nvim speaks it). JSON-RPC 2.0 over stdio is editor-agnostic; Neovim drives
it with `jobstart` + `vim.json`. One long-lived child per Neovim instance,
lazily started, with health ping, bounded auto-restart, and clean shutdown.

## D7 — Bundled tree-sitter grammars
Ship grammar crates for Rust, TypeScript, TSX, JavaScript, bash, toml, yaml,
json, markdown, and sql. Tours work without the user installing editor parsers.
Unsupported files degrade gracefully.

## D8 — Distribution: prebuilt Linux binaries primary, cargo fallback
GitHub Releases with Linux x86_64/aarch64 (gnu+musl) binaries + checksums.
`build.lua` runs `cargo build --release` as a fallback. Optional auto-download
is opt-in. Non-Linux targets are out of scope for v1.

## D9 — Layered context per stop
Each stop's prompt includes: local code region; structural context (callers,
callees, imports, parent module, siblings); provenance (blame age, recent commit
subjects, churn); prose (doc comments, NOTE/WHY/TODO, README/ADR references);
and the narrative (tour outline). This is what makes "why it is here" possible.

## D10 — Streaming narration via a front-matter envelope
Strict JSON fights token streaming. Narration uses `TITLE:` / `WHY:` lines, then
`---`, then a streamed markdown body. Comprehension stays strict JSON (not
streamed). Streaming improves perceived latency; it does not reduce compute time.

## D11 — Redaction is always on
Layered, in Rust, applied before send/cache/log: embedded gitleaks-derived
ruleset + key-name-aware redaction for env/toml/yaml/json + session-stable
placeholders + user allowlist. Secrets are never sent, logged, or persisted.

## D12 — Hybrid pipeline: global comprehension + lazy narration
Stage 0 recon (deterministic) → Stage 1 comprehension (LLM, cached, whole-repo
model) → Stage 2 plan (deterministic) → Stage 3 narration (LLM, lazy, streamed).
The model always answers from whole-codebase understanding, not a myopic view of
one function.

## D13 — The codebase model is invisible but parseable
Users do not edit it in v1, but it is emitted as structured JSON
(`.laudit/model.json`) so future tools can consume it.

## D14 — Tour UX: real buffers, persistent split, free exploration
Tour the real files in a normal window. A persistent split (not a float) shows
the explanation so code is never occluded. The tour window auto-follows to the
next stop's buffer/range. Users may explore freely (`gd`, jumplist, links); tour
state is authoritative and a "return" action snaps back to the current stop.
Resume is persisted across restarts.

## D15 — RPC lifecycle
One active tour at a time (cancel-and-replace). All long work is async with
progress + cancellation keyed by `tour_id`/`stop_id`. Neovim never blocks.
Health ping, bounded auto-restart, clean shutdown on `VimLeave`.

## D16 — Artifacts live in the repo under `.laudit/`
`.laudit/model.json`, `.laudit/tours/<id>.json`, `.laudit/redactions.json` are
committable. `.laudit/index/` is machine-local and git-ignored by default via a
shipped `.laudit/.gitignore`. Whether to commit the rest is the team's choice.

## D17 — Config: global + per-repo, single provider, explicit model
Global `~/.config/laudit/config.toml`; per-repo `.laudit/config.toml`. Key
resolution: env → user config → repo config (env wins). One provider for now;
profiles later. `model` is required; no hidden default. Local endpoints need no
key.

## D18 — Prompts are embedded, installed on first run, versioned
Defaults are embedded in the binary and installed to `~/.config/laudit/prompts/`
on first run, then loaded from there (user-editable). Each prompt is versioned;
a newer shipped version warns rather than silently overwriting. `laudit prompts`
inspects them. `prompt_version` is recorded in artifacts.

## D19 — Scale: incremental index, hierarchical comprehension, sequential first
Content-hash incremental index; hierarchical summarization for oversized repos;
sequential parse and narration initially, with seams for bounded concurrency.
No rayon.

## D20 — Failure and offline behavior
Recon artifacts are always produced even if the LLM fails. Per-stop failure =
skip + warn + log (mark status), never abort the tour. `--offline` is a
first-class mode that runs recon only.

## D21 — Testing: fixtures + mock LLM + golden + headless
Unit tests in `laudit-core`; integration tests against committed fixture repos
with a mock LLM HTTP server; `insta` golden artifacts; JSON-RPC protocol tests;
headless-Neovim tests with a fake engine. No network in tests.

## D22 — Default tour order and test handling
On-ramp → subsystems (dependency direction) → infra → config. Tests are skipped
by default and opt-in. Config is included but skippable.

## D23 — Editor-free frontends
The engine is usable without Neovim via: `laudit explain` / `tour --print`
(cheapest), `export --md`, `--offline --check` for CI, `export --html`, the
`laudit tour` TUI, `laudit mcp`, source adapters + PR change tours, and
`laudit ask`. Build order: explain/print → md → check → html → TUI → MCP →
adapters → PR tour → ask.

## D24 — Stops are hard-anchored; unknown refs are dropped
The LLM may only reference engine-provided symbol IDs. Unknown refs are dropped
with a warning and logged. Ranges always come from the engine.

## D25 — One active tour per instance; artifacts are the source of truth
Frontends render from artifacts and RPC events. Tour position and per-stop
status live in the tour artifact so any frontend can resume.

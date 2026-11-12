# laudit testing

No test may require network access (D21).

## Layers

| Layer | Location | What it covers |
|---|---|---|
| Unit | `crates/laudit-core/src/**` `#[cfg(test)]` | graph, PageRank, redaction, grammar extraction, schema round-trips, config precedence, plan validation. |
| Integration | `crates/laudit-core/tests/` | full pipeline against fixture repos with a mock LLM. |
| Golden | `crates/laudit-core/tests/` using `insta` | `model.json` and `tour.json` snapshots. |
| Protocol | `crates/laudit-core/tests/` | drive `laudit serve` over stdio JSON-RPC. |
| Plugin | `tests/nvim/` | headless Neovim with a fake engine. |

## Fixtures

`tests/fixtures/` contains small, committed repos:

- `rust-app/` — a Rust binary with modules, a hub symbol, and an entrypoint.
- `ts-app/` — a TypeScript/JS project with `package.json` main/bin.
- `git-repo/` — a repo with a few commits for provenance and adapter tests.
- `secrets/` — files containing fake AWS/GitHub/JWT/PEM/env secrets for
  redaction tests (never real secrets).

## Mock LLM

A mock HTTP server (`wiremock` or `httpmock`) returns canned structured
responses for comprehension and narration. Tests assert:

- the request body contains **no secrets** (redaction-before-send),
- schema violations trigger one retry then skip+warn,
- streaming chunks are emitted in order.

## Golden artifacts

`insta` snapshots of `model.json` and `tour.json` for the fixtures. Snapshots
must be stable across runs (deterministic recon; mocked LLM).

## Headless Neovim

- A fake engine script speaks the JSON-RPC protocol with canned responses and
  notifications.
- Tests (plenary or busted) cover: `setup`, health, tour start, advance, return,
  resume, outline.
- Run with `nvim --headless`.

## CI

GitHub Actions runs: `cargo fmt --check`, `cargo clippy -- -D warnings`,
`cargo test`, headless Neovim tests, `cargo-deny`, and `cargo-audit`. Cargo is
cached. Runs on push and PR.

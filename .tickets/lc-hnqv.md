---
id: lc-hnqv
status: open
deps: [lc-ycp2]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-yiw2
tags: [ci]
---
# CI pipeline

Automate quality gates.

## Design

Add GitHub Actions: fmt, clippy -D warnings, cargo test, headless nvim tests, cargo-deny and cargo-audit. Cache cargo. Run on push and PR.

## Acceptance Criteria

CI is green on a clean checkout; failures block merge; jobs are reasonably fast.


## References

See `docs/testing.md` (CI) and `docs/release.md`.

## Additional context

Workflow file: `.github/workflows/ci.yml`. Jobs: (1) rust — `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; (2) nvim — install Neovim, run `nvim --headless -u tests/nvim/minimal.lua -c "PlenaryBustedDirectory tests/nvim"`; (3) supply-chain — `cargo deny check`, `cargo audit`. Cache `~/.cargo` and `target/`. Trigger on push and pull_request.

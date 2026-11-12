# laudit

`laudit` is a guided codebase tour. It walks you through a repository, highlights
real code regions, and explains **what the code does** and **why it is there**,
grounded in a whole-codebase understanding.

The Rust engine is the product. Neovim, the terminal TUI, the CLI, and the MCP
server are thin frontends over the same `laudit-core` pipeline. See
[docs/architecture.md](docs/architecture.md) for the authoritative design.

## Install

### Prebuilt binaries (recommended)

Linux x86_64/aarch64 binaries are published on GitHub Releases with checksums.
See [docs/release.md](docs/release.md) for the download and verification steps.

### Build from source

With a Rust toolchain (1.88 or newer):

```sh
cargo build --release
./target/release/laudit --version
```

`build.lua` is the source fallback for Neovim plugin users:

```sh
nvim --headless -u NONE -l build.lua
```

### Nix flake

The flake installs the binary without a Rust toolchain:

```sh
nix run .#laudit -- --version                # run without installing
nix build .#laudit                           # ./result/bin/laudit
nix profile install .#laudit                 # install the binary
nix develop                                  # development shell
```

From GitHub:

```sh
nix run github:ryardley/laudit -- --version
nix profile install github:ryardley/laudit
```

The flake also exposes `packages.<system>.laudit-nvim` (the Lua plugin) and
`overlays.default`. See [docs/release.md](docs/release.md) for details.

### Neovim plugin

The plugin lives in `lua/laudit/` and `plugin/laudit.lua`. Put the repository on
your Neovim runtimepath (or install `laudit-nvim` from the flake), then:

```lua
require('laudit').setup({
  binary = nil,          -- explicit path; else $PATH
  split = { side = 'right', size = 40 },
  keymaps = true,        -- install the default <leader>l maps
})
```

`setup{}` holds UI options only. Engine configuration lives in the config files
described below.

#### lazy.nvim

Add the plugin to your lazy.nvim spec. The `laudit` binary must be on `$PATH`
(see the install sections above), or point `binary` at it explicitly:

```lua
{
  'ryardley/laudit',
  config = function()
    require('laudit').setup({
      split = { side = 'right', size = 40 },
      keymaps = true,
    })
  end,
}
```

To compile the binary from source on install instead of installing it
separately, add a `build` step (requires a Rust toolchain):

```lua
{
  'ryardley/laudit',
  build = 'nvim --headless -u NONE -l build.lua',
  config = function()
    require('laudit').setup({
      binary = vim.fn.stdpath('data') .. '/lazy/laudit/target/release/laudit',
    })
  end,
}
```

Run `:Lazy build laudit` to rebuild after updating.

## Quickstart

### Offline (no LLM, no network)

Recon is deterministic and works without a provider:

```sh
cd /path/to/repo
laudit build --offline
laudit tour --print --offline
```

`build --offline` writes the structural artifacts under `.laudit/`. `tour
--print --offline` prints the planned tour without prose. This is also the CI
path: `laudit build --offline --check` exits non-zero when the committed
artifacts are stale.

### With an LLM

Create a global config. Provider settings and API keys belong here, never in
the repo:

```sh
mkdir -p ~/.config/laudit
cat > ~/.config/laudit/config.toml <<'EOF'
[provider]
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"
EOF

export OPENAI_API_KEY=sk-...
```

Per-repo `.laudit/config.toml` is for repo-specific settings (`roots`,
`languages`, `excludes`, budgets); a `provider.api_key` there is rejected.

Then build and tour:

```sh
cd /path/to/repo
laudit build                 # recon + comprehension + plan
laudit tour --print          # narrated tour to stdout
laudit tour                  # interactive terminal TUI
```

Any OpenAI-compatible endpoint works. For a local model, no key is needed:

```toml
[provider]
base_url = "http://localhost:11434/v1"
model = "llama3.1"
```

### PR change tours in CI

Render a pull request's changes as markdown for a comment or build artifact:

```sh
laudit tour --range origin/main..HEAD --md --out tour.md
```

Add `--offline` to produce a structure-only tour without an API key. See
[docs/frontends.md](docs/frontends.md#pr-change-tours-in-ci) for a GitHub
Actions example and the `LAUDIT_API_KEY` fallback behavior.

## Commands

| Command | Purpose |
|---|---|
| `laudit build [--offline] [--check] [--quiet] [--include-tests] [--skip-config]` | Run recon (+ comprehension/plan unless `--offline`). `--check` exits non-zero if artifacts are stale. |
| `laudit tour [--print \| --md [--out <path>]] [--uncommitted \| --range <base..head> [--per-commit]]` | Run the interactive TUI, print the tour with `--print`, or render markdown with `--md`. Source flags tour uncommitted changes or a commit range. |
| `laudit explain <symbol>` | Print a grounded explanation for one symbol. |
| `laudit ask [<question>]` | Answer a grounded question with `file:line` citations; starts a REPL when the question is omitted. |
| `laudit export --md [--out <path>]` | Render the tour to markdown. |
| `laudit export --html [--out <dir\|file>]` | Render the tour to a self-contained HTML document. |
| `laudit status [--tour <id>]` | Report model/tour presence and per-status stop counts. |
| `laudit prompts [<name>] [--list] [--all]` | List or print installed prompts. |
| `laudit serve` | Start the JSON-RPC 2.0 stdio server. |
| `laudit mcp` | Start the MCP stdio server. |

Global flags: `--repo <path>`, `--offline`, `--json`, `--color auto|always|never`,
`--version`, `--help`.

`laudit outline` and `laudit clear` are reserved commands that currently exit
with "not implemented yet".

## Neovim keymaps

Default global keymaps (disable with `setup({ keymaps = false })`):

| Key | Action |
|---|---|
| `<leader>lt` | start tour |
| `<leader>lr` | resume |
| `<leader>ln` | next stop |
| `<leader>lp` | previous stop |
| `<leader>lo` | outline picker |
| `<leader>lb` | return to current stop |
| `<leader>ls` | stop tour |
| `<leader>lh` | health |

Commands: `:Laudit tour | resume | stop | outline | next | prev | return | model |
explain | status | clear | health`.

## Terminal TUI keys

| Key | Action |
|---|---|
| `n` / `p` | next / previous stop |
| `o` | toggle outline |
| `g` | return to the current stop |
| `r` | retry narration for the current stop |
| `j` / `k` | scroll the code pane |
| `PageDown` / `PageUp` | scroll a page |
| `Home` / `End` | jump to the top / bottom |
| `q` | quit |

In the outline overlay: `j`/`k` move, `Enter` selects, `Esc` closes, `q` quits.

## Configuration

- Global: `~/.config/laudit/config.toml` (or `$XDG_CONFIG_HOME/laudit/config.toml`)
- Per-repo: `<repo>/.laudit/config.toml`

`laudit build` installs a default global config on first run if the file is
missing; existing files are never overwritten.

Per-repo values override global values. API keys resolve from the environment
first, then the global config; keys are never read from the repo config.
`model` is required for LLM-backed commands; local endpoints need no key.
Redaction is always on.

See [docs/config.md](docs/config.md) for the full schema, key resolution, and
redaction rules.

## `.laudit/` layout and Git tracking

```text
.laudit/
  .gitignore          # ignores index/ by default
  config.toml         # per-repo engine config
  skeleton.json       # deterministic Stage 0 structure
  model.json          # codebase model (Stage 1)
  tours/<id>.json     # tour plan + prose + position
  redactions.json     # placeholder map (never contains secret values)
  index/              # machine-local parse/graph cache
```

The engine creates `.laudit/.gitignore` with `index/` and preserves any existing
content. `index/` is machine-local and should not be committed.

`config.toml`, `skeleton.json`, `model.json`, `tours/`, and `redactions.json`
are committable. Committing them lets teammates and CI reuse the tour and lets
`laudit build --offline --check` verify freshness. Whether to commit them is the
team's choice; the engine never requires it. Artifacts contain redacted text
only.

See [docs/artifacts.md](docs/artifacts.md) for the schemas and versioning rules.

## Documentation

- [docs/architecture.md](docs/architecture.md) — engine/frontend split, pipeline, artifacts, RPC.
- [docs/decisions.md](docs/decisions.md) — numbered decision log.
- [docs/config.md](docs/config.md) — config, key resolution, redaction, prompts.
- [docs/artifacts.md](docs/artifacts.md) — `.laudit/` layout and JSON schemas.
- [docs/protocol.md](docs/protocol.md) — JSON-RPC and MCP protocols.
- [docs/frontends.md](docs/frontends.md) — CLI, TUI, Neovim, exports.
- [docs/testing.md](docs/testing.md) — test layers and fixtures.
- [docs/release.md](docs/release.md) — release targets and install paths.

## Development

```sh
cargo fmt
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
make test-nvim        # headless Neovim tests
make check            # fmt-check + clippy + tests
```

No test requires network access.

## License

MIT.

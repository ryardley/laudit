# laudit frontends

All frontends are thin layers over `laudit-core`. The engine is CLI-first (D23).

## CLI (`laudit`)

| Command | Purpose |
|---|---|
| `laudit build [--offline] [--check] [--quiet] [--include-tests] [--skip-config]` | Run recon (+ comprehension/plan unless `--offline`). `--check` exits non-zero if artifacts are stale. |
| `laudit serve` | Start the JSON-RPC 2.0 stdio server (see `docs/protocol.md`). |
| `laudit tour [--print \| --md [--out <path>]] [--uncommitted \| --range <base..head> [--per-commit]]` | Run the interactive TUI, print the tour with `--print`, or render markdown with `--md`. Source flags tour uncommitted changes or a commit range. |
| `laudit explain <symbol>` | Print a grounded explanation for one symbol. |
| `laudit export --md [--out <path>]` | Render a tour to markdown. |
| `laudit export --html [--out <dir\|file>]` | Render a tour to a self-contained HTML document. |
| `laudit mcp` | Start the MCP stdio server. |
| `laudit status [--tour <id>]` | Report model/tour presence and counts. |
| `laudit prompts [<name>] [--list] [--all]` | List/print installed prompts. |

Global flags: `--repo <path>`, `--offline`, `--json`, `--color`, `--version`,
`--help`.

Reserved but not yet implemented: `laudit ask`, `laudit outline`, and
`laudit clear`. They parse their documented flags and exit non-zero with
"not implemented yet".

## Terminal TUI (`laudit tour`)

Full-screen ratatui + crossterm layout:

```
+--------------------------------------------------+
| Chapter 2/5 · Stop 3/12        laudit            |  header
+----------------+---------------------------------+
| Chapters/Stops |  code pane (target range        |
|  > On-ramp     |  highlighted)                   |
|    HTTP layer  |                                 |
|    ...         |                                 |
+----------------+---------------------------------+
| explanation pane (streams markdown)              |
+--------------------------------------------------+
| n/p next/prev  o outline  g return  q quit       |  footer
+--------------------------------------------------+
```

- Runs in-process against the engine (no RPC).
- Restores the terminal on exit and on panic.
- Handles resize.
- Keys: `n`/`p` next/previous, `o` outline, `g` return to current stop, `r`
  retry narration, `j`/`k` scroll the code pane, `PageUp`/`PageDown` scroll a
  page, `Home`/`End` jump, `q` quit. In the outline overlay: `j`/`k` move,
  `Enter` select, `Esc` close, `q` quit.

## Neovim plugin (`lua/laudit/`)

Thin UI over `laudit serve` (D6, D14).

### Commands

`:Laudit tour | resume | stop | outline | next | prev | return | model | explain | status | clear | health`

### Default keymaps (`<leader>l`, global, disable-able)

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

### UX

- Real buffers in a normal window; the tour window auto-follows to the next
  stop's buffer/range.
- A **persistent split** (not a float) shows the explanation, so code is never
  occluded.
- `extmark` highlight over the stop range; header `Chapter x/y · Stop n/m`.
- Free exploration allowed (`gd`, jumplist, links); tour state is authoritative;
  `return` snaps back to the current stop and re-highlights.
- Resume persists across restarts.

### `setup{}` (UI only)

```lua
require('laudit').setup({
  binary = nil,          -- explicit path; else $PATH
  split = { side = 'right', size = 40 },
  keymaps = true,        -- install default <leader>l maps
  auto_download = false, -- opt-in seam; not implemented in the Lua frontend yet
})
```

## Exports

- **Markdown:** document title, chapter headings, per-stop heading with
  `file:line`, fenced code of the range, `why_here`, explanation, refs as links.
- **HTML:** self-contained page with chapter nav, code pane, prose pane, and
  cross-links to definitions. Works offline.

## PR change tours in CI

`laudit tour` can render a change tour as markdown for a pull-request comment or
build artifact. Combine a source flag (`--range` or `--uncommitted`) with `--md`;
`--out` writes the document to a file instead of stdout.

```sh
laudit tour --range origin/main..HEAD --md --out tour.md
```

The markdown is deterministic for a given diff and is redacted before it is
written (D11). With `--offline` the tour contains the structure and code ranges
but no LLM prose, so it works without a provider or API key. Without `--offline`
the stops are narrated first, then rendered.

### GitHub Actions example

Set a repository secret named `LAUDIT_API_KEY` to enable narration. When the
secret is absent the workflow falls back to `--offline`, producing a
structure-only tour instead of failing. `provider.api_key_env` can point at a
different variable if the team already has one.

```yaml
name: PR change tour
on: pull_request
permissions:
  contents: read
  pull-requests: write
jobs:
  tour:
    runs-on: ubuntu-latest
    env:
      LAUDIT_API_KEY: ${{ secrets.LAUDIT_API_KEY }}
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: dtolnay/rust-toolchain@stable
      - name: Build laudit
        run: cargo build --release -p laudit-cli
      - name: Generate change tour
        run: |
          if [ -n "$LAUDIT_API_KEY" ]; then
            ./target/release/laudit tour \
              --range "origin/${{ github.base_ref }}..HEAD" \
              --md --out tour.md
          else
            ./target/release/laudit tour \
              --range "origin/${{ github.base_ref }}..HEAD" \
              --md --out tour.md --offline
          fi
      - name: Upload tour artifact
        uses: actions/upload-artifact@v4
        with:
          name: laudit-tour
          path: tour.md
      - name: Comment on the pull request
        if: env.LAUDIT_API_KEY != ''
        env:
          GH_TOKEN: ${{ github.token }}
        run: gh pr comment "${{ github.event.pull_request.number }}" --body-file tour.md
```

The `pr-tour` job in `.github/workflows/ci.yml` exercises the same path against
a throwaway fixture PR with `--offline`, and asserts that the markdown is stable
and redacted.

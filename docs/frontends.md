# laudit frontends

All frontends are thin layers over `laudit-core`. The engine is CLI-first (D23).

## CLI (`laudit`)

| Command | Purpose |
|---|---|
| `laudit build [--offline] [--check] [--quiet] [--repo <path>] [--json]` | Run recon (+ comprehension/plan unless `--offline`). `--check` exits non-zero if artifacts are stale. |
| `laudit serve` | Start the JSON-RPC 2.0 stdio server (see `docs/protocol.md`). |
| `laudit tour [--print] [--uncommitted] [--range <base..head>] [--per-commit] [--color auto\|always\|never]` | Run/print a tour. `--print` is pipe-friendly. |
| `laudit explain <symbol>` | Print a grounded explanation for one symbol. |
| `laudit export --md [--out <path>]` | Render a tour to markdown. |
| `laudit export --html [--out <dir\|file>]` | Render a tour to a self-contained HTML document. |
| `laudit ask [<question>]` | Grounded Q&A over the codebase model; REPL when no question. |
| `laudit mcp` | Start the MCP stdio server. |
| `laudit outline` | Print the tour outline. |
| `laudit status` | Report model/tour presence and counts. |
| `laudit prompts` | List/print installed prompts. |
| `laudit clear` | Remove generated artifacts (with confirmation). |

Global flags: `--repo <path>`, `--offline`, `--json`, `--color`, `--version`,
`--help`.

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

## Neovim plugin (`lua/laudit/`)

Thin UI over `laudit serve` (D6, D14).

### Commands

`:Laudit tour | resume | stop | outline | next | prev | return | model | explain | status | clear`

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
  binary = nil,          -- explicit path; else $PATH; else optional auto-download
  split = { side = 'right', size = 40 },
  keymaps = true,        -- install default <leader>l maps
})
```

## Exports

- **Markdown:** document title, chapter headings, per-stop heading with
  `file:line`, fenced code of the range, `why_here`, explanation, refs as links.
- **HTML:** self-contained page with chapter nav, code pane, prose pane, and
  cross-links to definitions. Works offline.

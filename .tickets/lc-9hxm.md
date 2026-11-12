---
id: lc-9hxm
status: open
deps: [lc-lz7a, lc-1yrq]
links: []
created: 2026-11-12T09:02:36Z
type: task
priority: 1
parent: lc-32ar
tags: [tui, frontend]
---
# laudit tour TUI

The flagship editor-free experience: an interactive terminal walkthrough equivalent to the Neovim tour.

## Design

Add a new workspace crate laudit-tui using ratatui + crossterm. Full-screen layout: left chapter/stop list, main code pane scrolled to the stop range with highlighted lines, bottom explanation pane streaming from narration, header progress (Chapter x/y, Stop n/m), footer key hints. Keys: n/p next/prev, o outline, q quit, g return-to-current. Reuse the engine in-process (no RPC). Handle terminal resize and restore the terminal on exit/panic.

## Acceptance Criteria

Launches on a plain tty; explanations stream into the pane; next/prev/outline/quit work; terminal is restored on exit; no Neovim required.


## References

See `docs/frontends.md` (Terminal TUI) and `docs/decisions.md` D23.

## Additional context

Crate deps: `ratatui`, `crossterm`, `laudit-core`. Install a panic hook that restores the terminal (leave alternate screen, disable raw mode) before printing the panic. Use an event loop that polls crossterm events and narration stream chunks without blocking the UI. The code pane reads the file from disk and highlights `start_line..=end_line`. Reuse the same tour artifact and narration path as the Neovim frontend.

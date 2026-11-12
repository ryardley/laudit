---
id: lc-uuim
status: closed
deps: [lc-q94s]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-ej4t
tags: [nvim, ui]
---
# Tour UI: split, extmarks, auto-follow

Render the tour on real buffers with a persistent explanation split.

## Design

In lua/laudit/ui.lua open the stop's file in the tour window, move cursor to the range, set an extmark highlight over start-end, and render the explanation in a persistent split (not a float) with header 'Chapter x/y · Stop n/m'. On advance, auto-move the tour window to the next stop's buffer/range. Stream explanation chunks into the split.

## Acceptance Criteria

Headless test: starting a tour opens the file, sets an extmark, shows the split header; advancing moves to the next buffer/range; streamed chunks append.


## References

See `docs/frontends.md` (Neovim UX) and `docs/decisions.md` D14.

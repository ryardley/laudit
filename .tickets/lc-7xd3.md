---
id: lc-7xd3
status: open
deps: [lc-uuim]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 1
parent: lc-ej4t
tags: [nvim, ui]
---
# Commands, keymaps, outline, return-to-stop, resume

Give users control over the tour while allowing free exploration.

## Design

Implement :Laudit tour|resume|stop|outline|next|prev|return|model|explain|status|clear and default <leader>l global keymaps (disable-able). Outline picker jumps to any chapter/stop. Free exploration allowed; tour state is authoritative; 'return' snaps the window back to the current stop and re-highlights. Resume persists across restarts.

## Acceptance Criteria

Headless test: commands exist; next/prev advance; return restores current stop after wandering; outline jumps; resume restores position.


## References

See `docs/frontends.md` (Neovim commands/keymaps) and `docs/decisions.md` D14.

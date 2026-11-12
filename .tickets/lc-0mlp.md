---
id: lc-0mlp
status: open
deps: []
links: []
created: 2026-11-12T20:58:14Z
type: bug
priority: 2
tags: [cli, ui, progress]
---
# laudit build shows no loader animation or progress bar on first run

Running `laudit build` for the first time (LLM-backed comprehension + planning) provides no visual progress feedback. The terminal appears idle with no spinner, loader animation, or progress bar while recon/comprehension/planning run, so the user cannot tell whether work is progressing or the command is stuck. Offline and online paths both lack progress indication. Compare: `laudit tour` is interactive, but the build phase is silent.

## Acceptance Criteria

Running `laudit build` shows a spinner/loader (or progress indicator) during long-running stages (recon, comprehension, plan). Progress stays visible until the command completes or fails. No regressions to `--quiet` and `--json` output modes.


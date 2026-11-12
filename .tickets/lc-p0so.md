---
id: lc-p0so
status: open
deps: [lc-7xd3]
links: []
created: 2026-11-12T08:57:02Z
type: task
priority: 2
parent: lc-yiw2
tags: [docs]
---
# Documentation

Document usage, configuration, and architecture.

## Design

Write README (install, quickstart, commands, keymaps), docs/config.md (global + repo config, key resolution, redaction), docs/architecture.md (engine/frontend split, pipeline stages, artifacts, RPC). Include the .laudit/ layout and Git-tracking guidance.

## Acceptance Criteria

README quickstart works end to end; config and architecture docs match the implementation; .laudit layout documented.


## References

See `docs/README.md` and all documents under `docs/`.

## Notes

**2026-11-12T09:14:35Z**

Document the Nix flake install path (nix run / nix profile install) in the README and docs/release.md; see lc-um4w.

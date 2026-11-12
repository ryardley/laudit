---
id: lc-1str
status: open
deps: [lc-taeh]
links: []
created: 2026-11-12T09:02:36Z
type: task
priority: 2
parent: lc-32ar
tags: [export]
---
# laudit export --html

Produce a self-contained, browsable tour for onboarding and sharing.

## Design

Extend laudit-core::export to render markdown to HTML with a small template, CSS, and syntax highlighting (syntect or tree-sitter->HTML). Include a chapter navigation sidebar, a code pane, a prose pane, and cross-links to definitions within the document. Support --out <dir|file>. Single-file output preferred.

## Acceptance Criteria

Output opens in a browser; highlighting is correct; definition cross-links resolve within the document; works offline.


## References

See `docs/frontends.md` (Exports) and `docs/decisions.md` D23.

## Additional context

Prefer a single self-contained HTML file (inline CSS + highlighted code) so it can be shared as one artifact. If assets are needed, emit a directory with `index.html`. Use a stable, minimal template; no external network assets. Cross-links use in-document anchors keyed by symbol id.

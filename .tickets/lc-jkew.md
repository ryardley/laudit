---
id: lc-jkew
status: open
deps: []
links: []
created: 2026-11-12T12:56:04Z
type: bug
priority: 1
tags: [ci, supply-chain, dependencies]
---
# CI: cargo deny fails on paste advisory and webpki-roots license

The Supply chain (deny, audit) job fails on master (run for 65afbb3) with exit code 5: advisories FAILED and licenses FAILED.

1. advisories: `paste 1.0.15` is unmaintained (RUSTSEC-2024-0436), pulled in via `ratatui 0.29.0` -> `laudit-tui` -> `laudit-cli`. cargo-deny reports 'No safe upgrade is available!'. Suggested alternatives are `pastey` (drop-in fork) or `with_builtin_macros`.
2. licenses: `webpki-roots 1.0.12` is licensed `CDLA-Permissive-2.0`, which is not in the `deny.toml` allow list. It comes in via `hyper-rustls` -> `reqwest` -> `laudit-core`.

Also emitted (warnings only, not failing): duplicate versions of base64, foldhash, hashbrown, syn, unicode-width, windows-sys; and license-not-encountered for BSD-2-Clause, CC0-1.0, MPL-2.0.

## Design

Fix the two hard failures:

- Licenses: add `CDLA-Permissive-2.0` to `[licenses].allow` in `deny.toml` (it is a permissive data license and is the correct license for the Mozilla root store data in webpki-roots).
- Advisories: remove the unmaintained `paste` dependency. Preferred: bump `ratatui` to a release that no longer depends on `paste` (verify with `cargo tree -i paste`). If no such release exists, either patch `paste` to `pastey` via `[patch.crates-io]` or add a scoped `[advisories].ignore` entry for RUSTSEC-2024-0436 with a comment explaining why it is acceptable. Do not blanket-ignore advisories.

Optionally clean up the duplicate-version warnings if cheap, but they are not required to make CI green.

## Acceptance Criteria

cargo deny check exits 0 locally and in CI; no unmaintained advisory for paste remains (or a documented, scoped ignore exists); CDLA-Permissive-2.0 is explicitly allowed; the supply-chain job passes on master.


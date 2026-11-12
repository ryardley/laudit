---
id: lc-um4w
status: open
deps: [lc-n5h7]
links: []
created: 2026-11-12T09:14:27Z
type: task
priority: 1
parent: lc-yiw2
tags: [nix, release, packaging]
---
# Nix flake packaging (flake-installable)

Make the project installable as a Nix flake so users can run, install, or consume it without a Rust toolchain: `nix run`, `nix profile install`, or as a flake input. This is an additional distribution path alongside the GitHub Releases binaries (see lc-flk7).

## Design

Add a flake.nix at the repo root with a single nixpkgs input (use nixpkgs.lib.genAttrs for x86_64-linux and aarch64-linux; no flake-utils dependency). Build the Rust workspace with rustPlatform.buildRustPackage using cargoLock.lockFile = ./Cargo.lock. Expose: packages.<system>.default and packages.<system>.laudit (the binary); apps.<system>.default and apps.<system>.laudit (nix run); devShells.<system>.default (rust toolchain + tk + opencode for development); optionally packages.<system>.laudit-nvim (installs lua/ and plugin/ into $out for use as a Neovim plugin) and overlays.default. Keep devenv.nix for the dev environment; the flake is for packaging/installation. Ensure the flake builds the whole workspace (laudit-core, laudit-cli, and later laudit-tui/laudit-mcp) and that the binary resolves its version from the crate. Document the flake install path in docs/release.md.

## Acceptance Criteria

`nix build .#laudit` produces the binary; `nix run .#laudit -- --version` prints the version; `nix flake check` passes; `nix profile install .#laudit` installs a working binary; the flake exposes a devShell; docs/release.md documents the flake install path.


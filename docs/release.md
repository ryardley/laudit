# laudit release and distribution

## Targets (v1)

Linux only (D8):

- `x86_64-unknown-linux-gnu`
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-gnu`
- `aarch64-unknown-linux-musl`

Each release artifact ships with a checksum file.

## Tooling

- Use `cargo-dist` or `cross` to build the four targets on tag.
- Publish to GitHub Releases.
- Add `cargo-deny` and `cargo-audit` to CI.

## Binary resolution (Neovim plugin)

Order:

1. `setup({ binary = "..." })` explicit path.
2. `vim.fn.exepath('laudit')` (i.e. `$PATH`).
3. Optional auto-download (opt-in) of the matching release artifact into a
   plugin data dir, verified against the checksum.

## Source fallback

`build.lua` runs `cargo build --release` for users who prefer to build locally.
It must fail with a clear message if a Rust toolchain is missing.

## Nix flake

The project is also installable as a Nix flake (ticket `lc-um4w`), so users need
no Rust toolchain:

```sh
nix run github:<owner>/laudit -- --version   # run without installing
nix profile install github:<owner>/laudit    # install the binary
```

The flake exposes `packages.<system>.laudit` (and `default`), `apps.<system>.laudit`,
a `devShells.<system>.default`, and optionally `packages.<system>.laudit-nvim`
(the Lua plugin). It builds the workspace with `rustPlatform.buildRustPackage`
and `cargoLock.lockFile = ./Cargo.lock`. `devenv.nix` remains the development
environment; the flake is the packaging/installation path. `nix flake check`
must pass.

## Versioning

- `tool_version` is the crate version and is stamped into every artifact.
- `schema_version` is independent and bumps only on breaking artifact changes.
- A tagged release produces the four Linux artifacts with checksums.

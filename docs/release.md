# laudit release and distribution

## Targets (v1)

Linux only (D8):

- `x86_64-unknown-linux-gnu`
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-gnu`
- `aarch64-unknown-linux-musl`

Each release artifact ships with a checksum file.

## Install

### Prebuilt binaries (recommended)

Tagged releases publish one tarball per target to GitHub Releases:

```
laudit-<version>-<target>.tar.gz
laudit-<version>-<target>.tar.gz.sha256
SHA256SUMS
```

Download the artifact matching your platform, verify it, and put `laudit` on
`$PATH`:

```sh
version=0.1.0
target=x86_64-unknown-linux-gnu
base="https://github.com/<owner>/laudit/releases/download/v${version}"

curl -fsSLO "${base}/laudit-${version}-${target}.tar.gz"
curl -fsSLO "${base}/laudit-${version}-${target}.tar.gz.sha256"
sha256sum -c "laudit-${version}-${target}.tar.gz.sha256"

tar -xzf "laudit-${version}-${target}.tar.gz"
install -m 0755 "laudit-${version}-${target}/laudit" ~/.local/bin/laudit
laudit --version
```

Use the `musl` artifacts for a fully static binary that runs on any Linux
distribution; use the `gnu` artifacts when you want the system libc.

### Build from source

With a Rust toolchain installed:

```sh
cargo build --release
./target/release/laudit --version
```

`build.lua` is the source fallback for plugin users. It runs
`cargo build --release` from the repository root and fails with a clear message
if `cargo` is not on `$PATH`:

```sh
nvim --headless -u NONE -l build.lua
# or, from Neovim:
:luafile build.lua
```

The resulting binary is `target/release/laudit`.

### Nix flake

The project is also installable as a Nix flake (ticket `lc-um4w`), so users need
no Rust toolchain. From a checkout:

```sh
nix build .#laudit                           # build ./result/bin/laudit
nix run .#laudit -- --version                # run without installing
nix profile install .#laudit                 # install the binary
nix develop                                  # dev shell (Rust, tk, opencode)
```

Or directly from GitHub:

```sh
nix run github:<owner>/laudit -- --version   # run without installing
nix profile install github:<owner>/laudit    # install the binary
```

The flake exposes `packages.<system>.laudit` (and `default`), `apps.<system>.laudit`
(and `default`), `devShells.<system>.default`, `packages.<system>.laudit-nvim`
(the Lua plugin, installed under `$out/lua` and `$out/plugin`), and
`overlays.default`. It supports `x86_64-linux` and `aarch64-linux`, builds the
whole workspace with `rustPlatform.buildRustPackage`, and pins dependencies with
`cargoLock.lockFile = ./Cargo.lock`. The binary reports the crate version
(`laudit --version`). `devenv.nix` remains the development environment; the
flake is the packaging/installation path. `nix flake check` must pass.

## Tooling

- `.github/workflows/release.yml` uses `cross` to build the four targets on tag
  (`v*`), packages each binary, and writes a `.sha256` per artifact plus a
  combined `SHA256SUMS`.
- The workflow publishes the tarballs and checksums to GitHub Releases.
- `cargo-deny` and `cargo-audit` run in CI (`.github/workflows/ci.yml`).

## Binary resolution (Neovim plugin)

Order:

1. `setup({ binary = "..." })` explicit path.
2. `vim.fn.exepath('laudit')` (i.e. `$PATH`).
3. Optional auto-download (opt-in) of the matching release artifact into a
   plugin data dir, verified against the checksum.

Auto-download is off by default (`setup({ auto_download = false })`). The Lua
frontend exposes the opt-in seam but does not perform the download itself:
network I/O must happen in Rust (architecture invariant 2), and the Rust-side
installer is not implemented yet. Enabling `auto_download` therefore reports a
clear "auto-download is not available in the Lua frontend" error and leaves any
existing binary untouched. Install `laudit` from a release or the Nix flake, or
set `setup({ binary = ... })`, until the installer lands.

## Source fallback

`build.lua` runs `cargo build --release` for users who prefer to build locally.
It must fail with a clear message if a Rust toolchain is missing.

## Versioning

- `tool_version` is the crate version and is stamped into every artifact.
- `schema_version` is independent and bumps only on breaking artifact changes.
- A tagged release produces the four Linux artifacts with checksums.

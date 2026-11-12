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

Auto-download is off by default. When enabled, the plugin selects the artifact
for the current OS/architecture (`x86_64`/`aarch64` × `gnu`/`musl`), downloads
the tarball and its `.sha256` from the matching GitHub Release, verifies the
checksum, extracts `laudit` into the plugin data directory, and executes it from
there. A checksum mismatch aborts the install and leaves any existing binary
untouched.

## Source fallback

`build.lua` runs `cargo build --release` for users who prefer to build locally.
It must fail with a clear message if a Rust toolchain is missing.

## Versioning

- `tool_version` is the crate version and is stamped into every artifact.
- `schema_version` is independent and bumps only on breaking artifact changes.
- A tagged release produces the four Linux artifacts with checksums.

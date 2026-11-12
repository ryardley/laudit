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

## Versioning

- `tool_version` is the crate version and is stamped into every artifact.
- `schema_version` is independent and bumps only on breaking artifact changes.
- A tagged release produces the four Linux artifacts with checksums.

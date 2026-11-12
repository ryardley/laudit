#!/usr/bin/env bash
# Run the headless Neovim RPC client tests.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

nvim --headless -u NONE -l tests/nvim/rpc_test.lua
nvim --headless -u NONE -l tests/nvim/ui_test.lua

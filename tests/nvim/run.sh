#!/usr/bin/env bash
# Run the headless Neovim plugin tests (setup/health, RPC client, tour UI,
# commands, and error paths).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

nvim --headless -u NONE -l tests/nvim/setup_test.lua
nvim --headless -u NONE -l tests/nvim/rpc_test.lua
nvim --headless -u NONE -l tests/nvim/ui_test.lua
nvim --headless -u NONE -l tests/nvim/commands_test.lua
nvim --headless -u NONE -l tests/nvim/error_test.lua

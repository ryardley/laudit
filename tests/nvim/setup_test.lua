-- Headless test for setup{}, binary resolution, and health reporting.
--
-- Run from the repository root:
--   nvim --headless -u NONE -l tests/nvim/setup_test.lua
--
-- No engine process is started for the tour; the fake engine is only used as
-- the resolved binary and for its --version output. No network.

local source = debug.getinfo(1, "S").source
if source:sub(1, 1) == "@" then
  source = source:sub(2)
end
local root = vim.fn.fnamemodify(source, ":p:h:h:h")
vim.opt.runtimepath:prepend(root)

local laudit = require("laudit")
local config = require("laudit.config")
local health = require("laudit.health")
local fake = root .. "/tests/nvim/fake_engine.sh"

local failures = {}

local function check(condition, message)
  if not condition then
    failures[#failures + 1] = message
    io.stderr:write("FAIL: " .. message .. "\n")
  end
end

-- 1. setup{} works and returns the merged UI configuration.
config.reset()
local cfg = laudit.setup({})
check(type(cfg) == "table", "setup returns a table")
check(cfg.keymaps == true, "keymaps default to enabled")
check(cfg.split.side == "right", "split side defaults to right")
check(cfg.split.size == 40, "split size defaults to 40")

-- 2. An explicit binary path overrides $PATH and is reported as config-sourced.
config.reset()
laudit.setup({ binary = fake })
local resolved = laudit.resolve_binary()
check(resolved.found == true, "configured fake binary resolves")
check(resolved.source == "config", "configured binary source is config")
check(resolved.path == fake, "configured binary path is preserved")

-- 3. A missing explicit binary is authoritative and reports an error.
config.reset()
laudit.setup({ binary = "/definitely/not/a/laudit-binary" })
local missing = laudit.resolve_binary()
check(missing.found == false, "missing configured binary does not resolve")
check(#missing.errors > 0, "missing configured binary reports an error")

-- 4. Default keymaps install and can be disabled.
config.reset()
laudit.setup({ keymaps = true })
check(vim.fn.maparg("\\lt", "n") ~= "", "default <leader>lt keymap installed")
laudit.setup({ keymaps = false })
check(vim.fn.maparg("\\lt", "n") == "", "keymaps can be disabled")

-- 5. setup{} validates UI options.
local ok = pcall(laudit.setup, { keymaps = "yes" })
check(ok == false, "setup rejects a non-boolean keymaps option")
ok = pcall(laudit.setup, { split = { side = "middle" } })
check(ok == false, "setup rejects an invalid split side")

-- 6. health reports a found binary and its version.
config.reset()
laudit.setup({ binary = fake })
local report = laudit.health()
check(report.binary.found == true, "health reports the binary as found")
check(report.engine == "ok", "health reports the engine as ok")
check(
  type(report.version) == "string" and report.version:find("laudit", 1, true) ~= nil,
  "health reports the fake engine version: " .. tostring(report.version)
)

-- 7. health reports a missing binary without starting an engine.
config.reset()
laudit.setup({ binary = "/definitely/not/a/laudit-binary" })
local missing_report = laudit.health()
check(missing_report.binary.found == false, "health reports a missing binary")
check(missing_report.engine == "unavailable", "health reports engine unavailable")
local lines = health.format(missing_report)
local text = table.concat(lines, "\n")
check(text:find("binary: missing", 1, true) ~= nil, "health format shows missing binary")
check(text:find("hint:", 1, true) ~= nil, "health format shows an install hint")

-- 8. :Laudit health dispatches through the command surface.
config.reset()
laudit.setup({ binary = fake })
local echoed
local original_echo = vim.api.nvim_echo
vim.api.nvim_echo = function(chunks)
  echoed = chunks
end
laudit.command({ fargs = { "health" } })
vim.api.nvim_echo = original_echo
check(echoed ~= nil, "health command echoes a report")

config.reset()
laudit.setup({ keymaps = false })

if #failures > 0 then
  io.stderr:write(string.format("%d failure(s)\n", #failures))
  os.exit(1)
end

print("setup_test: ok")
os.exit(0)

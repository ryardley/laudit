-- Headless test for the :Laudit command surface, keymaps, outline, return, and
-- resume.
--
-- Run from the repository root:
--   nvim --headless -u NONE -l tests/nvim/commands_test.lua
--
-- The test starts tests/nvim/fake_engine.sh and exercises the commands exposed
-- by lua/laudit/init.lua plus the outline/return/resume behaviour in
-- lua/laudit/ui.lua. No network.

local source = debug.getinfo(1, "S").source
if source:sub(1, 1) == "@" then
  source = source:sub(2)
end
local root = vim.fn.fnamemodify(source, ":p:h:h:h")
vim.opt.runtimepath:prepend(root)

local laudit = require("laudit")
local rpc = require("laudit.rpc")
local ui = require("laudit.ui")
local fake = root .. "/tests/nvim/fake_engine.sh"

local failures = {}

local function check(condition, message)
  if not condition then
    failures[#failures + 1] = message
    io.stderr:write("FAIL: " .. message .. "\n")
  end
end

local function wait_for(predicate, timeout, message)
  local ok = vim.wait(timeout or 2000, predicate, 10)
  if not ok then
    failures[#failures + 1] = message or "timed out"
    io.stderr:write("FAIL: " .. (message or "timed out") .. "\n")
  end
  return ok
end

local function write_file(path, lines)
  vim.fn.mkdir(vim.fn.fnamemodify(path, ":h"), "p")
  vim.fn.writefile(lines, path)
end

local function current_id()
  local stop = ui.current_stop()
  return stop and stop.id or nil
end

local function read_state(path)
  local file = io.open(path, "r")
  if not file then
    return nil
  end
  local content = file:read("*a")
  file:close()
  return content
end

-- 1. Every documented command exists.
local expected = {
  "clear",
  "explain",
  "health",
  "model",
  "next",
  "outline",
  "prev",
  "resume",
  "return",
  "status",
  "stop",
  "tour",
}
local names = laudit.command_names()
for _, name in ipairs(expected) do
  check(vim.tbl_contains(names, name), "command exists: " .. name)
end

-- 2. Default keymaps install and can be disabled.
laudit.setup({ keymaps = true })
check(vim.fn.maparg("\\lt", "n") ~= "", "default <leader>lt keymap installed")
laudit.setup({ keymaps = false })
check(vim.fn.maparg("\\lt", "n") == "", "keymaps can be disabled")
laudit.setup({ keymaps = true })

-- 3. Start a tour against the fake engine.
local repo = vim.fn.tempname()
write_file(repo .. "/src/main.rs", { "fn main() {", '    println!("hi");', "}" })
write_file(repo .. "/src/lib.rs", { "pub fn helper() {}" })

local state_file = vim.fn.tempname()
rpc.reset()
rpc.setup({
  binary = fake,
  args = {},
  env = { FAKE_ENGINE_STATE = state_file },
  restart = { max = 0, delay = 10 },
})
ui.reset()

ui.start({ repo = repo, offline = false })
wait_for(function()
  return current_id() ~= nil
end, 3000, "tour start")
check(current_id() == "stop-1", "first stop is stop-1")

-- 4. next/prev advance.
ui.next()
wait_for(function()
  return current_id() == "stop-2"
end, 2000, "next advances to stop-2")
ui.prev()
wait_for(function()
  return current_id() == "stop-1"
end, 2000, "prev returns to stop-1")

-- 5. return restores the current stop after wandering.
local state = ui.state()
local code_win = state.code_win
check(code_win ~= nil and vim.api.nvim_win_is_valid(code_win), "code window exists")
local scratch = vim.api.nvim_create_buf(false, true)
vim.api.nvim_win_set_buf(code_win, scratch)
vim.api.nvim_win_set_cursor(code_win, { 1, 0 })

ui.return_to_current()
local restored_buf = vim.api.nvim_win_get_buf(code_win)
local restored_name = vim.api.nvim_buf_get_name(restored_buf)
check(restored_name:match("src/main%.rs$") ~= nil, "return restores the stop buffer")
local marks = vim.api.nvim_buf_get_extmarks(restored_buf, ui.namespace, 0, -1, { details = true })
check(#marks > 0, "return re-highlights the current stop")

-- 6. outline jumps to any stop.
ui.next()
wait_for(function()
  return current_id() == "stop-2"
end, 2000, "advance to stop-2 before outline")
local original_select = vim.ui.select
vim.ui.select = function(items, _, on_choice)
  on_choice(items[1], 1)
end
ui.outline()
vim.ui.select = original_select
wait_for(function()
  return current_id() == "stop-1"
end, 2000, "outline jumps to stop-1")

-- 7. resume restores the saved position across a UI restart.
ui.next()
wait_for(function()
  return current_id() == "stop-2"
end, 2000, "advance to stop-2 before resume")
wait_for(function()
  local content = read_state(state_file)
  return content ~= nil and content:match("0 1") ~= nil
end, 2000, "position persisted to the engine")

ui.reset()
ui.resume({ repo = repo })
wait_for(function()
  return current_id() == "stop-2"
end, 3000, "resume restores stop-2")

-- 8. status reports through the engine.
local echoed
local original_echo = vim.api.nvim_echo
vim.api.nvim_echo = function(chunks)
  echoed = chunks
end
ui.status({ repo = repo })
wait_for(function()
  return echoed ~= nil
end, 2000, "status reports")
vim.api.nvim_echo = original_echo
check(echoed ~= nil, "status command runs")

-- 9. model opens the model artifact.
write_file(repo .. "/.laudit/model.json", { "{}" })
ui.model({ repo = repo })
local model_buf
for _, buf in ipairs(vim.api.nvim_list_bufs()) do
  if vim.api.nvim_buf_get_name(buf):match("laudit://model$") then
    model_buf = buf
    break
  end
end
check(model_buf ~= nil, "model command opens the model artifact")

-- 10. clear removes generated artifacts.
local original_notify = vim.notify
vim.notify = function() end
ui.clear({ repo = repo, force = true })
vim.notify = original_notify
check(vim.fn.filereadable(repo .. "/.laudit/model.json") == 0, "clear removes model.json")

ui.stop()
rpc.reset()
vim.fn.delete(repo, "rf")
vim.fn.delete(state_file)

if #failures > 0 then
  io.stderr:write(string.format("%d failure(s)\n", #failures))
  os.exit(1)
end

print("commands_test: ok")
os.exit(0)

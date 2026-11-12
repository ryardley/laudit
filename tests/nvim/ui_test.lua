-- Headless test for lua/laudit/ui.lua.
--
-- Run from the repository root:
--   nvim --headless -u NONE -l tests/nvim/ui_test.lua
--
-- The test starts tests/nvim/fake_engine.sh, begins a tour, and checks that the
-- stop file is opened, the range is highlighted with an extmark, the persistent
-- explanation split shows the progress header, advancing auto-follows to the
-- next buffer/range, and streamed chunks append to the split. No network.

local source = debug.getinfo(1, "S").source
if source:sub(1, 1) == "@" then
  source = source:sub(2)
end
local root = vim.fn.fnamemodify(source, ":p:h:h:h")
vim.opt.runtimepath:prepend(root)

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

local function buffer_text(buf)
  return table.concat(vim.api.nvim_buf_get_lines(buf, 0, -1, false), "\n")
end

local repo = vim.fn.tempname()
write_file(repo .. "/src/main.rs", { "fn main() {", '    println!("hi");', "}" })
write_file(repo .. "/src/lib.rs", { "pub fn helper() {}" })

rpc.reset()
rpc.setup({
  binary = fake,
  args = {},
  restart = { max = 0, delay = 10 },
})
ui.reset()

ui.start({ repo = repo, offline = false })

wait_for(function()
  return ui.current_stop() ~= nil
end, 3000, "tour start")

local stop = ui.current_stop()
check(stop ~= nil, "current stop exists")
check(stop ~= nil and stop.id == "stop-1", "first stop is stop-1")

local state = ui.state()
check(state.code_win ~= nil and vim.api.nvim_win_is_valid(state.code_win), "code window exists")
check(
  state.split_win ~= nil and vim.api.nvim_win_is_valid(state.split_win),
  "explanation split exists"
)
if state.split_win and vim.api.nvim_win_is_valid(state.split_win) then
  check(
    vim.api.nvim_win_get_config(state.split_win).relative == "",
    "explanation split is not a float"
  )
end

local code_buf = vim.api.nvim_win_get_buf(state.code_win)
local code_name = vim.api.nvim_buf_get_name(code_buf)
check(code_name:match("src/main%.rs$") ~= nil, "first stop file opened: " .. code_name)

local cursor = vim.api.nvim_win_get_cursor(state.code_win)
check(cursor[1] == 1, "cursor moved to the first stop start line")

local marks = vim.api.nvim_buf_get_extmarks(code_buf, ui.namespace, 0, -1, { details = true })
check(#marks > 0, "extmark set on the first stop")
if #marks > 0 then
  check(marks[1][2] == 0, "extmark starts at the first stop row")
end

local split_buf = vim.api.nvim_win_get_buf(state.split_win)
wait_for(function()
  local header = vim.api.nvim_buf_get_lines(split_buf, 0, 1, false)[1] or ""
  return header:find("Chapter 1/1", 1, true) ~= nil
    and header:find("Stop 1/2", 1, true) ~= nil
end, 3000, "split header shows chapter and stop progress")

local header = vim.api.nvim_buf_get_lines(split_buf, 0, 1, false)[1] or ""
check(header:find("Chapter 1/1", 1, true) ~= nil, "split header shows chapter progress: " .. header)
check(header:find("Stop 1/2", 1, true) ~= nil, "split header shows stop progress: " .. header)

wait_for(function()
  return buffer_text(split_buf):find("Hello world", 1, true) ~= nil
end, 3000, "streamed chunks append to the split")

ui.next()

wait_for(function()
  local current = ui.current_stop()
  return current ~= nil and current.id == "stop-2"
end, 2000, "advance to the second stop")

local next_state = ui.state()
local next_buf = vim.api.nvim_win_get_buf(next_state.code_win)
local next_name = vim.api.nvim_buf_get_name(next_buf)
check(next_name:match("src/lib%.rs$") ~= nil, "second stop file opened: " .. next_name)

local next_cursor = vim.api.nvim_win_get_cursor(next_state.code_win)
check(next_cursor[1] == 1, "cursor moved to the second stop start line")

local next_marks = vim.api.nvim_buf_get_extmarks(next_buf, ui.namespace, 0, -1, { details = true })
check(#next_marks > 0, "extmark set on the second stop")

local next_header = vim.api.nvim_buf_get_lines(
  vim.api.nvim_win_get_buf(next_state.split_win),
  0,
  1,
  false
)[1] or ""
check(
  next_header:find("Stop 2/2", 1, true) ~= nil,
  "split header advanced: " .. next_header
)

ui.stop()
rpc.reset()
vim.fn.delete(repo, "rf")

if #failures > 0 then
  io.stderr:write(string.format("%d failure(s)\n", #failures))
  os.exit(1)
end

print("ui_test: ok")
os.exit(0)

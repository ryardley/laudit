-- Headless test for engine error paths.
--
-- Run from the repository root:
--   nvim --headless -u NONE -l tests/nvim/error_test.lua
--
-- The fake engine is told to fail build_tour, explain_stop, get_outline, and to
-- emit invalid JSON. The tests assert the Lua client surfaces each failure
-- without crashing. No network.

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

local function current_id()
  local stop = ui.current_stop()
  return stop and stop.id or nil
end

local function contains_notification(notifications, needle)
  for _, notification in ipairs(notifications) do
    if notification.message:find(needle, 1, true) ~= nil then
      return true
    end
  end
  return false
end

local repo = vim.fn.tempname()
write_file(repo .. "/src/main.rs", { "fn main() {", '    println!("hi");', "}" })
write_file(repo .. "/src/lib.rs", { "pub fn helper() {}" })
local state_file = vim.fn.tempname()

-- 1. build_tour failure is surfaced and leaves no current stop.
local notifications = {}
local original_notify = vim.notify
vim.notify = function(message)
  notifications[#notifications + 1] = { message = message }
end

rpc.reset()
rpc.setup({
  binary = fake,
  args = {},
  env = {
    FAKE_ENGINE_STATE = state_file,
    FAKE_ENGINE_BUILD_ERROR = "1",
  },
  restart = { max = 0, delay = 10 },
})
ui.reset()
ui.start({ repo = repo })

wait_for(function()
  return contains_notification(notifications, "no model configured")
end, 3000, "build_tour error notification")
check(
  contains_notification(notifications, "no model configured"),
  "build_tour error is surfaced to the user"
)
check(ui.current_stop() == nil, "no stop is shown after a build_tour error")

ui.stop()
rpc.reset()

-- 2. explain_stop failure is surfaced for the current stop.
notifications = {}
rpc.setup({
  binary = fake,
  args = {},
  env = {
    FAKE_ENGINE_STATE = state_file,
    FAKE_ENGINE_EXPLAIN_ERROR = "1",
  },
  restart = { max = 0, delay = 10 },
})
ui.reset()
ui.start({ repo = repo })
wait_for(function()
  return current_id() == "stop-1" and #ui.state().stops >= 2
end, 3000, "tour start before explain error")
ui.next()
wait_for(function()
  return current_id() == "stop-2"
end, 2000, "advance before explain error")
ui.explain()

wait_for(function()
  return contains_notification(notifications, "LLM failure")
end, 3000, "explain_stop error notification")
check(
  contains_notification(notifications, "LLM failure"),
  "explain_stop error is surfaced to the user"
)

ui.stop()
rpc.reset()

-- 3. get_outline failure is returned as a JSON-RPC error.
rpc.setup({
  binary = fake,
  args = {},
  env = { FAKE_ENGINE_OUTLINE_ERROR = "1" },
  restart = { max = 0, delay = 10 },
})
local outline_error
rpc.request("get_outline", { tour_id = "tour-fake" }, function(err)
  outline_error = err
end)
wait_for(function()
  return outline_error ~= nil
end, 2000, "get_outline error response")
check(
  outline_error ~= nil and outline_error.code == -32002,
  "get_outline returns the tour-not-found error code"
)
rpc.reset()

-- 4. Invalid JSON is reported without losing the following response.
notifications = {}
rpc.setup({
  binary = fake,
  args = {},
  env = { FAKE_ENGINE_INVALID_JSON = "1" },
  restart = { max = 0, delay = 10 },
})
local pong
rpc.request("ping", {}, function(_, result)
  pong = result
end)
wait_for(function()
  return pong ~= nil and contains_notification(notifications, "invalid JSON")
end, 2000, "invalid JSON warning and ping response")
check(pong ~= nil and pong.pong == true, "ping still succeeds after invalid JSON")
check(
  contains_notification(notifications, "invalid JSON"),
  "invalid JSON is reported as a warning"
)

vim.notify = original_notify
rpc.reset()
ui.reset()
vim.fn.delete(repo, "rf")
vim.fn.delete(state_file)

if #failures > 0 then
  io.stderr:write(string.format("%d failure(s)\n", #failures))
  os.exit(1)
end

print("error_test: ok")
os.exit(0)

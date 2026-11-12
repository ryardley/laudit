-- Headless test for lua/laudit/rpc.lua.
--
-- Run from the repository root:
--   nvim --headless -u NONE -l tests/nvim/rpc_test.lua
--
-- The test starts tests/nvim/fake_engine.sh, exercises request/response,
-- notification dispatch, and bounded restart after a crash. No network.

local source = debug.getinfo(1, "S").source
if source:sub(1, 1) == "@" then
  source = source:sub(2)
end
local root = vim.fn.fnamemodify(source, ":p:h:h:h")
vim.opt.runtimepath:prepend(root)

local rpc = require("laudit.rpc")
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

local function reset()
  rpc.reset()
  rpc.setup({
    binary = fake,
    args = {},
    restart = { max = 3, delay = 20 },
  })
end

-- 1. request/response
reset()
local ping_result
local ping_error
rpc.request("ping", {}, function(err, result)
  ping_error = err
  ping_result = result
end)
wait_for(function()
  return ping_result ~= nil or ping_error ~= nil
end, 2000, "ping response")
check(ping_error == nil, "ping returned an error: " .. vim.inspect(ping_error))
check(type(ping_result) == "table" and ping_result.pong == true, "ping result has pong=true")
check(ping_result and ping_result.version == "fake", "ping result has fake version")

-- 2. notification dispatch
local progress
local notify_result
rpc.on("progress", function(params)
  progress = params
end)
rpc.request("notify", {}, function(_, result)
  notify_result = result
end)
wait_for(function()
  return progress ~= nil and notify_result ~= nil
end, 2000, "notification dispatch")
check(progress ~= nil and progress.stage == "recon", "progress notification dispatched")
check(notify_result ~= nil and notify_result.ok == true, "notify request completed")

-- 3. error responses are correlated to the request
local unknown_error
rpc.request("does_not_exist", {}, function(err)
  unknown_error = err
end)
wait_for(function()
  return unknown_error ~= nil
end, 2000, "error response")
check(
  unknown_error ~= nil and unknown_error.code == -32601,
  "unknown method returns method-not-found error"
)

-- 4. crash triggers restart
local before = rpc.restart_count()
rpc.request("crash", {}, function() end)
wait_for(function()
  return rpc.restart_count() > before
end, 3000, "restart after crash")
wait_for(function()
  return rpc.is_running()
end, 3000, "engine running after restart")
local pong_after
rpc.request("ping", {}, function(_, result)
  pong_after = result
end)
wait_for(function()
  return pong_after ~= nil
end, 2000, "ping after restart")
check(pong_after ~= nil and pong_after.pong == true, "engine responds after restart")

-- 5. restart attempts are bounded
rpc.reset()
rpc.setup({
  binary = fake,
  args = {},
  env = { FAKE_ENGINE_CRASH_ON_START = "1" },
  restart = { max = 2, delay = 10 },
})
rpc.start()
wait_for(function()
  return rpc.restart_count() >= 2
end, 3000, "bounded restart attempts")
wait_for(function()
  return not rpc.is_running()
end, 3000, "engine stops after restart limit")
check(rpc.restart_count() == 2, "restart attempts are bounded to max")

rpc.reset()

if #failures > 0 then
  io.stderr:write(string.format("%d failure(s)\n", #failures))
  os.exit(1)
end

print("rpc_test: ok")
os.exit(0)

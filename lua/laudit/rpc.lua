-- laudit JSON-RPC 2.0 client over jobstart (D6, D15).
--
-- The engine is a long-lived child process speaking newline-delimited JSON-RPC
-- on stdio. This module owns the child, correlates request ids, dispatches
-- notifications, and restarts the child a bounded number of times after a
-- crash. It is UI-agnostic: callers use request()/on().

local M = {}

local defaults = {
  -- Explicit engine binary. When nil, laudit.config resolves it.
  binary = nil,
  -- Arguments passed to the engine. `laudit serve` is the RPC entry point.
  args = { "serve" },
  cwd = nil,
  env = nil,
  -- Bounded auto-restart after an unexpected exit (D15).
  restart = { max = 3, delay = 100 },
  -- Optional stderr sink; stderr is never part of the protocol channel.
  on_stderr = nil,
}

local config = vim.deepcopy(defaults)

local state = {
  job = nil,
  next_id = 0,
  pending = {},
  handlers = {},
  exit_handlers = {},
  buffer = "",
  stderr = "",
  stopping = false,
  restart_attempts = 0,
  restarts = 0,
  restart_timer = nil,
}

local function restart_config()
  return config.restart or defaults.restart
end

local function merge_config(opts)
  opts = opts or {}
  for key, value in pairs(opts) do
    if key == "restart" and type(value) == "table" then
      config.restart = vim.tbl_extend("force", restart_config(), value)
    else
      config[key] = value
    end
  end
end

local function cancel_restart_timer()
  local timer = state.restart_timer
  state.restart_timer = nil
  if timer then
    pcall(function()
      timer:stop()
    end)
    pcall(function()
      timer:close()
    end)
  end
end

local function reject_pending(error)
  local pending = state.pending
  state.pending = {}
  for _, entry in pairs(pending) do
    if entry.cb then
      local cb = entry.cb
      vim.schedule(function()
        cb(error, nil)
      end)
    end
  end
end

local function notify_error(message)
  vim.schedule(function()
    vim.notify("laudit: " .. message, vim.log.levels.ERROR)
  end)
  for _, handler in ipairs(state.exit_handlers) do
    local h = handler
    vim.schedule(function()
      h(message)
    end)
  end
end

local function handle_response(message)
  local entry = state.pending[message.id]
  if not entry then
    return
  end
  state.pending[message.id] = nil
  -- Any response proves the engine is alive; reset the crash budget.
  state.restart_attempts = 0

  local cb = entry.cb
  if not cb then
    return
  end

  if message.error ~= nil then
    vim.schedule(function()
      cb(message.error, nil)
    end)
  else
    vim.schedule(function()
      cb(nil, message.result)
    end)
  end
end

local function handle_notification(message)
  local handlers = state.handlers[message.method]
  if not handlers or #handlers == 0 then
    return
  end

  local params = message.params
  if params == nil then
    params = vim.empty_dict()
  end

  local snapshot = {}
  for index, handler in ipairs(handlers) do
    snapshot[index] = handler
  end
  for _, handler in ipairs(snapshot) do
    local h = handler
    vim.schedule(function()
      h(params, message)
    end)
  end
end

local function handle_line(line)
  line = line:gsub("\r$", "")
  if line == "" then
    return
  end

  local ok, message = pcall(vim.json.decode, line)
  if not ok or type(message) ~= "table" then
    vim.schedule(function()
      vim.notify("laudit: invalid JSON from engine: " .. line, vim.log.levels.WARN)
    end)
    return
  end

  if message.method ~= nil then
    handle_notification(message)
  elseif message.id ~= nil then
    handle_response(message)
  end
end

local function on_stdout(job_id, data)
  if state.job ~= job_id or not data then
    return
  end

  -- jobstart splits on newlines; the first chunk continues any partial line
  -- left over from the previous callback.
  for index, chunk in ipairs(data) do
    if index == 1 then
      state.buffer = state.buffer .. chunk
    else
      handle_line(state.buffer)
      state.buffer = chunk
    end
  end
end

local function on_stderr(job_id, data)
  if state.job ~= job_id or not data then
    return
  end

  for _, chunk in ipairs(data) do
    state.stderr = state.stderr .. chunk .. "\n"
  end

  if type(config.on_stderr) == "function" then
    config.on_stderr(data)
  end
end

local schedule_restart
local attempt_restart
local vimleave_installed = false

local function install_vimleave()
  if vimleave_installed then
    return
  end
  vimleave_installed = true

  local group = vim.api.nvim_create_augroup("LauditRpc", { clear = true })
  vim.api.nvim_create_autocmd("VimLeavePre", {
    group = group,
    callback = function()
      M.stop()
    end,
  })
end

local function on_exit(job_id, code, event)
  if state.job ~= job_id then
    return
  end

  state.job = nil
  state.buffer = ""
  reject_pending({
    code = -32603,
    message = string.format(
      "engine exited (code %s, event %s)",
      tostring(code),
      tostring(event)
    ),
  })

  if state.stopping then
    return
  end

  if state.restart_attempts >= restart_config().max then
    notify_error("engine exited and restart limit reached")
    return
  end

  schedule_restart()
end

schedule_restart = function()
  if state.restart_timer then
    return
  end

  local delay = restart_config().delay * (2 ^ state.restart_attempts)
  state.restart_timer = vim.defer_fn(attempt_restart, delay)
end

attempt_restart = function()
  state.restart_timer = nil
  if state.stopping or M.is_running() then
    return
  end

  if state.restart_attempts >= restart_config().max then
    notify_error("engine restart limit reached")
    return
  end

  state.restart_attempts = state.restart_attempts + 1
  state.restarts = state.restarts + 1

  local ok, err = M.start()
  if not ok then
    if state.restart_attempts < restart_config().max then
      schedule_restart()
    else
      notify_error("engine restart failed: " .. tostring(err and err.message or err))
    end
  end
end

--- Merge client options. Only transport options live here; UI options belong
--- to laudit.config.
---@param opts table|nil
---@return table
function M.setup(opts)
  merge_config(opts)
  return vim.deepcopy(config)
end

---@return table
function M.config()
  return vim.deepcopy(config)
end

---@return boolean
function M.is_running()
  if not state.job then
    return false
  end

  local ok, result = pcall(vim.fn.jobwait, { state.job }, 0)
  if not ok or type(result) ~= "table" then
    return false
  end
  return result[1] == -1
end

--- Start the engine if it is not already running.
---@param opts table|nil
---@return boolean|nil ok
---@return table|nil error
function M.start(opts)
  merge_config(opts)

  if M.is_running() then
    return true
  end

  if state.job then
    -- The process is gone but on_exit has not run yet. Clear it so the stale
    -- callback is ignored, and fail any requests that were in flight.
    state.job = nil
    reject_pending({ code = -32603, message = "engine exited" })
  end

  cancel_restart_timer()
  state.stopping = false
  state.buffer = ""
  state.stderr = ""

  local binary = config.binary
  if binary == nil or binary == "" then
    local resolved = require("laudit.config").resolve_binary()
    if not resolved.found then
      local reason = resolved.errors and resolved.errors[1] or "laudit binary not found"
      return nil, { code = -32603, message = reason }
    end
    binary = resolved.path
  end

  local command = { binary }
  for _, arg in ipairs(config.args or {}) do
    command[#command + 1] = arg
  end

  local job = vim.fn.jobstart(command, {
    stdin = "pipe",
    stdout_buffered = false,
    stderr_buffered = false,
    cwd = config.cwd,
    env = config.env,
    on_stdout = on_stdout,
    on_stderr = on_stderr,
    on_exit = on_exit,
  })

  if job <= 0 then
    return nil, {
      code = -32603,
      message = "failed to start laudit engine: " .. tostring(binary),
    }
  end

  state.job = job
  install_vimleave()
  return true
end

--- Ensure the engine is running, starting it lazily if needed.
---@return boolean|nil ok
---@return table|nil error
function M.ensure_started()
  if M.is_running() then
    return true
  end
  return M.start()
end

--- Send a JSON-RPC request. The callback receives (error, result); error is a
--- JSON-RPC error object or a transport error with a standard code.
---@param method string
---@param params table|nil
---@param cb fun(error: table|nil, result: any)|nil
---@return integer|nil id
---@return table|nil error
function M.request(method, params, cb)
  if type(method) ~= "string" or method == "" then
    error("laudit.rpc.request: method must be a non-empty string", 2)
  end
  if cb ~= nil and type(cb) ~= "function" then
    error("laudit.rpc.request: cb must be a function or nil", 2)
  end

  local ok, start_error = M.ensure_started()
  if not ok then
    if cb then
      vim.schedule(function()
        cb(start_error, nil)
      end)
    end
    return nil, start_error
  end

  state.next_id = state.next_id + 1
  local id = state.next_id
  local message = {
    jsonrpc = "2.0",
    id = id,
    method = method,
    params = params == nil and vim.empty_dict() or params,
  }

  local encoded_ok, encoded = pcall(vim.json.encode, message)
  if not encoded_ok then
    local encode_error = {
      code = -32603,
      message = "failed to encode request: " .. tostring(encoded),
    }
    if cb then
      vim.schedule(function()
        cb(encode_error, nil)
      end)
    end
    return nil, encode_error
  end

  state.pending[id] = { cb = cb, method = method }
  local send_ok, sent = pcall(vim.fn.chansend, state.job, encoded .. "\n")
  if not send_ok or sent == 0 then
    state.pending[id] = nil
    local send_error = { code = -32603, message = "failed to send request to engine" }
    if cb then
      vim.schedule(function()
        cb(send_error, nil)
      end)
    end
    return nil, send_error
  end

  return id
end

--- Register a notification handler. Returns an unsubscribe function.
---@param notification string
---@param handler fun(params: table, message: table)
---@return fun()
function M.on(notification, handler)
  if type(notification) ~= "string" or notification == "" then
    error("laudit.rpc.on: notification must be a non-empty string", 2)
  end
  if type(handler) ~= "function" then
    error("laudit.rpc.on: handler must be a function", 2)
  end

  local handlers = state.handlers[notification]
  if not handlers then
    handlers = {}
    state.handlers[notification] = handlers
  end
  handlers[#handlers + 1] = handler

  return function()
    M.off(notification, handler)
  end
end

--- Remove a notification handler.
---@param notification string
---@param handler fun(params: table, message: table)
function M.off(notification, handler)
  local handlers = state.handlers[notification]
  if not handlers then
    return
  end

  for index = #handlers, 1, -1 do
    if handlers[index] == handler then
      table.remove(handlers, index)
    end
  end
  if #handlers == 0 then
    state.handlers[notification] = nil
  end
end

--- Register a handler for unexpected engine exits. Returns an unsubscribe
--- function.
---@param handler fun(message: string)
---@return fun()
function M.on_exit(handler)
  if type(handler) ~= "function" then
    error("laudit.rpc.on_exit: handler must be a function", 2)
  end

  state.exit_handlers[#state.exit_handlers + 1] = handler
  return function()
    for index = #state.exit_handlers, 1, -1 do
      if state.exit_handlers[index] == handler then
        table.remove(state.exit_handlers, index)
      end
    end
  end
end

--- Stop the engine and cancel any pending restart. Pending requests fail with
--- the JSON-RPC "cancelled" code.
function M.stop()
  state.stopping = true
  cancel_restart_timer()

  local job = state.job
  state.job = nil
  state.buffer = ""
  reject_pending({ code = -32004, message = "engine stopped" })

  if job and job > 0 then
    pcall(vim.fn.jobstop, job)
  end
end

--- Number of automatic restarts performed since the last reset.
---@return integer
function M.restart_count()
  return state.restarts
end

--- Number of requests awaiting a response.
---@return integer
function M.pending_count()
  local count = 0
  for _ in pairs(state.pending) do
    count = count + 1
  end
  return count
end

--- Last stderr captured from the engine.
---@return string
function M.stderr()
  return state.stderr
end

--- Stop the engine and clear all client state. Intended for tests.
function M.reset()
  M.stop()
  state.stopping = false
  state.next_id = 0
  state.pending = {}
  state.handlers = {}
  state.exit_handlers = {}
  state.buffer = ""
  state.stderr = ""
  state.restart_attempts = 0
  state.restarts = 0
  config = vim.deepcopy(defaults)
end

return M

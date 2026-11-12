-- laudit configuration and engine binary resolution.
--
-- The Neovim setup{} table holds UI options only. Engine configuration lives in
-- ~/.config/laudit/config.toml and <repo>/.laudit/config.toml (D17).

local M = {}

local defaults = {
  -- Explicit path to the laudit binary. When nil, $PATH is consulted.
  binary = nil,
  -- Explanation split geometry.
  split = { side = "right", size = 40 },
  -- Install the default <leader>l keymaps.
  keymaps = true,
  -- Opt-in auto-download. Off by default; network I/O is handled by the Rust
  -- engine (architecture invariant 2).
  auto_download = false,
}

local current = vim.deepcopy(defaults)

local function validate(opts)
  if opts.binary ~= nil and type(opts.binary) ~= "string" then
    error("laudit.setup: binary must be a string or nil", 3)
  end
  if opts.keymaps ~= nil and type(opts.keymaps) ~= "boolean" then
    error("laudit.setup: keymaps must be a boolean", 3)
  end
  if
    opts.auto_download ~= nil
    and type(opts.auto_download) ~= "boolean"
    and type(opts.auto_download) ~= "table"
  then
    error("laudit.setup: auto_download must be a boolean or table", 3)
  end
  if opts.split ~= nil then
    if type(opts.split) ~= "table" then
      error("laudit.setup: split must be a table", 3)
    end
    if opts.split.side ~= nil and opts.split.side ~= "left" and opts.split.side ~= "right" then
      error("laudit.setup: split.side must be 'left' or 'right'", 3)
    end
    if opts.split.size ~= nil and (type(opts.split.size) ~= "number" or opts.split.size <= 0) then
      error("laudit.setup: split.size must be a positive number", 3)
    end
  end
end

--- Merge UI options into the current configuration.
---@param opts table|nil
---@return table
function M.setup(opts)
  opts = opts or {}
  validate(opts)
  current = vim.tbl_deep_extend("force", current, opts)
  return current
end

---@return table
function M.get()
  return current
end

---@return table
function M.defaults()
  return vim.deepcopy(defaults)
end

function M.reset()
  current = vim.deepcopy(defaults)
end

local function executable(path)
  return type(path) == "string" and path ~= "" and vim.fn.executable(path) == 1
end

local function resolve_explicit(value)
  if type(value) ~= "string" or value == "" then
    return nil
  end

  local expanded = vim.fn.expand(value)
  local looks_like_path = expanded:find("[/\\]") ~= nil
    or expanded:sub(1, 1) == "."
    or expanded:sub(1, 1) == "~"

  if looks_like_path then
    if executable(expanded) then
      return vim.fn.fnamemodify(expanded, ":p")
    end
    return nil
  end

  -- A bare command name: resolve it through $PATH.
  local from_path = vim.fn.exepath(expanded)
  if executable(from_path) then
    return from_path
  end

  if executable(expanded) then
    return vim.fn.fnamemodify(expanded, ":p")
  end

  return nil
end

--- Auto-download is opt-in. Network I/O must happen in Rust (architecture
--- invariant 2), so the Lua frontend only exposes the seam; a Rust-side
--- installer will fill this in. Until then, report a clear error.
---@return table
function M.download_binary()
  return {
    found = false,
    error = "auto-download is not available in the Lua frontend; install laudit or set setup({ binary = ... })",
  }
end

--- Resolve the engine binary: config path -> $PATH -> opt-in auto-download.
---@return table { found, path, source, errors }
function M.resolve_binary()
  local errors = {}
  local cfg = current

  if cfg.binary and cfg.binary ~= "" then
    local path = resolve_explicit(cfg.binary)
    if path then
      return { found = true, path = path, source = "config", errors = errors }
    end
    -- An explicit path is authoritative: do not silently fall back to $PATH.
    table.insert(errors, string.format("configured binary is not executable: %s", cfg.binary))
    return { found = false, path = nil, source = nil, errors = errors }
  end

  local path = vim.fn.exepath("laudit")
  if executable(path) then
    return { found = true, path = path, source = "path", errors = errors }
  end
  table.insert(errors, "laudit not found on $PATH")

  if cfg.auto_download then
    local ok, result = pcall(M.download_binary)
    if ok and result and result.found then
      return { found = true, path = result.path, source = "download", errors = errors }
    end
    local reason = ok and (result and result.error) or tostring(result)
    table.insert(errors, "auto-download failed: " .. tostring(reason or "unknown error"))
  end

  return { found = false, path = nil, source = nil, errors = errors }
end

return M

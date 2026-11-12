-- laudit Neovim plugin: a thin UI over the Rust engine (D5, D6).
--
-- This module owns setup{} and command dispatch. Tour logic lives in later
-- modules; for now only health is implemented.

local M = {}

local config = require("laudit.config")
local health = require("laudit.health")

local keymaps = {
  { "<leader>lt", "tour", "laudit: start tour" },
  { "<leader>lr", "resume", "laudit: resume tour" },
  { "<leader>ln", "next", "laudit: next stop" },
  { "<leader>lp", "prev", "laudit: previous stop" },
  { "<leader>lo", "outline", "laudit: outline" },
  { "<leader>lb", "return", "laudit: return to current stop" },
  { "<leader>ls", "stop", "laudit: stop tour" },
  { "<leader>lh", "health", "laudit: health" },
}

local installed_keymaps = {}

local function remove_keymaps()
  for lhs in pairs(installed_keymaps) do
    pcall(vim.keymap.del, "n", lhs)
  end
  installed_keymaps = {}
end

local function install_keymaps()
  for _, map in ipairs(keymaps) do
    local lhs, action, description = map[1], map[2], map[3]
    if not installed_keymaps[lhs] then
      vim.keymap.set("n", lhs, function()
        M.command({ fargs = { action } })
      end, { desc = description, silent = true })
      installed_keymaps[lhs] = true
    end
  end
end

--- Configure the plugin. Only UI options are accepted (D17).
---@param opts table|nil
---@return table
function M.setup(opts)
  local cfg = config.setup(opts)
  remove_keymaps()
  if cfg.keymaps then
    install_keymaps()
  end
  return cfg
end

---@return table
function M.config()
  return config.get()
end

---@return table
function M.resolve_binary()
  return config.resolve_binary()
end

---@return string|nil
function M.binary()
  local resolved = config.resolve_binary()
  if resolved.found then
    return resolved.path
  end
  return nil
end

---@return table
function M.health()
  return health.check()
end

local function not_implemented(name)
  vim.notify("laudit: " .. name .. " is not implemented yet", vim.log.levels.WARN)
end

local commands = {
  health = function()
    health.show()
  end,
  tour = function()
    not_implemented("tour")
  end,
  resume = function()
    not_implemented("resume")
  end,
  stop = function()
    not_implemented("stop")
  end,
  outline = function()
    not_implemented("outline")
  end,
  next = function()
    not_implemented("next")
  end,
  prev = function()
    not_implemented("prev")
  end,
  ["return"] = function()
    not_implemented("return")
  end,
  model = function()
    not_implemented("model")
  end,
  explain = function()
    not_implemented("explain")
  end,
  status = function()
    not_implemented("status")
  end,
  clear = function()
    not_implemented("clear")
  end,
}

---@return string[]
function M.command_names()
  local names = {}
  for name in pairs(commands) do
    names[#names + 1] = name
  end
  table.sort(names)
  return names
end

--- Dispatch a :Laudit subcommand.
---@param args table|string
function M.command(args)
  local name
  if type(args) == "string" then
    name = args:match("%S+")
  else
    local fargs = args and args.fargs or {}
    name = fargs[1]
    if not name and args and type(args.args) == "string" then
      name = args.args:match("%S+")
    end
  end

  if not name or name == "" then
    vim.api.nvim_echo({
      { "Usage: :Laudit <" .. table.concat(M.command_names(), "|") .. ">", "None" },
    }, true, {})
    return
  end

  local handler = commands[name]
  if not handler then
    vim.notify("laudit: unknown command: " .. name, vim.log.levels.ERROR)
    return
  end

  handler(args)
end

return M

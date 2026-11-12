-- :Laudit health — report binary, version, and engine status.

local M = {}

local function trim(value)
  return (value:gsub("^%s+", ""):gsub("%s+$", ""))
end

local function first_line(value)
  return value:match("^[^\r\n]*") or ""
end

--- Inspect the resolved binary and ask it for its version.
---@return table
function M.check()
  local config = require("laudit.config")
  local resolved = config.resolve_binary()
  local report = {
    binary = resolved,
    version = nil,
    engine = "unavailable",
    errors = resolved.errors or {},
  }

  if not resolved.found then
    return report
  end

  local ok, output = pcall(vim.fn.system, { resolved.path, "--version" })
  if not ok then
    table.insert(report.errors, "failed to run binary: " .. tostring(output))
    return report
  end

  local code = vim.v.shell_error
  if code ~= 0 then
    table.insert(report.errors, string.format("binary exited with status %d", code))
    return report
  end

  local line = trim(first_line(output))
  report.version = line ~= "" and line or "unknown"
  report.engine = "ok"
  return report
end

--- Render a health report as lines of text.
---@param report table
---@return string[]
function M.format(report)
  local lines = { "laudit health" }

  if report.binary and report.binary.found then
    table.insert(
      lines,
      string.format(
        "  binary: found %s (%s)",
        report.binary.path,
        report.binary.source or "unknown"
      )
    )
  else
    table.insert(lines, "  binary: missing")
  end

  table.insert(lines, "  version: " .. (report.version or "-"))
  table.insert(lines, "  engine: " .. (report.engine or "unavailable"))

  for _, err in ipairs(report.errors or {}) do
    table.insert(lines, "  note: " .. err)
  end

  if not (report.binary and report.binary.found) then
    table.insert(
      lines,
      '  hint: install laudit, put it on $PATH, or set setup({ binary = "..." })'
    )
  end

  return lines
end

--- Print the health report and return it.
---@return table
function M.show()
  local report = M.check()
  local lines = M.format(report)
  local chunks = {}
  for index, line in ipairs(lines) do
    chunks[#chunks + 1] = { line .. (index < #lines and "\n" or ""), "None" }
  end
  vim.api.nvim_echo(chunks, true, {})
  return report
end

return M

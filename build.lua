-- build.lua — source fallback for laudit (D8).
--
-- Builds the release binary with cargo for users who prefer to compile
-- locally instead of downloading a prebuilt release artifact.
--
-- Usage:
--   nvim --headless -u NONE -l build.lua
--   :luafile build.lua
--   lua build.lua
--
-- The script fails with a clear message when a Rust toolchain is missing.

local function script_dir()
  local source = debug.getinfo(1, "S").source or ""
  if source:sub(1, 1) == "@" then
    source = source:sub(2)
  end
  if type(vim) == "table" and vim.fn and vim.fn.fnamemodify then
    return vim.fn.fnamemodify(source, ":p:h")
  end
  local dir = source:match("^(.*)[/\\][^/\\]*$")
  return dir or "."
end

local function shell_quote(value)
  if type(vim) == "table" and vim.fn and vim.fn.shellescape then
    return vim.fn.shellescape(value)
  end
  return "'" .. value:gsub("'", "'\\''") .. "'"
end

local function executable(name)
  if type(vim) == "table" and vim.fn and vim.fn.executable then
    return vim.fn.executable(name) == 1
  end
  local ok = os.execute("command -v " .. name .. " >/dev/null 2>&1")
  return ok == true or ok == 0
end

local function fail(message)
  if type(vim) == "table" and vim.api and vim.api.nvim_err_writeln then
    vim.api.nvim_err_writeln(message .. "\n")
  else
    io.stderr:write(message .. "\n")
  end
  os.exit(1)
end

local function run(command)
  if type(vim) == "table" and vim.fn and vim.fn.system then
    local output = vim.fn.system(command)
    return vim.v.shell_error, output
  end
  local ok, _, code = os.execute(command)
  if type(ok) == "number" then
    return ok, ""
  end
  return (ok and 0 or (code or 1)), ""
end

local root = script_dir()

if not executable("cargo") then
  fail(
    "laudit: Rust toolchain not found (cargo is not on PATH).\n"
      .. "Install Rust from https://rustup.rs and re-run build.lua."
  )
end

local command = "cd " .. shell_quote(root) .. " && cargo build --release"
local status, output = run(command)

if output and output ~= "" then
  io.write(output)
end

if status ~= 0 then
  fail("laudit: cargo build --release failed (exit " .. tostring(status) .. ").")
end

print("laudit: built " .. root .. "/target/release/laudit")

-- laudit tour UI: real buffers, extmark highlights, persistent explanation split.
--
-- This module is a thin renderer over the engine's JSON-RPC tour lifecycle
-- (D6, D14). It owns no tour logic: the engine plans stops and streams prose;
-- the UI only opens files, highlights ranges, and appends chunks.

local M = {}

local rpc = require("laudit.rpc")
local config = require("laudit.config")

local ns = vim.api.nvim_create_namespace("laudit")

local state = {
  active = false,
  tour_id = nil,
  repo = nil,
  offline = false,
  chapters = {},
  stops = {},
  provisional_stops = {},
  stop_meta = {},
  chunks = {},
  current = 0,
  stop = nil,
  explanation = "",
  code_win = nil,
  split_win = nil,
  split_buf = nil,
  extmark_id = nil,
  extmark_buf = nil,
  outline_loaded = false,
  outline_requesting = false,
  outline_attempts = 0,
  outline_timer = nil,
  outline_attempt = nil,
  outline_cb = nil,
  build_failed = false,
  subscriptions = {},
  request_explain = false,
  explaining = {},
  pending_position = nil,
}

local function notify(message, level)
  vim.notify("laudit: " .. message, level or vim.log.levels.WARN)
end

local function valid_win(win)
  return win ~= nil and vim.api.nvim_win_is_valid(win)
end

local function valid_buf(buf)
  return buf ~= nil and vim.api.nvim_buf_is_valid(buf)
end

local function cancel_timer(timer)
  if not timer then
    return
  end
  pcall(function()
    timer:stop()
  end)
  pcall(function()
    timer:close()
  end)
end

local function ensure_highlight()
  pcall(vim.api.nvim_set_hl, 0, "LauditStop", { link = "Visual", default = true })
end

local function clear_extmark()
  if state.extmark_id and valid_buf(state.extmark_buf) then
    pcall(vim.api.nvim_buf_del_extmark, state.extmark_buf, ns, state.extmark_id)
  end
  state.extmark_id = nil
  state.extmark_buf = nil
end

local function close_split()
  if valid_win(state.split_win) then
    pcall(vim.api.nvim_win_close, state.split_win, true)
  end
  if valid_buf(state.split_buf) then
    pcall(vim.api.nvim_buf_delete, state.split_buf, { force = true })
  end
  state.split_win = nil
  state.split_buf = nil
end

local function unsubscribe()
  for _, off in ipairs(state.subscriptions) do
    pcall(off)
  end
  state.subscriptions = {}
end

local function reset_state()
  cancel_timer(state.outline_timer)
  state.outline_timer = nil
  state.outline_attempt = nil
  state.outline_cb = nil
  state.outline_loaded = false
  state.outline_requesting = false
  state.outline_attempts = 0
  state.build_failed = false
  state.active = false
  state.tour_id = nil
  state.repo = nil
  state.offline = false
  state.chapters = {}
  state.stops = {}
  state.provisional_stops = {}
  state.stop_meta = {}
  state.chunks = {}
  state.current = 0
  state.stop = nil
  state.explanation = ""
  state.code_win = nil
  state.request_explain = false
  state.explaining = {}
  state.pending_position = nil
end

local function ensure_code_win()
  if valid_win(state.code_win) then
    return state.code_win
  end

  local current = vim.api.nvim_get_current_win()
  if current ~= state.split_win then
    state.code_win = current
    return current
  end

  for _, win in ipairs(vim.api.nvim_list_wins()) do
    if win ~= state.split_win then
      state.code_win = win
      return win
    end
  end

  -- Only the explanation split remains: create a code window beside it.
  vim.cmd("vsplit")
  state.code_win = vim.api.nvim_get_current_win()
  return state.code_win
end

local function ensure_split()
  if valid_win(state.split_win) and valid_buf(state.split_buf) then
    return
  end

  local code_win = ensure_code_win()
  vim.api.nvim_set_current_win(code_win)

  local cfg = config.get()
  local split = cfg.split or {}
  local side = split.side or "right"
  local size = split.size or 40

  if side == "left" then
    vim.cmd("topleft vsplit")
  else
    vim.cmd("botright vsplit")
  end

  state.split_win = vim.api.nvim_get_current_win()
  pcall(vim.cmd, "vertical resize " .. tostring(size))

  local buf = vim.api.nvim_create_buf(false, true)
  vim.bo[buf].buftype = "nofile"
  vim.bo[buf].bufhidden = "hide"
  vim.bo[buf].swapfile = false
  vim.bo[buf].filetype = "markdown"
  pcall(vim.api.nvim_buf_set_name, buf, "laudit://explanation")
  vim.api.nvim_win_set_buf(state.split_win, buf)
  state.split_buf = buf

  vim.wo[state.split_win].wrap = true
  vim.wo[state.split_win].number = false
  vim.wo[state.split_win].relativenumber = false
  vim.wo[state.split_win].signcolumn = "no"
  vim.wo[state.split_win].foldcolumn = "0"

  vim.api.nvim_set_current_win(code_win)
end

local function split_lines()
  local lines = {}
  local stop = state.stop
  local chapter = stop and stop.chapter_index or 0
  local chapter_count = #state.chapters
  local stop_index = stop and stop.stop_index or 0
  local stop_count = stop and stop.stop_count or 0

  lines[#lines + 1] = string.format(
    "Chapter %d/%d · Stop %d/%d",
    chapter,
    chapter_count,
    stop_index,
    stop_count
  )

  if stop then
    if stop.title and stop.title ~= "" then
      lines[#lines + 1] = ""
      lines[#lines + 1] = "# " .. stop.title
    end
    if stop.why_here and stop.why_here ~= "" then
      lines[#lines + 1] = ""
      lines[#lines + 1] = "**Why:** " .. stop.why_here
    end
  end

  if state.explanation ~= "" then
    lines[#lines + 1] = ""
    for line in (state.explanation .. "\n"):gmatch("(.-)\n") do
      lines[#lines + 1] = line
    end
  end

  return lines
end

local function render_split()
  if not (valid_win(state.split_win) and valid_buf(state.split_buf)) then
    return
  end

  local lines = split_lines()
  vim.bo[state.split_buf].modifiable = true
  vim.api.nvim_buf_set_lines(state.split_buf, 0, -1, false, lines)
  vim.bo[state.split_buf].modifiable = false

  if valid_win(state.split_win) then
    local count = vim.api.nvim_buf_line_count(state.split_buf)
    pcall(vim.api.nvim_win_set_cursor, state.split_win, { count, 0 })
  end
end

local function open_stop(stop)
  local code_win = ensure_code_win()
  local path = stop.file
  if state.repo and not path:match("^/") then
    path = state.repo .. "/" .. path
  end

  local buf = vim.fn.bufadd(path)
  vim.fn.bufload(buf)
  vim.api.nvim_win_set_buf(code_win, buf)
  vim.api.nvim_set_current_win(code_win)

  local line_count = vim.api.nvim_buf_line_count(buf)
  local line = math.max(1, math.min(stop.start_line or 1, line_count))
  pcall(vim.api.nvim_win_set_cursor, code_win, { line, 0 })
  pcall(vim.cmd, "normal! zz")

  return buf
end

local function set_extmark(buf, stop)
  clear_extmark()

  local line_count = vim.api.nvim_buf_line_count(buf)
  local start_row = math.max(0, math.min((stop.start_line or 1) - 1, line_count - 1))
  local end_row = math.max(start_row + 1, math.min(stop.end_line or (start_row + 1), line_count))

  state.extmark_id = vim.api.nvim_buf_set_extmark(buf, ns, start_row, 0, {
    end_row = end_row,
    end_col = 0,
    hl_group = "LauditStop",
    hl_eol = true,
  })
  state.extmark_buf = buf
end

local function persist_position(stop)
  if not state.tour_id or not rpc.is_running() then
    return
  end
  rpc.request("set_position", {
    tour_id = state.tour_id,
    chapter = math.max(0, (stop.chapter_index or 1) - 1),
    stop = math.max(0, (stop.stop_index or 1) - 1),
  }, function() end)
end

local function show_current()
  local stop = state.stops[state.current]
  if not stop then
    return
  end

  state.stop = stop
  state.explanation = state.chunks[stop.id] or stop.explanation or ""
  persist_position(stop)

  ensure_split()
  local buf = open_stop(stop)
  set_extmark(buf, stop)
  render_split()

  if
    state.request_explain
    and not state.offline
    and state.explanation == ""
    and not state.explaining[stop.id]
  then
    state.explaining[stop.id] = true
    rpc.request("explain_stop", { tour_id = state.tour_id, stop_id = stop.id }, function(err)
      state.explaining[stop.id] = nil
      if err then
        notify("failed to explain stop: " .. (err.message or "unknown error"))
      end
    end)
  end
end

local function provisional_outline()
  local stops = {}
  for _, stop in ipairs(state.provisional_stops) do
    stops[#stops + 1] = {
      id = stop.id,
      title = stop.title,
      file = stop.file,
      start_line = stop.start_line,
      end_line = stop.end_line,
    }
  end
  return { { id = "provisional", title = "", stops = stops } }
end

local function flatten_outline(chapters)
  local stops = {}
  for chapter_index, chapter in ipairs(chapters or {}) do
    local chapter_stops = chapter.stops or {}
    for stop_index, stop in ipairs(chapter_stops) do
      local meta = state.stop_meta[stop.id] or {}
      stops[#stops + 1] = {
        id = stop.id,
        title = meta.title or stop.title or "",
        why_here = meta.why_here or stop.why_here or "",
        file = stop.file,
        start_line = stop.start_line,
        end_line = stop.end_line,
        chapter_index = chapter_index,
        chapter_title = chapter.title or "",
        chapter_count = #chapters,
        stop_index = stop_index,
        stop_count = #chapter_stops,
        explanation = meta.explanation or stop.explanation or "",
        status = stop.status,
      }
    end
  end
  return stops
end

local function flat_index_for_position(chapters, position)
  if not position then
    return nil
  end
  local flat = 0
  for chapter_index, chapter in ipairs(chapters or {}) do
    for stop_index, _ in ipairs(chapter.stops or {}) do
      if
        (chapter_index - 1) == (position.chapter or 0)
        and (stop_index - 1) == (position.stop or 0)
      then
        return flat + 1
      end
      flat = flat + 1
    end
  end
  return nil
end

local function show_scratch(name, lines, filetype)
  local buf = vim.api.nvim_create_buf(false, true)
  vim.bo[buf].buftype = "nofile"
  vim.bo[buf].bufhidden = "wipe"
  vim.bo[buf].swapfile = false
  vim.bo[buf].filetype = filetype or ""
  pcall(vim.api.nvim_buf_set_name, buf, name)
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  vim.bo[buf].modifiable = false
  vim.cmd("botright split")
  vim.api.nvim_win_set_buf(0, buf)
end

local function load_outline(tour_id, cb)
  state.outline_cb = cb
  state.outline_loaded = false
  state.outline_attempts = 0

  local function attempt()
    cancel_timer(state.outline_timer)
    state.outline_timer = nil

    if state.tour_id ~= tour_id or state.outline_loaded or state.build_failed then
      return
    end
    if state.outline_requesting then
      return
    end

    state.outline_requesting = true
    rpc.request("get_outline", { tour_id = tour_id }, function(err, result)
      state.outline_requesting = false
      if state.tour_id ~= tour_id or state.outline_loaded then
        return
      end

      if err then
        state.outline_attempts = state.outline_attempts + 1
        if state.outline_attempts < 1200 and not state.build_failed then
          local delay = state.outline_attempts < 20 and 100 or 500
          state.outline_timer = vim.defer_fn(attempt, delay)
        elseif #state.provisional_stops == 0 then
          notify("failed to load tour outline: " .. (err.message or "unknown error"))
        end
        return
      end

      local previous_id = state.stop and state.stop.id
      state.outline_loaded = true
      state.chapters = result.chapters or {}
      state.stops = flatten_outline(state.chapters)
      if #state.stops == 0 then
        notify("tour has no stops")
        return
      end

      local saved_index = flat_index_for_position(state.chapters, state.pending_position)
      state.pending_position = nil
      state.current = saved_index or 1
      if previous_id then
        for index, stop in ipairs(state.stops) do
          if stop.id == previous_id then
            state.current = index
            break
          end
        end
      end
      if state.outline_cb then
        state.outline_cb()
      end
    end)
  end

  state.outline_attempt = attempt
  attempt()
end

local function matches_tour(params)
  return state.tour_id == nil or params.tour_id == nil or params.tour_id == state.tour_id
end

local function on_stop_ready(params)
  if not params or not params.stop_id or not matches_tour(params) then
    return
  end

  local meta = state.stop_meta[params.stop_id] or {}
  meta.title = params.title or meta.title
  meta.why_here = params.why_here or meta.why_here
  state.stop_meta[params.stop_id] = meta

  if not state.outline_loaded then
    local provisional
    for _, stop in ipairs(state.provisional_stops) do
      if stop.id == params.stop_id then
        provisional = stop
        break
      end
    end
    if not provisional then
      provisional = { id = params.stop_id }
      state.provisional_stops[#state.provisional_stops + 1] = provisional
    end
    provisional.file = params.file or provisional.file
    provisional.start_line = params.start_line or provisional.start_line
    provisional.end_line = params.end_line or provisional.end_line
    provisional.title = meta.title or provisional.title
    provisional.why_here = meta.why_here or provisional.why_here

    state.chapters = provisional_outline()
    state.stops = flatten_outline(state.chapters)
    if state.current == 0 and #state.stops > 0 then
      state.current = 1
      show_current()
    elseif state.stop then
      for _, stop in ipairs(state.stops) do
        if stop.id == state.stop.id then
          state.stop = stop
          break
        end
      end
      render_split()
    end
    return
  end

  for _, stop in ipairs(state.stops) do
    if stop.id == params.stop_id then
      stop.title = meta.title or stop.title
      stop.why_here = meta.why_here or stop.why_here
      if state.stop and state.stop.id == stop.id then
        state.stop = stop
        render_split()
      end
      break
    end
  end
end

local function on_chunk(params)
  if not params or not params.stop_id or not params.delta or not matches_tour(params) then
    return
  end

  state.chunks[params.stop_id] = (state.chunks[params.stop_id] or "") .. params.delta
  if state.stop and state.stop.id == params.stop_id then
    state.explanation = state.chunks[params.stop_id]
    render_split()
  end
end

local function on_progress(params)
  if not params or not matches_tour(params) then
    return
  end
  if params.stage == "plan" and (params.pct == nil or params.pct >= 100) then
    if not state.outline_loaded and state.outline_attempt then
      state.outline_attempt()
    end
  end
end

local function on_error(params)
  if not params or not matches_tour(params) then
    return
  end
  if not params.stop_id then
    state.build_failed = true
  end
  notify(params.message or "engine error", vim.log.levels.ERROR)
end

local function subscribe()
  unsubscribe()
  state.subscriptions[#state.subscriptions + 1] = rpc.on("stop_ready", on_stop_ready)
  state.subscriptions[#state.subscriptions + 1] = rpc.on("explanation_chunk", on_chunk)
  state.subscriptions[#state.subscriptions + 1] = rpc.on("progress", on_progress)
  state.subscriptions[#state.subscriptions + 1] = rpc.on("error", on_error)
end

--- Start a tour. With `opts.tour_id`, resume that tour; otherwise ask the
--- engine to build a new one.
---@param opts table|nil { repo?, offline?, tour_id?, explain?, position? }
function M.start(opts)
  opts = opts or {}
  local repo = opts.repo or vim.fn.getcwd()
  local offline = opts.offline == true
  local tour_id = opts.tour_id

  M.reset()
  ensure_highlight()

  state.active = true
  state.repo = repo
  state.offline = offline
  state.request_explain = opts.explain == true or (tour_id ~= nil and opts.explain ~= false)
  state.pending_position = opts.position
  subscribe()

  if tour_id then
    state.tour_id = tour_id
    load_outline(tour_id, show_current)
    return
  end

  rpc.request("build_tour", { repo = repo, offline = offline }, function(err, result)
    if err then
      state.build_failed = true
      notify("failed to start tour: " .. (err.message or "unknown error"), vim.log.levels.ERROR)
      return
    end
    if not result or not result.tour_id then
      state.build_failed = true
      notify("engine did not return a tour id", vim.log.levels.ERROR)
      return
    end

    state.tour_id = result.tour_id
    load_outline(result.tour_id, show_current)
  end)
end

--- Resume the most recent tour (or `opts.tour_id`) from its saved position.
---@param opts table|nil { repo?, tour_id? }
function M.resume(opts)
  opts = opts or {}
  local repo = opts.repo or vim.fn.getcwd()
  rpc.request("resume_tour", { repo = repo, tour_id = opts.tour_id }, function(err, result)
    if err then
      notify("failed to resume tour: " .. (err.message or "unknown error"), vim.log.levels.ERROR)
      return
    end
    if not result or not result.tour_id then
      notify("engine did not return a tour id", vim.log.levels.ERROR)
      return
    end
    M.start({
      repo = repo,
      tour_id = result.tour_id,
      position = result.position,
      offline = opts.offline == true,
      explain = true,
    })
  end)
end

--- Labels for the outline picker, one per stop.
---@return string[]
function M.outline_items()
  local items = {}
  for index, stop in ipairs(state.stops) do
    local title = stop.title ~= "" and stop.title or stop.id
    items[#items + 1] = string.format(
      "%d. [%s] %s — %s:%d",
      index,
      stop.chapter_title or "",
      title,
      stop.file or "",
      stop.start_line or 0
    )
  end
  return items
end

--- Jump to a flat stop index from the outline. Returns false when out of range.
---@param index integer
---@return boolean
function M.outline_select(index)
  if not state.active or #state.stops == 0 then
    return false
  end
  index = tonumber(index)
  if not index or index < 1 or index > #state.stops then
    return false
  end
  M.show_stop(index)
  return true
end

--- Open the outline picker and jump to the chosen chapter/stop.
function M.outline()
  if not state.active or #state.stops == 0 then
    notify("no active tour; start one with :Laudit tour")
    return
  end
  local items = M.outline_items()
  vim.ui.select(items, { prompt = "laudit outline" }, function(_, index)
    if index then
      M.outline_select(index)
    end
  end)
end

--- Show the codebase model artifact.
---@param opts table|nil { repo? }
function M.model(opts)
  opts = opts or {}
  local repo = opts.repo or vim.fn.getcwd()
  local path = repo .. "/.laudit/model.json"
  if vim.fn.filereadable(path) == 0 then
    notify("no model artifact at " .. path .. "; run :Laudit tour or laudit build")
    return
  end
  show_scratch("laudit://model", vim.fn.readfile(path), "json")
end

--- Explain the current stop, or a symbol via the engine CLI.
---@param args table|string|nil
function M.explain(args)
  local fargs = type(args) == "table" and args.fargs or {}
  local symbol = fargs[2]
  if symbol and symbol ~= "" then
    local resolved = config.resolve_binary()
    if not resolved.found then
      notify("laudit binary not found", vim.log.levels.ERROR)
      return
    end
    local repo = vim.fn.getcwd()
    local output = vim.fn.system({ resolved.path, "explain", symbol, "--repo", repo })
    if vim.v.shell_error ~= 0 then
      notify("explain failed: " .. output, vim.log.levels.ERROR)
      return
    end
    show_scratch("laudit://explain", vim.split(output, "\n", { plain = true }), "markdown")
    return
  end

  if not state.active or not state.stop then
    notify("no active tour; use :Laudit explain <symbol> or start a tour")
    return
  end
  local stop = state.stop
  if state.explanation ~= "" then
    notify("current stop is already explained")
    return
  end
  state.explaining[stop.id] = true
  rpc.request("explain_stop", { tour_id = state.tour_id, stop_id = stop.id }, function(err)
    state.explaining[stop.id] = nil
    if err then
      notify("failed to explain stop: " .. (err.message or "unknown error"))
    end
  end)
end

--- Report engine and tour status.
---@param opts table|nil { repo? }
function M.status(opts)
  opts = opts or {}
  local repo = opts.repo or vim.fn.getcwd()
  rpc.request("status", { repo = repo, tour_id = state.tour_id }, function(err, result)
    if err then
      notify("status failed: " .. (err.message or "unknown error"), vim.log.levels.ERROR)
      return
    end
    local lines = { "laudit status" }
    lines[#lines + 1] = "  engine: " .. tostring(result.engine or "unknown")
    local tour = result.tour
    if tour then
      lines[#lines + 1] = "  model: " .. (tour.model_present and "present" or "missing")
      lines[#lines + 1] = "  tour: "
        .. (tour.tour_present and (tour.tour_id or "present") or "missing")
      if tour.tour_present then
        lines[#lines + 1] = string.format("  chapters: %d", tour.chapters or 0)
        lines[#lines + 1] = string.format("  stops: %d", tour.stops or 0)
        lines[#lines + 1] = string.format("  explained: %d", tour.explained or 0)
        lines[#lines + 1] = string.format("  pending: %d", tour.pending or 0)
        lines[#lines + 1] = string.format("  skipped: %d", tour.skipped or 0)
        lines[#lines + 1] = string.format("  failed: %d", tour.failed or 0)
        lines[#lines + 1] = string.format("  stale: %d", tour.stale or 0)
        if tour.position then
          lines[#lines + 1] = string.format(
            "  position: %d:%d",
            tour.position.chapter,
            tour.position.stop
          )
        end
      end
    end
    vim.api.nvim_echo({ { table.concat(lines, "\n"), "None" } }, true, {})
  end)
end

--- Remove generated artifacts under `.laudit/` after confirmation.
---@param opts table|nil { repo?, force?, yes? }
function M.clear(opts)
  opts = opts or {}
  local repo = opts.repo or vim.fn.getcwd()
  local base = repo .. "/.laudit"
  if vim.fn.isdirectory(base) == 0 then
    notify("no .laudit directory in " .. repo)
    return
  end
  if not (opts.force or opts.yes) then
    local choice = vim.fn.confirm("Remove laudit artifacts in " .. base .. "?", "&Yes\n&No", 2)
    if choice ~= 1 then
      return
    end
  end

  M.stop()
  for _, name in ipairs({ "model.json", "redactions.json", "tours", "index" }) do
    local path = base .. "/" .. name
    if vim.fn.isdirectory(path) == 1 then
      vim.fn.delete(path, "rf")
    elseif vim.fn.filereadable(path) == 1 then
      vim.fn.delete(path)
    end
  end
  notify("removed laudit artifacts in " .. base, vim.log.levels.INFO)
end

--- Move to the next stop and auto-follow its buffer/range.
function M.next()
  if not state.active or #state.stops == 0 then
    return
  end
  if state.current < #state.stops then
    state.current = state.current + 1
    show_current()
  end
end

--- Move to the previous stop and auto-follow its buffer/range.
function M.prev()
  if not state.active or #state.stops == 0 then
    return
  end
  if state.current > 1 then
    state.current = state.current - 1
    show_current()
  end
end

--- Jump to a flat stop index (1-based).
---@param index integer
function M.show_stop(index)
  if not state.active or #state.stops == 0 then
    return
  end
  state.current = math.max(1, math.min(index, #state.stops))
  show_current()
end

--- Snap the tour window back to the current stop and re-highlight it.
function M.return_to_current()
  if not state.active or #state.stops == 0 then
    return
  end
  show_current()
end

--- Stop the tour and tear down the UI.
function M.stop()
  local tour_id = state.tour_id
  M.reset()
  if tour_id and rpc.is_running() then
    pcall(rpc.request, "cancel", { tour_id = tour_id }, function() end)
  end
end

--- Tear down the UI and clear all state. Intended for tests.
function M.reset()
  unsubscribe()
  clear_extmark()
  close_split()
  reset_state()
end

---@return boolean
function M.is_active()
  return state.active
end

---@return table|nil
function M.current_stop()
  return state.stop
end

---@return table
function M.state()
  return state
end

M.namespace = ns

return M

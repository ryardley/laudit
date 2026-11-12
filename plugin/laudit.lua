-- laudit plugin entry point. Commands and default keymaps are installed here;
-- setup{} may be called before or after this file loads.

if vim.g.loaded_laudit then
  return
end
vim.g.loaded_laudit = 1

local laudit = require("laudit")

vim.api.nvim_create_user_command("Laudit", function(opts)
  laudit.command(opts)
end, {
  nargs = "*",
  force = true,
  desc = "laudit guided codebase tours",
  complete = function(arg_lead)
    local matches = {}
    for _, name in ipairs(laudit.command_names()) do
      if name:sub(1, #arg_lead) == arg_lead then
        matches[#matches + 1] = name
      end
    end
    return matches
  end,
})

laudit.setup()

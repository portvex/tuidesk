-- tuidesk Lua plugins: ~/.config/tuidesk/init.lua (hot-reloaded on save)
--
-- API
--   tuidesk.notify(title, body?, timeout_secs?)   show a notification
--   tuidesk.spawn(command)                         open a window running command (via /bin/sh)
--   tuidesk.action(name)                           run a builtin action, e.g. "maximize", "tile_left"
--   tuidesk.bind(key, fn)                          direct global keybinding, e.g. "alt+g"
--   tuidesk.bind_prefix(key, fn)                   binding used after the prefix key
--   tuidesk.on(event, fn)                          events: "startup", "tick" (every second),
--                                                "window_open", "window_close", "focus"
--                                                (handlers receive a window table)
--   tuidesk.widget(name, fn, interval_secs?)       panel widget; add "lua:name" to panel.widgets
--   tuidesk.rule{command=, title=, width=, height=, x=, y=, maximized=, floating=}
--   tuidesk.windows()                              list of {id, title, command, focused, minimized}
--   tuidesk.focus(id) / tuidesk.close(id)
--
-- Window tables have: id, title, command, focused, minimized.

-- Example: prefix + g opens lazygit maximized.
tuidesk.bind_prefix("g", function()
  tuidesk.spawn("lazygit")
end)
tuidesk.rule { command = "lazygit", maximized = true }

-- Example: a load-average panel widget. Enable it by adding "lua:load" to panel.widgets.
tuidesk.widget("load", function()
  local f = io.popen("sysctl -n vm.loadavg 2>/dev/null || cut -d' ' -f1-3 /proc/loadavg")
  if not f then return "" end
  local out = f:read("*l") or ""
  f:close()
  return "load " .. out:gsub("[{}]", ""):gsub("^%s+", ""):gsub("%s+$", "")
end, 5)

-- Example: report programs that exit with an error.
tuidesk.on("window_close", function(win)
  if win.exit_code and win.exit_code ~= 0 then
    tuidesk.notify(win.title, "exited with code " .. win.exit_code)
  end
end)

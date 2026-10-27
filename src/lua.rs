use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use mlua::{Function, Lua, Table, Value};

use crate::config::Rule;
use crate::keys::KeySpec;

/// Requests from Lua that the app carries out after the script returns.
pub enum LuaCmd {
    Notify(String, String, Option<u64>),
    Spawn(String),
    Action(String),
    Focus(u64),
    Close(u64),
}

#[derive(Debug, Clone, Default)]
pub struct WinInfo {
    pub id: u64,
    pub title: String,
    pub command: String,
    pub focused: bool,
    pub minimized: bool,
    pub exit_code: Option<u32>,
    /// 1-based virtual desktop.
    pub desktop: usize,
}

struct Widget {
    name: String,
    func: Function,
    interval: Duration,
    last: Option<Instant>,
    text: String,
}

#[derive(Default)]
struct Shared {
    cmds: Vec<LuaCmd>,
    /// (key, key as written, after prefix, function)
    binds: Vec<(KeySpec, String, bool, Function)>,
    handlers: HashMap<String, Vec<Function>>,
    widgets: Vec<Widget>,
    rules: Vec<Rule>,
    windows: Vec<WinInfo>,
}

pub struct Plugins {
    lua: Lua,
    pub errors: Vec<String>,
    /// When the Lua code running now must stop by (see `arm`).
    deadline: std::rc::Rc<std::cell::Cell<Option<Instant>>>,
}

/// How long one call into Lua may run before it is stopped, so a script stuck in a loop
/// cannot freeze the desktop.
const LUA_BUDGET: Duration = Duration::from_secs(2);

fn win_table(lua: &Lua, w: &WinInfo) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("id", w.id)?;
    t.set("title", w.title.clone())?;
    t.set("command", w.command.clone())?;
    t.set("focused", w.focused)?;
    t.set("minimized", w.minimized)?;
    t.set("exit_code", w.exit_code)?;
    t.set("desktop", w.desktop)?;
    Ok(t)
}

fn push_cmd(lua: &Lua, cmd: LuaCmd) {
    if let Some(mut s) = lua.app_data_mut::<Shared>() {
        s.cmds.push(cmd);
    }
}

fn install_api(lua: &Lua) -> mlua::Result<()> {
    let api = lua.create_table()?;

    api.set(
        "notify",
        lua.create_function(|lua, (title, body, timeout): (String, Option<String>, Option<u64>)| {
            push_cmd(lua, LuaCmd::Notify(title, body.unwrap_or_default(), timeout));
            Ok(())
        })?,
    )?;
    api.set(
        "spawn",
        lua.create_function(|lua, cmd: String| {
            push_cmd(lua, LuaCmd::Spawn(cmd));
            Ok(())
        })?,
    )?;
    api.set(
        "action",
        lua.create_function(|lua, name: String| {
            push_cmd(lua, LuaCmd::Action(name));
            Ok(())
        })?,
    )?;
    api.set(
        "focus",
        lua.create_function(|lua, id: u64| {
            push_cmd(lua, LuaCmd::Focus(id));
            Ok(())
        })?,
    )?;
    api.set(
        "close",
        lua.create_function(|lua, id: u64| {
            push_cmd(lua, LuaCmd::Close(id));
            Ok(())
        })?,
    )?;

    for (name, prefix) in [("bind", false), ("bind_prefix", true)] {
        api.set(
            name,
            lua.create_function(move |lua, (key, func): (String, Function)| {
                let spec = KeySpec::parse(&key).ok_or_else(|| mlua::Error::runtime(format!("bad key '{key}'")))?;
                if let Some(mut s) = lua.app_data_mut::<Shared>() {
                    s.binds.retain(|(k, _, p, _)| !(*k == spec && *p == prefix));
                    s.binds.push((spec, key, prefix, func));
                }
                Ok(())
            })?,
        )?;
    }

    api.set(
        "on",
        lua.create_function(|lua, (event, func): (String, Function)| {
            const EVENTS: [&str; 5] = ["startup", "tick", "window_open", "window_close", "focus"];
            if !EVENTS.contains(&event.as_str()) {
                return Err(mlua::Error::runtime(format!("unknown event '{event}'")));
            }
            if let Some(mut s) = lua.app_data_mut::<Shared>() {
                s.handlers.entry(event).or_default().push(func);
            }
            Ok(())
        })?,
    )?;

    api.set(
        "widget",
        lua.create_function(|lua, (name, func, interval): (String, Function, Option<f64>)| {
            let interval = Duration::from_secs_f64(interval.unwrap_or(1.0).max(0.2));
            if let Some(mut s) = lua.app_data_mut::<Shared>() {
                s.widgets.retain(|w| w.name != name);
                s.widgets.push(Widget { name, func, interval, last: None, text: String::new() });
            }
            Ok(())
        })?,
    )?;

    api.set(
        "rule",
        lua.create_function(|lua, t: Table| {
            let rule = Rule {
                command: t.get("command")?,
                title: t.get("title")?,
                width: t.get("width")?,
                height: t.get("height")?,
                x: t.get("x")?,
                y: t.get("y")?,
                maximized: t.get("maximized")?,
                floating: t.get("floating")?,
                desktop: t.get("desktop")?,
                minimized: t.get("minimized")?,
                above: t.get("above")?,
            };
            if let Some(mut s) = lua.app_data_mut::<Shared>() {
                s.rules.push(rule);
            }
            Ok(())
        })?,
    )?;

    api.set(
        "windows",
        lua.create_function(|lua, ()| {
            let wins = lua.app_data_ref::<Shared>().map(|s| s.windows.clone()).unwrap_or_default();
            let list = lua.create_table()?;
            for (i, w) in wins.iter().enumerate() {
                list.set(i + 1, win_table(lua, w)?)?;
            }
            Ok(list)
        })?,
    )?;

    lua.globals().set("tuidesk", api)?;
    Ok(())
}

impl Plugins {
    /// Create a Lua state and run `init.lua` from `dir` if present.
    pub fn load(dir: &Path) -> Self {
        let lua = Lua::new();
        lua.set_app_data(Shared::default());
        let mut errors = Vec::new();
        let deadline: std::rc::Rc<std::cell::Cell<Option<Instant>>> = Default::default();
        let watch = deadline.clone();
        let hook =
            lua.set_hook(mlua::HookTriggers::new().every_nth_instruction(10_000), move |_, _| match watch.get() {
                Some(t) if Instant::now() > t => {
                    Err(mlua::Error::runtime(format!("stopped after {}s (endless loop?)", LUA_BUDGET.as_secs())))
                }
                _ => Ok(mlua::VmState::Continue),
            });
        if let Err(e) = hook {
            errors.push(format!("lua hook: {e}"));
        }
        if let Err(e) = install_api(&lua) {
            errors.push(format!("lua api: {e}"));
        }
        // Allow require("name") for files in ~/.config/tuidesk/lua/.
        let lua_dir = dir.join("lua");
        let setup = format!(
            "package.path = {:?} .. ';' .. package.path",
            format!("{0}/?.lua;{0}/?/init.lua", lua_dir.display())
        );
        let _ = lua.load(setup).exec();

        let init = dir.join("init.lua");
        if init.exists() {
            match std::fs::read_to_string(&init) {
                Ok(src) => {
                    deadline.set(Some(Instant::now() + LUA_BUDGET));
                    if let Err(e) = lua.load(src).set_name("init.lua").exec() {
                        errors.push(format!("init.lua: {e}"));
                    }
                    deadline.set(None);
                }
                Err(e) => errors.push(format!("init.lua: {e}")),
            }
        }
        Self { lua, errors, deadline }
    }

    /// Start the time budget for one call into Lua.
    fn arm(&self) {
        self.deadline.set(Some(Instant::now() + LUA_BUDGET));
    }

    fn disarm(&self) {
        self.deadline.set(None);
    }

    pub fn set_windows(&self, wins: Vec<WinInfo>) {
        if let Some(mut s) = self.lua.app_data_mut::<Shared>() {
            s.windows = wins;
        }
    }

    pub fn take_cmds(&self) -> Vec<LuaCmd> {
        self.lua.app_data_mut::<Shared>().map(|mut s| std::mem::take(&mut s.cmds)).unwrap_or_default()
    }

    pub fn rules(&self) -> Vec<Rule> {
        self.lua.app_data_ref::<Shared>().map(|s| s.rules.clone()).unwrap_or_default()
    }

    /// Run the Lua function bound to `key`, if any. Returns true if one was bound.
    pub fn run_binding(&mut self, key: &KeySpec, prefix: bool) -> bool {
        let func = self
            .lua
            .app_data_ref::<Shared>()
            .and_then(|s| s.binds.iter().find(|(k, _, p, _)| k == key && *p == prefix).map(|(_, _, _, f)| f.clone()));
        match func {
            Some(f) => {
                self.arm();
                if let Err(e) = f.call::<()>(()) {
                    self.errors.push(format!("binding: {e}"));
                }
                self.disarm();
                true
            }
            None => false,
        }
    }

    /// Keys bound from Lua: (key as written, after prefix).
    pub fn bindings(&self) -> Vec<(String, bool)> {
        self.lua
            .app_data_ref::<Shared>()
            .map(|s| s.binds.iter().map(|(_, k, p, _)| (k.clone(), *p)).collect())
            .unwrap_or_default()
    }

    /// Run a chunk of Lua and return its results as text (for `tuidesk msg lua`).
    pub fn exec(&mut self, code: &str) -> Result<String, String> {
        self.arm();
        let values = self.lua.load(code).set_name("msg").eval::<mlua::MultiValue>();
        self.disarm();
        let values = values.map_err(|e| e.to_string())?;
        let tostring: Function = self.lua.globals().get("tostring").map_err(|e| e.to_string())?;
        let parts: Vec<String> = values.into_iter().map(|v| tostring.call::<String>(v).unwrap_or_default()).collect();
        Ok(parts.join("\t"))
    }

    /// Call a global Lua function by name (for "lua <name>" actions).
    pub fn call_global(&mut self, name: &str) {
        match self.lua.globals().get::<Value>(name) {
            Ok(Value::Function(f)) => {
                self.arm();
                if let Err(e) = f.call::<()>(()) {
                    self.errors.push(format!("{name}: {e}"));
                }
                self.disarm();
            }
            _ => self.errors.push(format!("no Lua function named '{name}'")),
        }
    }

    pub fn emit(&mut self, event: &str, win: Option<&WinInfo>) {
        let handlers =
            self.lua.app_data_ref::<Shared>().and_then(|s| s.handlers.get(event).cloned()).unwrap_or_default();
        for h in handlers {
            let arg = match win {
                Some(w) => win_table(&self.lua, w).map(Value::Table),
                None => Ok(Value::Nil),
            };
            self.arm();
            let result = arg.and_then(|a| h.call::<()>(a));
            self.disarm();
            if let Err(e) = result {
                self.errors.push(format!("on('{event}'): {e}"));
            }
        }
    }

    /// Re-run widgets whose interval has passed. Returns true if any text changed.
    pub fn update_widgets(&mut self) -> bool {
        let due: Vec<(usize, Function)> = self
            .lua
            .app_data_ref::<Shared>()
            .map(|s| {
                s.widgets
                    .iter()
                    .enumerate()
                    .filter(|(_, w)| w.last.is_none_or(|l| l.elapsed() >= w.interval))
                    .map(|(i, w)| (i, w.func.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut changed = false;
        for (i, f) in due {
            self.arm();
            let called = f.call::<Option<String>>(());
            self.disarm();
            let text = match called {
                Ok(t) => t.unwrap_or_default(),
                Err(e) => {
                    self.errors.push(format!("widget: {e}"));
                    "!".into()
                }
            };
            if let Some(mut s) = self.lua.app_data_mut::<Shared>()
                && let Some(w) = s.widgets.get_mut(i)
            {
                w.last = Some(Instant::now());
                if w.text != text {
                    w.text = text;
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn widget_text(&self, name: &str) -> Option<String> {
        self.lua.app_data_ref::<Shared>()?.widgets.iter().find(|w| w.name == name).map(|w| w.text.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugins_from(src: &str) -> Plugins {
        let dir = std::env::temp_dir().join(format!("tuidesk-test-{}-{}", std::process::id(), src.len()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("init.lua"), src).unwrap();
        let p = Plugins::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        p
    }

    #[test]
    #[cfg_attr(miri, ignore = "Lua is a C library; Miri cannot call it")]
    fn api_queues_commands_and_bindings() {
        let mut p = plugins_from(
            r#"
            tuidesk.notify("hi", "there")
            tuidesk.bind_prefix("g", function() tuidesk.spawn("lazygit") end)
            tuidesk.widget("w", function() return "ok" end)
            tuidesk.rule { command = "btop", maximized = true }
            "#,
        );
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert!(matches!(p.take_cmds().as_slice(), [LuaCmd::Notify(t, b, None)] if t == "hi" && b == "there"));
        assert!(p.run_binding(&KeySpec::parse("g").unwrap(), true));
        assert!(matches!(p.take_cmds().as_slice(), [LuaCmd::Spawn(c)] if c == "lazygit"));
        assert!(!p.run_binding(&KeySpec::parse("g").unwrap(), false));
        assert!(p.update_widgets());
        assert_eq!(p.widget_text("w").as_deref(), Some("ok"));
        assert_eq!(p.rules()[0].maximized, Some(true));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Lua is a C library; Miri cannot call it")]
    fn endless_loops_are_stopped() {
        let start = Instant::now();
        let mut p = plugins_from("tuidesk.bind('x', function() while true do end end)");
        assert!(p.run_binding(&KeySpec::parse("x").unwrap(), false));
        assert!(p.errors.iter().any(|e| e.contains("stopped after")), "{:?}", p.errors);
        assert!(p.exec("while true do end").unwrap_err().contains("stopped after"));
        assert!(start.elapsed() < Duration::from_secs(10));
        // Normal code still runs afterwards.
        assert_eq!(p.exec("return 1 + 1").unwrap(), "2");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Lua is a C library; Miri cannot call it")]
    fn errors_are_reported() {
        let p = plugins_from("tuidesk.on('nope', function() end)");
        assert_eq!(p.errors.len(), 1);
    }
}

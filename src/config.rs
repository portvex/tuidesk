use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

pub const DEFAULT_CONFIG: &str = include_str!("../config/default.toml");
pub const DEFAULT_INIT_LUA: &str = include_str!("../config/init.lua");
pub const BUILTIN_APPS: &str = include_str!("../config/apps.toml");

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub general: General,
    pub theme: ThemeConfig,
    pub panel: PanelConfig,
    pub keys: Keys,
    /// The user's launcher entries; `all_apps` adds them to the built-in catalogue.
    #[serde(default)]
    pub apps: Vec<AppEntry>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Keyed by "default" or a 1-based desktop number.
    #[serde(default)]
    pub wallpaper: HashMap<String, Wallpaper>,
    #[serde(default)]
    pub desktop_widgets: Vec<DesktopWidget>,
    pub screenshot: ScreenshotConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScreenshotConfig {
    pub dir: String,
    /// txt, ansi, html or svg.
    pub format: String,
    /// After saving, copy "path", "text" or "none" to the clipboard.
    pub copy: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Wallpaper {
    /// "logo" (default), "none", "art", "pattern" or "lua".
    pub kind: Option<String>,
    pub color: Option<String>,
    pub fg: Option<String>,
    /// ASCII art inline ("art") or the repeated text ("pattern").
    pub text: Option<String>,
    /// ASCII art from a file ("art").
    pub file: Option<String>,
    /// Lua widget name whose text is drawn ("lua").
    pub widget: Option<String>,
    /// Where art is placed: center (default), top-left, top, top-right, left, right,
    /// bottom-left, bottom, bottom-right.
    pub position: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DesktopWidget {
    /// "clock", "notes", "sysinfo" or "lua".
    pub kind: String,
    #[serde(default = "default_position")]
    pub position: String,
    /// 1-based desktops to show on; all when empty.
    #[serde(default)]
    pub desktops: Vec<usize>,
    pub title: Option<String>,
    /// File shown by "notes".
    pub file: Option<String>,
    /// Lua widget name for "lua".
    pub widget: Option<String>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    /// strftime format for "clock" (default "%H:%M").
    pub format: Option<String>,
}

fn default_position() -> String {
    "top-right".into()
}

/// Replace a leading "~" with the home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some(rest) => dirs::home_dir().map(|h| h.join(rest.trim_start_matches('/'))).unwrap_or_else(|| path.into()),
        None => PathBuf::from(path),
    }
}

impl Config {
    /// Wallpaper settings for a 0-based desktop: its own entry over "default".
    pub fn wallpaper_for(&self, desktop: usize) -> Wallpaper {
        let base = self.wallpaper.get("default").cloned().unwrap_or_default();
        let Some(own) = self.wallpaper.get(&(desktop + 1).to_string()) else { return base };
        Wallpaper {
            kind: own.kind.clone().or(base.kind),
            color: own.color.clone().or(base.color),
            fg: own.fg.clone().or(base.fg),
            text: own.text.clone().or(base.text),
            file: own.file.clone().or(base.file),
            widget: own.widget.clone().or(base.widget),
            position: own.position.clone().or(base.position),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct General {
    pub shell: String,
    pub prefix: String,
    pub layout: String,
    pub master_ratio: f32,
    pub gaps: u16,
    pub scrollback: usize,
    pub color_mode: String,
    pub window_width: u16,
    pub window_height: u16,
    pub close_on_exit: bool,
    pub focus_follows_mouse: bool,
    pub dim_inactive: bool,
    pub notification_timeout: u64,
    pub hide_missing_apps: bool,
    pub builtin_apps: bool,
    pub hidden_apps: Vec<String>,
    pub autostart: Vec<String>,
    pub desktops: usize,
    pub restore_session: bool,
    pub inherit_cwd: bool,
    pub desktop_names: Vec<String>,
    pub images: bool,
    pub image_protocol: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThemeConfig {
    pub preset: String,
    #[serde(flatten)]
    pub overrides: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PanelConfig {
    pub position: String,
    pub widgets: Vec<String>,
    pub kickoff_label: String,
    pub clock_format: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Keys {
    pub global: HashMap<String, String>,
    pub prefix: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppEntry {
    pub name: String,
    pub command: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub description: String,
}

fn default_category() -> String {
    "Other".into()
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Rule {
    pub command: Option<String>,
    pub title: Option<String>,
    /// Percent of the desktop.
    pub width: Option<u16>,
    pub height: Option<u16>,
    /// Absolute cell position.
    pub x: Option<u16>,
    pub y: Option<u16>,
    pub maximized: Option<bool>,
    pub floating: Option<bool>,
    /// 1-based virtual desktop to open on.
    pub desktop: Option<usize>,
    pub minimized: Option<bool>,
    /// Keep above other windows.
    pub above: Option<bool>,
}

impl Rule {
    pub fn matches(&self, command: &str, title: &str) -> bool {
        let cmd_ok = self.command.as_deref().is_none_or(|c| command_name(command) == c);
        let title_ok = self.title.as_deref().is_none_or(|t| title.contains(t));
        (self.command.is_some() || self.title.is_some()) && cmd_ok && title_ok
    }
}

/// First word of a command line, without its directory.
pub fn command_name(command: &str) -> String {
    let first = command.split_whitespace().next().unwrap_or(command);
    let first = first.trim_start_matches('$');
    Path::new(first).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| first.to_string())
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("tuidesk");
    }
    // %APPDATA%\tuidesk
    #[cfg(windows)]
    if let Some(dir) = dirs::config_dir() {
        return dir.join("tuidesk");
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".config").join("tuidesk")
}

/// Write default config files the first time tuidesk runs. Never overwrites.
pub fn ensure_default_files(dir: &Path) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let cfg = dir.join("config.toml");
    if !cfg.exists() {
        let _ = std::fs::write(&cfg, DEFAULT_CONFIG);
    }
    let lua = dir.join("init.lua");
    if !lua.exists() {
        let _ = std::fs::write(&lua, DEFAULT_INIT_LUA);
    }
}

pub fn load(path: &Path) -> Result<Config> {
    if !path.exists() {
        return parse("");
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&text).with_context(|| format!("in {}", path.display()))
}

/// Parse user config text, merged over the built-in defaults.
pub fn parse(text: &str) -> Result<Config> {
    let cfg: Config = toml::Value::Table(merged(text)?).try_into().context("invalid configuration")?;
    Ok(cfg)
}

/// User config text merged over the built-in defaults, before it is checked against `Config`.
pub fn merged(text: &str) -> Result<toml::Table> {
    let mut base: toml::Table = toml::from_str(DEFAULT_CONFIG).expect("built-in config is valid");
    let user: toml::Table = toml::from_str(text).context("parsing config")?;
    merge(&mut base, user);
    Ok(base)
}

/// Recursively merge `over` into `base`. Tables merge, everything else (arrays included) replaces.
fn merge(base: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

impl Config {
    pub fn shell(&self) -> String {
        if !self.general.shell.is_empty() {
            return self.general.shell.clone();
        }
        let var = if cfg!(windows) { "COMSPEC" } else { "SHELL" };
        let fallback = if cfg!(windows) { "cmd.exe" } else { "/bin/sh" };
        std::env::var(var).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| fallback.into())
    }

    /// Launcher entries: the built-in catalogue with the user's entries over it (a user entry
    /// replaces the built-in one with the same name, others are added), minus `hidden_apps`.
    pub fn all_apps(&self) -> Vec<AppEntry> {
        let mut apps = if self.general.builtin_apps { builtin_apps() } else { Vec::new() };
        for a in &self.apps {
            match apps.iter_mut().find(|b| b.name.eq_ignore_ascii_case(&a.name)) {
                Some(b) => *b = a.clone(),
                None => apps.push(a.clone()),
            }
        }
        apps.retain(|a| !self.general.hidden_apps.iter().any(|h| h.eq_ignore_ascii_case(&a.name)));
        apps
    }

    /// Launcher entries, optionally filtered to installed programs.
    pub fn visible_apps(&self) -> Vec<AppEntry> {
        let mut apps = self.all_apps();
        apps.retain(|a| !self.general.hide_missing_apps || program_exists(&a.command));
        apps
    }
}

/// The built-in launcher catalogue, `config/apps.toml`.
pub fn builtin_apps() -> Vec<AppEntry> {
    #[derive(Deserialize)]
    struct Catalogue {
        apps: Vec<AppEntry>,
    }
    toml::from_str::<Catalogue>(BUILTIN_APPS).expect("built-in apps are valid").apps
}

fn program_exists(command: &str) -> bool {
    let name = command.split_whitespace().next().unwrap_or("");
    if name.starts_with('$') || name.is_empty() {
        return true;
    }
    if name.contains('/') {
        return Path::new(name).exists();
    }
    let exts: Vec<String> = if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        std::iter::once(String::new()).chain(pathext.split(';').map(str::to_string)).collect()
    } else {
        vec![String::new()]
    };
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|p| exts.iter().any(|e| p.join(format!("{name}{e}")).is_file())))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let cfg = load(Path::new("/nonexistent/tuidesk.toml")).unwrap();
        assert_eq!(cfg.general.prefix, "ctrl+space");
        assert!(cfg.apps.is_empty());
        assert_eq!(cfg.all_apps().len(), builtin_apps().len());
    }

    #[test]
    fn user_apps_extend_the_builtin_catalogue() {
        let cfg = parse(
            "[general]\nhidden_apps = [\"python\"]\n\
             [[apps]]\nname = \"editor\"\ncommand = \"hx\"\n\
             [[apps]]\nname = \"Notes\"\ncommand = \"nvim ~/notes.md\"\n",
        )
        .unwrap();
        let apps = cfg.all_apps();
        let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
        // Same name (any case) replaces the built-in entry in place; new names come last.
        assert_eq!(names.iter().filter(|n| n.eq_ignore_ascii_case("editor")).count(), 1);
        assert_eq!(apps.iter().find(|a| a.name == "editor").unwrap().command, "hx");
        assert_eq!(names.last(), Some(&"Notes"));
        assert!(names.contains(&"Terminal") && !names.contains(&"Python"));

        let own = parse("[general]\nbuiltin_apps = false\n[[apps]]\nname = \"Notes\"\ncommand = \"nvim\"\n").unwrap();
        assert_eq!(own.all_apps().len(), 1);
    }

    #[test]
    fn builtin_app_names_are_unique() {
        let apps = builtin_apps();
        for (i, a) in apps.iter().enumerate() {
            assert!(!apps[..i].iter().any(|b| b.name.eq_ignore_ascii_case(&a.name)), "duplicate {}", a.name);
        }
    }

    #[test]
    fn user_values_override_defaults() {
        let mut base: toml::Table = toml::from_str(DEFAULT_CONFIG).unwrap();
        let user: toml::Table = toml::from_str("[general]\nlayout = \"tiling\"\n[keys.prefix]\nz = \"quit\"").unwrap();
        merge(&mut base, user);
        let cfg: Config = toml::Value::Table(base).try_into().unwrap();
        assert_eq!(cfg.general.layout, "tiling");
        assert_eq!(cfg.general.prefix, "ctrl+space");
        assert_eq!(cfg.keys.prefix.get("z").map(String::as_str), Some("quit"));
        assert!(cfg.keys.prefix.contains_key("x"));
    }

    #[test]
    fn wallpaper_per_desktop_overrides_default() {
        let cfg: Config = {
            let mut base: toml::Table = toml::from_str(DEFAULT_CONFIG).unwrap();
            let user: toml::Table = toml::from_str(
                "[wallpaper.default]\nkind = \"pattern\"\ntext = \". \"\n[wallpaper.2]\nkind = \"art\"\ncolor = \"#102030\"\n\
                 [[desktop_widgets]]\nkind = \"clock\"\ndesktops = [1]\n",
            )
            .unwrap();
            merge(&mut base, user);
            toml::Value::Table(base).try_into().unwrap()
        };
        assert_eq!(cfg.wallpaper_for(0).kind.as_deref(), Some("pattern"));
        let w2 = cfg.wallpaper_for(1);
        assert_eq!((w2.kind.as_deref(), w2.text.as_deref()), (Some("art"), Some(". ")));
        assert_eq!(cfg.desktop_widgets[0].position, "top-right");
        assert_eq!(expand_tilde("/x"), PathBuf::from("/x"));
    }

    #[test]
    fn rule_matching() {
        let r = Rule { command: Some("btop".into()), ..Default::default() };
        assert!(r.matches("btop --utf-force", "x"));
        assert!(!r.matches("htop", "btop"));
        assert_eq!(command_name("/usr/bin/nvim foo"), "nvim");
    }
}

//! The Settings dialog: a form over config.toml. Edits go into a toml_edit document, so the
//! user's comments and layout survive saving, and every change is previewed live by parsing the
//! document over the built-in defaults, exactly as loading the file would.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use toml_edit::{DocumentMut, Item, TableLike, Value};

use crate::app::Action;
use crate::config::{self, Config};
use crate::keys::KeySpec;

pub enum Kind {
    Bool,
    Choice(&'static [&'static str]),
    Int {
        min: i64,
        max: i64,
        step: i64,
    },
    Ratio {
        min: f64,
        max: f64,
        step: f64,
    },
    Text,
    /// A theme color; empty means the preset's.
    Color,
    /// A key combination, set by pressing it.
    Key,
}

pub struct Field {
    /// Dotted config path, e.g. "general.gaps".
    pub path: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub help: &'static str,
}

pub enum Extra {
    None,
    /// The panel's widget list: toggles and order.
    Widgets,
    /// The direct and prefix key bindings.
    Bindings,
}

pub struct Page {
    pub name: &'static str,
    pub fields: &'static [Field],
    pub extra: Extra,
}

const fn field(path: &'static str, label: &'static str, kind: Kind, help: &'static str) -> Field {
    Field { path, label, kind, help }
}

const fn color(path: &'static str, label: &'static str) -> Field {
    field(path, label, Kind::Color, "#rrggbb, a color name or 0-255; empty uses the preset")
}

pub const LAYOUTS: &[&str] = &["floating", "master", "monocle", "grid", "columns", "bsp"];
pub const PRESETS: &[&str] = &["breeze-dark", "breeze-light", "tty16"];
pub const BORDERS: &[&str] = &["plain", "rounded", "double", "thick", "ascii"];
pub const WIDGETS: &[&str] = &["kickoff", "pager", "tasks", "layout", "cpu", "mem", "battery", "bell", "clock"];

pub const PAGES: &[Page] = &[
    Page {
        name: "General",
        fields: &[
            field("general.shell", "Shell", Kind::Text, "Program for new terminals; empty uses $SHELL"),
            field("general.desktops", "Virtual desktops", Kind::Int { min: 1, max: 9, step: 1 }, "Number of desktops"),
            field(
                "general.restore_session",
                "Restore session",
                Kind::Bool,
                "Reopen the windows of the last session on start",
            ),
            field("general.inherit_cwd", "Inherit directory", Kind::Bool, "New windows start in the focused one's"),
            field("general.close_on_exit", "Close on exit", Kind::Bool, "Close a window when its program exits"),
            field("general.hide_missing_apps", "Hide missing apps", Kind::Bool, "Kickoff hides uninstalled programs"),
            field("general.builtin_apps", "Built-in apps", Kind::Bool, "Kickoff lists common terminal programs"),
            field(
                "general.notification_timeout",
                "Notification seconds",
                Kind::Int { min: 1, max: 60, step: 1 },
                "How long popups stay",
            ),
            field(
                "general.scrollback",
                "Scrollback lines",
                Kind::Int { min: 0, max: 100_000, step: 1000 },
                "Lines kept per window",
            ),
            field("general.images", "Images", Kind::Bool, "Show images from programs such as yazi and icat"),
            field(
                "general.image_protocol",
                "Image protocol",
                Kind::Choice(&["auto", "kitty", "iterm", "sixel", "none"]),
                "How images reach your terminal; auto detects it",
            ),
            field(
                "general.pixel_buttons",
                "Icon buttons",
                Kind::Bool,
                "Draw the title bar buttons as icons when the terminal shows images",
            ),
        ],
        extra: Extra::None,
    },
    Page {
        name: "Windows",
        fields: &[
            field("general.layout", "Layout", Kind::Choice(LAYOUTS), "Arrangement of windows"),
            field(
                "general.master_ratio",
                "Master ratio",
                Kind::Ratio { min: 0.1, max: 0.9, step: 0.05 },
                "Width share of the master window when tiled",
            ),
            field("general.gaps", "Gaps", Kind::Int { min: 0, max: 10, step: 1 }, "Cells between tiled windows"),
            field(
                "general.window_width",
                "New window width",
                Kind::Int { min: 10, max: 100, step: 5 },
                "Percent of the desktop",
            ),
            field(
                "general.window_height",
                "New window height",
                Kind::Int { min: 10, max: 100, step: 5 },
                "Percent of the desktop",
            ),
            field("general.focus_follows_mouse", "Focus follows mouse", Kind::Bool, "Hovering a window focuses it"),
            field("general.dim_inactive", "Dim inactive", Kind::Bool, "Dim the content of unfocused windows"),
        ],
        extra: Extra::None,
    },
    Page {
        name: "Appearance",
        fields: &[
            field("theme.preset", "Theme", Kind::Choice(PRESETS), "Color preset; the colors below override it"),
            field("theme.border", "Borders", Kind::Choice(BORDERS), "Window and menu border style"),
            field(
                "general.color_mode",
                "Colors",
                Kind::Choice(&["auto", "truecolor", "256", "16"]),
                "Use 16 on the Linux console",
            ),
            field(
                "wallpaper.default.kind",
                "Wallpaper",
                Kind::Choice(&["logo", "none", "pattern", "art"]),
                "Per-desktop wallpapers are set in config.toml",
            ),
            field("wallpaper.default.text", "Wallpaper text", Kind::Text, "Repeated text (pattern) or ASCII art"),
            color("theme.accent", "Accent"),
            color("theme.wallpaper", "Wallpaper color"),
            color("theme.wallpaper_fg", "Wallpaper text color"),
            color("theme.panel_bg", "Panel background"),
            color("theme.panel_fg", "Panel text"),
            color("theme.title_active_bg", "Active title"),
            color("theme.title_active_fg", "Active title text"),
            color("theme.title_inactive_bg", "Inactive title"),
            color("theme.title_inactive_fg", "Inactive title text"),
            color("theme.border_active", "Active border"),
            color("theme.border_inactive", "Inactive border"),
            color("theme.term_bg", "Terminal background"),
            color("theme.term_fg", "Terminal text"),
            color("theme.menu_bg", "Menu background"),
            color("theme.menu_fg", "Menu text"),
            color("theme.menu_sel_bg", "Menu selection"),
            color("theme.menu_sel_fg", "Menu selection text"),
            color("theme.notif_bg", "Notification background"),
            color("theme.notif_fg", "Notification text"),
            color("theme.urgent", "Urgent"),
        ],
        extra: Extra::None,
    },
    Page {
        name: "Panel",
        fields: &[
            field("panel.position", "Position", Kind::Choice(&["bottom", "top"]), "Screen edge of the panel"),
            field("panel.kickoff_label", "Kickoff label", Kind::Text, "Text of the menu button"),
            field("panel.clock_format", "Clock format", Kind::Text, "strftime format, e.g. %H:%M"),
        ],
        extra: Extra::Widgets,
    },
    Page {
        name: "Keys",
        fields: &[field(
            "general.prefix",
            "Prefix key",
            Kind::Key,
            "Press Enter, then the new key; window manager commands follow it",
        )],
        extra: Extra::Bindings,
    },
    Page {
        name: "Screenshots",
        fields: &[
            field("screenshot.dir", "Folder", Kind::Text, "Where screenshots are saved"),
            field(
                "screenshot.format",
                "Format",
                Kind::Choice(&["html", "svg", "ansi", "txt"]),
                "File type of new screenshots",
            ),
            field(
                "screenshot.copy",
                "Copy to clipboard",
                Kind::Choice(&["path", "text", "none"]),
                "What is copied after saving",
            ),
        ],
        extra: Extra::None,
    },
];

/// Binding tables and their headings.
pub const BINDING_TABLES: &[(&str, &str)] = &[("keys.global", "Direct keys"), ("keys.prefix", "After the prefix")];

#[derive(Clone, Copy)]
pub enum Row {
    Heading(&'static str),
    Field(&'static Field),
    /// Index into `Settings::widgets()`.
    Widget(usize),
    /// Index into `Settings::bindings(table)`.
    Binding(&'static str, usize),
    AddBinding(&'static str),
}

impl Row {
    pub fn selectable(&self) -> bool {
        !matches!(self, Row::Heading(_))
    }
}

/// What is being typed or captured.
#[derive(Clone)]
pub enum Edit {
    Text {
        field: &'static Field,
        text: String,
    },
    /// The action of a binding; `key` is new when the binding is being added.
    Action {
        table: &'static str,
        key: String,
        text: String,
    },
    /// Waiting for a key: for the prefix field, or a new binding in a table.
    CaptureField(&'static Field),
    CaptureBinding(&'static str),
}

pub enum Outcome {
    None,
    /// The document changed; this is the config to preview.
    Changed(Box<Config>),
    Save,
    SaveAndClose,
    /// Close, reverting unsaved changes.
    Close,
}

pub struct Settings {
    pub path: PathBuf,
    doc: DocumentMut,
    /// The file's text when opened or last saved.
    saved: String,
    /// The document merged over the defaults: the values in effect.
    merged: toml::Table,
    defaults: toml::Table,
    pub page: usize,
    /// Index into `rows()`.
    pub sel: usize,
    pub scroll: usize,
    pub edit: Option<Edit>,
    /// Asking what to do with unsaved changes before closing.
    pub confirm: bool,
    /// Last error or warning, shown instead of the field help.
    pub status: String,
}

impl Settings {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        Self::from_text(path, text)
    }

    pub fn from_text(path: PathBuf, text: String) -> Result<Self, String> {
        let doc: DocumentMut = text.parse().map_err(|e| format!("config.toml: {e}"))?;
        let merged = config::merged(&text).map_err(|e| format!("{e:#}"))?;
        let mut s = Self {
            path,
            doc,
            saved: text,
            merged,
            defaults: config::merged("").expect("built-in config is valid"),
            page: 0,
            sel: 0,
            scroll: 0,
            edit: None,
            confirm: false,
            status: String::new(),
        };
        s.select(0, 1);
        Ok(s)
    }

    pub fn text(&self) -> String {
        self.doc.to_string()
    }

    pub fn saved_text(&self) -> &str {
        &self.saved
    }

    pub fn modified(&self) -> bool {
        self.text() != self.saved
    }

    // ------------------------------------------------------------ reading

    fn lookup<'a>(table: &'a toml::Table, path: &str) -> Option<&'a toml::Value> {
        let mut parts = path.split('.');
        let mut v = table.get(parts.next()?)?;
        for p in parts {
            v = v.as_table()?.get(p)?;
        }
        Some(v)
    }

    /// The value in effect for a field, as text; theme keys fall back to the preset's.
    pub fn value(&self, f: &Field) -> Option<String> {
        if let Some(v) = Self::lookup(&self.merged, f.path) {
            return Some(match v {
                toml::Value::String(s) => s.clone(),
                toml::Value::Float(x) => format!("{x:.2}"),
                other => other.to_string(),
            });
        }
        if let Some(key) = f.path.strip_prefix("theme.") {
            let preset = Self::lookup(&self.merged, "theme.preset").and_then(|v| v.as_str()).unwrap_or("breeze-dark");
            return crate::theme::preset_value(preset, key).map(str::to_string);
        }
        // Unset choices, such as the wallpaper kind, default to the first option.
        match f.kind {
            Kind::Choice(opts) => opts.first().map(|o| o.to_string()),
            _ => None,
        }
    }

    /// Whether the field is set in the file rather than coming from a default or the preset.
    pub fn is_set(&self, f: &Field) -> bool {
        let Some((table, key)) = f.path.rsplit_once('.') else { return false };
        table_ref(&self.doc, table).is_some_and(|t| t.contains_key(key))
    }

    /// Panel widgets: (name, enabled), enabled ones first in panel order.
    pub fn widgets(&self) -> Vec<(String, bool)> {
        let on: Vec<String> = Self::lookup(&self.merged, "panel.widgets")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let mut out: Vec<(String, bool)> = on.iter().map(|w| (w.clone(), true)).collect();
        out.extend(WIDGETS.iter().filter(|w| !on.iter().any(|o| o == *w)).map(|w| (w.to_string(), false)));
        out
    }

    /// Bindings of a table, sorted by key: (key, action).
    pub fn bindings(&self, table: &str) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = Self::lookup(&self.merged, table)
            .and_then(|t| t.as_table())
            .map(|t| t.iter().map(|(k, a)| (k.clone(), a.as_str().unwrap_or("").to_string())).collect())
            .unwrap_or_default();
        v.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then(a.0.cmp(&b.0)));
        v
    }

    pub fn rows(&self) -> Vec<Row> {
        let Some(page) = PAGES.get(self.page) else { return Vec::new() };
        let mut rows: Vec<Row> = page.fields.iter().map(Row::Field).collect();
        match page.extra {
            Extra::None => {}
            Extra::Widgets => {
                rows.push(Row::Heading("Widgets"));
                rows.extend((0..self.widgets().len()).map(Row::Widget));
            }
            Extra::Bindings => {
                for &(table, heading) in BINDING_TABLES {
                    rows.push(Row::Heading(heading));
                    rows.extend((0..self.bindings(table).len()).map(|i| Row::Binding(table, i)));
                    rows.push(Row::AddBinding(table));
                }
            }
        }
        rows
    }

    /// One line describing the selected row.
    pub fn help(&self) -> String {
        match self.rows().get(self.sel) {
            Some(Row::Field(f)) => f.help.to_string(),
            Some(Row::Widget(_)) => {
                "Enter toggles; shift+up/down move it. Widgets before tasks sit left, after it right".into()
            }
            Some(Row::Binding(..)) => {
                "Enter edits the action, Delete unbinds; actions are listed in config.toml".into()
            }
            Some(Row::AddBinding(_)) => "Enter, then press the key to bind".into(),
            _ => String::new(),
        }
    }

    // ------------------------------------------------------------ navigation

    /// Move the selection by `delta` rows, skipping headings; `dir` breaks ties at headings.
    fn select(&mut self, from: usize, dir: isize) {
        let rows = self.rows();
        let n = rows.len() as isize;
        if !rows.iter().any(Row::selectable) {
            self.sel = 0;
            return;
        }
        let mut i = (from as isize).clamp(0, n - 1);
        while !rows[i as usize].selectable() {
            i += dir;
            if !(0..n).contains(&i) {
                // Ran off the end: search the other way.
                return self.select(from.min(n as usize - 1), -dir);
            }
        }
        self.sel = i as usize;
    }

    pub fn move_sel(&mut self, delta: isize) {
        let n = self.rows().len() as isize;
        let to = (self.sel as isize + delta).clamp(0, (n - 1).max(0));
        self.select(to as usize, if delta < 0 { -1 } else { 1 });
    }

    pub fn set_page(&mut self, page: usize) {
        if page < PAGES.len() && page != self.page {
            self.page = page;
            self.scroll = 0;
            self.edit = None;
            self.select(0, 1);
        }
        self.status.clear();
    }

    // ------------------------------------------------------------ changing

    /// Apply `f` to the document and parse the result; a change that makes the config invalid
    /// is undone.
    fn change(&mut self, f: impl FnOnce(&mut DocumentMut) -> Result<(), String>) -> Outcome {
        let backup = self.doc.clone();
        if let Err(e) = f(&mut self.doc) {
            self.status = e;
            return Outcome::None;
        }
        let text = self.doc.to_string();
        let parsed = config::merged(&text).and_then(|m| {
            let cfg: Config = toml::Value::Table(m.clone()).try_into()?;
            Ok((m, cfg))
        });
        match parsed {
            Ok((merged, cfg)) => {
                self.merged = merged;
                self.status.clear();
                Outcome::Changed(Box::new(cfg))
            }
            Err(e) => {
                self.doc = backup;
                self.status = format!("{e:#}");
                Outcome::None
            }
        }
    }

    fn set_field(&mut self, f: &'static Field, value: Value) -> Outcome {
        self.change(|doc| {
            let (table, key) = f.path.rsplit_once('.').ok_or("bad path")?;
            set_in(doc, table, key, value)
        })
    }

    /// Step a field or toggle: `dir` is +1 or -1.
    pub fn adjust(&mut self, dir: i64) -> Outcome {
        match self.rows().get(self.sel).copied() {
            Some(Row::Field(f)) => {
                let cur = self.value(f);
                match f.kind {
                    Kind::Bool => self.set_field(f, Value::from(cur.as_deref() != Some("true"))),
                    Kind::Choice(opts) => {
                        let n = opts.len() as i64;
                        let next = match cur.and_then(|c| opts.iter().position(|o| *o == c)) {
                            Some(i) => (i as i64 + dir).rem_euclid(n),
                            None => 0,
                        };
                        self.set_field(f, Value::from(opts[next as usize]))
                    }
                    Kind::Int { min, max, step } => {
                        let v = cur.and_then(|c| c.parse::<i64>().ok()).unwrap_or(min);
                        self.set_field(f, Value::from((v + dir * step).clamp(min, max)))
                    }
                    Kind::Ratio { min, max, step } => {
                        let v = cur.and_then(|c| c.parse::<f64>().ok()).unwrap_or(min);
                        let v = ((v + dir as f64 * step).clamp(min, max) * 100.0).round() / 100.0;
                        self.set_field(f, Value::from(v))
                    }
                    Kind::Text | Kind::Color | Kind::Key => Outcome::None,
                }
            }
            Some(Row::Widget(i)) => self.toggle_widget(i),
            _ => Outcome::None,
        }
    }

    fn write_widgets(&mut self, on: Vec<String>) -> Outcome {
        self.change(|doc| set_in(doc, "panel", "widgets", Value::Array(on.into_iter().collect())))
    }

    fn toggle_widget(&mut self, i: usize) -> Outcome {
        let ws = self.widgets();
        let Some((name, enabled)) = ws.get(i).cloned() else { return Outcome::None };
        let mut on: Vec<String> = ws.into_iter().filter(|(_, e)| *e).map(|(n, _)| n).collect();
        if enabled {
            on.retain(|n| *n != name);
        } else {
            on.push(name.clone());
        }
        let out = self.write_widgets(on);
        // Follow the widget to its new row.
        if let Some(j) = self.widgets().iter().position(|(n, _)| *n == name)
            && let Some(row) = self.rows().iter().position(|r| matches!(r, Row::Widget(k) if *k == j))
        {
            self.sel = row;
        }
        out
    }

    /// Move the selected enabled widget left (-1) or right (+1) in the panel.
    pub fn move_widget(&mut self, dir: isize) -> Outcome {
        let Some(Row::Widget(i)) = self.rows().get(self.sel).copied() else { return Outcome::None };
        let ws = self.widgets();
        let mut on: Vec<String> = ws.iter().filter(|(_, e)| *e).map(|(n, _)| n.clone()).collect();
        let j = i as isize + dir;
        if i >= on.len() || j < 0 || j as usize >= on.len() {
            return Outcome::None;
        }
        on.swap(i, j as usize);
        let out = self.write_widgets(on);
        self.sel = (self.sel as isize + dir) as usize;
        out
    }

    fn set_binding(&mut self, table: &'static str, key: &str, action: &str) -> Outcome {
        self.change(|doc| set_in(doc, table, key, Value::from(action)))
    }

    /// Reset the selected field to its default, or unbind the selected key.
    pub fn reset(&mut self) -> Outcome {
        match self.rows().get(self.sel).copied() {
            Some(Row::Field(f)) => {
                // Writing the default keeps the comments around the key; values without one
                // (theme colors fall back to the preset) are removed.
                let default = Self::lookup(&self.defaults, f.path).and_then(|v| v.to_string().parse::<Value>().ok());
                self.change(|doc| {
                    let (table, key) = f.path.rsplit_once('.').ok_or("bad path")?;
                    match default {
                        Some(v) => set_in(doc, table, key, v),
                        None => {
                            if let Some(t) = table_mut(doc, table, false) {
                                t.remove(key);
                            }
                            Ok(())
                        }
                    }
                })
            }
            Some(Row::Binding(table, i)) => {
                let Some((key, _)) = self.bindings(table).get(i).cloned() else { return Outcome::None };
                // A default binding stays unless it is explicitly turned off.
                let default = Self::lookup(&self.defaults, table)
                    .and_then(|t| t.as_table())
                    .is_some_and(|t| t.contains_key(&key));
                let out = self.change(|doc| {
                    if default {
                        set_in(doc, table, &key, Value::from("none"))
                    } else {
                        if let Some(t) = table_mut(doc, table, false) {
                            t.remove(&key);
                        }
                        Ok(())
                    }
                });
                self.select(self.sel, -1);
                out
            }
            _ => Outcome::None,
        }
    }

    /// Enter on the selected row.
    pub fn activate(&mut self) -> Outcome {
        match self.rows().get(self.sel).copied() {
            Some(Row::Field(f)) => match f.kind {
                Kind::Text | Kind::Color => {
                    let text = if f.path.starts_with("theme.") && !self.is_set(f) {
                        String::new()
                    } else {
                        self.value(f).unwrap_or_default()
                    };
                    self.edit = Some(Edit::Text { field: f, text });
                    Outcome::None
                }
                Kind::Key => {
                    self.edit = Some(Edit::CaptureField(f));
                    Outcome::None
                }
                _ => self.adjust(1),
            },
            Some(Row::Widget(_)) => self.adjust(1),
            Some(Row::Binding(table, i)) => {
                if let Some((key, action)) = self.bindings(table).get(i).cloned() {
                    let text = if action == "none" { String::new() } else { action };
                    self.edit = Some(Edit::Action { table, key, text });
                }
                Outcome::None
            }
            Some(Row::AddBinding(table)) => {
                self.edit = Some(Edit::CaptureBinding(table));
                Outcome::None
            }
            _ => Outcome::None,
        }
    }

    fn commit(&mut self, edit: Edit) -> Outcome {
        match edit {
            Edit::Text { field: f, text } => {
                // Spaces matter in text such as the Kickoff label; colors are trimmed.
                let text = if matches!(f.kind, Kind::Color) { text.trim().to_string() } else { text };
                match f.kind {
                    Kind::Color if text.is_empty() => {
                        // Back to the preset's color.
                        self.reset()
                    }
                    Kind::Color if crate::theme::parse_color(&text).is_none() => {
                        self.status = format!("'{text}' is not a color: use #rrggbb, a name or 0-255");
                        self.edit = Some(Edit::Text { field: f, text });
                        Outcome::None
                    }
                    _ => self.set_field(f, Value::from(text)),
                }
            }
            Edit::Action { table, key, text } => {
                let action = text.trim();
                let action = if action.is_empty() { "none" } else { action };
                if action != "none" && Action::parse(action).is_none() {
                    self.status = format!("unknown action '{action}'");
                    self.edit = Some(Edit::Action { table, key, text });
                    return Outcome::None;
                }
                let out = self.set_binding(table, &key, action);
                if let Some(i) = self.bindings(table).iter().position(|(k, _)| *k == key)
                    && let Some(row) =
                        self.rows().iter().position(|r| matches!(r, Row::Binding(t, j) if *t == table && *j == i))
                {
                    self.sel = row;
                }
                out
            }
            Edit::CaptureField(_) | Edit::CaptureBinding(_) => Outcome::None,
        }
    }

    /// The key captured for a key field or a new binding.
    fn captured(&mut self, edit: Edit, k: &KeyEvent) -> Outcome {
        let Some(name) = KeySpec::from_event(k).to_config() else {
            self.status = "that key cannot be bound".into();
            self.edit = Some(edit);
            return Outcome::None;
        };
        match edit {
            Edit::CaptureField(f) => self.set_field(f, Value::from(name)),
            Edit::CaptureBinding(table) => {
                let text = self.bindings(table).into_iter().find(|(key, _)| *key == name).map(|(_, a)| a);
                let text = text.filter(|a| a != "none").unwrap_or_default();
                self.edit = Some(Edit::Action { table, key: name, text });
                Outcome::None
            }
            other => {
                self.edit = Some(other);
                Outcome::None
            }
        }
    }

    // ------------------------------------------------------------ input

    pub fn key(&mut self, k: &KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        if self.confirm {
            self.confirm = false;
            return match k.code {
                KeyCode::Char('s' | 'S') | KeyCode::Enter => Outcome::SaveAndClose,
                KeyCode::Char('d' | 'D') => Outcome::Close,
                _ => Outcome::None,
            };
        }
        if let Some(edit) = self.edit.take() {
            return match edit {
                Edit::CaptureField(_) | Edit::CaptureBinding(_) => {
                    if k.code == KeyCode::Esc {
                        self.status.clear();
                        Outcome::None
                    } else {
                        self.captured(edit, k)
                    }
                }
                Edit::Text { field, mut text } => self.type_key(k, &mut text, |text| Edit::Text { field, text }),
                Edit::Action { table, key, mut text } => {
                    self.type_key(k, &mut text, |text| Edit::Action { table, key, text })
                }
            };
        }
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Char('s') if ctrl => return Outcome::Save,
            KeyCode::Esc | KeyCode::Char('q') if !ctrl && !alt => {
                if self.modified() {
                    self.confirm = true;
                } else {
                    return Outcome::Close;
                }
            }
            KeyCode::Tab => self.set_page((self.page + 1) % PAGES.len()),
            KeyCode::BackTab => self.set_page((self.page + PAGES.len() - 1) % PAGES.len()),
            KeyCode::Char(c @ '1'..='9') if !ctrl && !alt => self.set_page(c as usize - '1' as usize),
            KeyCode::Up | KeyCode::Char('K') if shift || k.code == KeyCode::Char('K') => return self.move_widget(-1),
            KeyCode::Down | KeyCode::Char('J') if shift || k.code == KeyCode::Char('J') => return self.move_widget(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::PageUp => self.move_sel(-10),
            KeyCode::PageDown => self.move_sel(10),
            KeyCode::Home => self.select(0, 1),
            KeyCode::End => self.select(usize::MAX, -1),
            KeyCode::Left | KeyCode::Char('h') => return self.adjust(-1),
            KeyCode::Right | KeyCode::Char('l') => return self.adjust(1),
            KeyCode::Enter | KeyCode::Char(' ') => return self.activate(),
            KeyCode::Delete | KeyCode::Backspace => return self.reset(),
            _ => {}
        }
        Outcome::None
    }

    fn type_key(&mut self, k: &KeyEvent, text: &mut String, rebuild: impl FnOnce(String) -> Edit) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => {
                self.status.clear();
                return Outcome::None;
            }
            KeyCode::Enter => return self.commit(rebuild(std::mem::take(text))),
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char('u') if ctrl => text.clear(),
            KeyCode::Char(c) if !ctrl && !k.modifiers.contains(KeyModifiers::ALT) => text.push(c),
            _ => {}
        }
        self.edit = Some(rebuild(std::mem::take(text)));
        Outcome::None
    }

    /// Pasted text goes into the field being typed.
    pub fn paste(&mut self, line: &str) {
        if let Some(Edit::Text { text, .. } | Edit::Action { text, .. }) = self.edit.as_mut() {
            text.push_str(line);
        }
    }

    /// A click on row `i`: select it, or activate it when already selected.
    pub fn click(&mut self, i: usize) -> Outcome {
        if self.edit.is_some() || !self.rows().get(i).is_some_and(Row::selectable) {
            return Outcome::None;
        }
        if self.sel == i {
            return self.activate();
        }
        self.sel = i;
        self.status.clear();
        Outcome::None
    }

    /// Write the document to the file, unless the file changed on disk since it was read.
    pub fn save(&mut self) -> Result<(), String> {
        let on_disk = std::fs::read_to_string(&self.path).unwrap_or_default();
        if on_disk != self.saved {
            return Err("config.toml changed on disk since Settings opened; close and reopen to edit it".into());
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = self.text();
        // Written in place, so a config.toml symlinked from a dotfiles repository stays a link.
        std::fs::write(&self.path, &text).map_err(|e| format!("{}: {e}", self.path.display()))?;
        self.saved = text;
        Ok(())
    }
}

fn table_ref<'a>(doc: &'a DocumentMut, path: &str) -> Option<&'a dyn TableLike> {
    let mut t: &dyn TableLike = doc.as_table();
    for part in path.split('.') {
        t = t.get(part)?.as_table_like()?;
    }
    Some(t)
}

/// The table at a dotted path, created (as implicit parents and a normal table) when `create`.
fn table_mut<'a>(doc: &'a mut DocumentMut, path: &str, create: bool) -> Option<&'a mut dyn TableLike> {
    let mut t: &mut dyn TableLike = doc.as_table_mut();
    let parts: Vec<&str> = path.split('.').collect();
    for (i, part) in parts.iter().enumerate() {
        if t.get(part).is_none() {
            if !create {
                return None;
            }
            let mut new = toml_edit::Table::new();
            // Parents of a new table need no header of their own: [keys.global], not [keys].
            new.set_implicit(i + 1 < parts.len());
            t.insert(part, Item::Table(new));
        }
        t = t.get_mut(part)?.as_table_like_mut()?;
    }
    Some(t)
}

/// Set `key` in a table, keeping the comment after an existing value.
fn set_in(doc: &mut DocumentMut, table: &str, key: &str, mut value: Value) -> Result<(), String> {
    let t = table_mut(doc, table, true).ok_or_else(|| format!("{table} is not a table in config.toml"))?;
    match t.get_mut(key) {
        Some(Item::Value(old)) => {
            let decor = old.decor().clone();
            *value.decor_mut() = decor;
            *old = value;
        }
        _ => {
            t.insert(key, Item::Value(value));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(s: &mut Settings, code: KeyCode) -> Outcome {
        s.key(&KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn open(text: &str) -> Settings {
        Settings::from_text(PathBuf::from("/nonexistent/config.toml"), text.into()).unwrap()
    }

    fn select_field(s: &mut Settings, path: &str) {
        let (page, row) = PAGES
            .iter()
            .enumerate()
            .find_map(|(p, pg)| pg.fields.iter().position(|f| f.path == path).map(|r| (p, r)))
            .unwrap();
        s.set_page(page);
        s.sel = row;
    }

    #[test]
    fn edits_keep_comments_and_other_values() {
        let mut s = open("# mine\n[general]\n# gaps between tiles\ngaps = 2 # small\nshell = \"zsh\"\n");
        select_field(&mut s, "general.gaps");
        let Outcome::Changed(cfg) = press(&mut s, KeyCode::Right) else { panic!("no change") };
        assert_eq!(cfg.general.gaps, 3);
        assert_eq!(s.text(), "# mine\n[general]\n# gaps between tiles\ngaps = 3 # small\nshell = \"zsh\"\n");
        assert!(s.modified());
        // Back to the default value, comments kept.
        s.reset();
        assert_eq!(s.text(), "# mine\n[general]\n# gaps between tiles\ngaps = 0 # small\nshell = \"zsh\"\n");
    }

    #[test]
    fn new_values_create_tables() {
        let mut s = open("");
        select_field(&mut s, "theme.preset");
        let Outcome::Changed(cfg) = press(&mut s, KeyCode::Right) else { panic!("no change") };
        assert_eq!(cfg.theme.preset, "breeze-light");
        select_field(&mut s, "general.focus_follows_mouse");
        press(&mut s, KeyCode::Enter);
        assert_eq!(s.text(), "[theme]\npreset = \"breeze-light\"\n\n[general]\nfocus_follows_mouse = true\n");
        select_field(&mut s, "general.master_ratio");
        press(&mut s, KeyCode::Right);
        assert!(s.text().contains("master_ratio = 0.6\n"), "{}", s.text());
    }

    #[test]
    fn text_and_color_fields_validate() {
        let mut s = open("");
        select_field(&mut s, "theme.accent");
        assert_eq!(
            s.value(PAGES[2].fields.iter().find(|f| f.path == "theme.accent").unwrap()).as_deref(),
            Some("#3daee9")
        );
        press(&mut s, KeyCode::Enter);
        for c in "nocolor".chars() {
            press(&mut s, KeyCode::Char(c));
        }
        assert!(matches!(press(&mut s, KeyCode::Enter), Outcome::None));
        assert!(s.status.contains("not a color") && s.edit.is_some());
        s.key(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        s.paste("#ff0000");
        let Outcome::Changed(cfg) = press(&mut s, KeyCode::Enter) else { panic!("no change") };
        assert_eq!(cfg.theme.overrides.get("accent").map(String::as_str), Some("#ff0000"));
        // Emptying a color returns to the preset's.
        press(&mut s, KeyCode::Enter);
        s.key(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        press(&mut s, KeyCode::Enter);
        assert!(!s.text().contains("accent"));
    }

    #[test]
    fn widgets_toggle_and_move() {
        let mut s = open("[panel]\nwidgets = [\"kickoff\", \"tasks\", \"clock\"]\n");
        s.set_page(3);
        let first = s.rows().iter().position(|r| matches!(r, Row::Widget(0))).unwrap();
        s.sel = first + 2; // clock
        s.key(&KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
        assert_eq!(s.widgets()[1], ("clock".to_string(), true));
        assert_eq!(s.sel, first + 1);
        // Turning "pager" on appends it.
        let pager = s.widgets().iter().position(|(n, _)| n == "pager").unwrap();
        s.sel = first + pager;
        let Outcome::Changed(cfg) = press(&mut s, KeyCode::Enter) else { panic!("no change") };
        assert_eq!(cfg.panel.widgets, ["kickoff", "clock", "tasks", "pager"]);
        assert_eq!(s.widgets()[s.sel - first], ("pager".to_string(), true));
    }

    #[test]
    fn bindings_add_edit_and_unbind() {
        let mut s = open("");
        s.set_page(4);
        let add = s.rows().iter().position(|r| matches!(r, Row::AddBinding("keys.global"))).unwrap();
        s.sel = add;
        press(&mut s, KeyCode::Enter);
        s.key(&KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT));
        assert!(matches!(&s.edit, Some(Edit::Action { key, .. }) if key == "alt+g"));
        for c in "bogus".chars() {
            press(&mut s, KeyCode::Char(c));
        }
        press(&mut s, KeyCode::Enter);
        assert!(s.status.contains("unknown action"));
        s.key(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        s.paste("overview");
        let Outcome::Changed(cfg) = press(&mut s, KeyCode::Enter) else { panic!("no change") };
        assert_eq!(cfg.keys.global.get("alt+g").map(String::as_str), Some("overview"));
        assert!(s.text().contains("[keys.global]\n\"alt+g\" = \"overview\"\n"), "{}", s.text());
        assert!(!s.text().contains("[keys]\n"));
        // The selection follows the new binding; deleting a user binding removes it.
        assert!(matches!(s.rows()[s.sel], Row::Binding("keys.global", _)));
        press(&mut s, KeyCode::Delete);
        assert!(!s.text().contains("alt+g"));
        // Deleting a default binding turns it off explicitly.
        let i = s.bindings("keys.global").iter().position(|(k, _)| k == "alt+f4").unwrap();
        s.sel = s.rows().iter().position(|r| matches!(r, Row::Binding("keys.global", j) if *j == i)).unwrap();
        let Outcome::Changed(cfg) = press(&mut s, KeyCode::Delete) else { panic!("no change") };
        assert_eq!(cfg.keys.global.get("alt+f4").map(String::as_str), Some("none"));
    }

    #[test]
    fn prefix_key_is_captured() {
        let mut s = open("");
        select_field(&mut s, "general.prefix");
        press(&mut s, KeyCode::Enter);
        let Outcome::Changed(cfg) = s.key(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)) else {
            panic!("no change")
        };
        assert_eq!(cfg.general.prefix, "ctrl+a");
    }

    #[test]
    fn closing_asks_about_unsaved_changes() {
        let mut s = open("");
        assert!(matches!(press(&mut s, KeyCode::Esc), Outcome::Close));
        select_field(&mut s, "general.gaps");
        press(&mut s, KeyCode::Right);
        assert!(matches!(press(&mut s, KeyCode::Esc), Outcome::None));
        assert!(s.confirm);
        assert!(matches!(press(&mut s, KeyCode::Esc), Outcome::None));
        assert!(!s.confirm);
        press(&mut s, KeyCode::Esc);
        assert!(matches!(press(&mut s, KeyCode::Char('d')), Outcome::Close));
    }

    #[test]
    #[cfg_attr(miri, ignore = "writes files")]
    fn save_writes_and_refuses_after_outside_changes() {
        let dir = std::env::temp_dir().join(format!("tuidesk-settings-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Settings::open(path.clone()).unwrap();
        select_field(&mut s, "general.dim_inactive");
        press(&mut s, KeyCode::Enter);
        s.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[general]\ndim_inactive = true\n");
        assert!(!s.modified());
        std::fs::write(&path, "# edited elsewhere\n").unwrap();
        press(&mut s, KeyCode::Enter);
        assert!(s.save().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_values_follow_the_preset() {
        let mut s = open("[theme]\npreset = \"tty16\"\n");
        let border = PAGES[2].fields.iter().find(|f| f.path == "theme.border").unwrap();
        assert_eq!(s.value(border).as_deref(), Some("ascii"));
        select_field(&mut s, "theme.border");
        let Outcome::Changed(cfg) = press(&mut s, KeyCode::Right) else { panic!("no change") };
        assert_eq!(cfg.theme.overrides.get("border").map(String::as_str), Some("plain"));
    }

    #[test]
    fn every_field_has_a_value_or_preset() {
        let s = open("");
        for page in PAGES {
            for f in page.fields {
                let optional = f.path == "wallpaper.default.text";
                assert!(optional || s.value(f).is_some(), "{} has no value", f.path);
            }
        }
    }
}

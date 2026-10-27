//! Keyboard and mouse handling for `App`.

use std::time::{Duration, Instant};

use alacritty_terminal::selection::SelectionType;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::app::{Action, App, CopyState, Kickoff, MenuItem, Mode, PanelHit, RenameTarget, SpawnRequest};
use crate::keys::{KeySpec, encode_key, encode_mouse};
use crate::settings::Outcome;
use crate::term::{MouseMode, ViMotion};
use crate::wm::{Button, Edges, Hit, MIN_H, MIN_W, Side};

#[derive(Clone, Copy)]
pub enum Drag {
    /// `origin` is the frame before the move, restored when dropped on a pager desktop.
    Move {
        id: u64,
        grab_dx: u16,
        grab_dy: u16,
        moved: bool,
        origin: Rect,
    },
    /// A task button being dragged, possibly onto a pager desktop.
    Task {
        id: u64,
        moved: bool,
    },
    /// An overview card being dragged, possibly onto a desktop button.
    Card {
        id: u64,
        moved: bool,
    },
    Resize {
        id: u64,
        edges: Edges,
        start: Rect,
        mx: u16,
        my: u16,
    },
    MasterRatio,
    /// Kickoff's scrollbar thumb.
    MenuScrollbar,
    /// Selecting text in a window.
    Select {
        id: u64,
        moved: bool,
    },
}

const MULTI_CLICK: Duration = Duration::from_millis(400);
/// Without key release events, the switcher commits after this much inactivity.
const SWITCHER_IDLE: Duration = Duration::from_millis(900);

pub(crate) fn inside(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// Lower score sorts first; None means no match.
pub(crate) fn match_score(query: &str, fields: &[&str]) -> Option<u8> {
    if query.trim().is_empty() {
        return Some(3);
    }
    let q = query.trim();
    let name = fields[0].to_lowercase();
    if name.starts_with(q) {
        return Some(0);
    }
    if name.contains(q) {
        return Some(1);
    }
    if fields[1..].iter().any(|f| f.to_lowercase().contains(q)) {
        return Some(2);
    }
    let mut chars = name.chars();
    q.chars().all(|c| c == ' ' || chars.any(|n| n == c)).then_some(4)
}

fn open_url(url: &str) {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        // Not `cmd /c start`: cmd would interpret & and ^ in the URL.
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    let _ = cmd
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

impl App {
    // ---------------------------------------------------------------- kickoff

    pub fn categories(&self) -> Vec<String> {
        let mut cats = vec!["All".to_string()];
        for a in &self.apps {
            if !cats.contains(&a.category) {
                cats.push(a.category.clone());
            }
        }
        cats.push("Session".into());
        cats
    }

    pub fn menu_items(&self) -> Vec<MenuItem> {
        let cats = self.categories();
        let cat = cats.get(self.kickoff.category).map(String::as_str).unwrap_or("All");
        let q = self.kickoff.query.to_lowercase();
        let mut scored: Vec<(u8, MenuItem)> = Vec::new();
        for (i, a) in self.apps.iter().enumerate() {
            if cat != "All" && cat != a.category {
                continue;
            }
            if let Some(s) = match_score(&q, &[&a.name, &a.description, &a.command]) {
                scored.push((s, MenuItem::App(i)));
            }
        }
        if cat == "All" || cat == "Session" {
            let mut session = vec![("System Settings", MenuItem::Settings), ("Reload configuration", MenuItem::Reload)];
            if self.server {
                session.push(("Detach", MenuItem::Detach));
            }
            session.push(("Quit tuidesk", MenuItem::Quit));
            for (label, item) in session {
                if let Some(s) = match_score(&q, &[label]) {
                    scored.push((s, item));
                }
            }
        }
        scored.sort_by_key(|(s, _)| *s);
        let mut items: Vec<MenuItem> = scored.into_iter().map(|(_, i)| i).collect();
        if !q.trim().is_empty() {
            items.push(MenuItem::Run(self.kickoff.query.trim().to_string()));
        }
        items
    }

    pub fn menu_label(&self, item: &MenuItem) -> (String, String) {
        match item {
            MenuItem::App(i) => (self.apps[*i].name.clone(), self.apps[*i].description.clone()),
            MenuItem::Run(q) => (format!("Run: {q}"), "in a new window".into()),
            MenuItem::Settings => ("System Settings".into(), "theme, panel, keys and more".into()),
            MenuItem::Reload => ("Reload configuration".into(), "config.toml and init.lua".into()),
            MenuItem::Detach => ("Detach".into(), "keep running, reattach with tuidesk".into()),
            MenuItem::Quit => ("Quit tuidesk".into(), "closes all windows".into()),
        }
    }

    fn activate(&mut self, item: MenuItem) {
        self.mode = Mode::Normal;
        match item {
            MenuItem::App(i) => {
                let a = self.apps[i].clone();
                self.spawn(SpawnRequest { command: Some(a.command), name: a.name, cwd: None, restore: None });
            }
            MenuItem::Run(q) => {
                self.spawn_command(q);
            }
            MenuItem::Settings => self.open_settings(),
            MenuItem::Reload => self.reload(),
            MenuItem::Detach => self.detach("detached"),
            MenuItem::Quit => self.quit = true,
        }
    }

    fn kickoff_key(&mut self, k: &KeyEvent) {
        let items = self.menu_items();
        let n_cats = self.categories().len();
        let ko = &mut self.kickoff;
        match k.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                if let Some(item) = items.get(ko.selected).cloned() {
                    self.activate(item);
                }
            }
            KeyCode::Up => ko.selected = ko.selected.saturating_sub(1),
            KeyCode::Down => ko.selected = (ko.selected + 1).min(items.len().saturating_sub(1)),
            KeyCode::PageUp => ko.selected = ko.selected.saturating_sub(8),
            KeyCode::PageDown => ko.selected = (ko.selected + 8).min(items.len().saturating_sub(1)),
            KeyCode::Home => ko.selected = 0,
            KeyCode::End => ko.selected = items.len().saturating_sub(1),
            KeyCode::Right | KeyCode::Tab => {
                ko.category = (ko.category + 1) % n_cats;
                ko.selected = 0;
            }
            KeyCode::Left | KeyCode::BackTab => {
                ko.category = (ko.category + n_cats - 1) % n_cats;
                ko.selected = 0;
            }
            KeyCode::Backspace => {
                ko.query.pop();
                ko.selected = 0;
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                ko.query.clear();
                ko.selected = 0;
            }
            KeyCode::Char(c) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                ko.query.push(c);
                ko.selected = 0;
                ko.category = 0;
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- keyboard

    pub(crate) fn handle_key(&mut self, k: KeyEvent) {
        self.dirty = true;
        if self.mode == Mode::Switcher {
            self.switcher_key(&k);
            return;
        }
        if k.kind == KeyEventKind::Release || matches!(k.code, KeyCode::Modifier(_)) {
            return;
        }
        let spec = KeySpec::from_event(&k);
        match self.mode {
            Mode::Prefix => {
                self.mode = Mode::Normal;
                if spec == self.prefix_key {
                    self.send_key_to_focused(&k);
                } else if self.lua_binding(&spec, true) {
                } else if let Some(a) = self.prefix_keys.get(&spec).cloned() {
                    self.run_action(a);
                }
            }
            Mode::Kickoff => match self.global_keys.get(&spec) {
                Some(Action::Kickoff) => self.mode = Mode::Normal,
                _ if spec == self.prefix_key => self.mode = Mode::Normal,
                _ => self.kickoff_key(&k),
            },
            Mode::History => match k.code {
                KeyCode::Char('c') => self.notes.clear(),
                _ => self.mode = Mode::Normal,
            },
            Mode::Move => self.window_mode_key(&k),
            Mode::Copy => self.copy_key(&k),
            Mode::Clipboard => self.clipboard_key(&k),
            Mode::Help => self.help_key(&k),
            Mode::WindowMenu => self.win_menu_key(&k),
            Mode::Rename => self.rename_key(&k),
            Mode::Runner => self.runner_key(&k),
            Mode::Settings => {
                if let Some(s) = self.settings.as_mut() {
                    let outcome = s.key(&k);
                    self.settings_outcome(outcome);
                } else {
                    self.mode = Mode::Normal;
                }
            }
            Mode::Overview => self.overview_key(&k),
            Mode::Region => match k.code {
                KeyCode::Enter => self.capture_region(),
                _ => self.mode = Mode::Normal,
            },
            Mode::Switcher => {}
            Mode::Normal => {
                if spec == self.prefix_key {
                    self.mode = Mode::Prefix;
                } else if self.lua_binding(&spec, false) {
                } else if let Some(a) = self.global_keys.get(&spec).cloned() {
                    self.run_action(a);
                } else {
                    self.send_key_to_focused(&k);
                }
            }
        }
    }

    fn switcher_key(&mut self, k: &KeyEvent) {
        let alt_key = matches!(
            k.code,
            KeyCode::Modifier(
                ModifierKeyCode::LeftAlt
                    | ModifierKeyCode::RightAlt
                    | ModifierKeyCode::LeftMeta
                    | ModifierKeyCode::RightMeta
                    | ModifierKeyCode::IsoLevel3Shift
            )
        );
        if k.kind == KeyEventKind::Release {
            // Letting go of Alt picks the highlighted window.
            if alt_key || !k.modifiers.contains(KeyModifiers::ALT) {
                self.close_switcher(true);
            }
            return;
        }
        let backwards = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Modifier(_) => {}
            KeyCode::Tab if backwards => self.run_action(Action::SwitcherPrev),
            KeyCode::Tab | KeyCode::Down | KeyCode::Right | KeyCode::Char('j') => self.run_action(Action::Switcher),
            KeyCode::BackTab | KeyCode::Up | KeyCode::Left | KeyCode::Char('k') => {
                self.run_action(Action::SwitcherPrev)
            }
            KeyCode::Esc => self.close_switcher(false),
            _ => self.close_switcher(true),
        }
    }

    // ---------------------------------------------------------------- copy mode

    fn copy_key(&mut self, k: &KeyEvent) {
        let Some(state) = self.copy.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        let id = state.id;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);

        // Typing a search pattern.
        if let Some((pattern, forward)) = state.search.as_mut() {
            match k.code {
                KeyCode::Esc => state.search = None,
                KeyCode::Enter => {
                    let (pattern, forward) = (pattern.clone(), *forward);
                    state.search = None;
                    if !pattern.is_empty() {
                        self.copy_search(&pattern, forward);
                    }
                }
                KeyCode::Backspace => {
                    pattern.pop();
                }
                KeyCode::Char(c) if !ctrl => pattern.push(c),
                _ => {}
            }
            return;
        }
        state.message.clear();

        let Some(w) = self.wm.get_mut(id) else {
            self.exit_copy_mode();
            return;
        };
        let half = (w.content().height / 2).max(1) as i32;
        let page = w.content().height.max(1) as i32;
        let t = &mut w.term;
        use alacritty_terminal::selection::SelectionType as Sel;
        match k.code {
            KeyCode::Char('u') if ctrl => t.vi_lines(-half),
            KeyCode::Char('d') if ctrl => t.vi_lines(half),
            KeyCode::Char('b') if ctrl => t.vi_lines(-page),
            KeyCode::Char('f') if ctrl => t.vi_lines(page),
            KeyCode::Char('v') if ctrl => t.vi_toggle_selection(Sel::Block),
            KeyCode::PageUp => t.vi_lines(-page),
            KeyCode::PageDown => t.vi_lines(page),
            KeyCode::Char('h') | KeyCode::Left => t.vi_motion(ViMotion::Left),
            KeyCode::Char('j') | KeyCode::Down => t.vi_motion(ViMotion::Down),
            KeyCode::Char('k') | KeyCode::Up => t.vi_motion(ViMotion::Up),
            KeyCode::Char('l') | KeyCode::Right => t.vi_motion(ViMotion::Right),
            KeyCode::Char('w') => t.vi_motion(ViMotion::SemanticRight),
            KeyCode::Char('b') => t.vi_motion(ViMotion::SemanticLeft),
            KeyCode::Char('e') => t.vi_motion(ViMotion::SemanticRightEnd),
            KeyCode::Char('W') => t.vi_motion(ViMotion::WordRight),
            KeyCode::Char('B') => t.vi_motion(ViMotion::WordLeft),
            KeyCode::Char('E') => t.vi_motion(ViMotion::WordRightEnd),
            KeyCode::Char('0') | KeyCode::Home => t.vi_motion(ViMotion::First),
            KeyCode::Char('$') | KeyCode::End => t.vi_motion(ViMotion::Last),
            KeyCode::Char('^') => t.vi_motion(ViMotion::FirstOccupied),
            KeyCode::Char('H') => t.vi_motion(ViMotion::High),
            KeyCode::Char('M') => t.vi_motion(ViMotion::Middle),
            KeyCode::Char('L') => t.vi_motion(ViMotion::Low),
            KeyCode::Char('%') => t.vi_motion(ViMotion::Bracket),
            KeyCode::Char('g') => t.vi_edge(true),
            KeyCode::Char('G') => t.vi_edge(false),
            KeyCode::Char('v') => t.vi_toggle_selection(Sel::Simple),
            KeyCode::Char('V') => t.vi_toggle_selection(Sel::Lines),
            KeyCode::Char('y') | KeyCode::Enter => {
                let text = t.selection_text();
                self.exit_copy_mode();
                match text {
                    Some(text) => {
                        let n = text.lines().count().max(1);
                        self.copy(text);
                        self.notify("Copied", format!("{n} line{}", if n == 1 { "" } else { "s" }));
                    }
                    None => self.notify("Copy mode", "nothing selected (press v to start a selection)"),
                }
            }
            KeyCode::Char('/') => state_search(&mut self.copy, true),
            KeyCode::Char('?') => state_search(&mut self.copy, false),
            KeyCode::Char('n') | KeyCode::Char('N') => {
                let last = self.copy.as_ref().and_then(|c| c.last_search.clone());
                match last {
                    Some((p, fwd)) => self.copy_search(&p, fwd == (k.code == KeyCode::Char('n'))),
                    None => self.set_copy_message("no previous search"),
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                if t.has_selection() && k.code == KeyCode::Esc {
                    t.clear_selection();
                } else {
                    self.exit_copy_mode();
                }
            }
            _ => {}
        }
    }

    fn set_copy_message(&mut self, msg: &str) {
        if let Some(c) = self.copy.as_mut() {
            c.message = msg.to_string();
        }
    }

    fn copy_search(&mut self, pattern: &str, forward: bool) {
        let Some(id) = self.copy.as_ref().map(|c| c.id) else { return };
        let Some(w) = self.wm.get_mut(id) else { return };
        let result = w.term.vi_search(pattern, forward);
        if let Some(c) = self.copy.as_mut() {
            c.last_search = Some((pattern.to_string(), forward));
            c.message = match result {
                Ok(true) => String::new(),
                Ok(false) => format!("not found: {pattern}"),
                Err(e) => format!("bad pattern: {}", e.lines().next().unwrap_or("")),
            };
        }
    }

    // ---------------------------------------------------------------- pickers

    fn clipboard_key(&mut self, k: &KeyEvent) {
        let n = self.clips.len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.clip_sel = self.clip_sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.clip_sel = (self.clip_sel + 1).min(n.saturating_sub(1)),
            KeyCode::Enter => self.paste_clip(self.clip_sel),
            KeyCode::Char(c @ '1'..='9') => self.paste_clip(c as usize - '1' as usize),
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
                if self.clip_sel < n {
                    self.clips.remove(self.clip_sel);
                }
                self.clip_sel = self.clip_sel.min(self.clips.len().saturating_sub(1));
                if self.clips.is_empty() {
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Char('c') => {
                self.clips.clear();
                self.mode = Mode::Normal;
            }
            _ => self.mode = Mode::Normal,
        }
    }

    pub(crate) fn paste_clip(&mut self, idx: usize) {
        self.mode = Mode::Normal;
        let Some(text) = self.clips.get(idx).cloned() else { return };
        self.clipboard = text.clone();
        // Picking an older entry moves it to the top, as in Klipper.
        self.clips.remove(idx);
        self.clips.push_front(text.clone());
        if let Some(id) = self.wm.focused {
            self.paste_into(id, &text);
        }
    }

    /// Rows for the cheat sheet: (section, key, action), filtered by the help search.
    pub fn help_rows(&self) -> Vec<(&'static str, String, String)> {
        let prefix = &self.cfg.general.prefix;
        let mut rows: Vec<(&'static str, String, String)> = Vec::new();
        let mut sorted = |map: &std::collections::HashMap<String, String>, section: &'static str, pre: bool| {
            let mut v: Vec<_> = map.iter().collect();
            v.sort_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)));
            for (key, action) in v.into_iter().filter(|(_, a)| a.trim() != "none") {
                let key = if pre { format!("{prefix}, {key}") } else { key.clone() };
                rows.push((section, key, action.clone()));
            }
        };
        sorted(&self.cfg.keys.global, "Direct", false);
        sorted(&self.cfg.keys.prefix, "After prefix", true);
        for (key, pre) in self.plugins.bindings() {
            let key = if pre { format!("{prefix}, {key}") } else { key };
            rows.push(("Lua", key, "init.lua binding".into()));
        }
        for (key, what) in HELP_STATIC {
            let section = if key.starts_with("copy:") { "Copy mode" } else { "Mouse" };
            rows.push((section, key.trim_start_matches("copy:").to_string(), what.to_string()));
        }
        let f = self.help.filter.to_lowercase();
        rows.retain(|(sec, k, a)| f.is_empty() || format!("{sec} {k} {a}").to_lowercase().contains(&f));
        rows
    }

    fn help_key(&mut self, k: &KeyEvent) {
        let h = &mut self.help;
        match k.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Up => h.scroll = h.scroll.saturating_sub(1),
            KeyCode::Down => h.scroll += 1,
            KeyCode::PageUp => h.scroll = h.scroll.saturating_sub(10),
            KeyCode::PageDown => h.scroll += 10,
            KeyCode::Backspace => {
                h.filter.pop();
                h.scroll = 0;
            }
            KeyCode::Char('?') if h.filter.is_empty() => self.mode = Mode::Normal,
            KeyCode::Char(c) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                h.filter.push(c);
                h.scroll = 0;
            }
            _ => {}
        }
    }

    fn settings_mouse(&mut self, m: &MouseEvent) {
        let (x, y) = (m.column, m.row);
        let Some(s) = self.settings.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        let outcome = match m.kind {
            MouseEventKind::ScrollUp => {
                s.move_sel(-1);
                Outcome::None
            }
            MouseEventKind::ScrollDown => {
                s.move_sel(1);
                Outcome::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, page)) = self.hits.menu_tabs.iter().find(|(r, _)| inside(*r, x, y)) {
                    s.set_page(page);
                    Outcome::None
                } else if let Some(&(_, i)) = self.hits.list.iter().find(|(r, _)| inside(*r, x, y)) {
                    s.click(i)
                } else if self.hits.overlay.is_some_and(|r| inside(r, x, y)) {
                    Outcome::None
                } else {
                    // A click outside is Esc.
                    s.key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
                }
            }
            _ => return,
        };
        self.settings_outcome(outcome);
    }

    fn win_menu_key(&mut self, k: &KeyEvent) {
        let Some(menu) = self.win_menu.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        let n = menu.items.len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => menu.sel = (menu.sel + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => menu.sel = (menu.sel + 1) % n,
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_win_menu(),
            _ => {
                self.win_menu = None;
                self.mode = Mode::Normal;
            }
        }
    }

    fn rename_key(&mut self, k: &KeyEvent) {
        let Some((target, text)) = self.rename.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        match k.code {
            KeyCode::Esc => {
                self.rename = None;
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let (target, text) = (*target, text.trim().to_string());
                match target {
                    RenameTarget::Window(id) => {
                        if let Some(w) = self.wm.get_mut(id) {
                            // An empty name goes back to the program's own title.
                            w.title_override = (!text.is_empty()).then_some(text);
                        }
                    }
                    RenameTarget::Desktop(d) => {
                        if let Some(name) = self.desktop_names.get_mut(d) {
                            *name = text;
                        }
                    }
                }
                self.rename = None;
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
            KeyCode::Char(c) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => text.push(c),
            _ => {}
        }
    }

    fn capture_region(&mut self) {
        self.mode = Mode::Normal;
        if let Some(r) = self.region.rect() {
            let _ = self.screenshot(r.intersection(self.screen), None, None);
        }
    }

    // ---------------------------------------------------------------- overview

    fn overview_key(&mut self, k: &KeyEvent) {
        let list = self.overview_windows();
        let n = list.len();
        let cols = self.overview.cols.max(1);
        let o = &mut self.overview;
        match k.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                if let Some(&id) = list.get(o.sel) {
                    self.mode = Mode::Normal;
                    self.wm.focus(id);
                }
            }
            KeyCode::Left | KeyCode::BackTab => o.sel = o.sel.saturating_sub(1),
            KeyCode::Right | KeyCode::Tab => o.sel = (o.sel + 1).min(n.saturating_sub(1)),
            KeyCode::Up => o.sel = o.sel.saturating_sub(cols),
            KeyCode::Down => o.sel = (o.sel + cols).min(n.saturating_sub(1)),
            KeyCode::Home => o.sel = 0,
            KeyCode::End => o.sel = n.saturating_sub(1),
            KeyCode::Delete => {
                if let Some(&id) = list.get(o.sel) {
                    self.close_window(id);
                    self.mode = Mode::Overview;
                }
            }
            KeyCode::Backspace => {
                o.filter.pop();
                o.sel = 0;
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                o.filter.clear();
                o.sel = 0;
            }
            KeyCode::Char(c) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                o.filter.push(c);
                o.sel = 0;
            }
            _ => {}
        }
    }

    fn overview_mouse(&mut self, m: &MouseEvent) {
        let (x, y) = (m.column, m.row);
        let card = self.hits.cards.iter().find(|(r, _)| inside(*r, x, y)).map(|&(_, id)| id);
        let button = self.hits.desk_buttons.iter().find(|(r, _)| inside(*r, x, y)).map(|&(_, d)| d);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => match (card, button) {
                (Some(id), _) => self.drag = Some(Drag::Card { id, moved: false }),
                (None, Some(d)) => {
                    self.mode = Mode::Normal;
                    self.wm.switch_desktop(d);
                }
                (None, None) => self.mode = Mode::Normal,
            },
            MouseEventKind::Down(MouseButton::Middle) => {
                if let Some(id) = card {
                    self.close_window(id);
                    self.mode = Mode::Overview;
                }
            }
            MouseEventKind::Moved => {
                if let Some(i) = card.and_then(|id| self.overview_windows().iter().position(|&w| w == id))
                    && self.overview.sel != i
                {
                    self.overview.sel = i;
                    self.dirty = true;
                }
            }
            MouseEventKind::ScrollDown => self.overview_key(&KeyEvent::from(KeyCode::Right)),
            MouseEventKind::ScrollUp => self.overview_key(&KeyEvent::from(KeyCode::Left)),
            _ => {}
        }
    }

    pub(crate) fn switcher_timeout(&mut self) {
        if let Some(s) = &self.switcher
            && !s.releases
            && s.last.elapsed() >= SWITCHER_IDLE
        {
            self.close_switcher(true);
        }
    }

    fn lua_binding(&mut self, spec: &KeySpec, prefix: bool) -> bool {
        self.sync_lua();
        let ran = self.plugins.run_binding(spec, prefix);
        if ran {
            self.drain_lua();
        }
        ran
    }

    fn send_key_to_focused(&mut self, k: &KeyEvent) {
        let close_on_exit = self.cfg.general.close_on_exit;
        let Some(w) = self.wm.focused_mut() else { return };
        if w.exit_code.is_some() && !close_on_exit {
            let id = w.id;
            self.close_window(id);
            return;
        }
        w.term.scroll_reset();
        w.term.clear_selection();
        let bytes = encode_key(k, w.term.app_cursor());
        w.term.write(&bytes);
    }

    fn window_mode_key(&mut self, k: &KeyEvent) {
        let Some(id) = self.wm.focused else {
            self.mode = Mode::Normal;
            return;
        };
        let resize = k.modifiers.contains(KeyModifiers::SHIFT);
        let (dx, dy): (i32, i32) = match k.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => (-1, 0),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('L') => (1, 0),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => (0, -1),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => (0, 1),
            KeyCode::Char('m') => {
                self.wm.toggle_maximize(id);
                return;
            }
            _ => {
                self.mode = Mode::Normal;
                return;
            }
        };
        let resize = resize || matches!(k.code, KeyCode::Char('H' | 'J' | 'K' | 'L'));
        let area = self.wm.area;
        if let Some(w) = self.wm.get_mut(id) {
            w.maximized = false;
            w.restore_rect = None;
            let r = &mut w.float_rect;
            if resize {
                // Minimums give way on screens smaller than them (clamp panics if min > max).
                let (min_w, min_h) = (MIN_W.min(area.width) as i32, MIN_H.min(area.height) as i32);
                r.width = (r.width as i32 + dx * 2).clamp(min_w, (area.width as i32).max(min_w)) as u16;
                r.height = (r.height as i32 + dy).clamp(min_h, (area.height as i32).max(min_h)) as u16;
            } else {
                r.x = (r.x as i32 + dx * 2).max(area.x as i32) as u16;
                r.y = (r.y as i32 + dy).max(area.y as i32) as u16;
            }
        }
        self.wm.arrange();
    }

    pub(crate) fn handle_paste(&mut self, text: String) {
        self.dirty = true;
        // Text fields take the first line; never let a paste reach a window hidden behind them.
        let line = text.lines().next().unwrap_or("").to_string();
        match self.mode {
            Mode::Normal | Mode::Prefix => {
                self.mode = Mode::Normal;
                if let Some(id) = self.wm.focused {
                    self.paste_into(id, &text);
                }
            }
            Mode::Kickoff => self.kickoff.query.push_str(&line),
            Mode::Runner => {
                self.runner.query.push_str(&line);
                self.runner.sel = 0;
            }
            Mode::Overview => self.overview.filter.push_str(&line),
            Mode::Help => self.help.filter.push_str(&line),
            Mode::Settings => {
                if let Some(s) = self.settings.as_mut() {
                    s.paste(&line);
                }
            }
            Mode::Rename => {
                if let Some((_, t)) = self.rename.as_mut() {
                    t.push_str(&line);
                }
            }
            Mode::Copy => {
                if let Some((p, _)) = self.copy.as_mut().and_then(|c| c.search.as_mut()) {
                    p.push_str(&line);
                }
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- mouse

    pub(crate) fn handle_mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        if !matches!(m.kind, MouseEventKind::Moved) {
            self.dirty = true;
        }

        if self.drag.is_some() {
            match m.kind {
                MouseEventKind::Drag(_) => {
                    self.drag_to(x, y);
                    return;
                }
                MouseEventKind::Up(_) => {
                    self.end_drag(x, y);
                    return;
                }
                // The release got lost (e.g. outside the terminal): drop the drag, then
                // handle this press normally.
                MouseEventKind::Down(_) => self.cancel_drag(),
                _ => return,
            }
        }
        // A program that got a button press also gets the drag and release, even outside it.
        if let Some(id) = self.mouse_grab {
            match m.kind {
                MouseEventKind::Drag(_) => {
                    self.forward_mouse_clamped(id, &m);
                    return;
                }
                MouseEventKind::Up(_) => {
                    self.mouse_grab = None;
                    self.forward_mouse_clamped(id, &m);
                    return;
                }
                MouseEventKind::Down(_) => self.mouse_grab = None,
                _ => {}
            }
        }

        let down = matches!(m.kind, MouseEventKind::Down(_));

        // Overlays first.
        if self.mode == Mode::Region {
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    self.region.start = Some((x, y));
                    self.region.end = (x, y);
                }
                MouseEventKind::Drag(MouseButton::Left) => self.region.end = (x, y),
                MouseEventKind::Up(MouseButton::Left) => {
                    self.region.end = (x, y);
                    self.capture_region();
                }
                MouseEventKind::Down(_) => self.mode = Mode::Normal,
                _ => {}
            }
            return;
        }
        if self.mode == Mode::Overview {
            self.overview_mouse(&m);
            return;
        }
        if self.mode == Mode::Settings {
            self.settings_mouse(&m);
            return;
        }
        match self.mode {
            Mode::Clipboard | Mode::Help | Mode::WindowMenu | Mode::Rename | Mode::Runner => {
                if !down {
                    if let (MouseEventKind::Moved, Mode::WindowMenu) = (m.kind, self.mode)
                        && let Some(&(_, i)) = self.hits.list.iter().find(|(r, _)| inside(*r, x, y))
                        && let Some(menu) = self.win_menu.as_mut()
                    {
                        menu.sel = i;
                        self.dirty = true;
                    }
                    if let (MouseEventKind::ScrollDown | MouseEventKind::ScrollUp, Mode::Help) = (m.kind, self.mode) {
                        let h = &mut self.help;
                        h.scroll = if m.kind == MouseEventKind::ScrollDown {
                            h.scroll + 3
                        } else {
                            h.scroll.saturating_sub(3)
                        };
                    }
                    return;
                }
                let hit = self.hits.list.iter().find(|(r, _)| inside(*r, x, y)).map(|&(_, i)| i);
                match (self.mode, hit) {
                    (Mode::Clipboard, Some(i)) => self.paste_clip(i),
                    (Mode::Runner, Some(i)) => {
                        if let Some(item) = self.runner_items().get(i).cloned() {
                            self.runner_activate(item);
                        }
                    }
                    (Mode::WindowMenu, Some(i)) => {
                        if let Some(menu) = self.win_menu.as_mut() {
                            menu.sel = i;
                        }
                        self.activate_win_menu();
                    }
                    _ if self.hits.overlay.is_some_and(|r| inside(r, x, y)) => {}
                    _ => {
                        self.win_menu = None;
                        self.rename = None;
                        self.mode = Mode::Normal;
                    }
                }
                return;
            }
            Mode::Copy => {
                let id = self.copy.as_ref().map(|c| c.id);
                match m.kind {
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                        if let Some(w) = id.and_then(|id| self.wm.get_mut(id)) {
                            w.term.scroll(if m.kind == MouseEventKind::ScrollUp { 3 } else { -3 });
                        }
                        return;
                    }
                    // Any click leaves copy mode and is then handled normally.
                    MouseEventKind::Down(_) => self.exit_copy_mode(),
                    _ => return,
                }
            }
            _ => {}
        }
        if self.mode == Mode::Switcher {
            if down {
                let hit = self.hits.switcher.iter().find(|(r, _)| inside(*r, x, y)).map(|&(_, i)| i);
                if let (Some(i), Some(s)) = (hit, self.switcher.as_mut()) {
                    s.idx = i;
                }
                self.close_switcher(hit.is_some());
            }
            return;
        }
        if self.mode == Mode::Kickoff {
            if let Some(menu) = self.hits.menu
                && inside(menu, x, y)
            {
                self.kickoff_mouse(&m);
                return;
            }
            if !down {
                return;
            }
            self.mode = Mode::Normal;
            if self.hits.panel.iter().any(|(r, h)| *h == PanelHit::Kickoff && inside(*r, x, y)) {
                return;
            }
        }
        if self.mode == Mode::History {
            if self.hits.history.is_some_and(|r| inside(r, x, y)) || !down {
                return;
            }
            self.mode = Mode::Normal;
            if self.hits.panel.iter().any(|(r, h)| *h == PanelHit::Bell && inside(*r, x, y)) {
                return;
            }
        }

        if let Some(&(_, idx)) = self.hits.popups.iter().find(|(r, _)| inside(*r, x, y)) {
            if down {
                self.notes.dismiss(idx);
            }
            return;
        }

        if let Some(&(_, hit)) = self.hits.panel.iter().find(|(r, _)| inside(*r, x, y)) {
            self.panel_mouse(&m, hit);
            return;
        }
        if self.hits.panel.iter().any(|(r, _)| r.y == y) {
            if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
                self.wm.cycle(m.kind == MouseEventKind::ScrollDown);
            }
            return;
        }

        let hit = self.wm.hit_test(x, y);
        if matches!(m.kind, MouseEventKind::Moved)
            && self.cfg.general.focus_follows_mouse
            && let Some((id, _)) = hit
            && self.wm.focused != Some(id)
        {
            self.wm.focus(id);
            self.dirty = true;
        }
        let Some((id, hit)) = hit else {
            if m.kind == MouseEventKind::Down(MouseButton::Left)
                && let Some(&(_, i)) = self.hits.widgets.iter().find(|(r, _)| inside(*r, x, y))
            {
                self.desktop_widget_click(i);
            } else if down {
                // Clicking the wallpaper unfocuses, like clicking the desktop in Plasma.
                self.wm.focused = None;
            }
            return;
        };

        let alt = m.modifiers.contains(KeyModifiers::ALT);
        match (m.kind, hit) {
            (MouseEventKind::Down(MouseButton::Left), _) if alt => {
                self.wm.focus(id);
                self.start_move(id, x, y);
            }
            (MouseEventKind::Down(MouseButton::Right), _) if alt => {
                self.wm.focus(id);
                let edges = Edges { left: false, right: true, bottom: true };
                self.start_resize(id, edges, x, y);
            }
            (MouseEventKind::Down(MouseButton::Left), Hit::Title) => {
                self.wm.focus(id);
                if self.click_count(x, y) == 2 {
                    self.last_click = None;
                    self.wm.toggle_maximize(id);
                } else {
                    self.start_move(id, x, y);
                }
            }
            (MouseEventKind::Down(MouseButton::Right), Hit::Title | Hit::Button(_)) => {
                self.wm.focus(id);
                self.open_window_menu(id, x, y + 1);
            }
            (MouseEventKind::Down(MouseButton::Middle), Hit::Title) => {
                // Middle-click a title bar to send the window to the back, as KWin does,
                // and focus whatever is now on top.
                if let Some(idx) = self.wm.windows.iter().position(|w| w.id == id) {
                    let w = self.wm.windows.remove(idx);
                    self.wm.windows.insert(0, w);
                    let top = self.wm.stacking().filter(|w| w.id != id).last().map(|w| w.id);
                    match top {
                        Some(other) => self.wm.focus(other),
                        None => self.wm.focused = Some(id),
                    }
                }
            }
            (MouseEventKind::Down(MouseButton::Left), Hit::Button(b)) => {
                self.wm.focus(id);
                match b {
                    Button::Minimize => self.wm.minimize(id),
                    Button::Maximize => self.wm.toggle_maximize(id),
                    Button::Close => self.close_window(id),
                }
            }
            (MouseEventKind::Down(MouseButton::Left), Hit::Border(edges)) => {
                self.wm.focus(id);
                self.start_resize(id, edges, x, y);
            }
            (_, Hit::Content) => self.content_mouse(id, &m),
            (MouseEventKind::ScrollUp | MouseEventKind::ScrollDown, _) => self.forward_mouse(id, &m),
            _ => {}
        }
    }

    /// Count consecutive clicks at the same spot (1, 2 or 3).
    fn click_count(&mut self, x: u16, y: u16) -> u8 {
        let count = match self.last_click {
            Some((t, lx, ly, n)) if t.elapsed() < MULTI_CLICK && lx == x && ly == y => n % 3 + 1,
            _ => 1,
        };
        self.last_click = Some((Instant::now(), x, y, count));
        count
    }

    fn content_mouse(&mut self, id: u64, m: &MouseEvent) {
        let Some(w) = self.wm.get(id) else { return };
        let c = w.content();
        let (col, row) = (m.column.saturating_sub(c.x), m.row.saturating_sub(c.y));
        // Shift bypasses programs that grab the mouse, as in other terminals.
        let program_mouse = w.term.mouse().mode != MouseMode::None && !m.modifiers.contains(KeyModifiers::SHIFT);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.wm.focus(id);
                if m.modifiers.contains(KeyModifiers::CONTROL)
                    && let Some(url) = self.wm.get(id).and_then(|w| w.term.hyperlink_at(row, col))
                {
                    open_url(&url);
                    return;
                }
                if program_mouse {
                    self.forward_mouse(id, m);
                    self.mouse_grab = Some(id);
                    return;
                }
                let ty = match self.click_count(m.column, m.row) {
                    2 => SelectionType::Semantic,
                    3 => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                for other in &mut self.wm.windows {
                    other.term.clear_selection();
                }
                if let Some(w) = self.wm.get_mut(id) {
                    w.term.start_selection(ty, row, col);
                }
                self.drag = Some(Drag::Select { id, moved: ty != SelectionType::Simple });
            }
            MouseEventKind::Down(MouseButton::Middle) if !program_mouse => {
                self.wm.focus(id);
                let text = self.clipboard.clone();
                self.paste_into(id, &text);
            }
            MouseEventKind::Down(_) => {
                self.wm.focus(id);
                self.forward_mouse(id, m);
                if program_mouse {
                    self.mouse_grab = Some(id);
                }
            }
            _ if self.wm.focused == Some(id) => self.forward_mouse(id, m),
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => self.forward_mouse(id, m),
            _ => {}
        }
    }

    fn kickoff_mouse(&mut self, m: &MouseEvent) {
        let (x, y) = (m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let n = self.menu_items().len();
                let ko = &mut self.kickoff;
                let scroll =
                    if m.kind == MouseEventKind::ScrollUp { ko.scroll.saturating_sub(3) } else { ko.scroll + 3 };
                ko.scroll_to(scroll, n);
            }
            MouseEventKind::Down(MouseButton::Left) if self.hits.menu_scrollbar.is_some_and(|b| inside(b, x, y)) => {
                self.drag = Some(Drag::MenuScrollbar);
                self.kickoff_scrollbar(y);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, c)) = self.hits.menu_tabs.iter().find(|(r, _)| inside(*r, x, y)) {
                    self.kickoff.category = c;
                    self.kickoff.selected = 0;
                } else if let Some(&(_, i)) = self.hits.menu_items.iter().find(|(r, _)| inside(*r, x, y))
                    && let Some(item) = self.menu_items().get(i).cloned()
                {
                    self.activate(item);
                }
            }
            MouseEventKind::Moved => {
                if let Some(&(_, i)) = self.hits.menu_items.iter().find(|(r, _)| inside(*r, x, y))
                    && self.kickoff.selected != i
                {
                    self.kickoff.selected = i;
                    self.dirty = true;
                }
            }
            _ => {}
        }
    }

    /// Click or drag on Kickoff's scrollbar: center the list on that point.
    fn kickoff_scrollbar(&mut self, y: u16) {
        let Some(bar) = self.hits.menu_scrollbar else { return };
        let n = self.menu_items().len();
        let row = (y.clamp(bar.y, bar.bottom() - 1) - bar.y) as usize;
        let ko = &mut self.kickoff;
        ko.scroll_to((row * n / bar.height as usize).saturating_sub(ko.rows / 2), n);
    }

    fn panel_mouse(&mut self, m: &MouseEvent, hit: PanelHit) {
        let scroll = matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown);
        let left = m.kind == MouseEventKind::Down(MouseButton::Left);
        match hit {
            PanelHit::Kickoff if left => {
                self.kickoff = Kickoff::default();
                self.run_action(Action::Kickoff);
            }
            PanelHit::Layout if left => self.run_action(Action::ToggleLayout),
            PanelHit::Bell if left => self.run_action(Action::Notifications),
            PanelHit::Desktop(n) if left => self.wm.switch_desktop(n),
            PanelHit::Desktop(n) if m.kind == MouseEventKind::Down(MouseButton::Right) => {
                self.rename = Some((RenameTarget::Desktop(n), self.desktop_names.get(n).cloned().unwrap_or_default()));
                self.mode = Mode::Rename;
            }
            PanelHit::Desktop(_) if scroll => {
                let a = if m.kind == MouseEventKind::ScrollDown { Action::DesktopNext } else { Action::DesktopPrev };
                self.run_action(a);
            }
            // Acts on release, so the button can be dragged onto a pager desktop instead.
            PanelHit::Task(id) if left => self.drag = Some(Drag::Task { id, moved: false }),
            PanelHit::Task(id) if m.kind == MouseEventKind::Down(MouseButton::Middle) => self.close_window(id),
            _ if scroll => self.wm.cycle(m.kind == MouseEventKind::ScrollDown),
            _ => {}
        }
    }

    /// Forward to a window that grabbed the mouse, pinning the position inside its content.
    fn forward_mouse_clamped(&mut self, id: u64, m: &MouseEvent) {
        let Some(c) = self.wm.get(id).map(|w| w.content()) else { return };
        if c.width == 0 || c.height == 0 {
            return;
        }
        let mut m = *m;
        m.column = m.column.clamp(c.x, c.right() - 1);
        m.row = m.row.clamp(c.y, c.bottom() - 1);
        self.forward_mouse(id, &m);
    }

    fn forward_mouse(&mut self, id: u64, m: &MouseEvent) {
        let Some(w) = self.wm.get_mut(id) else { return };
        let c = w.content();
        if !inside(c, m.column, m.row) {
            return;
        }
        let (col, row) = (m.column - c.x, m.row - c.y);
        if let Some(bytes) = encode_mouse(w.term.mouse(), m, col, row) {
            w.term.scroll_reset();
            w.term.write(&bytes);
            return;
        }
        // Program doesn't want the mouse: the wheel scrolls history, or acts as arrow keys
        // in full-screen programs (less, man, ...).
        let up = match m.kind {
            MouseEventKind::ScrollUp => true,
            MouseEventKind::ScrollDown => false,
            _ => return,
        };
        if w.term.alt_screen() {
            let seq: &[u8] = match (up, w.term.app_cursor()) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            w.term.write(&seq.repeat(3));
        } else {
            w.term.scroll(if up { 3 } else { -3 });
        }
        self.dirty = true;
    }

    // ---------------------------------------------------------------- dragging

    fn start_move(&mut self, id: u64, x: u16, y: u16) {
        let Some(w) = self.wm.get(id) else { return };
        let f = w.frame;
        self.drag = Some(Drag::Move {
            id,
            grab_dx: x.saturating_sub(f.x),
            grab_dy: y.saturating_sub(f.y),
            moved: false,
            origin: w.float_rect,
        });
    }

    fn start_resize(&mut self, id: u64, edges: Edges, x: u16, y: u16) {
        let Some(w) = self.wm.get(id) else { return };
        if w.maximized {
            return;
        }
        if self.wm.is_tiled(w) {
            if edges.right && self.wm.master() == Some(id) {
                self.drag = Some(Drag::MasterRatio);
            }
            return;
        }
        self.drag = Some(Drag::Resize { id, edges, start: w.frame, mx: x, my: y });
    }

    fn drag_to(&mut self, x: u16, y: u16) {
        let area = self.wm.area;
        self.drop_target = self.pager_desktop_at(x, y);
        match self.drag {
            Some(Drag::Task { id, .. }) => self.drag = Some(Drag::Task { id, moved: true }),
            Some(Drag::Card { id, .. }) => self.drag = Some(Drag::Card { id, moved: true }),
            Some(Drag::Move { id, mut grab_dx, grab_dy, moved, origin }) => {
                if !moved {
                    // Dragging a maximized, tiled or quick-tiled window restores a floating size.
                    let detach = self.wm.get(id).is_some_and(|w| w.maximized || self.wm.is_tiled(w));
                    if detach {
                        let old_w = self.wm.get(id).map(|w| w.frame.width).unwrap_or(1).max(1);
                        self.make_floating(id);
                        if let Some(w) = self.wm.get_mut(id) {
                            let r = w.restore_rect.take().unwrap_or(w.float_rect);
                            let width = if r.width >= area.width { area.width * 2 / 3 } else { r.width };
                            let height = if r.height >= area.height { area.height * 2 / 3 } else { r.height };
                            w.float_rect.width = width.max(MIN_W);
                            w.float_rect.height = height.max(MIN_H);
                            grab_dx = (grab_dx as u32 * w.float_rect.width as u32 / old_w as u32) as u16;
                        }
                    } else if let Some(r) = self.wm.untile(id) {
                        grab_dx = grab_dx.min(r.width.saturating_sub(2));
                    }
                }
                self.drag = Some(Drag::Move { id, grab_dx, grab_dy, moved: true, origin });
                if let Some(w) = self.wm.get_mut(id) {
                    w.float_rect.x = x.saturating_sub(grab_dx);
                    w.float_rect.y = y.saturating_sub(grab_dy).max(area.y);
                }
                self.wm.arrange();
            }
            Some(Drag::Resize { id, edges, start, mx, my }) => {
                let dx = x as i32 - mx as i32;
                let dy = y as i32 - my as i32;
                let mut r = start;
                // All bounds in i32 with max >= min, so tiny screens and a terminal that shrank
                // during the drag cannot underflow or make clamp panic.
                let (min_w, min_h) = (MIN_W.min(area.width) as i32, MIN_H.min(area.height) as i32);
                if edges.left {
                    let max_x = (start.right() as i32 - min_w).max(area.x as i32);
                    let nx = (start.x as i32 + dx).clamp(area.x as i32, max_x);
                    r.x = nx as u16;
                    r.width = start.right().saturating_sub(r.x).max(1);
                }
                if edges.right {
                    let max_w = (area.right() as i32 - start.x as i32).max(min_w);
                    r.width = (start.width as i32 + dx).clamp(min_w, max_w) as u16;
                }
                if edges.bottom {
                    let max_h = (area.bottom() as i32 - start.y as i32).max(min_h);
                    r.height = (start.height as i32 + dy).clamp(min_h, max_h) as u16;
                }
                if let Some(w) = self.wm.get_mut(id) {
                    w.float_rect = r;
                    w.restore_rect = None;
                }
                self.wm.arrange();
            }
            Some(Drag::MenuScrollbar) => self.kickoff_scrollbar(y),
            Some(Drag::MasterRatio) => {
                if area.width > 0 {
                    self.wm.set_master_ratio(x.saturating_sub(area.x) as f32 / area.width as f32);
                }
            }
            Some(Drag::Select { id, .. }) => {
                let Some(w) = self.wm.get_mut(id) else { return };
                let c = w.content();
                if c.width == 0 || c.height == 0 {
                    return;
                }
                // Dragging past the top or bottom edge scrolls through history.
                let row = if y < c.y {
                    w.term.scroll(1);
                    0
                } else if y >= c.bottom() {
                    w.term.scroll(-1);
                    c.height - 1
                } else {
                    y - c.y
                };
                let col = x.clamp(c.x, c.right() - 1) - c.x;
                w.term.update_selection(row, col, true);
                self.drag = Some(Drag::Select { id, moved: true });
            }
            None => {}
        }
    }

    /// The pager desktop (or overview desktop button) at a screen position, if any.
    fn pager_desktop_at(&self, x: u16, y: u16) -> Option<usize> {
        let button = self.hits.desk_buttons.iter().find(|(r, _)| inside(*r, x, y)).map(|&(_, d)| d);
        button.or_else(|| {
            self.hits.panel.iter().find_map(|(r, h)| match h {
                PanelHit::Desktop(d) if inside(*r, x, y) => Some(*d),
                _ => None,
            })
        })
    }

    /// Clicking a desktop widget: notes open in $EDITOR.
    fn desktop_widget_click(&mut self, i: usize) {
        let Some(w) = self.cfg.desktop_widgets.get(i).cloned() else { return };
        if w.kind == "notes"
            && let Some(file) = w.file
        {
            self.spawn_command(edit_command(&file));
        }
    }

    /// Abandon a drag whose release never arrived: no tiling, moving or dropping happens.
    fn cancel_drag(&mut self) {
        self.drop_target = None;
        // A selection is still copied, as if released where it was last dragged.
        if let Some(Drag::Select { id, moved: true }) = self.drag.take()
            && let Some(t) = self.wm.get(id).and_then(|w| w.term.selection_text())
        {
            self.copy(t);
        }
    }

    fn end_drag(&mut self, x: u16, y: u16) {
        let target = self.drop_target.take().or_else(|| self.pager_desktop_at(x, y));
        match self.drag.take() {
            Some(Drag::Card { id, moved }) => match target {
                Some(d) if moved => self.wm.move_to_desktop(id, d),
                _ if moved => {}
                _ => {
                    self.mode = Mode::Normal;
                    self.wm.focus(id);
                }
            },
            Some(Drag::Task { id, moved }) => match target {
                Some(d) if moved => self.wm.move_to_desktop(id, d),
                _ => {
                    let visible_focused = self.wm.focused == Some(id) && self.wm.get(id).is_some_and(|w| !w.minimized);
                    if visible_focused {
                        self.wm.minimize(id);
                    } else {
                        self.wm.focus(id);
                    }
                }
            },
            // Dropped on the pager: put the window back where it was, on that desktop.
            Some(Drag::Move { id, moved: true, origin, .. }) if target.is_some() => {
                if let Some(w) = self.wm.get_mut(id) {
                    w.float_rect = origin;
                }
                self.wm.move_to_desktop(id, target.unwrap_or_default());
            }
            Some(Drag::Move { id, moved: true, .. }) => {
                // Drop on a screen edge to quick tile, or on the top edge to maximize.
                let area = self.wm.area;
                if x <= area.x {
                    self.wm.quick_tile(id, Side::Left);
                } else if x + 1 >= area.right() {
                    self.wm.quick_tile(id, Side::Right);
                } else if y <= area.y {
                    if let Some(w) = self.wm.get_mut(id) {
                        w.float_rect.y = area.y;
                    }
                    self.wm.toggle_maximize(id);
                }
            }
            Some(Drag::Select { id, moved }) => {
                let text = self.wm.get(id).filter(|_| moved).and_then(|w| w.term.selection_text());
                match text {
                    Some(t) => self.copy(t),
                    None => {
                        if let Some(w) = self.wm.get_mut(id) {
                            w.term.clear_selection();
                        }
                    }
                }
            }
            _ => {}
        }
        self.drag = None;
    }
}

/// A command line that opens `path` ("~" expanded) in $EDITOR: vi when unset on Unix, Notepad on
/// Windows.
fn edit_command(path: &str) -> String {
    let p = crate::config::expand_tilde(path).display().to_string();
    if cfg!(windows) {
        let editor = std::env::var("EDITOR").ok().filter(|e| !e.is_empty()).unwrap_or_else(|| "notepad".into());
        format!("{editor} \"{p}\"")
    } else {
        format!("${{EDITOR:-vi}} '{}'", p.replace('\'', "'\\''"))
    }
}

fn state_search(copy: &mut Option<CopyState>, forward: bool) {
    if let Some(c) = copy.as_mut() {
        c.search = Some((String::new(), forward));
    }
}

/// Fixed entries for the cheat sheet; "copy:" marks copy mode keys.
const HELP_STATIC: &[(&str, &str)] = &[
    ("drag title / alt+drag", "move window (drop on an edge to tile)"),
    ("drag border / alt+right-drag", "resize window"),
    ("double-click title", "maximize / restore"),
    ("right-click title", "window menu"),
    ("middle-click title", "send window to the back"),
    ("drag in window", "select text and copy (double: word, triple: line)"),
    ("shift+drag", "select in programs that use the mouse"),
    ("middle-click in window", "paste"),
    ("ctrl+click link", "open hyperlink"),
    ("copy:h j k l / arrows", "move"),
    ("copy:w b e  W B E", "word motions"),
    ("copy:0 ^ $  H M L  g G", "line start/end, screen top/middle/bottom, history top/bottom"),
    ("copy:ctrl+u ctrl+d  ctrl+b ctrl+f", "half / full page"),
    ("copy:v  V  ctrl+v", "select characters / lines / block"),
    ("copy:/ ?  n N", "search forward / backward, next / previous"),
    ("copy:y or enter", "copy selection and leave"),
    ("copy:esc  q", "clear selection / leave"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_matches() {
        assert_eq!(match_score("fil", &["Files", "yazi"]), Some(0));
        assert_eq!(match_score("yazi", &["Files", "yazi file manager"]), Some(2));
        assert_eq!(match_score("sm", &["System Monitor", ""]), Some(4));
        assert_eq!(match_score("zz", &["Files", ""]), None);
    }
}

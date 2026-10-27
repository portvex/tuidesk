//! KRunner-style launcher: one search line, results from several sources.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Action, App, Mode, SpawnRequest};
use crate::config;
use crate::input::match_score;

#[derive(Debug, Clone, PartialEq)]
pub enum RunnerItem {
    /// Calculator result, copied on Enter.
    Calc(String),
    Window(u64),
    App(usize),
    /// Open a shell in this directory.
    Dir(PathBuf),
    Man(String),
    Action(String),
    Run(String),
}

const MAX_PER_SOURCE: usize = 5;

/// Actions offered by name; the ones that need an argument are typed in full.
const ACTION_NAMES: &[&str] = &[
    "kickoff",
    "spawn_shell",
    "close",
    "maximize",
    "minimize",
    "switcher",
    "toggle_layout",
    "cycle_layout",
    "toggle_floating",
    "swap_master",
    "window_mode",
    "copy_mode",
    "clipboard",
    "help",
    "window_menu",
    "toggle_above",
    "rename",
    "rename_desktop",
    "desktop_next",
    "desktop_prev",
    "move_next_desktop",
    "move_prev_desktop",
    "notifications",
    "settings",
    "detach",
    "reload",
    "quit",
    "layout floating",
    "layout master",
    "layout monocle",
    "layout grid",
    "layout columns",
    "layout bsp",
    "tile_left",
    "tile_right",
    "overview",
    "screenshot",
    "screenshot window",
    "screenshot region",
];

impl App {
    pub fn runner_items(&self) -> Vec<RunnerItem> {
        let q = self.runner.query.trim();
        if q.is_empty() {
            return Vec::new();
        }
        let lower = q.to_lowercase();
        let mut items = Vec::new();

        if let Some(result) = calculate(q) {
            items.push(RunnerItem::Calc(result));
        }

        let mut wins: Vec<(u8, u64)> = self
            .wm
            .order
            .iter()
            .filter_map(|&id| self.wm.get(id))
            .filter_map(|w| match_score(&lower, &[&w.title(), &w.command]).map(|s| (s, w.id)))
            .collect();
        wins.sort_by_key(|(s, _)| *s);
        items.extend(wins.into_iter().take(MAX_PER_SOURCE).map(|(_, id)| RunnerItem::Window(id)));

        let mut apps: Vec<(u8, usize)> = self
            .apps
            .iter()
            .enumerate()
            .filter_map(|(i, a)| match_score(&lower, &[&a.name, &a.description, &a.command]).map(|s| (s, i)))
            .collect();
        apps.sort_by_key(|(s, _)| *s);
        items.extend(apps.into_iter().take(MAX_PER_SOURCE).map(|(_, i)| RunnerItem::App(i)));

        items.extend(self.runner_dirs(q).into_iter().map(RunnerItem::Dir));

        if let Some(page) = q.strip_prefix("man ").map(str::trim).filter(|p| !p.is_empty()) {
            items.push(RunnerItem::Man(page.to_string()));
        }

        if Action::parse(q).is_some() && !ACTION_NAMES.contains(&q) {
            items.push(RunnerItem::Action(q.to_string()));
        }
        items.extend(
            ACTION_NAMES
                .iter()
                .filter(|a| a.starts_with(&lower) || (lower.len() >= 3 && a.contains(&lower)))
                .take(MAX_PER_SOURCE)
                .map(|a| RunnerItem::Action(a.to_string())),
        );

        items.push(RunnerItem::Run(q.to_string()));
        items
    }

    fn runner_dirs(&self, q: &str) -> Vec<PathBuf> {
        let mut cache = self.runner.dirs.borrow_mut();
        if let Some((cached_q, dirs)) = cache.as_ref()
            && cached_q == q
        {
            return dirs.clone();
        }
        let dirs = directories(q, self.runner.base.as_deref());
        *cache = Some((q.to_string(), dirs.clone()));
        dirs
    }

    /// (kind, text) shown for a result.
    pub fn runner_label(&self, item: &RunnerItem) -> (&'static str, String) {
        match item {
            RunnerItem::Calc(r) => ("Calc", format!("= {r}")),
            RunnerItem::Window(id) => match self.wm.get(*id) {
                Some(w) => ("Window", format!("{}  (desktop {})", w.title(), self.desktop_label(w.desktop))),
                None => ("Window", "?".into()),
            },
            RunnerItem::App(i) => ("App", self.apps.get(*i).map(|a| a.name.clone()).unwrap_or_default()),
            RunnerItem::Dir(p) => ("Folder", tilde(p)),
            RunnerItem::Man(page) => ("Manual", format!("man {page}")),
            RunnerItem::Action(a) => ("Action", a.clone()),
            RunnerItem::Run(c) => ("Run", c.clone()),
        }
    }

    pub(crate) fn runner_activate(&mut self, item: RunnerItem) {
        self.mode = Mode::Normal;
        match item {
            RunnerItem::Calc(r) => {
                self.copy(r.clone());
                self.notify("Calculator", format!("Copied {r}"));
            }
            RunnerItem::Window(id) => self.wm.focus(id),
            RunnerItem::App(i) => {
                if let Some(a) = self.apps.get(i).cloned() {
                    self.spawn(SpawnRequest { command: Some(a.command), name: a.name, cwd: None, restore: None });
                }
            }
            RunnerItem::Dir(dir) => {
                let shell = self.cfg.shell();
                self.spawn(SpawnRequest {
                    command: None,
                    name: config::command_name(&shell),
                    cwd: Some(dir),
                    restore: None,
                });
            }
            RunnerItem::Man(page) => {
                self.spawn_command(format!("man {page}"));
            }
            RunnerItem::Action(a) => match Action::parse(&a) {
                Some(action) => self.run_action(action),
                None => self.notify("Runner", format!("unknown action '{a}'")),
            },
            RunnerItem::Run(c) => {
                self.spawn_command(c);
            }
        }
        self.dirty = true;
    }

    pub(crate) fn runner_key(&mut self, k: &KeyEvent) {
        let items = self.runner_items();
        let r = &mut self.runner;
        match k.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                if let Some(item) = items.get(r.sel).cloned() {
                    self.runner_activate(item);
                }
            }
            KeyCode::Up => r.sel = r.sel.saturating_sub(1),
            KeyCode::Down => r.sel = (r.sel + 1).min(items.len().saturating_sub(1)),
            // Tab completes a directory into the query.
            KeyCode::Tab => match items.get(r.sel) {
                Some(RunnerItem::Dir(p)) => {
                    r.query = format!("{}/", tilde(p).trim_end_matches('/'));
                    r.sel = 0;
                }
                _ => r.sel = (r.sel + 1).min(items.len().saturating_sub(1)),
            },
            KeyCode::BackTab => r.sel = r.sel.saturating_sub(1),
            KeyCode::Backspace => {
                r.query.pop();
                r.sel = 0;
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                r.query.clear();
                r.sel = 0;
            }
            KeyCode::Char(c) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                r.query.push(c);
                r.sel = 0;
            }
            _ => {}
        }
    }
}

/// Show a path with the home directory as "~".
fn tilde(p: &Path) -> String {
    match dirs::home_dir().and_then(|h| p.strip_prefix(&h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

/// Directories matching a path-like query ("/us", "~/Down", "./src"); relative queries start
/// from `base`.
fn directories(q: &str, base: Option<&Path>) -> Vec<PathBuf> {
    let relative = q.starts_with("./") || q.starts_with("../");
    // Only "~" and "~/..." mean home; "~user" is not supported.
    let home = q == "~" || q.starts_with("~/");
    if !(q.starts_with('/') || home || relative) {
        return Vec::new();
    }
    let expanded = if home {
        match dirs::home_dir() {
            Some(h) => format!("{}{}", h.display(), &q[1..]),
            None => return Vec::new(),
        }
    } else if relative {
        match base.map(Path::to_path_buf).or_else(|| std::env::current_dir().ok()) {
            Some(b) => format!("{}/{q}", b.display()),
            None => return Vec::new(),
        }
    } else {
        q.to_string()
    };
    let path = PathBuf::from(&expanded);
    // "/usr/lo" lists /usr entries starting with "lo"; "/usr/" lists all of /usr.
    let (parent, prefix) = if expanded.ends_with('/') {
        (path.clone(), String::new())
    } else {
        let parent = path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
        let prefix = path.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
        (parent, prefix)
    };
    let mut out = Vec::new();
    if path.is_dir() && !expanded.ends_with('/') {
        out.push(path.clone());
    }
    let Ok(entries) = std::fs::read_dir(&parent) else { return out };
    // Match names first; only matching entries are stat'ed (huge directories stay cheap).
    let mut found: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name.starts_with(&prefix) && (prefix.starts_with('.') || !name.starts_with('.'))
        })
        .take(200)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && *p != path)
        .collect();
    found.sort();
    out.extend(found.into_iter().take(MAX_PER_SOURCE));
    out
}

/// Evaluate an arithmetic expression; None unless the query is a real calculation.
/// Supports + - * / % ^, parentheses, sqrt sin cos tan ln log abs round floor ceil, pi and e.
pub fn calculate(q: &str) -> Option<String> {
    let q = q.trim().strip_prefix('=').unwrap_or(q.trim()).trim();
    // A bare number is not a calculation.
    let has_operand = q.chars().any(|c| c.is_ascii_digit()) || q.contains("pi");
    if q.parse::<f64>().is_ok() || !has_operand {
        return None;
    }
    if q.len() > 256 {
        return None;
    }
    let mut p = Parser { s: q.as_bytes(), i: 0, depth: 0 };
    let v = p.expr()?;
    p.ws();
    if p.i != p.s.len() || !v.is_finite() {
        return None;
    }
    Some(format_number(v))
}

fn format_number(v: f64) -> String {
    let v = if v == 0.0 { 0.0 } else { v }; // no "-0"
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    if v.abs() < 1e-6 || v.abs() >= 1e15 {
        // 1.5e-12 rather than 0.0000000000
        let s = format!("{v:.9e}");
        let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        return format!("{mantissa}e{exp}");
    }
    let s = format!("{v:.10}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    /// Nesting depth, limited so long inputs cannot overflow the stack.
    depth: usize,
}

const MAX_DEPTH: usize = 64;

impl Parser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            if self.eat(b'+') {
                v += self.term()?;
            } else if self.eat(b'-') {
                v -= self.term()?;
            } else {
                return Some(v);
            }
        }
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            if self.eat(b'*') {
                v *= self.unary()?;
            } else if self.eat(b'/') {
                v /= self.unary()?;
            } else if self.eat(b'%') {
                v %= self.unary()?;
            } else {
                return Some(v);
            }
        }
    }

    /// Sign binds looser than ^, so -2^2 = -(2^2) = -4, as in maths and most calculators.
    fn unary(&mut self) -> Option<f64> {
        self.depth += 1;
        let v = if self.depth > MAX_DEPTH {
            None
        } else if self.eat(b'-') {
            self.unary().map(|v| -v)
        } else if self.eat(b'+') {
            self.unary()
        } else {
            self.power()
        };
        self.depth -= 1;
        v
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.atom()?;
        if self.eat(b'^') {
            // Right associative (2^3^2 = 2^9); the exponent may carry a sign (2^-1).
            return Some(base.powf(self.unary()?));
        }
        Some(base)
    }

    fn atom(&mut self) -> Option<f64> {
        self.ws();
        if self.eat(b'(') {
            self.depth += 1;
            let v = if self.depth > MAX_DEPTH { None } else { self.expr() };
            self.depth -= 1;
            return self.eat(b')').then_some(v?);
        }
        let start = self.i;
        if self.s.get(self.i).is_some_and(|c| c.is_ascii_alphabetic()) {
            while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphabetic()) {
                self.i += 1;
            }
            let name = std::str::from_utf8(&self.s[start..self.i]).ok()?;
            return match name {
                "pi" => Some(std::f64::consts::PI),
                "e" => Some(std::f64::consts::E),
                _ => {
                    let f: fn(f64) -> f64 = match name {
                        "sqrt" => f64::sqrt,
                        "sin" => f64::sin,
                        "cos" => f64::cos,
                        "tan" => f64::tan,
                        "ln" => f64::ln,
                        "log" => f64::log10,
                        "abs" => f64::abs,
                        "round" => f64::round,
                        "floor" => f64::floor,
                        "ceil" => f64::ceil,
                        _ => return None,
                    };
                    let arg = self.atom()?;
                    Some(f(arg))
                }
            };
        }
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'.' || *c == b'_') {
            self.i += 1;
        }
        let text: String = std::str::from_utf8(&self.s[start..self.i]).ok()?.replace('_', "");
        text.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculates() {
        assert_eq!(calculate("2*(3+4)").as_deref(), Some("14"));
        assert_eq!(calculate("= 1/4").as_deref(), Some("0.25"));
        assert_eq!(calculate("2^3^2").as_deref(), Some("512"));
        assert_eq!(calculate("-3 + 10 % 4").as_deref(), Some("-1"));
        assert_eq!(calculate("sqrt(16) + abs(-2)").as_deref(), Some("6"));
        assert_eq!(calculate("1_000 * 3").as_deref(), Some("3000"));
        assert!(calculate("2 * pi").unwrap().starts_with("6.28318"));
        assert_eq!(calculate("42"), None);
        assert_eq!(calculate("firefox"), None);
        assert_eq!(calculate("2 +"), None);
        assert_eq!(calculate("1/0"), None);
        assert_eq!(calculate("-2^2").as_deref(), Some("-4"));
        assert_eq!(calculate("2^-1").as_deref(), Some("0.5"));
        assert_eq!(calculate("2*-3").as_deref(), Some("-6"));
        assert_eq!(calculate("1/10000000000000").as_deref(), Some("1e-13"));
        assert_eq!(calculate("-0*5").as_deref(), Some("0"));
        let deep = format!("{}1{}", "(".repeat(5000), ")".repeat(5000));
        assert_eq!(calculate(&format!("1+{deep}")), None);
        assert_eq!(calculate(&format!("1+{}1", "-".repeat(5000))), None);
    }

    #[test]
    fn lists_directories() {
        let dir = std::env::temp_dir().join(format!("tuidesk-runner-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("alpha")).unwrap();
        std::fs::create_dir_all(dir.join("albert")).unwrap();
        std::fs::create_dir_all(dir.join("beta")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        let q = format!("{}/al", dir.display());
        assert_eq!(directories(&q, None), vec![dir.join("albert"), dir.join("alpha")]);
        let all = directories(&format!("{}/", dir.display()), None);
        assert_eq!(all.len(), 3, "{all:?}");
        assert!(directories("firefox", None).is_empty());
        assert!(directories("~nobody", None).is_empty());
        // Relative queries start from the given base.
        let rel = directories("./be", Some(&dir));
        assert_eq!(rel.len(), 1);
        assert!(rel[0].ends_with("beta"), "{rel:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

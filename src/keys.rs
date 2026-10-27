use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::term::{MouseMode, MouseProtocol};

/// A key combination such as `alt+f4` or `ctrl+space`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeySpec {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

const MOD_MASK: KeyModifiers = KeyModifiers::SHIFT.union(KeyModifiers::CONTROL).union(KeyModifiers::ALT);

impl KeySpec {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        // A trailing "+" means the plus key itself, e.g. "ctrl++".
        let (mods_part, key_part) = match s.strip_suffix("++") {
            Some(rest) => (rest, "+"),
            None => match s.rfind('+') {
                Some(i) if i + 1 < s.len() => (&s[..i], &s[i + 1..]),
                _ => ("", s),
            },
        };
        let mut mods = KeyModifiers::NONE;
        for m in mods_part.split('+').filter(|m| !m.is_empty()) {
            mods |= match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "c" => KeyModifiers::CONTROL,
                "alt" | "meta" | "opt" | "option" | "m" => KeyModifiers::ALT,
                "shift" | "s" => KeyModifiers::SHIFT,
                _ => return None,
            };
        }
        let lower = key_part.to_ascii_lowercase();
        let code = match lower.as_str() {
            "space" => KeyCode::Char(' '),
            "enter" | "return" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "esc" | "escape" => KeyCode::Esc,
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "plus" => KeyCode::Char('+'),
            "minus" => KeyCode::Char('-'),
            f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() => KeyCode::F(f[1..].parse().ok()?),
            _ => {
                let mut chars = key_part.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                KeyCode::Char(c)
            }
        };
        Some(Self { code, mods }.normalized())
    }

    pub fn from_event(ev: &KeyEvent) -> Self {
        Self { code: ev.code, mods: ev.modifiers & MOD_MASK }.normalized()
    }

    /// The config spelling that `parse` reads back, e.g. "ctrl+shift+c" or "alt+f4".
    /// None for keys that cannot be written in the config.
    pub fn to_config(&self) -> Option<String> {
        let key = match self.code {
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Char('+') => "plus".to_string(),
            KeyCode::Char(c) if !c.is_control() => c.to_string(),
            KeyCode::Enter => "enter".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::BackTab => "shift+tab".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Delete => "delete".into(),
            KeyCode::Insert => "insert".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pageup".into(),
            KeyCode::PageDown => "pagedown".into(),
            KeyCode::Up => "up".into(),
            KeyCode::Down => "down".into(),
            KeyCode::Left => "left".into(),
            KeyCode::Right => "right".into(),
            KeyCode::F(n) => format!("f{n}"),
            _ => return None,
        };
        let mut out = String::new();
        for (m, name) in
            [(KeyModifiers::CONTROL, "ctrl+"), (KeyModifiers::ALT, "alt+"), (KeyModifiers::SHIFT, "shift+")]
        {
            if self.mods.contains(m) {
                out.push_str(name);
            }
        }
        out.push_str(&key);
        Some(out)
    }

    /// Canonical form so that "shift+tab" == BackTab and "shift+a" == "A".
    fn normalized(mut self) -> Self {
        match self.code {
            KeyCode::BackTab => self.mods.remove(KeyModifiers::SHIFT),
            KeyCode::Tab if self.mods.contains(KeyModifiers::SHIFT) => {
                self.code = KeyCode::BackTab;
                self.mods.remove(KeyModifiers::SHIFT);
            }
            KeyCode::Char(c) => {
                if self.mods.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase() {
                    self.code = KeyCode::Char(c.to_ascii_uppercase());
                }
                let ctrl_alt = self.mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
                if let KeyCode::Char(c) = self.code
                    && ctrl_alt
                    && c.is_ascii_alphabetic()
                {
                    // With ctrl/alt, shift stays significant: ctrl+shift+c is not ctrl+c.
                    // Letters are kept lowercase with an explicit shift.
                    if c.is_ascii_uppercase() {
                        self.mods.insert(KeyModifiers::SHIFT);
                    }
                    self.code = KeyCode::Char(c.to_ascii_lowercase());
                } else if self.code != KeyCode::Char(' ') {
                    self.mods.remove(KeyModifiers::SHIFT);
                }
            }
            _ => {}
        }
        self
    }
}

/// Translate a key press into the bytes a program in a terminal expects.
pub fn encode_key(ev: &KeyEvent, app_cursor: bool) -> Vec<u8> {
    let mods = ev.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let shift = mods.contains(KeyModifiers::SHIFT);
    let param = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;

    let mut out = Vec::new();
    let csi_mod = |out: &mut Vec<u8>, final_byte: char, ss3: bool| {
        if param > 1 {
            out.extend_from_slice(format!("\x1b[1;{param}{final_byte}").as_bytes());
        } else if ss3 {
            out.extend_from_slice(format!("\x1bO{final_byte}").as_bytes());
        } else {
            out.extend_from_slice(format!("\x1b[{final_byte}").as_bytes());
        }
    };
    let tilde = |out: &mut Vec<u8>, n: u8| {
        if param > 1 {
            out.extend_from_slice(format!("\x1b[{n};{param}~").as_bytes());
        } else {
            out.extend_from_slice(format!("\x1b[{n}~").as_bytes());
        }
    };

    match ev.code {
        KeyCode::Char(c) => {
            // With the kitty keyboard protocol, shifted letters can arrive lowercase + SHIFT.
            let c = if shift && c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c };
            if alt {
                out.push(0x1b);
            }
            if ctrl {
                let b = match c {
                    'a'..='z' => Some(c as u8 - b'a' + 1),
                    'A'..='Z' => Some(c as u8 - b'A' + 1),
                    ' ' | '@' | '2' | '`' => Some(0),
                    '[' | '3' => Some(0x1b),
                    '\\' | '4' => Some(0x1c),
                    ']' | '5' => Some(0x1d),
                    '^' | '6' => Some(0x1e),
                    '_' | '-' | '7' | '/' => Some(0x1f),
                    '?' | '8' => Some(0x7f),
                    _ => None,
                };
                match b {
                    Some(b) => out.push(b),
                    None => push_char(&mut out, c),
                }
            } else {
                push_char(&mut out, c);
            }
        }
        KeyCode::Enter => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        KeyCode::Tab => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\t');
        }
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x08 } else { 0x7f });
        }
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => csi_mod(&mut out, 'A', app_cursor),
        KeyCode::Down => csi_mod(&mut out, 'B', app_cursor),
        KeyCode::Right => csi_mod(&mut out, 'C', app_cursor),
        KeyCode::Left => csi_mod(&mut out, 'D', app_cursor),
        KeyCode::Home => csi_mod(&mut out, 'H', app_cursor),
        KeyCode::End => csi_mod(&mut out, 'F', app_cursor),
        KeyCode::Insert => tilde(&mut out, 2),
        KeyCode::Delete => tilde(&mut out, 3),
        KeyCode::PageUp => tilde(&mut out, 5),
        KeyCode::PageDown => tilde(&mut out, 6),
        KeyCode::F(n @ 1..=4) => csi_mod(&mut out, (b'P' + n - 1) as char, true),
        KeyCode::F(n) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => return out,
            };
            tilde(&mut out, code);
        }
        _ => {}
    }
    out
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// Encode a mouse event for a program that enabled mouse reporting.
/// `col`/`row` are relative to the window content (0-based).
pub fn encode_mouse(proto: MouseProtocol, ev: &MouseEvent, col: u16, row: u16) -> Option<Vec<u8>> {
    let mode = proto.mode;
    let button_code = |b: MouseButton| match b {
        MouseButton::Left => 0u16,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (mut code, release) = match ev.kind {
        MouseEventKind::Down(b) => (button_code(b), false),
        MouseEventKind::Up(b) => (button_code(b), true),
        MouseEventKind::Drag(b) => {
            if !matches!(mode, MouseMode::Drag | MouseMode::Motion) {
                return None;
            }
            (button_code(b) + 32, false)
        }
        MouseEventKind::Moved => {
            if mode != MouseMode::Motion {
                return None;
            }
            (3 + 32, false)
        }
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
    };
    if mode == MouseMode::None {
        return None;
    }
    if ev.modifiers.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if ev.modifiers.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if ev.modifiers.contains(KeyModifiers::CONTROL) {
        code += 16;
    }
    let (x, y) = (col + 1, row + 1);
    if proto.sgr {
        let fin = if release { 'm' } else { 'M' };
        Some(format!("\x1b[<{code};{x};{y}{fin}").into_bytes())
    } else {
        let code = if release { 3 + (code & !3) } else { code };
        let mut out = b"\x1b[M".to_vec();
        out.push((32 + code).min(255) as u8);
        for v in [x, y] {
            let v = 32 + v;
            if proto.utf8 {
                push_char(&mut out, char::from_u32(v as u32).unwrap_or(' '));
            } else if v > 255 {
                return None;
            } else {
                out.push(v as u8);
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    #[test]
    fn config_names_parse_back() {
        for s in ["ctrl+space", "alt+f4", "shift+tab", "ctrl+shift+c", "T", ".", "alt+enter", "ctrl+plus", "pageup"] {
            let k = KeySpec::parse(s).unwrap();
            let name = k.to_config().unwrap();
            assert_eq!(KeySpec::parse(&name), Some(k), "{s} -> {name}");
        }
        let ev = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(KeySpec::from_event(&ev).to_config().as_deref(), Some("ctrl+shift+c"));
        assert_eq!(KeySpec::parse("shift+tab").unwrap().to_config().as_deref(), Some("shift+tab"));
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers: mods, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }

    #[test]
    fn parses_specs() {
        let k = KeySpec::parse("ctrl+space").unwrap();
        assert_eq!(k, KeySpec { code: KeyCode::Char(' '), mods: KeyModifiers::CONTROL });
        assert_eq!(KeySpec::parse("alt+shift+tab"), KeySpec::parse("alt+backtab"));
        assert_eq!(KeySpec::parse("F4").unwrap().code, KeyCode::F(4));
        assert_eq!(KeySpec::parse("ctrl++").unwrap().code, KeyCode::Char('+'));
        assert!(KeySpec::parse("hyper+x").is_none());
        assert_ne!(KeySpec::parse("ctrl+shift+c"), KeySpec::parse("ctrl+c"));
        assert_eq!(KeySpec::parse("ctrl+shift+c"), KeySpec::parse("ctrl+C"));
        assert_eq!(KeySpec::parse("alt+shift+a"), KeySpec::parse("alt+A"));
        assert_eq!(KeySpec::parse("shift+r"), KeySpec::parse("R"));
    }

    #[test]
    fn events_match_specs() {
        let ev = key(KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!(KeySpec::from_event(&ev), KeySpec::parse("shift+a").unwrap());
        let ev = key(KeyCode::Char('C'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(KeySpec::from_event(&ev), KeySpec::parse("ctrl+shift+c").unwrap());
        let ev = key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(KeySpec::from_event(&ev), KeySpec::parse("ctrl+c").unwrap());
        let ev = key(KeyCode::BackTab, KeyModifiers::SHIFT | KeyModifiers::ALT);
        assert_eq!(KeySpec::from_event(&ev), KeySpec::parse("alt+shift+tab").unwrap());
    }

    #[test]
    fn encodes_keys() {
        assert_eq!(encode_key(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), false), vec![3]);
        assert_eq!(encode_key(&key(KeyCode::Char('x'), KeyModifiers::ALT), false), b"\x1bx");
        assert_eq!(encode_key(&key(KeyCode::Char('a'), KeyModifiers::ALT | KeyModifiers::SHIFT), false), b"\x1bA");
        assert_eq!(encode_key(&key(KeyCode::Char('/'), KeyModifiers::CONTROL), false), vec![0x1f]);
        assert_eq!(encode_key(&key(KeyCode::Up, KeyModifiers::NONE), true), b"\x1bOA");
        assert_eq!(encode_key(&key(KeyCode::Up, KeyModifiers::NONE), false), b"\x1b[A");
        assert_eq!(encode_key(&key(KeyCode::Left, KeyModifiers::CONTROL), false), b"\x1b[1;5D");
        assert_eq!(encode_key(&key(KeyCode::F(5), KeyModifiers::NONE), false), b"\x1b[15~");
    }

    #[test]
    fn encodes_mouse() {
        let ev = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        let sgr = MouseProtocol { mode: MouseMode::Click, sgr: true, utf8: false };
        assert_eq!(encode_mouse(sgr, &ev, 4, 2).unwrap(), b"\x1b[<0;5;3M");
        let x10 = MouseProtocol { sgr: false, ..sgr };
        assert_eq!(encode_mouse(x10, &ev, 4, 2).unwrap(), b"\x1b[M %#");
        let off = MouseProtocol { mode: MouseMode::None, ..sgr };
        assert!(encode_mouse(off, &ev, 4, 2).is_none());
    }
}

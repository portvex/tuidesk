use std::collections::HashMap;

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
    Ansi16,
}

impl ColorMode {
    /// Color mode from config, auto-detecting from the viewing terminal's TERM/COLORTERM.
    pub fn resolve(config: &str, term: &str, colorterm: &str) -> Self {
        match config {
            "truecolor" | "24bit" => Self::TrueColor,
            "256" => Self::Ansi256,
            "16" => Self::Ansi16,
            _ => Self::detect(term, colorterm),
        }
    }

    fn detect(term: &str, colorterm: &str) -> Self {
        if colorterm.contains("truecolor") || colorterm.contains("24bit") {
            Self::TrueColor
        } else if term == "linux" || term == "vt100" || term == "vt220" {
            Self::Ansi16
        } else if term.contains("256") {
            Self::Ansi256
        } else {
            Self::Ansi16
        }
    }

    /// Downgrade a color to what the host terminal can show.
    pub fn adapt(self, c: Color) -> Color {
        match (self, c) {
            (Self::TrueColor, _) => c,
            (Self::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(rgb_to_256(r, g, b)),
            (Self::Ansi16, Color::Rgb(r, g, b)) => ansi16(nearest_16(r, g, b)),
            (Self::Ansi16, Color::Indexed(i)) => {
                if i < 16 {
                    ansi16(i)
                } else {
                    let (r, g, b) = index_to_rgb(i);
                    ansi16(nearest_16(r, g, b))
                }
            }
            _ => c,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    Plain,
    Rounded,
    Double,
    Thick,
    Ascii,
}

impl BorderStyle {
    /// [horizontal, vertical, top-left, top-right, bottom-left, bottom-right]
    pub fn chars(self) -> [&'static str; 6] {
        match self {
            Self::Plain => ["─", "│", "┌", "┐", "└", "┘"],
            Self::Rounded => ["─", "│", "╭", "╮", "╰", "╯"],
            Self::Double => ["═", "║", "╔", "╗", "╚", "╝"],
            Self::Thick => ["━", "┃", "┏", "┓", "┗", "┛"],
            Self::Ascii => ["-", "|", "+", "+", "+", "+"],
        }
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub mode: ColorMode,
    pub border: BorderStyle,
    pub wallpaper: Color,
    pub wallpaper_fg: Color,
    pub accent: Color,
    pub panel_bg: Color,
    pub panel_fg: Color,
    pub task_active_bg: Color,
    pub task_active_fg: Color,
    pub title_active_bg: Color,
    pub title_active_fg: Color,
    pub title_inactive_bg: Color,
    pub title_inactive_fg: Color,
    pub border_active: Color,
    pub border_inactive: Color,
    pub term_bg: Color,
    pub term_fg: Color,
    pub menu_bg: Color,
    pub menu_fg: Color,
    pub menu_dim: Color,
    pub menu_sel_bg: Color,
    pub menu_sel_fg: Color,
    pub notif_bg: Color,
    pub notif_fg: Color,
    pub urgent: Color,
    pub button_close: Color,
}

/// A preset's value for a theme key, such as "accent" or "border".
pub fn preset_value(preset_name: &str, key: &str) -> Option<&'static str> {
    preset(preset_name).into_iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

fn preset(name: &str) -> Vec<(&'static str, &'static str)> {
    match name {
        "breeze-light" => vec![
            ("border", "plain"),
            ("wallpaper", "#c9d5e3"),
            ("wallpaper_fg", "#7f8c8d"),
            ("accent", "#3daee9"),
            ("panel_bg", "#eff0f1"),
            ("panel_fg", "#232627"),
            ("task_active_bg", "#93cee9"),
            ("task_active_fg", "#232627"),
            ("title_active_bg", "#3daee9"),
            ("title_active_fg", "#ffffff"),
            ("title_inactive_bg", "#e3e5e7"),
            ("title_inactive_fg", "#7f8c8d"),
            ("border_active", "#3daee9"),
            ("border_inactive", "#bdc3c7"),
            ("term_bg", "#fcfcfc"),
            ("term_fg", "#232627"),
            ("menu_bg", "#fcfcfc"),
            ("menu_fg", "#232627"),
            ("menu_dim", "#7f8c8d"),
            ("menu_sel_bg", "#3daee9"),
            ("menu_sel_fg", "#ffffff"),
            ("notif_bg", "#eff0f1"),
            ("notif_fg", "#232627"),
            ("urgent", "#f67400"),
            ("button_close", "#da4453"),
        ],
        "tty16" => vec![
            ("border", "ascii"),
            ("wallpaper", "blue"),
            ("wallpaper_fg", "lightblue"),
            ("accent", "lightcyan"),
            ("panel_bg", "black"),
            ("panel_fg", "white"),
            ("task_active_bg", "cyan"),
            ("task_active_fg", "black"),
            ("title_active_bg", "cyan"),
            ("title_active_fg", "black"),
            ("title_inactive_bg", "gray"),
            ("title_inactive_fg", "black"),
            ("border_active", "lightcyan"),
            ("border_inactive", "gray"),
            ("term_bg", "black"),
            ("term_fg", "white"),
            ("menu_bg", "black"),
            ("menu_fg", "white"),
            ("menu_dim", "gray"),
            ("menu_sel_bg", "cyan"),
            ("menu_sel_fg", "black"),
            ("notif_bg", "black"),
            ("notif_fg", "white"),
            ("urgent", "lightred"),
            ("button_close", "red"),
        ],
        // breeze-dark
        _ => vec![
            ("border", "plain"),
            ("wallpaper", "#1b2b3a"),
            ("wallpaper_fg", "#3b5a73"),
            ("accent", "#3daee9"),
            ("panel_bg", "#202326"),
            ("panel_fg", "#fcfcfc"),
            ("task_active_bg", "#2a4a5e"),
            ("task_active_fg", "#fcfcfc"),
            ("title_active_bg", "#31363b"),
            ("title_active_fg", "#fcfcfc"),
            ("title_inactive_bg", "#2a2e32"),
            ("title_inactive_fg", "#7f8c8d"),
            ("border_active", "#3daee9"),
            ("border_inactive", "#3c4146"),
            ("term_bg", "#141618"),
            ("term_fg", "#fcfcfc"),
            ("menu_bg", "#2a2e32"),
            ("menu_fg", "#fcfcfc"),
            ("menu_dim", "#8a9095"),
            ("menu_sel_bg", "#3daee9"),
            ("menu_sel_fg", "#141618"),
            ("notif_bg", "#31363b"),
            ("notif_fg", "#fcfcfc"),
            ("urgent", "#f67400"),
            ("button_close", "#da4453"),
        ],
    }
}

impl Theme {
    /// Build a theme from a preset plus user overrides. Returns warnings for bad values.
    pub fn build(preset_name: &str, overrides: &HashMap<String, String>, mode: ColorMode) -> (Self, Vec<String>) {
        let mut values: HashMap<String, String> =
            preset(preset_name).into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let mut warnings = Vec::new();
        if !["breeze-dark", "breeze-light", "tty16"].contains(&preset_name) {
            warnings.push(format!("unknown theme preset '{preset_name}' (using breeze-dark)"));
        }
        if let Some(b) = overrides.get("border")
            && !["plain", "rounded", "double", "thick", "ascii"].contains(&b.as_str())
        {
            warnings.push(format!("unknown border style '{b}' (using plain)"));
        }
        for (k, v) in overrides {
            if values.contains_key(k) {
                values.insert(k.clone(), v.clone());
            } else {
                warnings.push(format!("unknown theme key '{k}'"));
            }
        }
        let mut get = |key: &str| -> Color {
            let raw = &values[key];
            match parse_color(raw) {
                Some(c) => mode.adapt(c),
                None => {
                    warnings.push(format!("bad color '{raw}' for theme.{key}"));
                    Color::Reset
                }
            }
        };
        let theme = Theme {
            mode,
            border: parse_border(&values["border"]),
            wallpaper: get("wallpaper"),
            wallpaper_fg: get("wallpaper_fg"),
            accent: get("accent"),
            panel_bg: get("panel_bg"),
            panel_fg: get("panel_fg"),
            task_active_bg: get("task_active_bg"),
            task_active_fg: get("task_active_fg"),
            title_active_bg: get("title_active_bg"),
            title_active_fg: get("title_active_fg"),
            title_inactive_bg: get("title_inactive_bg"),
            title_inactive_fg: get("title_inactive_fg"),
            border_active: get("border_active"),
            border_inactive: get("border_inactive"),
            term_bg: get("term_bg"),
            term_fg: get("term_fg"),
            menu_bg: get("menu_bg"),
            menu_fg: get("menu_fg"),
            menu_dim: get("menu_dim"),
            menu_sel_bg: get("menu_sel_bg"),
            menu_sel_fg: get("menu_sel_fg"),
            notif_bg: get("notif_bg"),
            notif_fg: get("notif_fg"),
            urgent: get("urgent"),
            button_close: get("button_close"),
        };
        (theme, warnings)
    }
}

fn parse_border(s: &str) -> BorderStyle {
    match s {
        "rounded" => BorderStyle::Rounded,
        "double" => BorderStyle::Double,
        "thick" => BorderStyle::Thick,
        "ascii" => BorderStyle::Ascii,
        _ => BorderStyle::Plain,
    }
}

pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let v = u32::from_str_radix(hex, 16).ok()?;
            return Some(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
        }
        return None;
    }
    if let Ok(i) = s.parse::<u8>() {
        return Some(Color::Indexed(i));
    }
    Some(match s.as_str() {
        "reset" | "default" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        _ => return None,
    })
}

fn ansi16(i: u8) -> Color {
    match i {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::Gray,
        8 => Color::DarkGray,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        _ => Color::White,
    }
}

const ANSI16_RGB: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (205, 0, 0),
    (0, 205, 0),
    (205, 205, 0),
    (0, 0, 238),
    (205, 0, 205),
    (0, 205, 205),
    (229, 229, 229),
    (127, 127, 127),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (92, 92, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

fn dist(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2) as u32;
    d(a.0, b.0) * 3 + d(a.1, b.1) * 4 + d(a.2, b.2) * 2
}

fn nearest_16(r: u8, g: u8, b: u8) -> u8 {
    (0..16u8).min_by_key(|&i| dist((r, g, b), ANSI16_RGB[i as usize])).unwrap_or(7)
}

const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    let level = |v: u8| CUBE.iter().enumerate().min_by_key(|(_, c)| (v as i32 - **c as i32).abs()).unwrap().0 as u8;
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (CUBE[ri as usize], CUBE[gi as usize], CUBE[bi as usize]);
    let cube_idx = 16 + 36 * ri + 6 * gi + bi;
    let avg = ((r as u16 + g as u16 + b as u16) / 3) as u8;
    let gray_i = if avg < 8 { 0 } else { ((avg - 8) / 10).min(23) };
    let gray_v = 8 + gray_i * 10;
    if dist((r, g, b), (gray_v, gray_v, gray_v)) < dist((r, g, b), cube) { 232 + gray_i } else { cube_idx }
}

pub fn index_to_rgb(i: u8) -> (u8, u8, u8) {
    match i {
        0..=15 => ANSI16_RGB[i as usize],
        16..=231 => {
            let i = i - 16;
            (CUBE[(i / 36) as usize], CUBE[((i / 6) % 6) as usize], CUBE[(i % 6) as usize])
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colors() {
        assert_eq!(parse_color("#3daee9"), Some(Color::Rgb(0x3d, 0xae, 0xe9)));
        assert_eq!(parse_color("LightBlue"), Some(Color::LightBlue));
        assert_eq!(parse_color("42"), Some(Color::Indexed(42)));
        assert_eq!(parse_color("nope"), None);
    }

    #[test]
    fn downgrades() {
        assert_eq!(rgb_to_256(255, 0, 0), 196);
        assert_eq!(rgb_to_256(128, 128, 128), 244);
        assert_eq!(ColorMode::Ansi16.adapt(Color::Rgb(250, 250, 250)), Color::White);
        assert_eq!(ColorMode::Ansi16.adapt(Color::Indexed(196)), Color::LightRed);
    }

    #[test]
    fn all_presets_valid() {
        for p in ["breeze-dark", "breeze-light", "tty16"] {
            let (_, warnings) = Theme::build(p, &HashMap::new(), ColorMode::TrueColor);
            assert!(warnings.is_empty(), "{p}: {warnings:?}");
        }
    }
}

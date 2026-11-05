//! Icons drawn as pictures on terminals that show images: the title bar buttons `[_] [^] [x]`
//! (also as they look under the pointer), app icons in Kickoff and the task manager, and the
//! battery in the tray. Each icon is an image a few cells wide, sent and placed like a window's
//! image under an owner id no window uses. Icons are opaque, drawn on the color of the cells
//! they replace, so they look the same with every image protocol (Sixel has no partial
//! transparency, and iTerm2 images could show stale screen content through clear pixels).
//! Where a terminal shows no images, callers keep drawing text.

use std::collections::HashMap;

use ratatui::style::Color;

use super::{Format, Graphics, ImageData};
use crate::theme::Theme;
use crate::wm::Button;

/// The owner of the icons in `HostGraphics`, in place of a window id.
pub const OWNER: u64 = u64::MAX;

/// A title bar button's picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    Minimize,
    Maximize,
    Restore,
    Close,
}

const SHAPES: [Shape; 4] = [Shape::Minimize, Shape::Maximize, Shape::Restore, Shape::Close];

/// What an app icon shows: a launcher category, or a session action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Terminal,
    Development,
    System,
    Files,
    Internet,
    Office,
    Multimedia,
    Other,
    Power,
}

const KINDS: [Kind; 9] = [
    Kind::Terminal,
    Kind::Development,
    Kind::System,
    Kind::Files,
    Kind::Internet,
    Kind::Office,
    Kind::Multimedia,
    Kind::Other,
    Kind::Power,
];

impl Kind {
    /// The icon for an app in a launcher category (config/apps.toml); others get `Other`.
    pub fn of_category(category: &str) -> Self {
        match category.to_ascii_lowercase().as_str() {
            "development" => Self::Development,
            "system" => Self::System,
            "files" => Self::Files,
            "internet" => Self::Internet,
            "office" => Self::Office,
            "multimedia" => Self::Multimedia,
            "terminal" | "shell" => Self::Terminal,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    /// 3 x 1 cells on the title bar.
    Button { shape: Shape, focused: bool, hover: bool },
    /// 2 x 1 cells on a background color.
    App { kind: Kind, bg: [u8; 3] },
    /// 2 x 1 cells on the panel: charge in tenths.
    Battery { level: u8 },
}

impl Icon {
    pub fn button(button: Button, maximized: bool, focused: bool, hover: bool) -> Self {
        let shape = match button {
            Button::Minimize => Shape::Minimize,
            Button::Maximize if maximized => Shape::Restore,
            Button::Maximize => Shape::Maximize,
            Button::Close => Shape::Close,
        };
        Self::Button { shape, focused, hover }
    }

    pub fn app(kind: Kind, bg: Color) -> Self {
        Self::App { kind, bg: rgb(bg, [0, 0, 0]) }
    }

    pub fn battery(percent: f32) -> Self {
        Self::Battery { level: (percent / 10.0).round().clamp(0.0, 10.0) as u8 }
    }

    /// Size in cells.
    pub fn size(self) -> (u16, u16) {
        match self {
            Self::Button { .. } => (3, 1),
            Self::App { .. } | Self::Battery { .. } => (2, 1),
        }
    }
}

/// Theme colors icons are drawn with; a change redraws them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Colors {
    /// Title bar background and foreground, inactive then active.
    title: [[u8; 3]; 4],
    close: [u8; 3],
    panel_bg: [u8; 3],
    panel_fg: [u8; 3],
    urgent: [u8; 3],
}

impl Colors {
    fn of(theme: &Theme) -> Self {
        Self {
            title: [
                rgb(theme.title_inactive_bg, [0x31, 0x36, 0x3b]),
                rgb(theme.title_inactive_fg, [0xbd, 0xc3, 0xc7]),
                rgb(theme.title_active_bg, [0x3d, 0xae, 0xe9]),
                rgb(theme.title_active_fg, [0xff, 0xff, 0xff]),
            ],
            close: rgb(theme.button_close, [0xda, 0x44, 0x53]),
            panel_bg: rgb(theme.panel_bg, [0x31, 0x36, 0x3b]),
            panel_fg: rgb(theme.panel_fg, [0xef, 0xf0, 0xf1]),
            urgent: rgb(theme.urgent, [0xda, 0x44, 0x53]),
        }
    }
}

/// The icons in use, drawn when first wanted and again when the theme or the cell size changes.
#[derive(Default)]
pub struct Icons {
    pub gfx: Graphics,
    ids: HashMap<Icon, u32>,
    /// Image ids are never reused, so the host never mistakes a new picture for an old one.
    next: u32,
    drawn: Option<(Colors, (u16, u16))>,
}

impl Icons {
    /// Make sure the title bar buttons (with `buttons`), the app icons on the given backgrounds,
    /// and the battery icon for `battery` (percent) are drawn.
    pub fn prepare(&mut self, theme: &Theme, cell: (u16, u16), buttons: bool, apps: &[Color], battery: Option<f32>) {
        let colors = Colors::of(theme);
        if self.drawn != Some((colors, cell)) {
            for (_, id) in self.ids.drain() {
                self.gfx.remove_image(id);
            }
            self.drawn = Some((colors, cell));
        }
        let mut wanted = Vec::new();
        if buttons {
            for shape in SHAPES {
                for (focused, hover) in [(false, false), (true, false), (false, true), (true, true)] {
                    wanted.push(Icon::Button { shape, focused, hover });
                }
            }
        }
        for &bg in apps {
            wanted.extend(KINDS.map(|kind| Icon::app(kind, bg)));
        }
        wanted.extend(battery.map(Icon::battery));
        for icon in wanted {
            if self.ids.contains_key(&icon) {
                continue;
            }
            let (cols, rows) = icon.size();
            let (w, h) = (cols as u32 * cell.0.max(1) as u32, rows as u32 * cell.1.max(1) as u32);
            self.next += 1;
            let id = self.next;
            self.gfx.add_image(id, ImageData::new(Format::Rgba, w, h, false, draw(icon, w, h, &colors)));
            self.gfx.virtuals.insert((id, 0), (cols, rows));
            self.ids.insert(icon, id);
        }
    }

    /// The image id of a drawn icon.
    pub fn id(&self, icon: Icon) -> Option<u32> {
        self.ids.get(&icon).copied()
    }
}

/// A theme color as RGB; `fallback` for the terminal's default color, which is unknown.
pub fn rgb(c: Color, fallback: [u8; 3]) -> [u8; 3] {
    let index = match c {
        Color::Rgb(r, g, b) => return [r, g, b],
        Color::Indexed(i) => i,
        Color::Reset => return fallback,
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
    };
    let (r, g, b) = crate::theme::index_to_rgb(index);
    [r, g, b]
}

// ------------------------------------------------------------------ drawing

type P = (f32, f32);

/// An opaque picture painted in layers; each layer's coverage of a pixel (0 to 1) comes from a
/// signed distance at the pixel's center, which anti-aliases the edges.
struct Canvas {
    w: u32,
    h: u32,
    px: Vec<[f32; 3]>,
}

impl Canvas {
    fn new(w: u32, h: u32, bg: [u8; 3]) -> Self {
        Self { w, h, px: vec![bg.map(f32::from); (w * h) as usize] }
    }

    fn paint(&mut self, color: [u8; 3], coverage: impl Fn(P) -> f32) {
        for y in 0..self.h {
            for x in 0..self.w {
                let a = coverage((x as f32 + 0.5, y as f32 + 0.5)).clamp(0.0, 1.0);
                if a > 0.0 {
                    let c = &mut self.px[(y * self.w + x) as usize];
                    *c = [0, 1, 2].map(|i| c[i] + (f32::from(color[i]) - c[i]) * a);
                }
            }
        }
    }

    fn rgba(self) -> Vec<u8> {
        self.px.iter().flat_map(|c| [c[0].round() as u8, c[1].round() as u8, c[2].round() as u8, 255]).collect()
    }
}

/// Coverage of a shape with signed distance `d` (negative inside).
fn fill(d: f32) -> f32 {
    0.5 - d
}

/// Coverage of a line `t` wide along the edge of a shape with signed distance `d`.
fn stroke(d: f32, t: f32) -> f32 {
    t / 2.0 + 0.5 - d.abs()
}

fn len(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

fn circle(p: P, c: P, r: f32) -> f32 {
    len(p.0 - c.0, p.1 - c.1) - r
}

fn segment(p: P, a: P, b: P) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let k = if len2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) };
    len(p.0 - a.0 - k * dx, p.1 - a.1 - k * dy)
}

/// Distance to the nearest of several segments (a polyline's stroke).
fn segments(p: P, lines: &[(P, P)]) -> f32 {
    lines.iter().map(|&(a, b)| segment(p, a, b)).fold(f32::MAX, f32::min)
}

fn round_rect(p: P, c: P, half: P, r: f32) -> f32 {
    let (qx, qy) = ((p.0 - c.0).abs() - half.0 + r, (p.1 - c.1).abs() - half.1 + r);
    len(qx.max(0.0), qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// A convex polygon, corners clockwise on screen (y down).
fn polygon(p: P, corners: &[P]) -> f32 {
    let n = corners.len();
    (0..n)
        .map(|i| {
            let (a, b) = (corners[i], corners[(i + 1) % n]);
            let (ex, ey) = (b.0 - a.0, b.1 - a.1);
            let l = len(ex, ey).max(f32::EPSILON);
            // Outward normal of a clockwise edge.
            ((p.0 - a.0) * -ey + (p.1 - a.1) * ex) / -l
        })
        .fold(f32::MIN, f32::max)
}

fn lighten(c: [u8; 3], amount: f32) -> [u8; 3] {
    c.map(|v| (f32::from(v) + (255.0 - f32::from(v)) * amount).round() as u8)
}

const WHITE: [u8; 3] = [0xff, 0xff, 0xff];

fn draw(icon: Icon, w: u32, h: u32, colors: &Colors) -> Vec<u8> {
    match icon {
        Icon::Button { shape, focused, hover } => draw_button(shape, focused, hover, w, h, colors),
        Icon::App { kind, bg } => draw_app(kind, bg, w, h),
        Icon::Battery { level } => draw_battery(level, w, h, colors),
    }
}

fn draw_button(shape: Shape, focused: bool, hover: bool, w: u32, h: u32, colors: &Colors) -> Vec<u8> {
    let [bg, fg] = if focused { [colors.title[2], colors.title[3]] } else { [colors.title[0], colors.title[1]] };
    let mut cv = Canvas::new(w, h, bg);
    let c = (w as f32 / 2.0, h as f32 / 2.0);
    // Half the glyph's size, and the line width.
    let s = (h.min(w) as f32 * 0.28).max(2.5);
    let t = (h as f32 / 14.0).max(1.0);
    // Under the pointer a button gets a disc, as in Breeze; the close button is red then, and
    // always on the focused window.
    let disc = match shape {
        Shape::Close if hover => Some(lighten(colors.close, 0.2)),
        Shape::Close if focused => Some(colors.close),
        _ if hover => Some(fg),
        _ => None,
    };
    let glyph = match (shape, disc) {
        (Shape::Close, Some(_)) => WHITE,
        (_, Some(_)) => bg,
        _ => fg,
    };
    if let Some(color) = disc {
        cv.paint(color, |p| fill(circle(p, c, s * 1.15)));
    }
    // On a disc the glyph shrinks to fit inside it.
    let s = if disc.is_some() && shape != Shape::Close { s * 0.75 } else { s };
    let lines: Vec<(P, P)> = match shape {
        Shape::Minimize => {
            vec![((c.0 - s, c.1 - s / 2.0), (c.0, c.1 + s / 2.0)), ((c.0, c.1 + s / 2.0), (c.0 + s, c.1 - s / 2.0))]
        }
        Shape::Maximize => {
            vec![((c.0 - s, c.1 + s / 2.0), (c.0, c.1 - s / 2.0)), ((c.0, c.1 - s / 2.0), (c.0 + s, c.1 + s / 2.0))]
        }
        Shape::Restore => {
            let (n, e, so, we) = ((c.0, c.1 - s), (c.0 + s, c.1), (c.0, c.1 + s), (c.0 - s, c.1));
            vec![(n, e), (e, so), (so, we), (we, n)]
        }
        Shape::Close => {
            let d = s * if disc.is_some() { 0.55 } else { 0.75 };
            vec![((c.0 - d, c.1 - d), (c.0 + d, c.1 + d)), ((c.0 - d, c.1 + d), (c.0 + d, c.1 - d))]
        }
    };
    cv.paint(glyph, |p| stroke(segments(p, &lines), t));
    cv.rgba()
}

/// An app icon: a rounded tile in the category's color with a white symbol (a folder for Files).
fn draw_app(kind: Kind, bg: [u8; 3], w: u32, h: u32) -> Vec<u8> {
    let mut cv = Canvas::new(w, h, bg);
    let c = (w as f32 / 2.0, h as f32 / 2.0);
    let size = w.min(h) as f32;
    // Half the tile's side, its corner radius, a unit for the symbol, and the line width.
    let half = size * 0.42;
    let u = size * 0.2;
    let t = (size / 11.0).max(1.0);
    let tile = |cv: &mut Canvas, color: [u8; 3]| cv.paint(color, |p| fill(round_rect(p, c, (half, half), size * 0.12)));
    match kind {
        Kind::Terminal => {
            tile(&mut cv, [0x4d, 0x53, 0x5a]);
            let prompt = [
                ((c.0 - u * 1.2, c.1 - u * 0.8), (c.0 - u * 0.3, c.1)),
                ((c.0 - u * 0.3, c.1), (c.0 - u * 1.2, c.1 + u * 0.8)),
            ];
            cv.paint([0x2e, 0xcc, 0x71], |p| stroke(segments(p, &prompt), t));
            cv.paint(WHITE, |p| stroke(segment(p, (c.0 + u * 0.1, c.1 + u * 0.8), (c.0 + u * 1.2, c.1 + u * 0.8)), t));
        }
        Kind::Development => {
            tile(&mut cv, [0x8e, 0x44, 0xad]);
            let code = [
                ((c.0 - u * 0.4, c.1 - u * 0.9), (c.0 - u * 1.3, c.1)),
                ((c.0 - u * 1.3, c.1), (c.0 - u * 0.4, c.1 + u * 0.9)),
                ((c.0 + u * 0.4, c.1 - u * 0.9), (c.0 + u * 1.3, c.1)),
                ((c.0 + u * 1.3, c.1), (c.0 + u * 0.4, c.1 + u * 0.9)),
            ];
            cv.paint(WHITE, |p| stroke(segments(p, &code), t));
        }
        Kind::System => {
            tile(&mut cv, [0x7f, 0x8c, 0x8d]);
            // A gear: a ring with six teeth.
            let teeth: Vec<(P, P)> = (0..6)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::PI / 3.0;
                    let (x, y) = (a.cos(), a.sin());
                    ((c.0 + x * u * 0.8, c.1 + y * u * 0.8), (c.0 + x * u * 1.35, c.1 + y * u * 1.35))
                })
                .collect();
            cv.paint(WHITE, |p| stroke(segments(p, &teeth), t * 1.4));
            cv.paint(WHITE, |p| stroke(circle(p, c, u * 0.75), t * 1.3));
        }
        Kind::Files => {
            // A folder, without a tile: a tab and a body.
            let body = (c.0, c.1 + size * 0.05);
            cv.paint([0xe0, 0x98, 0x2a], |p| {
                fill(round_rect(p, (c.0 - half * 0.45, c.1 - half * 0.55), (half * 0.45, half * 0.3), size * 0.06))
            });
            cv.paint([0xfd, 0xbc, 0x4b], |p| fill(round_rect(p, body, (half, half * 0.7), size * 0.08)));
        }
        Kind::Internet => {
            tile(&mut cv, [0x1d, 0x6f, 0xb8]);
            // A globe: outline, equator and a meridian.
            let r = u * 1.3;
            cv.paint(WHITE, |p| stroke(circle(p, c, r), t));
            cv.paint(WHITE, |p| stroke(segment(p, (c.0 - r, c.1), (c.0 + r, c.1)), t));
            cv.paint(WHITE, |p| stroke(len((p.0 - c.0) / 0.45, p.1 - c.1) * 0.45 - r * 0.45, t));
        }
        Kind::Office => {
            tile(&mut cv, [0x27, 0xae, 0x60]);
            // Lines of text, the last one short.
            for (i, width) in [1.1f32, 1.1, 0.6].iter().enumerate() {
                let y = c.1 + (i as f32 - 1.0) * u * 0.9;
                let a = (c.0 - u * 1.1, y);
                let b = (c.0 - u * 1.1 + u * 2.2 * width / 1.1, y);
                cv.paint(WHITE, |p| stroke(segment(p, a, b), t));
            }
        }
        Kind::Multimedia => {
            tile(&mut cv, [0xda, 0x44, 0x53]);
            let play = [(c.0 - u * 0.7, c.1 - u * 1.1), (c.0 + u * 1.1, c.1), (c.0 - u * 0.7, c.1 + u * 1.1)];
            cv.paint(WHITE, |p| fill(polygon(p, &play)));
        }
        Kind::Other => {
            tile(&mut cv, [0x95, 0xa5, 0xa6]);
            for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                cv.paint(WHITE, |p| fill(circle(p, (c.0 + dx * u * 0.6, c.1 + dy * u * 0.6), u * 0.35)));
            }
        }
        Kind::Power => {
            tile(&mut cv, [0xc0, 0x39, 0x2b]);
            // An arc open at the top, and a bar through the gap.
            let r = u * 1.05;
            cv.paint(WHITE, |p| {
                let open = p.1 < c.1 && (p.0 - c.0).abs() < (c.1 - p.1) * 0.75;
                if open { 0.0 } else { stroke(circle(p, c, r), t) }
            });
            cv.paint(WHITE, |p| stroke(segment(p, (c.0, c.1 - r * 1.2), (c.0, c.1 - r * 0.15)), t));
        }
    }
    cv.rgba()
}

/// A battery on the panel: an outline filled to the charge, red when low. (Whether it charges
/// stays text next to it: a bolt does not read at a cell's size.)
fn draw_battery(level: u8, w: u32, h: u32, colors: &Colors) -> Vec<u8> {
    let mut cv = Canvas::new(w, h, colors.panel_bg);
    let size = w.min(h) as f32;
    let t = (size / 12.0).max(1.0);
    // The body, a little left of center to leave room for the terminal nub.
    let half = ((w as f32 * 0.4).min(size * 0.55), size * 0.26);
    let c = (w as f32 / 2.0 - half.0 * 0.08, h as f32 / 2.0);
    let nub = (c.0 + half.0 + t * 1.2, c.1);
    cv.paint(colors.panel_fg, |p| fill(round_rect(p, nub, (t * 0.9, half.1 * 0.45), t * 0.5)));
    cv.paint(colors.panel_fg, |p| stroke(round_rect(p, c, half, size * 0.06), t));
    let color = match level {
        0..=1 => colors.urgent,
        2..=3 => [0xf6, 0x74, 0x00],
        _ => [0x27, 0xae, 0x60],
    };
    if level > 0 {
        let inner = (half.0 - t * 1.6, half.1 - t * 1.6);
        let width = inner.0 * f32::from(level) / 10.0;
        let fc = (c.0 - inner.0 + width, c.1);
        cv.paint(color, |p| fill(round_rect(p, fc, (width, inner.1), t * 0.3)));
    }
    cv.rgba()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::build("breeze-dark", &Default::default(), crate::theme::ColorMode::TrueColor).0
    }

    #[test]
    fn button_icons_differ_by_shape_focus_and_hover() {
        let mut icons = Icons::default();
        icons.prepare(&theme(), (10, 20), true, &[], None);
        assert_eq!(icons.gfx.images.len(), 16);
        let id = |b, m, f, h| icons.id(Icon::button(b, m, f, h)).unwrap();
        // Minimize and close look the same maximized or not; maximize becomes restore.
        assert_eq!(id(Button::Close, true, true, false), id(Button::Close, false, true, false));
        assert_ne!(id(Button::Maximize, true, true, false), id(Button::Maximize, false, true, false));
        assert_ne!(id(Button::Minimize, false, true, true), id(Button::Minimize, false, true, false));
        let img = &icons.gfx.images[&id(Button::Close, false, true, false)];
        assert_eq!((img.width, img.height, img.data.len()), (30, 20, 30 * 20 * 4));
        assert_eq!(icons.gfx.virtuals[&(id(Button::Close, false, true, false), 0)], (3, 1));
        // The corner is title bar, the middle is the red close disc.
        let t = theme();
        assert_eq!(&img.data[..3], &rgb(t.title_active_bg, [0; 3]));
        let mid = ((10 * 30 + 15) * 4) as usize;
        assert_ne!(&img.data[mid..mid + 3], &img.data[..3]);
        // Hovering an inactive window's minimize button draws a disc in the title color.
        let hovered = &icons.gfx.images[&id(Button::Minimize, false, false, true)];
        let plain = &icons.gfx.images[&id(Button::Minimize, false, false, false)];
        let below_center = ((14 * 30 + 15) * 4) as usize;
        assert_ne!(&hovered.data[below_center..below_center + 3], &plain.data[below_center..below_center + 3]);
    }

    #[test]
    fn app_and_battery_icons_are_two_cells_on_their_background() {
        let mut icons = Icons::default();
        let t = theme();
        icons.prepare(&t, (10, 20), false, &[t.panel_bg, t.menu_sel_bg], Some(83.0));
        assert_eq!(icons.gfx.images.len(), KINDS.len() * 2 + 1);
        let on_panel = icons.id(Icon::app(Kind::Files, t.panel_bg)).unwrap();
        let on_sel = icons.id(Icon::app(Kind::Files, t.menu_sel_bg)).unwrap();
        assert_ne!(on_panel, on_sel);
        let img = &icons.gfx.images[&on_sel];
        assert_eq!((img.width, img.height), (20, 20));
        assert_eq!(&img.data[..3], &rgb(t.menu_sel_bg, [0; 3]), "corner shows the row's background");
        assert_eq!(icons.gfx.virtuals[&(on_sel, 0)], (2, 1));
        assert_eq!(Icon::battery(83.0), Icon::Battery { level: 8 });
        assert!(icons.id(Icon::battery(80.0)).is_some(), "same tenth, same icon");
        assert!(icons.id(Icon::battery(70.0)).is_none());
    }

    #[test]
    fn icons_are_redrawn_under_new_ids_when_theme_or_cell_size_changes() {
        let mut icons = Icons::default();
        let t = theme();
        icons.prepare(&t, (10, 20), true, &[t.panel_bg], None);
        let before = icons.id(Icon::button(Button::Close, false, true, false)).unwrap();
        let count = icons.gfx.images.len();
        icons.prepare(&t, (10, 20), true, &[t.panel_bg], None);
        assert_eq!(icons.id(Icon::button(Button::Close, false, true, false)), Some(before), "unchanged");
        icons.prepare(&t, (12, 24), true, &[t.panel_bg], None);
        let after = icons.id(Icon::button(Button::Close, false, true, false)).unwrap();
        assert_ne!(after, before);
        assert!(!icons.gfx.images.contains_key(&before), "old pictures are dropped");
        assert_eq!(icons.gfx.images.len(), count);
        assert_eq!((icons.gfx.images[&after].width, icons.gfx.images[&after].height), (36, 24));
    }

    #[test]
    fn categories_map_to_kinds_and_colors_to_rgb() {
        assert_eq!(Kind::of_category("Development"), Kind::Development);
        assert_eq!(Kind::of_category("files"), Kind::Files);
        assert_eq!(Kind::of_category("Games"), Kind::Other);
        assert_eq!(rgb(Color::Rgb(1, 2, 3), [0; 3]), [1, 2, 3]);
        assert_eq!(rgb(Color::Reset, [9, 9, 9]), [9, 9, 9]);
        assert_eq!(rgb(Color::Indexed(231), [0; 3]), [255, 255, 255]);
        assert_ne!(rgb(Color::Red, [0; 3]), rgb(Color::Blue, [0; 3]));
    }
}

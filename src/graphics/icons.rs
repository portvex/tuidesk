//! Title bar buttons as pictures. When the host terminal shows images, the `[_] [^] [x]`
//! buttons are drawn as small icons instead: each is an image the size of the button (3 x 1
//! cells), sent and placed like a window's image, under an owner id no window uses.

use ratatui::style::Color;

use super::{Format, Graphics, ImageData};
use crate::theme::Theme;
use crate::wm::Button;

/// The owner of the icons in `HostGraphics`, in place of a window id.
pub const OWNER: u64 = u64::MAX;
/// A button's size in cells.
pub const COLS: u16 = 3;
pub const ROWS: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Minimize,
    Maximize,
    Restore,
    Close,
}

const SHAPES: [Shape; 4] = [Shape::Minimize, Shape::Maximize, Shape::Restore, Shape::Close];

/// The icon id for a button in a window's state.
pub fn id(button: Button, maximized: bool, focused: bool) -> u32 {
    let shape = match button {
        Button::Minimize => Shape::Minimize,
        Button::Maximize if maximized => Shape::Restore,
        Button::Maximize => Shape::Maximize,
        Button::Close => Shape::Close,
    };
    shape_id(shape, focused)
}

fn shape_id(shape: Shape, focused: bool) -> u32 {
    shape as u32 * 2 + u32::from(focused) + 1
}

/// Colors an icon set is drawn with: title bar background and foreground (inactive, active),
/// and the close button's color.
type Colors = [[u8; 3]; 5];

/// The icons, redrawn when the theme or the cell size changes.
#[derive(Default)]
pub struct ButtonIcons {
    pub gfx: Graphics,
    drawn: Option<(Colors, (u16, u16))>,
}

impl ButtonIcons {
    pub fn update(&mut self, theme: &Theme, cell: (u16, u16)) {
        let colors = [
            rgb(theme.title_inactive_bg, [0x31, 0x36, 0x3b]),
            rgb(theme.title_inactive_fg, [0xbd, 0xc3, 0xc7]),
            rgb(theme.title_active_bg, [0x3d, 0xae, 0xe9]),
            rgb(theme.title_active_fg, [0xff, 0xff, 0xff]),
            rgb(theme.button_close, [0xda, 0x44, 0x53]),
        ];
        if self.drawn == Some((colors, cell)) {
            return;
        }
        let (w, h) = (COLS as u32 * cell.0.max(1) as u32, ROWS as u32 * cell.1.max(1) as u32);
        for focused in [false, true] {
            let (bg, fg) = if focused { (colors[2], colors[3]) } else { (colors[0], colors[1]) };
            for shape in SHAPES {
                let close = (shape == Shape::Close && focused).then_some(colors[4]);
                let px = draw(shape, w, h, bg, fg, close);
                let id = shape_id(shape, focused);
                self.gfx.add_image(id, ImageData::new(Format::Rgba, w, h, false, px));
                self.gfx.virtuals.insert((id, 0), (COLS, ROWS));
            }
        }
        self.drawn = Some((colors, cell));
    }
}

/// A theme color as RGB; `fallback` for the terminal's default color, which is unknown.
fn rgb(c: Color, fallback: [u8; 3]) -> [u8; 3] {
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

/// RGBA pixels of one icon, `w` x `h`, centered on the title bar color. Lines are anti-aliased
/// by their distance from each pixel's center.
fn draw(shape: Shape, w: u32, h: u32, bg: [u8; 3], fg: [u8; 3], close: Option<[u8; 3]>) -> Vec<u8> {
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    // Half the glyph's size, and the line width.
    let s = (h.min(w) as f32 * 0.28).max(2.5);
    let t = (h as f32 / 14.0).max(1.0);
    let lines: Vec<((f32, f32), (f32, f32))> = match shape {
        Shape::Minimize => {
            vec![((cx - s, cy - s / 2.0), (cx, cy + s / 2.0)), ((cx, cy + s / 2.0), (cx + s, cy - s / 2.0))]
        }
        Shape::Maximize => {
            vec![((cx - s, cy + s / 2.0), (cx, cy - s / 2.0)), ((cx, cy - s / 2.0), (cx + s, cy + s / 2.0))]
        }
        Shape::Restore => {
            let (n, e, so, we) = ((cx, cy - s), (cx + s, cy), (cx, cy + s), (cx - s, cy));
            vec![(n, e), (e, so), (so, we), (we, n)]
        }
        Shape::Close => {
            let d = s * if close.is_some() { 0.55 } else { 0.75 };
            vec![((cx - d, cy - d), (cx + d, cy + d)), ((cx - d, cy + d), (cx + d, cy - d))]
        }
    };
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let mut c = bg.map(f32::from);
            if let Some(red) = close {
                let r = s * 1.15;
                let cover = (r + 0.5 - ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()).clamp(0.0, 1.0);
                c = mix(c, red, cover);
            }
            let cover = lines.iter().map(|&(a, b)| (t / 2.0 + 0.5 - segment_distance(p, a, b)).clamp(0.0, 1.0));
            c = mix(c, fg, cover.fold(0.0, f32::max));
            px.extend(c.map(|v| v.round() as u8));
            px.push(255);
        }
    }
    px
}

fn mix(c: [f32; 3], to: [u8; 3], amount: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| c[i] + (f32::from(to[i]) - c[i]) * amount)
}

fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let k = if len2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) };
    ((p.0 - a.0 - k * dx).powi(2) + (p.1 - a.1 - k * dy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_distinct_per_shape_and_focus() {
        let mut ids: Vec<u32> = [Button::Minimize, Button::Maximize, Button::Close]
            .into_iter()
            .flat_map(|b| [(false, false), (true, false), (false, true), (true, true)].map(|(m, f)| id(b, m, f)))
            .collect();
        ids.sort();
        ids.dedup();
        // Minimize and close look the same maximized or not.
        assert_eq!(ids.len(), 8);
        assert!(ids.iter().all(|&i| i > 0));
    }

    #[test]
    fn icons_fill_the_button_and_follow_theme_and_cell_size() {
        let mut icons = ButtonIcons::default();
        let (theme, _) = Theme::build("breeze", &Default::default(), crate::theme::ColorMode::TrueColor);
        icons.update(&theme, (10, 20));
        assert_eq!(icons.gfx.images.len(), 8);
        let img = &icons.gfx.images[&id(Button::Close, false, true)];
        assert_eq!((img.width, img.height, img.data.len()), (30, 20, 30 * 20 * 4));
        // The corner is title bar, the middle is the red close circle.
        assert_eq!(&img.data[..3], &rgb(theme.title_active_bg, [0; 3]));
        let mid = ((10 * 30 + 15) * 4) as usize;
        assert_ne!(&img.data[mid..mid + 3], &img.data[..3]);
        assert_eq!(icons.gfx.virtuals[&(id(Button::Minimize, false, false), 0)], (COLS, ROWS));

        let generation = img.generation;
        icons.update(&theme, (10, 20));
        assert_eq!(icons.gfx.images[&id(Button::Close, false, true)].generation, generation, "unchanged");
        icons.update(&theme, (12, 24));
        let img = &icons.gfx.images[&id(Button::Close, false, true)];
        assert_ne!(img.generation, generation);
        assert_eq!((img.width, img.height), (36, 24));
    }

    #[test]
    fn named_and_indexed_colors_become_rgb() {
        assert_eq!(rgb(Color::Rgb(1, 2, 3), [0; 3]), [1, 2, 3]);
        assert_eq!(rgb(Color::Reset, [9, 9, 9]), [9, 9, 9]);
        assert_eq!(rgb(Color::Indexed(231), [0; 3]), [255, 255, 255]);
        assert_ne!(rgb(Color::Red, [0; 3]), rgb(Color::Blue, [0; 3]));
    }
}

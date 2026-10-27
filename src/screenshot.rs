//! Screenshots: write part of a rendered screen as text, ANSI, HTML or SVG.

use std::fmt::Write as _;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

use crate::theme::index_to_rgb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Ansi,
    Html,
    Svg,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "txt" | "text" => Self::Text,
            "ansi" => Self::Ansi,
            "html" | "htm" => Self::Html,
            "svg" => Self::Svg,
            _ => return None,
        })
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Text => "txt",
            Self::Ansi => "ansi",
            Self::Html => "html",
            Self::Svg => "svg",
        }
    }
}

type Rgb = (u8, u8, u8);

/// One cell's look, resolved to RGB.
#[derive(Clone, Copy, PartialEq)]
struct Look {
    fg: Rgb,
    bg: Rgb,
    bold: bool,
    italic: bool,
    underline: bool,
    dim: bool,
    strike: bool,
}

/// A horizontal run of cells with the same look.
struct Run {
    text: String,
    /// Width in cells.
    width: usize,
    look: Look,
}

/// A color as RGB; `default` stands in for "reset".
pub fn rgb(c: Color, default: Rgb) -> Rgb {
    match c {
        Color::Reset => default,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) => index_to_rgb(i),
        Color::Black => index_to_rgb(0),
        Color::Red => index_to_rgb(1),
        Color::Green => index_to_rgb(2),
        Color::Yellow => index_to_rgb(3),
        Color::Blue => index_to_rgb(4),
        Color::Magenta => index_to_rgb(5),
        Color::Cyan => index_to_rgb(6),
        Color::Gray => index_to_rgb(7),
        Color::DarkGray => index_to_rgb(8),
        Color::LightRed => index_to_rgb(9),
        Color::LightGreen => index_to_rgb(10),
        Color::LightYellow => index_to_rgb(11),
        Color::LightBlue => index_to_rgb(12),
        Color::LightMagenta => index_to_rgb(13),
        Color::LightCyan => index_to_rgb(14),
        Color::White => index_to_rgb(15),
    }
}

/// Split each row of `area` into runs of identically styled text.
fn rows(buf: &Buffer, area: Rect, defaults: (Rgb, Rgb)) -> Vec<Vec<Run>> {
    let area = area.intersection(buf.area);
    let mut out = Vec::with_capacity(area.height as usize);
    for y in area.top()..area.bottom() {
        let mut runs: Vec<Run> = Vec::new();
        let mut x = area.left();
        while x < area.right() {
            let cell = &buf[(x, y)];
            let mut sym = cell.symbol();
            if sym.is_empty() {
                sym = " ";
            }
            let width = sym.width().max(1);
            // A wide character that would spill out of the area becomes a space.
            let (sym, width) = if x as usize + width > area.right() as usize { (" ", 1) } else { (sym, width) };
            let m = cell.modifier;
            let (mut fg, mut bg) = (rgb(cell.fg, defaults.0), rgb(cell.bg, defaults.1));
            if m.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let look = Look {
                fg,
                bg,
                bold: m.contains(Modifier::BOLD),
                italic: m.contains(Modifier::ITALIC),
                underline: m.contains(Modifier::UNDERLINED),
                dim: m.contains(Modifier::DIM),
                strike: m.contains(Modifier::CROSSED_OUT),
            };
            match runs.last_mut() {
                Some(r) if r.look == look => {
                    r.text.push_str(sym);
                    r.width += width;
                }
                _ => runs.push(Run { text: sym.to_string(), width, look }),
            }
            x += width as u16;
        }
        out.push(runs);
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Render `area` of a buffer. `defaults` are the (foreground, background) for "reset" colors.
pub fn render(buf: &Buffer, area: Rect, format: Format, defaults: (Rgb, Rgb), title: &str) -> String {
    let rows = rows(buf, area, defaults);
    match format {
        Format::Text => {
            let mut out = String::new();
            for runs in &rows {
                let line: String = runs.iter().map(|r| r.text.as_str()).collect();
                out.push_str(line.trim_end());
                out.push('\n');
            }
            out
        }
        Format::Ansi => {
            let mut out = String::new();
            for runs in &rows {
                for r in runs {
                    let l = r.look;
                    let mut sgr =
                        format!("0;38;2;{};{};{};48;2;{};{};{}", l.fg.0, l.fg.1, l.fg.2, l.bg.0, l.bg.1, l.bg.2);
                    for (on, code) in
                        [(l.bold, "1"), (l.dim, "2"), (l.italic, "3"), (l.underline, "4"), (l.strike, "9")]
                    {
                        if on {
                            sgr.push(';');
                            sgr.push_str(code);
                        }
                    }
                    let _ = write!(out, "\x1b[{sgr}m{}", r.text);
                }
                out.push_str("\x1b[0m\n");
            }
            out
        }
        Format::Html => {
            let mut out = format!(
                "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{}</title>\n<style>\n\
                 body {{ margin: 0; background: {}; }}\n\
                 pre {{ margin: 0; padding: 12px; font: 14px/1.2 'JetBrains Mono', Menlo, Consolas, monospace; }}\n\
                 </style></head><body><pre>",
                escape(title),
                hex(defaults.1)
            );
            for runs in &rows {
                for r in runs {
                    let l = r.look;
                    let mut css = format!("color:{};background:{}", hex(l.fg), hex(l.bg));
                    if l.bold {
                        css.push_str(";font-weight:bold");
                    }
                    if l.italic {
                        css.push_str(";font-style:italic");
                    }
                    if l.dim {
                        css.push_str(";opacity:.6");
                    }
                    match (l.underline, l.strike) {
                        (true, true) => css.push_str(";text-decoration:underline line-through"),
                        (true, false) => css.push_str(";text-decoration:underline"),
                        (false, true) => css.push_str(";text-decoration:line-through"),
                        _ => {}
                    }
                    let _ = write!(out, "<span style=\"{css}\">{}</span>", escape(&r.text));
                }
                out.push('\n');
            }
            out.push_str("</pre></body></html>\n");
            out
        }
        Format::Svg => {
            const CW: f32 = 8.4;
            const LH: f32 = 17.0;
            let cols = area.width as f32;
            let (w, h) = (cols * CW, rows.len() as f32 * LH);
            let mut out = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n\
                 <title>{}</title>\n<rect width=\"100%\" height=\"100%\" fill=\"{}\"/>\n\
                 <g font-family=\"'JetBrains Mono', Menlo, Consolas, monospace\" font-size=\"14\" xml:space=\"preserve\">\n",
                escape(title),
                hex(defaults.1)
            );
            for (row, runs) in rows.iter().enumerate() {
                let y = row as f32 * LH;
                let mut col = 0usize;
                for r in runs {
                    let x = col as f32 * CW;
                    let width = r.width as f32 * CW;
                    let l = r.look;
                    if l.bg != defaults.1 {
                        let _ = writeln!(
                            out,
                            "<rect x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{LH}\" fill=\"{}\"/>",
                            hex(l.bg)
                        );
                    }
                    // Draw only the visible text, at its own cell, so renderers that collapse
                    // spaces cannot stretch it; inner spaces become non-breaking.
                    let lead = r.text.chars().take_while(|c| *c == ' ').count();
                    let trimmed = r.text.trim_matches(' ');
                    if !trimmed.is_empty() {
                        let text_x = x + lead as f32 * CW;
                        let text_w = trimmed.width() as f32 * CW;
                        let text = escape(trimmed).replace(' ', "\u{a0}");
                        let mut attrs = format!("fill=\"{}\"", hex(l.fg));
                        if l.bold {
                            attrs.push_str(" font-weight=\"bold\"");
                        }
                        if l.italic {
                            attrs.push_str(" font-style=\"italic\"");
                        }
                        if l.dim {
                            attrs.push_str(" opacity=\".6\"");
                        }
                        if l.underline {
                            attrs.push_str(" text-decoration=\"underline\"");
                        }
                        // textLength keeps runs on the cell grid whatever font is used.
                        let _ = writeln!(
                            out,
                            "<text x=\"{text_x}\" y=\"{}\" textLength=\"{text_w}\" lengthAdjust=\"spacingAndGlyphs\" {attrs}>{text}</text>",
                            y + LH * 0.78,
                        );
                    }
                    col += r.width;
                }
            }
            out.push_str("</g>\n</svg>\n");
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn sample() -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 2));
        buf.set_string(0, 0, "a<b & c", Style::new().fg(Color::Red));
        buf.set_string(8, 0, "OK", Style::new().fg(Color::Rgb(1, 2, 3)).bg(Color::White).add_modifier(Modifier::BOLD));
        buf.set_string(0, 1, "wide:日本", Style::new());
        buf
    }

    const DEFAULTS: (Rgb, Rgb) = ((250, 250, 250), (10, 10, 10));

    #[test]
    fn text_is_trimmed_and_handles_wide_chars() {
        let out = render(&sample(), Rect::new(0, 0, 12, 2), Format::Text, DEFAULTS, "t");
        assert_eq!(out, "a<b & c OK\nwide:日本\n");
        let crop = render(&sample(), Rect::new(8, 0, 2, 1), Format::Text, DEFAULTS, "t");
        assert_eq!(crop, "OK\n");
        // A wide character cut by the right edge turns into a space.
        let cut = render(&sample(), Rect::new(0, 1, 8, 1), Format::Text, DEFAULTS, "t");
        assert_eq!(cut, "wide:日\n");
    }

    #[test]
    fn html_escapes_and_styles_runs() {
        let out = render(&sample(), Rect::new(0, 0, 12, 1), Format::Html, DEFAULTS, "shot <1>");
        assert!(out.contains("<title>shot &lt;1&gt;</title>"));
        assert!(out.contains("a&lt;b &amp; c"));
        assert!(out.contains("color:#010203;background:#ffffff;font-weight:bold\">OK</span>"));
        assert!(out.contains("background: #0a0a0a"));
    }

    #[test]
    fn ansi_and_svg() {
        let ansi = render(&sample(), Rect::new(8, 0, 2, 1), Format::Ansi, DEFAULTS, "t");
        assert_eq!(ansi, "\x1b[0;38;2;1;2;3;48;2;255;255;255;1mOK\x1b[0m\n");
        let svg = render(&sample(), Rect::new(0, 0, 12, 2), Format::Svg, DEFAULTS, "t");
        assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains(">OK</text>") && svg.contains("fill=\"#ffffff\""));
        assert!(svg.contains("a&lt;b\u{a0}&amp;\u{a0}c"));
        // " OK" padded runs are drawn from their first visible cell, without the spaces.
        let padded = render(&sample(), Rect::new(7, 0, 3, 1), Format::Svg, DEFAULTS, "t");
        assert!(padded.contains("<text x=\"8.4\""), "{padded}");
    }
}

//! Images on terminals without kitty Unicode placeholders (iTerm2, WezTerm, VS Code, and Sixel
//! terminals such as foot, xterm or Konsole).
//!
//! Windows are drawn with placeholder cells as usual. After a frame is drawn, the placeholder
//! cells that are still visible (not covered by other windows or overlays) are grouped into
//! rectangles; those cells are marked so ratatui never paints them, and each rectangle is drawn
//! directly as a cropped, scaled image with the iTerm2 or Sixel protocol.

use std::collections::HashMap;
use std::io::Read as _;
use std::sync::Arc;

use base64::Engine as _;
use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[cfg(test)]
use super::DIACRITICS;
use super::{Format, ImageData, PLACEHOLDER, diacritic_index};

/// How the host terminal shows images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageMode {
    #[default]
    None,
    /// kitty graphics with Unicode placeholders (kitty, Ghostty).
    Placeholders,
    /// iTerm2 inline images (iTerm2, WezTerm, VS Code).
    Iterm,
    /// DEC Sixel (foot, xterm, Konsole, WezTerm, mlterm, ...).
    Sixel,
}

impl ImageMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" | "off" => Self::None,
            "kitty" | "placeholders" => Self::Placeholders,
            "iterm" | "iterm2" => Self::Iterm,
            "sixel" => Self::Sixel,
            _ => return None,
        })
    }

    /// Images are drawn directly by tuidesk (rather than by the terminal from placeholders).
    pub fn is_direct(self) -> bool {
        matches!(self, Self::Iterm | Self::Sixel)
    }
}

/// A visible piece of an image: where it is on screen and which cells of the image it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Region {
    pub host_id: u32,
    /// The image's size in cells (its placement).
    pub cols: u16,
    pub rows: u16,
    /// Screen cells covered.
    pub screen: Rect,
    /// Top-left image cell shown in the region's top-left.
    pub col: u16,
    pub row: u16,
}

fn color_u24(c: Color) -> Option<u32> {
    match c {
        Color::Rgb(r, g, b) => Some((r as u32) << 16 | (g as u32) << 8 | b as u32),
        _ => None,
    }
}

/// Find the visible placeholder cells in `buf`, group them into rectangles, and blank them so
/// ratatui leaves them alone (the images are drawn there afterwards). `area` limits the search.
pub fn take_regions(buf: &mut Buffer, area: Rect) -> Vec<Region> {
    // Horizontal runs: (host id, placement id, y, x start, length, image row, image col start).
    let mut runs: Vec<(u32, u32, u16, u16, u16, u16, u16)> = Vec::new();
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            let mut chars = cell.symbol().chars();
            if chars.next() != Some(PLACEHOLDER) {
                continue;
            }
            let (Some(r), Some(c)) = (chars.next().and_then(diacritic_index), chars.next().and_then(diacritic_index))
            else {
                continue;
            };
            let (Some(hid), Some(pid)) = (color_u24(cell.fg), color_u24(cell.underline_color)) else { continue };
            let (r, c) = (r as u16, c as u16);
            cell.set_symbol(" ");
            cell.set_diff_option(CellDiffOption::Skip);
            match runs.last_mut() {
                Some(run)
                    if run.0 == hid
                        && run.1 == pid
                        && run.2 == y
                        && run.3 + run.4 == x
                        && run.5 == r
                        && run.6 + run.4 == c =>
                {
                    run.4 += 1
                }
                _ => runs.push((hid, pid, y, x, 1, r, c)),
            }
        }
    }
    // Stack runs with the same columns on consecutive rows into rectangles.
    let mut regions: Vec<Region> = Vec::new();
    for (hid, pid, y, x, len, r, c) in runs {
        let (cols, rows) = ((pid >> 9) as u16, (pid & 511) as u16);
        let below = regions.iter_mut().rev().find(|g| {
            g.host_id == hid
                && g.screen.x == x
                && g.screen.width == len
                && g.col == c
                && g.screen.bottom() == y
                && g.row + g.screen.height == r
                && (g.cols, g.rows) == (cols, rows)
        });
        match below {
            Some(g) => g.screen.height += 1,
            None => regions.push(Region { host_id: hid, cols, rows, screen: Rect::new(x, y, len, 1), col: c, row: r }),
        }
    }
    regions
}

/// Decode an image to RGBA pixels.
pub fn decode(img: &ImageData) -> Option<image::RgbaImage> {
    let raw = if img.compressed {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(img.data.as_slice()).take(256 * 1024 * 1024).read_to_end(&mut out).ok()?;
        out
    } else {
        img.data.to_vec()
    };
    match img.format {
        Format::Png => Some(image::load_from_memory(&raw).ok()?.to_rgba8()),
        Format::Rgba => image::RgbaImage::from_raw(
            img.width,
            img.height,
            raw.get(..img.width as usize * img.height as usize * 4)?.to_vec(),
        ),
        Format::Rgb => {
            let rgb = image::RgbImage::from_raw(
                img.width,
                img.height,
                raw.get(..img.width as usize * img.height as usize * 3)?.to_vec(),
            )?;
            Some(image::DynamicImage::ImageRgb8(rgb).to_rgba8())
        }
    }
}

/// An image at a placement size: (host id, generation, cols, rows, cell size).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ScaledKey {
    host_id: u32,
    generation: u64,
    cols: u16,
    rows: u16,
    cell: (u16, u16),
}

/// A cropped piece of a scaled image encoded for one protocol (its screen position aside).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PieceKey {
    mode: ImageMode,
    image: ScaledKey,
    col: u16,
    row: u16,
    width: u16,
    height: u16,
}

/// Decoded images scaled to their placement size, and encoded pieces of them, kept between
/// frames. Moving a window reuses the pieces.
#[derive(Default)]
pub struct ScaledCache {
    scaled: HashMap<ScaledKey, Arc<image::RgbaImage>>,
    pieces: HashMap<PieceKey, Arc<Vec<u8>>>,
}

impl ScaledCache {
    fn scaled(&mut self, key: ScaledKey, img: &ImageData) -> Option<Arc<image::RgbaImage>> {
        if let Some(s) = self.scaled.get(&key) {
            return Some(s.clone());
        }
        let decoded = decode(img)?;
        let (cw, ch) = (key.cell.0.max(1) as u32, key.cell.1.max(1) as u32);
        let (w, h) = (key.cols as u32 * cw, key.rows as u32 * ch);
        let scaled = Arc::new(image::imageops::resize(&decoded, w, h, image::imageops::FilterType::Triangle));
        self.scaled.insert(key, scaled.clone());
        Some(scaled)
    }

    /// The encoded image command for a region (without positioning), made once per crop.
    pub fn piece(&mut self, mode: ImageMode, img: &ImageData, r: &Region, cell: (u16, u16)) -> Option<Arc<Vec<u8>>> {
        let image = ScaledKey { host_id: r.host_id, generation: img.generation, cols: r.cols, rows: r.rows, cell };
        let key = PieceKey { mode, image, col: r.col, row: r.row, width: r.screen.width, height: r.screen.height };
        if let Some(p) = self.pieces.get(&key) {
            return Some(p.clone());
        }
        let scaled = self.scaled(image, img)?;
        let encoded = Arc::new(encode(mode, &crop(&scaled, r, cell), r)?);
        self.pieces.insert(key, encoded.clone());
        Some(encoded)
    }

    /// Forget images not shown any more.
    pub fn retain(&mut self, live: &[(u32, u64)]) {
        self.scaled.retain(|k, _| live.contains(&(k.host_id, k.generation)));
        self.pieces.retain(|k, _| live.contains(&(k.image.host_id, k.image.generation)));
    }
}

/// The part of a scaled image a region shows.
fn crop(scaled: &image::RgbaImage, r: &Region, cell: (u16, u16)) -> image::RgbaImage {
    let (cw, ch) = (cell.0.max(1) as u32, cell.1.max(1) as u32);
    let x = (r.col as u32 * cw).min(scaled.width());
    let y = (r.row as u32 * ch).min(scaled.height());
    let w = (r.screen.width as u32 * cw).min(scaled.width() - x);
    let h = (r.screen.height as u32 * ch).min(scaled.height() - y);
    image::imageops::crop_imm(scaled, x, y, w, h).to_image()
}

/// The image command showing `piece` in a region's cells.
fn encode(mode: ImageMode, piece: &image::RgbaImage, r: &Region) -> Option<Vec<u8>> {
    if piece.width() == 0 || piece.height() == 0 {
        return None;
    }
    match mode {
        ImageMode::Iterm => {
            let mut png = Vec::new();
            piece.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
            // Sized in cells, so the terminal's own pixel scaling cannot make it overflow.
            Some(
                format!(
                    "\x1b]1337;File=inline=1;size={};width={};height={};preserveAspectRatio=0;doNotMoveCursor=1:{b64}\x07",
                    png.len(),
                    r.screen.width,
                    r.screen.height
                )
                .into_bytes(),
            )
        }
        ImageMode::Sixel => Some(sixel_encode(piece)),
        _ => None,
    }
}

/// Draw an encoded piece at its region's screen position, leaving the cursor where it was.
pub fn place(r: &Region, piece: &[u8]) -> Vec<u8> {
    let mut out = format!("\x1b7\x1b[{};{}H", r.screen.y + 1, r.screen.x + 1).into_bytes();
    out.extend_from_slice(piece);
    out.extend_from_slice(b"\x1b8");
    out
}

/// Encode RGBA pixels as Sixel with up to 255 colors. Transparent pixels are left unpainted.
pub fn sixel_encode(img: &image::RgbaImage) -> Vec<u8> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px = img.as_raw();
    // Sampling every pixel gives a noticeably better palette for gradients; larger images
    // sample a third of them to keep the first draw quick (pieces are cached afterwards).
    let sample = if w * h <= 120_000 { 1 } else { 3 };
    let quant = color_quant::NeuQuant::new(sample, 255, px);
    let palette = quant.color_map_rgb();
    let idx: Vec<u8> =
        px.as_chunks::<4>().0.iter().map(|p| if p[3] < 128 { 255 } else { quant.index_of(p) as u8 }).collect();

    let mut out = format!("\x1bP0;1;0q\"1;1;{w};{h}").into_bytes();
    let pct = |v: u8| (v as u32 * 100 + 127) / 255;
    for (i, rgb) in palette.as_chunks::<3>().0.iter().enumerate() {
        out.extend_from_slice(format!("#{i};2;{};{};{}", pct(rgb[0]), pct(rgb[1]), pct(rgb[2])).as_bytes());
    }
    let mut used = [false; 255];
    for band in (0..h).step_by(6) {
        used.iter_mut().for_each(|u| *u = false);
        for y in band..(band + 6).min(h) {
            for &c in &idx[y * w..(y + 1) * w] {
                if c != 255 {
                    used[c as usize] = true;
                }
            }
        }
        let mut first = true;
        for color in (0..255u8).filter(|c| used[*c as usize]) {
            if !first {
                out.push(b'$');
            }
            first = false;
            out.extend_from_slice(format!("#{color}").as_bytes());
            // Run-length encode the column bit patterns for this color.
            let mut run: Option<(u8, usize)> = None;
            let flush = |out: &mut Vec<u8>, run: Option<(u8, usize)>| {
                if let Some((ch, n)) = run {
                    if n > 3 {
                        out.extend_from_slice(format!("!{n}").as_bytes());
                        out.push(ch);
                    } else {
                        out.extend(std::iter::repeat_n(ch, n));
                    }
                }
            };
            for x in 0..w {
                let mut bits = 0u8;
                for dy in 0..6 {
                    let y = band + dy;
                    if y < h && idx[y * w + x] == color {
                        bits |= 1 << dy;
                    }
                }
                let ch = b'?' + bits;
                run = match run {
                    Some((c, n)) if c == ch => Some((c, n + 1)),
                    other => {
                        flush(&mut out, other);
                        Some((ch, 1))
                    }
                };
            }
            flush(&mut out, run);
        }
        out.push(b'-');
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

/// Placeholder text for (row, col), for tests that build buffers by hand.
#[cfg(test)]
fn cell_symbol(row: usize, col: usize) -> String {
    [PLACEHOLDER, DIACRITICS[row], DIACRITICS[col]].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn put_image(buf: &mut Buffer, x0: u16, y0: u16, cols: u16, rows: u16, hid: u32) {
        let pid = (cols as u32) << 9 | rows as u32;
        let rgb = |v: u32| Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        for r in 0..rows {
            for c in 0..cols {
                buf[(x0 + c, y0 + r)]
                    .set_symbol(&cell_symbol(r as usize, c as usize))
                    .set_style(Style::new().fg(rgb(hid)).underline_color(rgb(pid)));
            }
        }
    }

    #[test]
    fn visible_cells_become_rectangles() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        put_image(&mut buf, 2, 1, 6, 4, 7);
        // A window on top covers the image's right half on rows 2-3.
        for y in 2..4 {
            for x in 5..8 {
                buf[(x, y)].set_symbol("w");
            }
        }
        let area = buf.area;
        let regions = take_regions(&mut buf, area);
        assert_eq!(
            regions,
            vec![
                Region { host_id: 7, cols: 6, rows: 4, screen: Rect::new(2, 1, 6, 1), col: 0, row: 0 },
                Region { host_id: 7, cols: 6, rows: 4, screen: Rect::new(2, 2, 3, 2), col: 0, row: 1 },
                Region { host_id: 7, cols: 6, rows: 4, screen: Rect::new(2, 4, 6, 1), col: 0, row: 3 },
            ]
        );
        // Image cells are blanked and skipped by ratatui; covering cells are untouched.
        assert!(buf[(2, 1)].diff_option == CellDiffOption::Skip && buf[(2, 1)].symbol() == " ");
        assert!(buf[(5, 2)].diff_option == CellDiffOption::None && buf[(5, 2)].symbol() == "w");
    }

    #[test]
    fn sixel_round_trips_through_the_decoder() {
        let img = image::RgbaImage::from_fn(12, 13, |x, _| {
            if x < 6 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) }
        });
        let six = sixel_encode(&img);
        assert!(six.starts_with(b"\x1bP0;1;0q\"1;1;12;13") && six.ends_with(b"\x1b\\"));
        let body = &six[b"\x1bP0;1;0q".len()..six.len() - 2];
        let back = super::super::sixel_image(body).unwrap();
        assert_eq!((back.width, back.height), (12, 13));
        let px = &back.data;
        assert!(px[0] > 200 && px[2] < 60, "left is red: {:?}", &px[..4]);
        let right = (6 * 4) as usize;
        assert!(px[right + 2] > 200 && px[right] < 60, "right is blue");
    }

    #[test]
    fn iterm_output_is_sized_in_cells_and_keeps_the_cursor() {
        let scaled = image::RgbaImage::from_pixel(40, 40, image::Rgba([1, 2, 3, 255]));
        let r = Region { host_id: 1, cols: 4, rows: 2, screen: Rect::new(5, 3, 2, 1), col: 2, row: 1 };
        let piece = encode(ImageMode::Iterm, &crop(&scaled, &r, (10, 20)), &r).unwrap();
        let out = String::from_utf8(place(&r, &piece)).unwrap();
        assert!(out.starts_with("\x1b7\x1b[4;6H\x1b]1337;File=inline=1;"), "{out}");
        assert!(out.contains("width=2;height=1;preserveAspectRatio=0;doNotMoveCursor=1:"));
        assert!(out.ends_with("\x07\x1b8"));
    }
}

#[cfg(test)]
mod quality {
    /// Sixel keeps gradients reasonably close (255 colors): mean error per channel below 7.
    #[test]
    #[cfg_attr(
        miri,
        ignore = "full-size palette quantization is too slow under Miri; sixel_round_trips covers the code"
    )]
    fn sixel_gradient_quality() {
        let img = image::RgbaImage::from_fn(240, 150, |x, y| {
            let d = (((x as f32 - 120.0).powi(2) + (y as f32 - 75.0).powi(2)).sqrt() / 6.0).sin();
            image::Rgba([(127.0 + 127.0 * d) as u8, (x * 255 / 240) as u8, (y * 255 / 150) as u8, 255])
        });
        let six = super::sixel_encode(&img);
        let back = crate::graphics::sixel_image(&six[b"\x1bP0;1;0q".len()..six.len() - 2]).unwrap();
        let err: f64 =
            img.as_raw().iter().zip(back.data.iter()).map(|(a, b)| (*a as f64 - *b as f64).abs()).sum::<f64>()
                / img.as_raw().len() as f64;
        assert!(err < 7.0, "mean error {err:.2}");
    }
}

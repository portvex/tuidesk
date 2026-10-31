//! Images inside windows.
//!
//! Programs send images with the kitty graphics protocol, Sixel or iTerm2 inline images. They
//! are kept per window and shown on the host terminal with kitty Unicode placeholders: the host
//! receives each image once, and the cells where an image appears are drawn as U+10EEEE with
//! row/column diacritics and the image id in the foreground color. Because placeholders are
//! ordinary cells, overlapping windows, clipping and scrolling work like they do for text.

mod diacritics;
pub mod direct;
pub mod icons;
mod sixel;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;

pub use diacritics::DIACRITICS;

/// The kitty placeholder character.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Largest image (encoded) accepted from a program.
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// Images kept per window before the oldest unused ones are dropped.
const MAX_STORE_BYTES: usize = 192 * 1024 * 1024;
/// Largest escape sequence collected (images come base64 encoded inside them).
const MAX_SEQUENCE: usize = 96 * 1024 * 1024;

pub fn diacritic_index(c: char) -> Option<usize> {
    DIACRITICS.binary_search(&c).ok()
}

// ------------------------------------------------------------------ stream splitting

/// A piece of program output.
#[derive(Debug, PartialEq)]
pub enum Piece {
    /// Regular terminal output, for the emulator.
    Text(Vec<u8>),
    /// Body of a kitty graphics command (after "ESC _ G").
    Kitty(Vec<u8>),
    /// Sixel image data (after the 'q').
    Sixel(Vec<u8>),
    /// iTerm2 inline image (after "ESC ] 1337;File=").
    Iterm(Vec<u8>),
    /// CSI 16 t: report the cell size in pixels.
    CellSizeQuery,
    /// ED 2 or 3: the screen was cleared.
    Clear,
    /// RIS: full reset.
    Reset,
}

#[derive(Default)]
enum State {
    #[default]
    Ground,
    Esc,
    Csi,
    Apc,
    ApcEsc,
    Dcs,
    DcsEsc,
    OscHead,
    OscPass,
    OscPassEsc,
    Iterm,
    ItermEsc,
}

/// Splits program output into text for the emulator and image commands, keeping their order.
/// State carries over between reads, since sequences can be split anywhere.
#[derive(Default)]
pub struct Splitter {
    state: State,
    buf: Vec<u8>,
}

const ITERM_HEAD: &[u8] = b"1337;File=";

impl Splitter {
    pub fn split(&mut self, bytes: &[u8]) -> Vec<Piece> {
        let mut out = Vec::new();
        let mut text: Vec<u8> = Vec::with_capacity(bytes.len());
        let flush = |text: &mut Vec<u8>, out: &mut Vec<Piece>| {
            if !text.is_empty() {
                out.push(Piece::Text(std::mem::take(text)));
            }
        };
        for &b in bytes {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    } else {
                        text.push(b);
                    }
                }
                State::Esc => match b {
                    b'[' => {
                        text.extend_from_slice(b"\x1b[");
                        self.buf.clear();
                        self.state = State::Csi;
                    }
                    b'_' => {
                        self.buf.clear();
                        self.state = State::Apc;
                    }
                    b'P' => {
                        self.buf.clear();
                        self.state = State::Dcs;
                    }
                    b']' => {
                        self.buf.clear();
                        self.state = State::OscHead;
                    }
                    b'c' => {
                        text.extend_from_slice(b"\x1bc");
                        flush(&mut text, &mut out);
                        out.push(Piece::Reset);
                        self.state = State::Ground;
                    }
                    0x1b => text.push(0x1b),
                    _ => {
                        text.push(0x1b);
                        text.push(b);
                        self.state = State::Ground;
                    }
                },
                State::Csi => match b {
                    0x1b => self.state = State::Esc,
                    0x18 | 0x1a => {
                        text.push(b);
                        self.state = State::Ground;
                    }
                    0x40..=0x7e => {
                        text.push(b);
                        let params = std::mem::take(&mut self.buf);
                        if b == b'J' && (params == b"2" || params == b"3") {
                            flush(&mut text, &mut out);
                            out.push(Piece::Clear);
                        } else if b == b't' && params == b"16" {
                            flush(&mut text, &mut out);
                            out.push(Piece::CellSizeQuery);
                        }
                        self.state = State::Ground;
                    }
                    _ => {
                        text.push(b);
                        if self.buf.len() < 32 {
                            self.buf.push(b);
                        }
                    }
                },
                State::Apc | State::Dcs | State::Iterm => {
                    if b == 0x1b {
                        self.state = match self.state {
                            State::Apc => State::ApcEsc,
                            State::Dcs => State::DcsEsc,
                            _ => State::ItermEsc,
                        };
                    } else if b == 0x07 && !matches!(self.state, State::Dcs) {
                        let state = std::mem::take(&mut self.state);
                        self.finish(state, &mut text, &mut out);
                    } else if self.buf.len() < MAX_SEQUENCE {
                        self.buf.push(b);
                    } else {
                        // Too large: drop it.
                        self.buf = Vec::new();
                        self.state = State::Ground;
                    }
                }
                State::ApcEsc | State::DcsEsc | State::ItermEsc => {
                    let body = match self.state {
                        State::ApcEsc => State::Apc,
                        State::DcsEsc => State::Dcs,
                        _ => State::Iterm,
                    };
                    if b == b'\\' {
                        self.finish(body, &mut text, &mut out);
                    } else {
                        // ESC inside a string aborts it; the new sequence starts here.
                        self.buf.clear();
                        self.state = State::Esc;
                        // Re-run this byte as the character after ESC.
                        let mut rest = self.split(&[b]);
                        flush(&mut text, &mut out);
                        out.append(&mut rest);
                        continue;
                    }
                }
                State::OscHead => {
                    if b == 0x07 || b == 0x1b {
                        text.extend_from_slice(b"\x1b]");
                        text.extend_from_slice(&self.buf);
                        text.push(b);
                        self.buf.clear();
                        self.state = if b == 0x07 { State::Ground } else { State::OscPassEsc };
                        continue;
                    }
                    self.buf.push(b);
                    if !ITERM_HEAD.starts_with(&self.buf) {
                        text.extend_from_slice(b"\x1b]");
                        text.extend_from_slice(&self.buf);
                        self.buf.clear();
                        self.state = State::OscPass;
                    } else if self.buf.len() == ITERM_HEAD.len() {
                        self.buf.clear();
                        self.state = State::Iterm;
                    }
                }
                State::OscPass => {
                    text.push(b);
                    if b == 0x07 {
                        self.state = State::Ground;
                    } else if b == 0x1b {
                        self.state = State::OscPassEsc;
                    }
                }
                State::OscPassEsc => {
                    text.push(b);
                    self.state = State::Ground;
                }
            }
        }
        flush(&mut text, &mut out);
        out
    }

    fn finish(&mut self, body: State, text: &mut Vec<u8>, out: &mut Vec<Piece>) {
        let buf = std::mem::take(&mut self.buf);
        self.state = State::Ground;
        if !text.is_empty() {
            out.push(Piece::Text(std::mem::take(text)));
        }
        match body {
            State::Apc => {
                if let Some(rest) = buf.strip_prefix(b"G") {
                    out.push(Piece::Kitty(rest.to_vec()));
                }
            }
            State::Dcs => {
                // Sixel: optional numeric parameters, then 'q'.
                let params = buf.iter().take_while(|c| c.is_ascii_digit() || **c == b';').count();
                if buf.get(params) == Some(&b'q') {
                    out.push(Piece::Sixel(buf[params + 1..].to_vec()));
                } else {
                    let mut seq = b"\x1bP".to_vec();
                    seq.extend_from_slice(&buf);
                    seq.extend_from_slice(b"\x1b\\");
                    out.push(Piece::Text(seq));
                }
            }
            _ => out.push(Piece::Iterm(buf)),
        }
    }
}

// ------------------------------------------------------------------ images and placements

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Rgb,
    Rgba,
    Png,
}

impl Format {
    /// The kitty `f=` value.
    pub fn code(self) -> u32 {
        match self {
            Self::Rgb => 24,
            Self::Rgba => 32,
            Self::Png => 100,
        }
    }
}

/// An image as the host terminal will receive it.
#[derive(Debug, Clone)]
pub struct ImageData {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    /// zlib-compressed raw pixels (kitty `o=z`).
    pub compressed: bool,
    pub data: Arc<Vec<u8>>,
    /// Identifies this exact picture: changes when the image gets new data or an animation
    /// shows another frame, so the host copy can be refreshed.
    pub generation: u64,
    /// Frames, when the image is animated.
    pub animation: Option<Animation>,
}

impl ImageData {
    pub fn new(format: Format, width: u32, height: u32, compressed: bool, data: Vec<u8>) -> Self {
        Self { format, width, height, compressed, data: Arc::new(data), generation: 0, animation: None }
    }

    /// Memory held, including animation frames.
    pub fn bytes(&self) -> usize {
        self.data.len() + self.animation.as_ref().map_or(0, |a| a.frames.iter().map(|f| f.rgba.len()).sum())
    }
}

/// Frames of an animated image (kitty animation protocol, animated GIFs), as full RGBA canvases.
#[derive(Debug, Clone)]
pub struct Animation {
    pub frames: Vec<Frame>,
    /// Index of the frame shown.
    pub current: usize,
    pub running: bool,
    /// Loops left after the current one; None plays forever.
    pub loops_left: Option<u32>,
    /// When to show the next frame.
    pub next: Option<Instant>,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub rgba: Arc<Vec<u8>>,
    /// How long the frame is shown, in milliseconds; negative frames are skipped (gapless).
    pub gap_ms: i32,
    pub generation: u64,
}

/// Most frames kept per animation.
const MAX_FRAMES: usize = 1000;
/// Frame time when none is given (kitty's default).
const DEFAULT_GAP_MS: i32 = 40;

/// Alpha-blend (or copy, with `overwrite`) an RGBA patch onto an RGBA canvas at (x, y).
#[allow(clippy::too_many_arguments)]
fn composite(canvas: &mut [u8], cw: u32, ch: u32, patch: &[u8], pw: u32, ph: u32, x: u32, y: u32, overwrite: bool) {
    for py in 0..ph.min(ch.saturating_sub(y)) {
        for px in 0..pw.min(cw.saturating_sub(x)) {
            let s = ((py * pw + px) * 4) as usize;
            let d = (((y + py) * cw + x + px) * 4) as usize;
            let (Some(src), true) = (patch.get(s..s + 4), d + 4 <= canvas.len()) else { continue };
            if overwrite || src[3] == 255 {
                canvas[d..d + 4].copy_from_slice(src);
            } else if src[3] > 0 {
                let a = src[3] as u32;
                for c in 0..3 {
                    canvas[d + c] = ((src[c] as u32 * a + canvas[d + c] as u32 * (255 - a)) / 255) as u8;
                }
                canvas[d + 3] = (a + canvas[d + 3] as u32 * (255 - a) / 255) as u8;
            }
        }
    }
}

/// Where a placement's top-left cell is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// A row of the alternate screen (which never scrolls).
    Row(i32),
    /// The main screen: a marker hidden in the anchor cell, so the image scrolls with text.
    Marker(u64),
}

/// An image shown at a position (by the kitty protocol with a cursor position, Sixel or iTerm2).
#[derive(Debug, Clone)]
pub struct Placement {
    pub image: u32,
    pub id: u32,
    pub anchor: Anchor,
    pub col: u16,
    pub cols: u16,
    pub rows: u16,
    pub z: i32,
    pub alt: bool,
}

/// What a program asked of the window after a graphics command.
#[derive(Debug, Default, PartialEq)]
pub struct Effect {
    /// Bytes to send back to the program.
    pub reply: Vec<u8>,
    /// Place an image at the cursor with this size in cells.
    pub place: Option<(u32, u32, u16, u16, i32, bool)>, // (image, placement id, cols, rows, z, move cursor)
    /// Delete placements (selector), possibly freeing images.
    pub delete: Option<Delete>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delete {
    All { free: bool },
    Image { id: u32, placement: u32, free: bool },
    AtCursor { free: bool },
    AtCell { x: u16, y: u16, free: bool },
    Column { x: u16, free: bool },
    Row { y: u16, free: bool },
    Z { z: i32, free: bool },
}

/// Kitty command keys.
#[derive(Debug, Default, Clone)]
struct Cmd {
    action: u8,
    format: u32,
    medium: u8,
    compressed: bool,
    width: u32,
    height: u32,
    size: usize,
    offset: usize,
    id: u32,
    number: u32,
    placement: u32,
    cols: u32,
    rows: u32,
    no_cursor_move: bool,
    virtual_placement: bool,
    quiet: u32,
    more: bool,
    delete: u8,
    x: u32,
    y: u32,
    z: i32,
    /// X=1: frames overwrite instead of alpha blending.
    overwrite: bool,
    /// Y: background color (RGBA) of new frames.
    background: u32,
}

fn parse_cmd(keys: &[u8]) -> Cmd {
    let mut c = Cmd { action: b't', format: 32, medium: b'd', delete: b'a', ..Default::default() };
    for kv in keys.split(|b| *b == b',') {
        let Some(eq) = kv.iter().position(|b| *b == b'=') else { continue };
        let (k, v) = (&kv[..eq], &kv[eq + 1..]);
        let n = || std::str::from_utf8(v).ok().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        let u = || n().clamp(0, u32::MAX as i64) as u32;
        match k {
            b"a" => c.action = v.first().copied().unwrap_or(b't'),
            b"f" => c.format = u(),
            b"t" => c.medium = v.first().copied().unwrap_or(b'd'),
            b"o" => c.compressed = v == b"z",
            b"s" => c.width = u(),
            b"v" => c.height = u(),
            b"S" => c.size = u() as usize,
            b"O" => c.offset = u() as usize,
            b"i" => c.id = u(),
            b"I" => c.number = u(),
            b"p" => c.placement = u(),
            b"c" => c.cols = u(),
            b"r" => c.rows = u(),
            b"C" => c.no_cursor_move = n() == 1,
            b"U" => c.virtual_placement = n() == 1,
            b"q" => c.quiet = u(),
            b"m" => c.more = n() == 1,
            b"d" => c.delete = v.first().copied().unwrap_or(b'a'),
            b"x" => c.x = u(),
            b"y" => c.y = u(),
            b"z" => c.z = n().clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            b"X" => c.overwrite = n() == 1,
            b"Y" => c.background = u(),
            _ => {}
        }
    }
    c
}

/// Size of an image in cells: given columns/rows, or derived from its pixels and the cell size.
pub fn cells_for(width: u32, height: u32, cols: u32, rows: u32, cell: (u16, u16)) -> (u16, u16) {
    let (cw, ch) = (cell.0.max(1) as f64, cell.1.max(1) as f64);
    let (w, h) = (width.max(1) as f64, height.max(1) as f64);
    let (c, r) = match (cols, rows) {
        (0, 0) => ((w / cw).ceil(), (h / ch).ceil()),
        (c, 0) => (c as f64, (h * (c as f64 * cw / w) / ch).ceil()),
        (0, r) => ((w * (r as f64 * ch / h) / cw).ceil(), r as f64),
        (c, r) => (c as f64, r as f64),
    };
    // Placeholders can address 297 rows and columns.
    (c.clamp(1.0, 297.0) as u16, r.clamp(1.0, 297.0) as u16)
}

/// Images and placements of one window.
#[derive(Default)]
pub struct Graphics {
    pub images: HashMap<u32, ImageData>,
    /// Virtual placements (kitty `U=1`): (image, placement id) to size in cells. The program
    /// draws the placeholder cells itself.
    pub virtuals: HashMap<(u32, u32), (u16, u16)>,
    pub placements: Vec<Placement>,
    /// Image numbers (`I=`) to the id they were given.
    numbers: HashMap<u32, u32>,
    /// Insertion order of images, oldest first, for eviction.
    order: Vec<u32>,
    /// Images without a program-given id (Sixel, iTerm2), which a redraw at the same spot replaces.
    anonymous: std::collections::HashSet<u32>,
    next_auto_id: u32,
    next_marker: u64,
    generation: u64,
    /// A chunked transmission in progress: its first command and the payload so far.
    pending: Option<(Cmd, Vec<u8>)>,
    /// Changes since the host copy was last synced.
    pub changed: bool,
}

impl Graphics {
    pub fn new_marker(&mut self) -> u64 {
        self.next_marker += 1;
        self.next_marker
    }

    fn new_id(&mut self) -> u32 {
        // Ids kitty programs pick are usually small or random; count down from the top.
        self.next_auto_id = if self.next_auto_id <= 1 { u32::MAX - 1 } else { self.next_auto_id - 1 };
        while self.images.contains_key(&self.next_auto_id) {
            self.next_auto_id -= 1;
        }
        self.next_auto_id
    }

    pub fn add_image(&mut self, id: u32, mut image: ImageData) {
        self.generation += 1;
        image.generation = self.generation;
        if let Some(anim) = &mut image.animation {
            for frame in &mut anim.frames {
                self.generation += 1;
                frame.generation = self.generation;
            }
            if let Some(f) = anim.frames.get(anim.current) {
                image.generation = f.generation;
            }
        }
        self.order.retain(|&o| o != id);
        self.order.push(id);
        self.images.insert(id, image);
        self.changed = true;
        self.evict();
    }

    /// Add an image under a fresh id (Sixel, iTerm2) and return the id.
    pub fn add_anonymous(&mut self, image: ImageData) -> u32 {
        let id = self.new_id();
        self.add_image(id, image);
        self.anonymous.insert(id);
        id
    }

    /// Drop anonymous images whose placements match `same` (a program redrew at that spot, as
    /// Sixel and iTerm2 animations do), so frames do not pile up.
    pub fn replace_anonymous(&mut self, mut same: impl FnMut(&Placement) -> bool) {
        let old: Vec<u32> =
            self.placements.iter().filter(|p| self.anonymous.contains(&p.image) && same(p)).map(|p| p.image).collect();
        for id in old {
            self.remove_image(id);
        }
    }

    fn evict(&mut self) {
        let mut total: usize = self.images.values().map(ImageData::bytes).sum();
        let mut i = 0;
        while total > MAX_STORE_BYTES && i < self.order.len() {
            let id = self.order[i];
            let used = self.placements.iter().any(|p| p.image == id) || self.virtuals.keys().any(|(img, _)| *img == id);
            if used && total <= MAX_STORE_BYTES * 2 {
                i += 1;
                continue;
            }
            if let Some(img) = self.images.remove(&id) {
                total -= img.bytes();
            }
            self.placements.retain(|p| p.image != id);
            self.virtuals.retain(|(img, _), _| *img != id);
            self.order.remove(i);
        }
    }

    pub fn remove_image(&mut self, id: u32) {
        self.images.remove(&id);
        self.anonymous.remove(&id);
        self.order.retain(|&o| o != id);
        self.placements.retain(|p| p.image != id);
        self.virtuals.retain(|(img, _), _| *img != id);
        self.changed = true;
    }

    pub fn is_anonymous(&self, id: u32) -> bool {
        self.anonymous.contains(&id)
    }

    /// Drop images that nothing shows any more (after an uppercase delete).
    pub fn free_unused(&mut self) {
        let unused: Vec<u32> = self
            .images
            .keys()
            .copied()
            .filter(|id| {
                !self.placements.iter().any(|p| p.image == *id) && !self.virtuals.keys().any(|(img, _)| img == id)
            })
            .collect();
        for id in unused {
            self.remove_image(id);
        }
    }

    pub fn clear(&mut self) {
        *self =
            Self { next_marker: self.next_marker, generation: self.generation, changed: true, ..Default::default() };
    }

    /// Handle one kitty graphics command. `cell` is the cell size in pixels. `enabled` says
    /// whether images can be shown at all; when not, commands are ignored silently, like a
    /// terminal without graphics, so programs fall back to text.
    pub fn kitty(&mut self, body: &[u8], cell: (u16, u16), enabled: bool) -> Effect {
        let mut effect = Effect::default();
        if !enabled {
            return effect;
        }
        let (keys, payload) = match body.iter().position(|b| *b == b';') {
            Some(p) => (&body[..p], &body[p + 1..]),
            None => (body, &b""[..]),
        };
        // Continuation chunks carry only m= (and maybe q=); keys come from the first chunk.
        let (cmd, data) = match self.pending.take() {
            Some((first, mut data)) => {
                let cont = parse_cmd(keys);
                if data.len() + payload.len() > MAX_IMAGE_BYTES * 2 {
                    return effect;
                }
                data.extend_from_slice(payload);
                if cont.more {
                    self.pending = Some((first, data));
                    return effect;
                }
                (first, data)
            }
            None => {
                let cmd = parse_cmd(keys);
                if cmd.more && matches!(cmd.action, b't' | b'T' | b'q' | b'f') {
                    self.pending = Some((cmd, payload.to_vec()));
                    return effect;
                }
                (cmd, payload.to_vec())
            }
        };

        let id_from_number = cmd.number != 0 && cmd.id == 0;
        let reply = |cmd: &Cmd, id: u32, msg: &str| -> Vec<u8> {
            let quiet = (msg == "OK" && cmd.quiet >= 1) || cmd.quiet >= 2;
            if quiet || (cmd.id == 0 && cmd.number == 0) {
                return Vec::new();
            }
            let mut r = format!("\x1b_Gi={id}");
            if cmd.number != 0 {
                r.push_str(&format!(",I={}", cmd.number));
            }
            if cmd.placement != 0 {
                r.push_str(&format!(",p={}", cmd.placement));
            }
            r.push_str(&format!(";{msg}\x1b\\"));
            r.into_bytes()
        };

        match cmd.action {
            b'q' => {
                let msg = match load(&cmd, &data) {
                    Ok(_) => "OK".to_string(),
                    Err(e) => e,
                };
                effect.reply = reply(&cmd, cmd.id, &msg);
            }
            b't' | b'T' => {
                let image = match load(&cmd, &data) {
                    Ok(i) => i,
                    Err(e) => {
                        effect.reply = reply(&cmd, cmd.id, &e);
                        return effect;
                    }
                };
                let id = if id_from_number || cmd.id == 0 { self.new_id() } else { cmd.id };
                if cmd.number != 0 {
                    self.numbers.insert(cmd.number, id);
                }
                let (w, h) = (image.width, image.height);
                self.add_image(id, image);
                if cmd.action == b'T' {
                    let (cols, rows) = cells_for(w, h, cmd.cols, cmd.rows, cell);
                    if cmd.virtual_placement {
                        self.virtuals.insert((id, cmd.placement), (cols, rows));
                    } else {
                        effect.place = Some((id, cmd.placement, cols, rows, cmd.z, !cmd.no_cursor_move));
                    }
                }
                effect.reply = reply(&cmd, id, "OK");
            }
            b'p' => {
                let id = if cmd.id != 0 { cmd.id } else { self.numbers.get(&cmd.number).copied().unwrap_or(0) };
                let Some(img) = self.images.get(&id) else {
                    effect.reply = reply(&cmd, id, "ENOENT:image not found");
                    return effect;
                };
                let (cols, rows) = cells_for(img.width, img.height, cmd.cols, cmd.rows, cell);
                if cmd.virtual_placement {
                    self.virtuals.insert((id, cmd.placement), (cols, rows));
                    self.changed = true;
                } else {
                    effect.place = Some((id, cmd.placement, cols, rows, cmd.z, !cmd.no_cursor_move));
                }
                effect.reply = reply(&cmd, id, "OK");
            }
            b'f' => {
                let id = if cmd.id != 0 { cmd.id } else { self.numbers.get(&cmd.number).copied().unwrap_or(0) };
                effect.reply = match self.add_frame(id, &cmd, &data) {
                    Ok(()) => reply(&cmd, id, "OK"),
                    Err(e) => reply(&cmd, id, &e),
                };
            }
            b'a' => {
                let id = if cmd.id != 0 { cmd.id } else { self.numbers.get(&cmd.number).copied().unwrap_or(0) };
                effect.reply = match self.control_animation(id, &cmd) {
                    Ok(()) => reply(&cmd, id, "OK"),
                    Err(e) => reply(&cmd, id, &e),
                };
            }
            b'd' => {
                let free = cmd.delete.is_ascii_uppercase();
                let id = if cmd.id != 0 { cmd.id } else { self.numbers.get(&cmd.number).copied().unwrap_or(0) };
                let (x, y) = (cmd.x.saturating_sub(1) as u16, cmd.y.saturating_sub(1) as u16);
                effect.delete = Some(match cmd.delete.to_ascii_lowercase() {
                    b'i' | b'n' => Delete::Image { id, placement: cmd.placement, free },
                    b'c' => Delete::AtCursor { free },
                    b'p' => Delete::AtCell { x, y, free },
                    b'x' => Delete::Column { x, free },
                    b'y' => Delete::Row { y, free },
                    b'z' => Delete::Z { z: cmd.z, free },
                    _ => Delete::All { free },
                });
            }
            _ => {}
        }
        effect
    }

    /// Turn an image into an animation (if it is not one yet), with itself as the first frame.
    fn animation_of(&mut self, id: u32) -> Result<&mut ImageData, String> {
        let img = self.images.get_mut(&id).ok_or("ENOENT:image not found")?;
        if img.animation.is_none() {
            let first = direct::decode(img).ok_or("EBADPNG:cannot decode image")?.into_raw();
            img.animation = Some(Animation {
                frames: vec![Frame { rgba: Arc::new(first), gap_ms: DEFAULT_GAP_MS, generation: img.generation }],
                current: 0,
                running: false,
                loops_left: None,
                next: None,
            });
        }
        Ok(img)
    }

    /// kitty `a=f`: add a frame (or edit frame `r`), drawn onto base frame `c` or a background
    /// color, at offset (x, y).
    fn add_frame(&mut self, id: u32, cmd: &Cmd, data: &[u8]) -> Result<(), String> {
        let patch_img = load(cmd, data)?;
        let patch = direct::decode(&patch_img).ok_or("EBADPNG:cannot decode frame")?;
        self.generation += 1;
        let generation = self.generation;
        let img = self.animation_of(id)?;
        let (w, h) = (img.width, img.height);
        let anim = img.animation.as_mut().expect("animation_of creates it");
        let (edit, base) = (cmd.rows as usize, cmd.cols as usize);
        let mut canvas: Vec<u8> = if edit > 0 && edit <= anim.frames.len() {
            anim.frames[edit - 1].rgba.to_vec()
        } else if base > 0 && base <= anim.frames.len() {
            anim.frames[base - 1].rgba.to_vec()
        } else {
            let bg = cmd.background.to_be_bytes();
            bg.iter().copied().cycle().take((w * h * 4) as usize).collect()
        };
        let (pw, ph) = patch.dimensions();
        composite(&mut canvas, w, h, patch.as_raw(), pw, ph, cmd.x, cmd.y, cmd.overwrite);
        let gap = if cmd.z != 0 { cmd.z } else { DEFAULT_GAP_MS };
        let frame = Frame { rgba: Arc::new(canvas), gap_ms: gap, generation };
        if edit > 0 && edit <= anim.frames.len() {
            anim.frames[edit - 1] = frame;
        } else if anim.frames.len() < MAX_FRAMES {
            anim.frames.push(frame);
        } else {
            return Err("EFBIG:too many frames".into());
        }
        self.changed = true;
        self.evict();
        Ok(())
    }

    /// kitty `a=a`: play (s=3, or s=2 while loading) or stop (s=1), show frame c, set the gap
    /// of frame r (z), set the number of loops (v: 1 forever, n plays n-1 times).
    fn control_animation(&mut self, id: u32, cmd: &Cmd) -> Result<(), String> {
        let img = self.animation_of(id)?;
        let anim = img.animation.as_mut().expect("animation_of creates it");
        match cmd.width {
            1 => anim.running = false,
            2 | 3 => {
                anim.running = true;
                anim.next = None;
            }
            _ => {}
        }
        match cmd.height {
            0 => {}
            1 => anim.loops_left = None,
            n => anim.loops_left = Some(n - 2),
        }
        if cmd.rows > 0
            && cmd.z != 0
            && let Some(f) = anim.frames.get_mut(cmd.rows as usize - 1)
        {
            f.gap_ms = cmd.z;
        }
        if cmd.cols > 0 && (cmd.cols as usize) <= anim.frames.len() {
            anim.current = cmd.cols as usize - 1;
            show_frame(img);
        }
        self.changed = true;
        Ok(())
    }

    /// When the next animation frame is due, if any image is playing.
    pub fn next_frame_at(&self) -> Option<Instant> {
        self.images
            .values()
            .filter_map(|i| i.animation.as_ref())
            .filter(|a| a.running && a.frames.len() > 1)
            .map(|a| a.next.unwrap_or_else(Instant::now))
            .min()
    }

    /// Advance animations whose frame time has passed. Returns true if a frame changed.
    pub fn animate(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for img in self.images.values_mut() {
            let Some(anim) = img.animation.as_mut().filter(|a| a.running && a.frames.len() > 1) else { continue };
            let gap = |a: &Animation| Duration::from_millis(a.frames[a.current].gap_ms.max(10) as u64);
            let Some(due) = anim.next else {
                anim.next = Some(now + gap(anim));
                continue;
            };
            if now < due {
                continue;
            }
            // Next frame, skipping gapless ones; at the end loop or stop.
            let n = anim.frames.len();
            let mut next = anim.current;
            for _ in 0..n {
                next += 1;
                if next == n {
                    match anim.loops_left {
                        None => next = 0,
                        Some(0) => {
                            anim.running = false;
                            next = n - 1;
                            break;
                        }
                        Some(left) => {
                            anim.loops_left = Some(left - 1);
                            next = 0;
                        }
                    }
                }
                if anim.frames[next].gap_ms >= 0 {
                    break;
                }
            }
            if next == anim.current {
                // Stopped on the frame already shown.
                continue;
            }
            anim.current = next;
            anim.next = Some(now + gap(anim));
            show_frame(img);
            changed = true;
        }
        if changed {
            self.changed = true;
        }
        changed
    }

    /// Delete placements selected by `pred`, then free images if asked.
    pub fn delete_where(&mut self, free: bool, mut pred: impl FnMut(&Placement) -> bool) {
        self.placements.retain(|p| !pred(p));
        if free {
            self.free_unused();
        }
        self.changed = true;
    }

    /// Apply an image-id delete (which also covers virtual placements).
    pub fn delete_image(&mut self, id: u32, placement: u32, free: bool) {
        self.placements.retain(|p| !(p.image == id && (placement == 0 || p.id == placement)));
        self.virtuals.retain(|(img, pid), _| !(*img == id && (placement == 0 || *pid == placement)));
        if free {
            self.free_unused();
        }
        self.changed = true;
    }
}

/// Make an animated image show its current frame.
fn show_frame(img: &mut ImageData) {
    let Some(anim) = &img.animation else { return };
    let Some(frame) = anim.frames.get(anim.current) else { return };
    img.data = frame.rgba.clone();
    img.format = Format::Rgba;
    img.compressed = false;
    img.generation = frame.generation;
}

/// PNG width and height from its header.
fn png_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 || &data[..8] != b"\x89PNG\r\n\x1a\n" || &data[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(data[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(data[20..24].try_into().ok()?);
    Some((w, h))
}

/// Read the image a kitty command transmits. Errors are kitty error strings.
fn load(cmd: &Cmd, payload: &[u8]) -> Result<ImageData, String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let payload: Vec<u8> = payload.iter().copied().filter(|c| !c.is_ascii_whitespace()).collect();
    let decoded = b64
        .decode(&payload)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&payload))
        .map_err(|_| "EINVAL:bad base64".to_string())?;
    let data = match cmd.medium {
        b'd' => decoded,
        b'f' | b't' => {
            let path = String::from_utf8(decoded).map_err(|_| "EINVAL:bad path".to_string())?;
            let mut bytes = std::fs::read(&path).map_err(|e| format!("EBADF:{e}"))?;
            // Temporary files are deleted after reading, but only ones made for this purpose.
            if cmd.medium == b't' && path.contains("tty-graphics-protocol") {
                let _ = std::fs::remove_file(&path);
            }
            if cmd.offset > 0 || cmd.size > 0 {
                let start = cmd.offset.min(bytes.len());
                let end = if cmd.size > 0 { (start + cmd.size).min(bytes.len()) } else { bytes.len() };
                bytes = bytes[start..end].to_vec();
            }
            bytes
        }
        b's' => {
            let name = String::from_utf8(decoded).map_err(|_| "EINVAL:bad name".to_string())?;
            read_shm(&name, cmd.offset, cmd.size)?
        }
        _ => return Err("EINVAL:unsupported transmission medium".into()),
    };
    if data.len() > MAX_IMAGE_BYTES {
        return Err("EFBIG:image too large".into());
    }
    let (format, width, height) = match cmd.format {
        100 => {
            let (w, h) = png_size(&data).ok_or("EBADPNG:not a PNG")?;
            (Format::Png, w, h)
        }
        24 | 32 => {
            let bpp = if cmd.format == 24 { 3 } else { 4 };
            if cmd.width == 0 || cmd.height == 0 {
                return Err("EINVAL:missing width or height".into());
            }
            if !cmd.compressed && data.len() < cmd.width as usize * cmd.height as usize * bpp {
                return Err("ENODATA:not enough pixel data".into());
            }
            (if bpp == 3 { Format::Rgb } else { Format::Rgba }, cmd.width, cmd.height)
        }
        _ => return Err("EINVAL:unknown format".into()),
    };
    Ok(ImageData::new(format, width, height, cmd.compressed, data))
}

#[cfg(windows)]
fn read_shm(_name: &str, _offset: usize, _size: usize) -> Result<Vec<u8>, String> {
    Err("EINVAL:shared memory is not supported on Windows".into())
}

/// Read (and unlink) a POSIX shared memory object, as the kitty protocol requires.
#[cfg(unix)]
fn read_shm(name: &str, offset: usize, size: usize) -> Result<Vec<u8>, String> {
    let cname = std::ffi::CString::new(name).map_err(|_| "EINVAL:bad name".to_string())?;
    // SAFETY: plain POSIX calls; the mapping is read within its length and unmapped.
    unsafe {
        let fd = libc::shm_open(cname.as_ptr(), libc::O_RDONLY, 0);
        if fd < 0 {
            return Err("EBADF:cannot open shared memory".into());
        }
        let mut st: libc::stat = std::mem::zeroed();
        let len = if libc::fstat(fd, &mut st) == 0 { st.st_size as usize } else { 0 };
        let want = if size > 0 { size.min(len.saturating_sub(offset)) } else { len.saturating_sub(offset) };
        let result = if want == 0 || want > MAX_IMAGE_BYTES {
            Err("EINVAL:bad shared memory size".to_string())
        } else {
            let map = libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd, 0);
            if map == libc::MAP_FAILED {
                Err("EBADF:cannot map shared memory".to_string())
            } else {
                let bytes = std::slice::from_raw_parts((map as *const u8).add(offset), want).to_vec();
                libc::munmap(map, len);
                Ok(bytes)
            }
        };
        libc::close(fd);
        libc::shm_unlink(cname.as_ptr());
        result
    }
}

// ------------------------------------------------------------------ the host terminal

/// Mirrors the windows' images on the host terminal. Each (window, image) gets a host image id
/// (24 bits, as placeholders carry it in the foreground color), is transmitted once, and gets a
/// virtual placement per size it is shown at.
#[derive(Default)]
pub struct HostGraphics {
    /// (window, image) to (host id, generation sent).
    ids: HashMap<(u64, u32), (u32, u64)>,
    /// Virtual placements created on the host: (host id, placement id).
    places: std::collections::HashSet<(u32, u32)>,
    next: u32,
}

impl HostGraphics {
    pub fn host_id(&self, window: u64, image: u32) -> Option<u32> {
        self.ids.get(&(window, image)).map(|(hid, _)| *hid)
    }

    /// Host placement id for a size: one virtual placement per (image, size).
    pub fn placement_id(cols: u16, rows: u16) -> u32 {
        ((cols as u32) << 9) | rows as u32
    }

    /// Commands deleting every image we sent; the host copy is then forgotten.
    pub fn free_all(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        for (hid, _) in self.ids.values() {
            out.extend_from_slice(format!("\x1b_Ga=d,d=I,i={hid},q=2\x1b\\").as_bytes());
        }
        self.reset();
        out
    }

    /// The host terminal changed (a new client attached): everything must be sent again.
    pub fn reset(&mut self) {
        self.ids.clear();
        self.places.clear();
    }

    fn alloc(&mut self) -> u32 {
        let used: std::collections::HashSet<u32> = self.ids.values().map(|(h, _)| *h).collect();
        loop {
            self.next = if self.next >= 0xFF_FFFF { 1 } else { self.next + 1 };
            if !used.contains(&self.next) {
                return self.next;
            }
        }
    }

    /// Which window image a host id belongs to.
    pub fn owner(&self, hid: u32) -> Option<(u64, u32)> {
        self.ids.iter().find(|(_, (h, _))| *h == hid).map(|(k, _)| *k)
    }

    /// Bring the host up to date with all windows' images, appending commands to `out`.
    /// With `transmit` false (images drawn directly by tuidesk), only ids are assigned.
    pub fn sync<'a>(
        &mut self,
        windows: impl Iterator<Item = (u64, &'a mut Graphics)>,
        out: &mut Vec<u8>,
        transmit: bool,
    ) {
        let mut live = std::collections::HashSet::new();
        for (win, gfx) in windows {
            for (&img, data) in &gfx.images {
                live.insert((win, img));
                let known = self.ids.get(&(win, img)).copied();
                let hid = match known {
                    Some((_, generation)) if generation == data.generation => continue,
                    Some((hid, _)) => hid,
                    None => self.alloc(),
                };
                if transmit {
                    send_image(hid, data, out);
                }
                // A new transmission replaces the image and its placements on the host.
                self.places.retain(|(h, _)| *h != hid);
                self.ids.insert((win, img), (hid, data.generation));
            }
            let sizes = gfx
                .virtuals
                .iter()
                .map(|(&(img, _), &size)| (img, size))
                .chain(gfx.placements.iter().map(|p| (p.image, (p.cols, p.rows))));
            for (img, (cols, rows)) in sizes {
                if let Some(hid) = self.host_id(win, img).filter(|_| transmit) {
                    let pid = Self::placement_id(cols, rows);
                    if self.places.insert((hid, pid)) {
                        out.extend_from_slice(
                            format!("\x1b_Ga=p,U=1,q=2,i={hid},p={pid},c={cols},r={rows}\x1b\\").as_bytes(),
                        );
                    }
                }
            }
            gfx.changed = false;
        }
        let gone: Vec<((u64, u32), u32)> =
            self.ids.iter().filter(|(k, _)| !live.contains(*k)).map(|(k, (hid, _))| (*k, *hid)).collect();
        for (key, hid) in gone {
            if transmit {
                out.extend_from_slice(format!("\x1b_Ga=d,d=I,i={hid},q=2\x1b\\").as_bytes());
            }
            self.ids.remove(&key);
            self.places.retain(|(h, _)| *h != hid);
        }
    }
}

/// Send an image to the host in base64 chunks (the kitty protocol's limit is 4096 per chunk).
fn send_image(hid: u32, img: &ImageData, out: &mut Vec<u8>) {
    // Raw pixels (yazi's images, animation frames) compress well; PNG already is.
    let compressed_raw = (img.format != Format::Png && !img.compressed).then(|| {
        use std::io::Write as _;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        z.write_all(&img.data).ok()?;
        z.finish().ok()
    });
    let (payload, compressed) = match compressed_raw.flatten() {
        Some(z) => (z, true),
        None => (img.data.to_vec(), img.compressed),
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&payload);
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
    let mut keys = format!("a=t,q=2,i={hid},f={}", img.format.code());
    if img.format != Format::Png {
        keys.push_str(&format!(",s={},v={}", img.width, img.height));
    }
    if compressed {
        keys.push_str(",o=z");
    }
    if chunks.is_empty() {
        out.extend_from_slice(format!("\x1b_G{keys}\x1b\\").as_bytes());
        return;
    }
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        let head = if i == 0 { format!("{keys},m={more}") } else { format!("m={more},q=2") };
        out.extend_from_slice(b"\x1b_G");
        out.extend_from_slice(head.as_bytes());
        out.push(b';');
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
}

/// The placeholder cell for (row, column) of an image.
pub fn placeholder(row: usize, col: usize) -> Option<String> {
    let (r, c) = (DIACRITICS.get(row)?, DIACRITICS.get(col)?);
    Some([PLACEHOLDER, *r, *c].iter().collect())
}

// ------------------------------------------------------------------ Sixel and iTerm2

/// Decode a Sixel image into RGBA.
pub fn sixel_image(data: &[u8]) -> Option<ImageData> {
    let (w, h, px) = sixel::decode(data)?;
    Some(ImageData::new(Format::Rgba, w, h, false, px))
}

/// An animated GIF as a playing animation (None for other data or a single frame).
fn animated_gif(bytes: &[u8]) -> Option<ImageData> {
    use image::AnimationDecoder as _;
    if !bytes.starts_with(b"GIF8") {
        return None;
    }
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    let frames: Vec<image::Frame> = decoder.into_frames().take(MAX_FRAMES).collect::<Result<_, _>>().ok()?;
    if frames.len() < 2 {
        return None;
    }
    let (w, h) = frames[0].buffer().dimensions();
    let frames: Vec<Frame> = frames
        .into_iter()
        .filter(|f| f.buffer().dimensions() == (w, h))
        .map(|f| {
            let (num, den) = f.delay().numer_denom_ms();
            // Browsers show 0-10 ms GIF frames for 100 ms; so do we.
            let ms = num.checked_div(den).unwrap_or(100);
            let gap = if ms <= 10 { 100 } else { ms as i32 };
            Frame { rgba: Arc::new(f.into_buffer().into_raw()), gap_ms: gap, generation: 0 }
        })
        .collect();
    let mut img = ImageData::new(Format::Rgba, w, h, false, frames[0].rgba.to_vec());
    img.animation = Some(Animation { frames, current: 0, running: true, loops_left: None, next: None });
    Some(img)
}

/// An iTerm2 inline image: the image and its size in cells, or None if it is not shown inline.
pub fn iterm_image(body: &[u8], cell: (u16, u16), screen_cols: u16) -> Option<(ImageData, u16, u16)> {
    let colon = body.iter().position(|b| *b == b':')?;
    let args = std::str::from_utf8(&body[..colon]).ok()?;
    let mut inline = false;
    let (mut width, mut height) = ("auto".to_string(), "auto".to_string());
    let mut keep_aspect = true;
    for kv in args.split(';') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        match k {
            "inline" => inline = v == "1",
            "width" => width = v.to_string(),
            "height" => height = v.to_string(),
            "preserveAspectRatio" => keep_aspect = v != "0",
            _ => {}
        }
    }
    if !inline {
        return None;
    }
    let payload: Vec<u8> = body[colon + 1..].iter().copied().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(&payload).ok()?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let image = if let Some(gif) = animated_gif(&bytes) {
        gif
    } else if let Some((w, h)) = png_size(&bytes) {
        ImageData::new(Format::Png, w, h, false, bytes)
    } else {
        let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let (w, h) = img.dimensions();
        ImageData::new(Format::Rgba, w, h, false, img.into_raw())
    };
    // Sizes: "N" cells, "Npx" pixels, "N%" of the screen, or "auto".
    let to_cells = |spec: &str, cell_px: u16, screen: u16| -> u32 {
        if let Some(px) = spec.strip_suffix("px") {
            px.parse::<u32>().map(|p| p.div_ceil(cell_px.max(1) as u32)).unwrap_or(0)
        } else if let Some(p) = spec.strip_suffix('%') {
            p.parse::<u32>().map(|p| screen as u32 * p.min(100) / 100).unwrap_or(0)
        } else {
            spec.parse::<u32>().unwrap_or(0)
        }
    };
    let c = to_cells(&width, cell.0, screen_cols);
    let r = to_cells(&height, cell.1, 1000);
    let (c, r) = if keep_aspect && c != 0 && r != 0 { (c, 0) } else { (c, r) };
    let (cols, rows) = cells_for(image.width, image.height, c, r, cell);
    Some((image, cols, rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diacritic_table_is_sorted_and_indexed() {
        assert_eq!(DIACRITICS.len(), 297);
        assert_eq!(diacritic_index('\u{0305}'), Some(0));
        assert_eq!(diacritic_index('\u{030D}'), Some(1));
        assert_eq!(diacritic_index('a'), None);
    }

    #[test]
    fn splits_images_from_text_across_reads() {
        let mut s = Splitter::default();
        let mut pieces = s.split(b"ab\x1b_Ga=T,f=100;AA");
        pieces.extend(s.split(b"AA\x1b\\cd\x1b[2J\x1b]0;title\x07\x1bPq#0!3~\x1b\\\x1b[16t"));
        pieces.extend(s.split(b"\x1b]1337;File=inline=1:QUJD\x07\x1bc"));
        assert_eq!(
            pieces,
            vec![
                Piece::Text(b"ab".to_vec()),
                Piece::Kitty(b"a=T,f=100;AAAA".to_vec()),
                Piece::Text(b"cd\x1b[2J".to_vec()),
                Piece::Clear,
                Piece::Text(b"\x1b]0;title\x07".to_vec()),
                Piece::Sixel(b"#0!3~".to_vec()),
                Piece::Text(b"\x1b[16t".to_vec()),
                Piece::CellSizeQuery,
                Piece::Iterm(b"inline=1:QUJD".to_vec()),
                Piece::Text(b"\x1bc".to_vec()),
                Piece::Reset,
            ]
        );
        // Other DCS strings and private CSI pass through untouched.
        assert_eq!(
            s.split(b"\x1bP$q q\x1b\\\x1b[?2J"),
            vec![Piece::Text(b"\x1bP$q q\x1b\\".to_vec()), Piece::Text(b"\x1b[?2J".to_vec())]
        );
    }

    fn png_1x1() -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn kitty_transmit_place_delete() {
        let mut g = Graphics::default();
        let b64 = base64::engine::general_purpose::STANDARD.encode(png_1x1());
        // Query: OK, nothing stored.
        let q = g.kitty(format!("i=31,a=q,t=d,f=24,s=1,v=1;{}", "AAAA").as_bytes(), (8, 16), true);
        assert_eq!(q.reply, b"\x1b_Gi=31;OK\x1b\\");
        assert!(g.images.is_empty());
        // Chunked transmit and display.
        let (a, b) = b64.split_at(4);
        assert_eq!(g.kitty(format!("a=T,f=100,i=7,c=4,r=2,m=1;{a}").as_bytes(), (8, 16), true), Effect::default());
        let e = g.kitty(format!("m=0;{b}").as_bytes(), (8, 16), true);
        assert_eq!(e.place, Some((7, 0, 4, 2, 0, true)));
        assert_eq!(e.reply, b"\x1b_Gi=7;OK\x1b\\");
        assert_eq!((g.images[&7].width, g.images[&7].height), (3, 2));
        // Virtual placement sized from pixels and cell size.
        g.kitty(format!("a=T,U=1,f=100,i=8,q=2;{b64}").as_bytes(), (2, 1), true);
        assert_eq!(g.virtuals[&(8, 0)], (2, 2));
        // Errors are reported, quiet suppresses replies.
        let e = g.kitty(b"a=p,i=99", (8, 16), true);
        assert!(String::from_utf8_lossy(&e.reply).contains("ENOENT"));
        assert!(g.kitty(b"a=p,i=99,q=2", (8, 16), true).reply.is_empty());
        // Deleting by id frees image and virtual placements.
        let e = g.kitty(b"a=d,d=I,i=8", (8, 16), true);
        assert_eq!(e.delete, Some(Delete::Image { id: 8, placement: 0, free: true }));
        g.delete_image(8, 0, true);
        assert!(!g.images.contains_key(&8) && g.virtuals.is_empty());
        // Disabled: silent.
        assert_eq!(g.kitty(b"i=31,a=q,t=d,f=24,s=1,v=1;AAAA", (8, 16), false), Effect::default());
    }

    #[test]
    fn host_sync_transmits_once_and_cleans_up() {
        let mut g = Graphics::default();
        g.add_image(5, ImageData::new(Format::Rgb, 2, 1, false, vec![1, 2, 3, 4, 5, 6]));
        g.virtuals.insert((5, 0), (3, 2));
        let mut host = HostGraphics::default();
        let mut out = Vec::new();
        host.sync(std::iter::once((1, &mut g)), &mut out, true);
        let text = String::from_utf8_lossy(&out).to_string();
        let hid = host.host_id(1, 5).unwrap();
        let head = format!("a=t,q=2,i={hid},f=24,s=2,v=1,o=z,m=0;");
        let b64 = text.split(&head).nth(1).and_then(|r| r.split('\x1b').next()).expect("compressed transmission");
        let z = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        let mut raw = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&z[..]), &mut raw).unwrap();
        assert_eq!(raw, vec![1, 2, 3, 4, 5, 6]);
        assert!(text.contains(&format!("a=p,U=1,q=2,i={hid},p={},c=3,r=2", HostGraphics::placement_id(3, 2))));
        // Nothing new: nothing sent.
        out.clear();
        host.sync(std::iter::once((1, &mut g)), &mut out, true);
        assert!(out.is_empty());
        // Removed image: deleted on the host.
        g.remove_image(5);
        host.sync(std::iter::once((1, &mut g)), &mut out, true);
        assert_eq!(String::from_utf8_lossy(&out), format!("\x1b_Ga=d,d=I,i={hid},q=2\x1b\\"));
        assert_eq!(placeholder(1, 2).unwrap().chars().count(), 3);
    }

    #[test]
    fn kitty_animation_frames_and_playback() {
        let b64 = |px: &[u8]| base64::engine::general_purpose::STANDARD.encode(px);
        let mut g = Graphics::default();
        // A 2x1 RGBA image (red, red), then frame 2 drawn onto it: blue at x=1, gap 50 ms.
        g.kitty(format!("a=t,f=32,s=2,v=1,i=9;{}", b64(&[255, 0, 0, 255, 255, 0, 0, 255])).as_bytes(), (8, 16), true);
        let e =
            g.kitty(format!("a=f,i=9,f=32,s=1,v=1,x=1,c=1,z=50;{}", b64(&[0, 0, 255, 255])).as_bytes(), (8, 16), true);
        assert_eq!(e.reply, b"\x1b_Gi=9;OK\x1b\\");
        let anim = g.images[&9].animation.as_ref().unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(*anim.frames[1].rgba, vec![255, 0, 0, 255, 0, 0, 255, 255]);
        // Not playing until asked; then frames advance after their gap and loop forever.
        assert!(g.next_frame_at().is_none());
        g.kitty(b"a=a,i=9,s=3,v=1", (8, 16), true);
        let t0 = Instant::now();
        assert!(!g.animate(t0), "first call only schedules");
        let first = g.images[&9].generation;
        assert!(g.animate(t0 + Duration::from_millis(41)));
        assert_ne!(g.images[&9].generation, first);
        assert_eq!(*g.images[&9].data, vec![255, 0, 0, 255, 0, 0, 255, 255]);
        assert!(!g.animate(t0 + Duration::from_millis(60)), "frame 2 shows for 50 ms");
        assert!(g.animate(t0 + Duration::from_millis(95)));
        assert_eq!(g.images[&9].generation, first, "looped back to frame 1");
        // Stop.
        g.kitty(b"a=a,i=9,s=1", (8, 16), true);
        assert!(!g.animate(t0 + Duration::from_secs(5)));
    }

    #[test]
    fn animation_loop_count() {
        let mut g = Graphics::default();
        let px = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3, 255]);
        g.kitty(format!("a=t,f=32,s=1,v=1,i=4;{px}").as_bytes(), (8, 16), true);
        g.kitty(format!("a=f,i=4,f=32,s=1,v=1,z=10;{px}").as_bytes(), (8, 16), true);
        // v=n plays n-1 times in total (kitty): v=3 is two passes, then stop on the last frame.
        g.kitty(b"a=a,i=4,s=3,v=3", (8, 16), true);
        let mut t = Instant::now();
        g.animate(t);
        let mut frames_shown = 0;
        for _ in 0..20 {
            if g.next_frame_at().is_none() {
                break;
            }
            t += Duration::from_millis(50);
            frames_shown += usize::from(g.animate(t));
        }
        assert_eq!(frames_shown, 3, "frame 2, frame 1, frame 2, then stop");
        assert!(g.next_frame_at().is_none());
        assert_eq!(g.images[&4].animation.as_ref().unwrap().current, 1);
    }

    #[test]
    fn animated_gifs_play() {
        let mut gif = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut gif);
            for color in [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]] {
                let buf = image::RgbaImage::from_pixel(4, 3, image::Rgba(color));
                enc.encode_frame(image::Frame::from_parts(buf, 0, 0, image::Delay::from_numer_denom_ms(70, 1)))
                    .unwrap();
            }
        }
        let body = format!("inline=1:{}", base64::engine::general_purpose::STANDARD.encode(&gif));
        let (img, _, _) = iterm_image(body.as_bytes(), (8, 16), 80).unwrap();
        let anim = img.animation.as_ref().expect("animated");
        assert_eq!(anim.frames.len(), 3);
        assert!(anim.running && anim.frames.iter().all(|f| f.gap_ms == 70));
    }

    #[test]
    fn redrawn_sixel_replaces_the_old_one() {
        let mut g = Graphics::default();
        let a = g.add_anonymous(ImageData::new(Format::Rgba, 1, 1, false, vec![0; 4]));
        g.placements.push(Placement {
            image: a,
            id: 0,
            anchor: Anchor::Row(2),
            col: 3,
            cols: 4,
            rows: 2,
            z: 0,
            alt: true,
        });
        g.replace_anonymous(|p| p.anchor == Anchor::Row(2) && p.col == 3);
        assert!(g.images.is_empty() && g.placements.is_empty());
    }

    #[test]
    fn cell_sizes() {
        assert_eq!(cells_for(64, 48, 0, 0, (8, 16)), (8, 3));
        assert_eq!(cells_for(64, 48, 4, 0, (8, 16)), (4, 2));
        assert_eq!(cells_for(64, 48, 10, 5, (8, 16)), (10, 5));
        assert_eq!(cells_for(100_000, 1, 0, 0, (1, 1)), (297, 1));
    }

    #[test]
    fn iterm_inline_images() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(png_1x1());
        let body = format!("name=eA==;inline=1;width=6:{b64}");
        let (img, cols, rows) = iterm_image(body.as_bytes(), (8, 16), 80).unwrap();
        assert_eq!((img.format, cols, rows), (Format::Png, 6, 2));
        assert!(iterm_image(format!("inline=0:{b64}").as_bytes(), (8, 16), 80).is_none());
    }
}

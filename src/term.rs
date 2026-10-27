use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell, Hyperlink};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{Config as TermConfig, Term as Emulator, TermMode, viewport_to_point};
pub use alacritty_terminal::vi_mode::ViMotion;
use alacritty_terminal::vte::ansi::{CursorShape, NamedColor, Processor, Rgb};
use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::app::AppEvent;
use crate::graphics::{self, Anchor, Delete, Graphics, Piece, Placement, Splitter};
use std::collections::HashMap;

/// Collects events the emulator raises while parsing.
#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<TermEvent>>>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        if let Ok(mut q) = self.0.lock() {
            q.push(event);
        }
    }
}

#[derive(Clone, Copy)]
struct Size {
    rows: usize,
    cols: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseMode {
    None,
    /// Report presses and releases (1000).
    Click,
    /// Also report motion while a button is held (1002).
    Drag,
    /// Report all motion (1003).
    Motion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseProtocol {
    pub mode: MouseMode,
    pub sgr: bool,
    pub utf8: bool,
}

/// Picks out desktop notifications (OSC 9 and OSC 777), which the emulator ignores.
#[derive(Default)]
struct OscScanner {
    state: u8,
    buf: Vec<u8>,
}

const OSC_MAX: usize = 4096;

impl OscScanner {
    /// States: 0 ground, 1 after ESC, 2 in OSC, 3 ESC inside OSC.
    fn scan(&mut self, bytes: &[u8], out: &mut Vec<(String, String)>) {
        for &b in bytes {
            match (self.state, b) {
                (2, 0x07) => {
                    self.finish(out);
                    self.state = 0;
                }
                (2, 0x1b) => self.state = 3,
                (2, _) => {
                    if self.buf.len() < OSC_MAX {
                        self.buf.push(b);
                    }
                }
                (3, b'\\') => {
                    self.finish(out);
                    self.state = 0;
                }
                (3, b']') | (1, b']') => {
                    self.state = 2;
                    self.buf.clear();
                }
                (_, 0x1b) => self.state = 1,
                _ => self.state = 0,
            }
        }
    }

    fn finish(&mut self, out: &mut Vec<(String, String)>) {
        let text = String::from_utf8_lossy(&self.buf).into_owned();
        let mut parts = text.splitn(2, ';');
        match (parts.next(), parts.next()) {
            // "9;4;..." is a progress report, not a notification.
            // "9;<digits>;..." are ConEmu sub-commands (progress, cwd, ...), not notifications.
            (Some("9"), Some(body)) if !body.is_empty() && !is_conemu_command(body) => {
                out.push((String::new(), body.to_string()));
            }
            (Some("777"), Some(rest)) => {
                let mut f = rest.splitn(3, ';');
                if f.next() == Some("notify") {
                    out.push((f.next().unwrap_or("").to_string(), f.next().unwrap_or("").to_string()));
                }
            }
            _ => {}
        }
    }
}

fn is_conemu_command(body: &str) -> bool {
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && body.as_bytes().get(digits) == Some(&b';')
}

pub struct SpawnOptions<'a> {
    pub id: u64,
    /// Run through /bin/sh; None starts `shell` directly.
    pub command: Option<&'a str>,
    pub shell: &'a str,
    pub cwd: Option<PathBuf>,
    pub rows: u16,
    pub cols: u16,
    pub scrollback: usize,
    /// Default (foreground, background), reported for OSC 10/11 queries.
    pub colors: (Rgb, Rgb),
    /// Extra environment for the program.
    pub env: Vec<(String, String)>,
    /// Host cell size in pixels (programs size images with it).
    pub cell_px: (u16, u16),
    /// Whether images can be shown.
    pub graphics: bool,
}

/// PTY size including pixels, which programs such as yazi read to size images.
fn pty_size(rows: u16, cols: u16, cell_px: (u16, u16)) -> PtySize {
    PtySize { rows, cols, pixel_width: cols.saturating_mul(cell_px.0), pixel_height: rows.saturating_mul(cell_px.1) }
}

pub struct Term {
    emu: Emulator<Listener>,
    processor: Processor,
    events: Listener,
    osc: OscScanner,
    /// Bytes for the program, written by a separate thread so a program that stops reading
    /// its input can never block the UI.
    writer: std::sync::mpsc::Sender<Vec<u8>>,
    master: Box<dyn MasterPty + Send>,
    child: Option<Box<dyn Child + Send + Sync>>,
    default_colors: (Rgb, Rgb),
    pub title: Option<String>,
    pub bell: bool,
    pub notifications: Vec<(String, String)>,
    /// Text programs asked to copy (OSC 52).
    pub clipboard: Vec<String>,
    /// Images the program showed (kitty graphics, Sixel, iTerm2).
    pub gfx: Graphics,
    splitter: Splitter,
    /// Whether images can be shown; programs are told so when they ask.
    gfx_enabled: bool,
    /// Size of one cell in pixels on the host terminal.
    cell_px: (u16, u16),
    was_alt: bool,
}

/// Hyperlink URI used to mark the anchor cell of an image on the main screen.
pub const IMAGE_MARKER: &str = "tuidesk-image:";

/// How the cursor moves after an image is placed.
#[derive(Clone, Copy)]
enum CursorMove {
    None,
    /// kitty and iTerm2: like text, to after the image's last column on its last row.
    AfterImage,
    /// Sixel: to the start of the line below the image.
    NextLine,
}

impl Term {
    pub fn spawn(opts: SpawnOptions, tx: Sender<AppEvent>) -> Result<Self> {
        let SpawnOptions { id, command, shell, cwd, rows, cols, scrollback, colors, env, cell_px, graphics } = opts;
        let pair = native_pty_system().openpty(pty_size(rows, cols, cell_px)).context("opening pty")?;
        let mut cmd = match command {
            Some(c) => {
                // The shell runs the command line as typed (env prefixes, `a && b`), and replaces
                // itself with the last command where it can.
                #[cfg(unix)]
                let b = {
                    let mut b = CommandBuilder::new("/bin/sh");
                    b.args(["-c", c]);
                    b
                };
                // cmd does not expand $SHELL, which launcher entries use for "a terminal".
                #[cfg(windows)]
                let b = {
                    let mut b = CommandBuilder::new("cmd.exe");
                    b.args(["/C", &c.replace("$SHELL", shell)]);
                    b
                };
                b
            }
            None => CommandBuilder::new(shell),
        };
        if let Some(d) = cwd.filter(|d| d.is_dir()).or_else(|| std::env::current_dir().ok()) {
            cmd.cwd(d);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TUIDESK", "1");
        cmd.env("SHELL", shell);
        for (k, v) in env {
            cmd.env(k, v);
        }
        // The host terminal's identity no longer applies inside a tuidesk window.
        for var in ["TERM_PROGRAM", "TERM_PROGRAM_VERSION", "KITTY_WINDOW_ID", "WEZTERM_PANE", "TMUX"] {
            cmd.env_remove(var);
        }
        let child = pair.slave.spawn_command(cmd).context("spawning program")?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("cloning pty reader")?;
        let mut pty_writer = pair.master.take_writer().context("taking pty writer")?;
        let (writer, queue) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name(format!("pty-w-{id}"))
            .spawn(move || {
                for bytes in queue {
                    if pty_writer.write_all(&bytes).and_then(|_| pty_writer.flush()).is_err() {
                        break;
                    }
                }
            })
            .context("starting pty writer")?;
        std::thread::Builder::new()
            .name(format!("pty-{id}"))
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(AppEvent::PtyOutput(id, buf[..n].to_vec())).is_err() {
                                return;
                            }
                        }
                    }
                }
                let _ = tx.send(AppEvent::PtyExit(id));
            })
            .context("starting pty reader")?;

        let events = Listener::default();
        let config = TermConfig { scrolling_history: scrollback, ..Default::default() };
        let size = Size { rows: rows.max(1) as usize, cols: cols.max(1) as usize };
        Ok(Self {
            emu: Emulator::new(config, &size, events.clone()),
            processor: Processor::new(),
            events,
            osc: OscScanner::default(),
            writer,
            master: pair.master,
            child: Some(child),
            default_colors: colors,
            title: None,
            bell: false,
            notifications: Vec::new(),
            clipboard: Vec::new(),
            gfx: Graphics::default(),
            splitter: Splitter::default(),
            gfx_enabled: graphics,
            cell_px,
            was_alt: false,
        })
    }

    /// Feed program output into the emulator and handle what it asked for. Image commands are
    /// handled at their place in the stream, so they see the cursor where the program left it.
    pub fn process(&mut self, bytes: &[u8]) {
        self.osc.scan(bytes, &mut self.notifications);
        for piece in self.splitter.split(bytes) {
            match piece {
                Piece::Text(text) => {
                    self.processor.advance(&mut self.emu, &text);
                    // Replies go out in order with ours (programs match them up by order).
                    self.handle_events();
                    self.check_alt_screen();
                }
                Piece::Kitty(body) => self.kitty(&body),
                Piece::Sixel(data) => {
                    if self.gfx_enabled
                        && let Some(img) = graphics::sixel_image(&data)
                    {
                        let (cols, rows) = graphics::cells_for(img.width, img.height, 0, 0, self.cell_px);
                        let id = self.gfx.add_anonymous(img);
                        self.place(id, 0, cols, rows, 0, CursorMove::NextLine);
                    }
                }
                Piece::Iterm(body) => {
                    let screen_cols = self.size().1;
                    if self.gfx_enabled
                        && let Some((img, cols, rows)) = graphics::iterm_image(&body, self.cell_px, screen_cols)
                    {
                        let id = self.gfx.add_anonymous(img);
                        self.place(id, 0, cols, rows, 0, CursorMove::AfterImage);
                    }
                }
                Piece::CellSizeQuery => {
                    let (w, h) = self.cell_px;
                    self.write(format!("\x1b[6;{h};{w}t").as_bytes());
                }
                Piece::Clear => {
                    let alt = self.alt_screen();
                    self.gfx.delete_where(false, |p| p.alt == alt);
                }
                Piece::Reset => self.gfx.clear(),
            }
        }
        self.handle_events();
    }

    /// Alternate screen placements belong to the program that left it.
    fn check_alt_screen(&mut self) {
        let alt = self.alt_screen();
        if self.was_alt && !alt {
            self.gfx.delete_where(false, |p| p.alt);
        }
        self.was_alt = alt;
    }

    fn kitty(&mut self, body: &[u8]) {
        let effect = self.gfx.kitty(body, self.cell_px, self.gfx_enabled);
        if !effect.reply.is_empty() {
            self.write(&effect.reply);
        }
        if let Some((image, id, cols, rows, z, move_cursor)) = effect.place {
            let mv = if move_cursor { CursorMove::AfterImage } else { CursorMove::None };
            self.place(image, id, cols, rows, z, mv);
        }
        if let Some(sel) = effect.delete {
            self.delete_placements(sel);
        }
    }

    /// Show `image` at the cursor.
    fn place(&mut self, image: u32, id: u32, cols: u16, rows: u16, z: i32, mv: CursorMove) {
        let point = self.emu.grid().cursor.point;
        let alt = self.alt_screen();
        let col = point.column.0 as u16;
        // A Sixel or iTerm2 image drawn where the same-sized one already is replaces it.
        if self.gfx.is_anonymous(image) {
            let old_marker = self.emu.grid()[point]
                .hyperlink()
                .filter(|h| h.uri() == IMAGE_MARKER)
                .and_then(|h| h.id().strip_prefix("tuidesk-img-").and_then(|m| m.parse::<u64>().ok()));
            self.gfx.replace_anonymous(|p| {
                let here = match p.anchor {
                    Anchor::Row(r) => alt && p.alt && r == point.line.0 && p.col == col,
                    Anchor::Marker(m) => !alt && Some(m) == old_marker,
                };
                here && (p.cols, p.rows) == (cols, rows)
            });
        }
        let anchor = if alt {
            Anchor::Row(point.line.0)
        } else {
            let marker = self.gfx.new_marker();
            let link = Hyperlink::new(Some(format!("tuidesk-img-{marker}")), IMAGE_MARKER.to_string());
            self.emu.grid_mut()[point].set_hyperlink(Some(link));
            Anchor::Marker(marker)
        };
        // A placement id replaces the earlier placement with that id.
        if id != 0 {
            self.gfx.placements.retain(|p| !(p.image == image && p.id == id));
        }
        self.gfx.placements.push(Placement { image, id, anchor, col, cols, rows, z, alt });
        self.gfx.changed = true;
        let moves = match mv {
            CursorMove::None => String::new(),
            CursorMove::AfterImage => format!("{}\x1b[{cols}C", "\n".repeat(rows.saturating_sub(1) as usize)),
            CursorMove::NextLine => format!("\r{}", "\n".repeat(rows as usize)),
        };
        if !moves.is_empty() {
            self.processor.advance(&mut self.emu, moves.as_bytes());
        }
    }

    fn delete_placements(&mut self, sel: Delete) {
        let cursor = self.emu.grid().cursor.point;
        let (cy, cx) = (cursor.line.0, cursor.column.0 as u16);
        let positions: HashMap<usize, (i32, u16)> =
            self.placement_positions().into_iter().map(|(i, row, col)| (i, (row, col))).collect();
        let hits = |i: usize, p: &Placement, x: Option<u16>, y: Option<i32>| -> bool {
            let Some(&(row, col)) = positions.get(&i) else { return false };
            x.is_none_or(|x| x >= col && x < col + p.cols) && y.is_none_or(|y| y >= row && y < row + p.rows as i32)
        };
        let alt = self.alt_screen();
        let mut keep = Vec::new();
        let free = match sel {
            Delete::All { free }
            | Delete::AtCursor { free }
            | Delete::AtCell { free, .. }
            | Delete::Column { free, .. }
            | Delete::Row { free, .. }
            | Delete::Z { free, .. } => free,
            Delete::Image { id, placement, free } => {
                self.gfx.delete_image(id, placement, free);
                return;
            }
        };
        for (i, p) in self.gfx.placements.iter().enumerate() {
            let on_screen = p.alt == alt;
            let delete = on_screen
                && match sel {
                    Delete::All { .. } => true,
                    Delete::AtCursor { .. } => hits(i, p, Some(cx), Some(cy)),
                    Delete::AtCell { x, y, .. } => hits(i, p, Some(x), Some(y as i32)),
                    Delete::Column { x, .. } => hits(i, p, Some(x), None),
                    Delete::Row { y, .. } => hits(i, p, None, Some(y as i32)),
                    Delete::Z { z, .. } => p.z == z,
                    Delete::Image { .. } => false,
                };
            keep.push(!delete);
        }
        let mut k = keep.into_iter();
        self.gfx.placements.retain(|_| k.next().unwrap_or(true));
        if free {
            self.gfx.free_unused();
        }
        self.gfx.changed = true;
    }

    /// Placements on the current screen with their top-left cell in viewport coordinates:
    /// (index into `gfx.placements`, row, column). Rows can be negative (scrolled partly off).
    pub fn placement_positions(&self) -> Vec<(usize, i32, u16)> {
        let alt = self.alt_screen();
        let offset = self.scrollback() as i32;
        let mut out = Vec::new();
        let mut wanted: HashMap<u64, usize> = HashMap::new();
        let mut tallest = 0i32;
        for (i, p) in self.gfx.placements.iter().enumerate() {
            if p.alt != alt {
                continue;
            }
            match p.anchor {
                Anchor::Row(r) => out.push((i, r + offset, p.col)),
                Anchor::Marker(m) => {
                    wanted.insert(m, i);
                    tallest = tallest.max(p.rows as i32);
                }
            }
        }
        if wanted.is_empty() {
            return out;
        }
        // Find marker cells from just above the view (images reaching into it) to its bottom.
        let grid = self.emu.grid();
        let top = self.emu.topmost_line().0;
        let screen = self.emu.screen_lines() as i32;
        let cols = self.emu.columns();
        for row in -tallest..screen {
            let line = row - offset;
            if line < top {
                continue;
            }
            for col in 0..cols {
                let cell = &grid[Point::new(Line(line), Column(col))];
                let Some(link) = cell.hyperlink() else { continue };
                if link.uri() != IMAGE_MARKER {
                    continue;
                }
                let marker = link.id().strip_prefix("tuidesk-img-").and_then(|m| m.parse::<u64>().ok());
                if let Some(&i) = marker.and_then(|m| wanted.get(&m)) {
                    out.push((i, row, col as u16));
                }
            }
        }
        out
    }

    /// Tell the program whether images can be shown, and how big a cell is in pixels.
    pub fn set_graphics(&mut self, enabled: bool, cell_px: (u16, u16)) {
        self.gfx_enabled = enabled;
        if cell_px != self.cell_px && cell_px.0 > 0 && cell_px.1 > 0 {
            self.cell_px = cell_px;
            let (rows, cols) = self.size();
            let _ = self.master.resize(pty_size(rows, cols, cell_px));
        }
    }

    fn handle_events(&mut self) {
        let events = match self.events.0.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(_) => return,
        };
        for ev in events {
            match ev {
                // Advertise Sixel (4) in the primary device attributes when images work.
                TermEvent::PtyWrite(s) if s == "\x1b[?6c" && self.gfx_enabled => self.write(b"\x1b[?62;4;22c"),
                TermEvent::PtyWrite(s) => self.write(s.as_bytes()),
                TermEvent::Title(t) => self.title = Some(t),
                TermEvent::ResetTitle => self.title = None,
                TermEvent::Bell => self.bell = true,
                TermEvent::ClipboardStore(_, text) => self.clipboard.push(text),
                TermEvent::ColorRequest(idx, fmt) => {
                    let rgb = self.color_rgb(idx);
                    self.write(fmt(rgb).as_bytes());
                }
                TermEvent::TextAreaSizeRequest(fmt) => {
                    let (rows, cols) = self.size();
                    let (cw, ch) = self.cell_px;
                    let ws = WindowSize { num_lines: rows, num_cols: cols, cell_width: cw, cell_height: ch };
                    self.write(fmt(ws).as_bytes());
                }
                _ => {}
            }
        }
    }

    /// Current RGB of palette entry `idx`, for color queries.
    fn color_rgb(&self, idx: usize) -> Rgb {
        if let Some(c) = self.emu.colors()[idx] {
            return c;
        }
        let (fg, bg) = self.default_colors;
        match idx {
            i if i < 256 => {
                let (r, g, b) = crate::theme::index_to_rgb(i as u8);
                Rgb { r, g, b }
            }
            i if i == NamedColor::Background as usize => bg,
            _ => fg,
        }
    }

    /// Deadline of a pending synchronized update (DEC mode 2026), if any.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.processor.sync_timeout().sync_timeout()
    }

    /// Flush a synchronized update whose deadline passed. Returns true if it did.
    pub fn check_sync(&mut self) -> bool {
        match self.sync_deadline() {
            Some(t) if Instant::now() >= t => {
                self.processor.stop_sync(&mut self.emu);
                self.handle_events();
                true
            }
            _ => false,
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            let _ = self.writer.send(bytes.to_vec());
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.emu.screen_lines() as u16, self.emu.columns() as u16)
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if self.size() == (rows, cols) {
            return;
        }
        self.emu.resize(Size { rows: rows as usize, cols: cols as usize });
        let _ = self.master.resize(pty_size(rows, cols, self.cell_px));
    }

    fn mode(&self) -> TermMode {
        *self.emu.mode()
    }

    pub fn app_cursor(&self) -> bool {
        self.mode().contains(TermMode::APP_CURSOR)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.mode().contains(TermMode::BRACKETED_PASTE)
    }

    pub fn alt_screen(&self) -> bool {
        self.mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn mouse(&self) -> MouseProtocol {
        let m = self.mode();
        let mode = if m.contains(TermMode::MOUSE_MOTION) {
            MouseMode::Motion
        } else if m.contains(TermMode::MOUSE_DRAG) {
            MouseMode::Drag
        } else if m.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseMode::Click
        } else {
            MouseMode::None
        };
        MouseProtocol { mode, sgr: m.contains(TermMode::SGR_MOUSE), utf8: m.contains(TermMode::UTF8_MOUSE) }
    }

    /// Lines scrolled back from the bottom.
    pub fn scrollback(&self) -> usize {
        self.emu.grid().display_offset()
    }

    pub fn scroll(&mut self, delta: isize) {
        self.emu.scroll_display(Scroll::Delta(delta as i32));
    }

    pub fn scroll_reset(&mut self) {
        self.emu.scroll_display(Scroll::Bottom);
    }

    /// Cursor (row, col, shape) in viewport coordinates, if it should be drawn.
    pub fn cursor(&self) -> Option<(u16, u16, CursorShape)> {
        if !self.mode().contains(TermMode::SHOW_CURSOR) || self.scrollback() > 0 {
            return None;
        }
        let content = self.emu.renderable_content();
        let p = content.cursor.point;
        let row = p.line.0 + content.display_offset as i32;
        if row < 0 || content.cursor.shape == CursorShape::Hidden {
            return None;
        }
        Some((row as u16, p.column.0 as u16, self.emu.cursor_style().shape))
    }

    /// Visit every visible cell: (row, col, cell, selected, palette overrides).
    pub fn for_each_cell(&self, mut f: impl FnMut(u16, u16, &Cell, bool, &Colors)) {
        let content = self.emu.renderable_content();
        let offset = content.display_offset as i32;
        let selection = content.selection;
        let colors = content.colors;
        for item in content.display_iter {
            let row = item.point.line.0 + offset;
            if row < 0 {
                continue;
            }
            let selected = selection.is_some_and(|s| s.contains(item.point));
            f(row as u16, item.point.column.0 as u16, item.cell, selected, colors);
        }
    }

    fn point(&self, row: u16, col: u16) -> Point {
        let (rows, cols) = self.size();
        let vp = Point::new(row.min(rows - 1) as usize, Column(col.min(cols - 1) as usize));
        viewport_to_point(self.scrollback(), vp)
    }

    pub fn start_selection(&mut self, ty: SelectionType, row: u16, col: u16) {
        let p = self.point(row, col);
        self.emu.selection = Some(Selection::new(ty, p, Side::Left));
    }

    pub fn update_selection(&mut self, row: u16, col: u16, side_right: bool) {
        let p = self.point(row, col);
        if let Some(sel) = self.emu.selection.as_mut() {
            sel.update(p, if side_right { Side::Right } else { Side::Left });
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        self.emu.selection_to_string().filter(|s| !s.is_empty())
    }

    pub fn clear_selection(&mut self) {
        self.emu.selection = None;
    }

    // ------------------------------------------------------------ copy mode

    pub fn set_copy_mode(&mut self, on: bool) {
        if self.mode().contains(TermMode::VI) != on {
            self.emu.toggle_vi_mode();
        }
        if !on {
            self.emu.selection = None;
            self.scroll_reset();
        }
    }

    pub fn vi_motion(&mut self, motion: ViMotion) {
        self.emu.vi_motion(motion);
        let p = self.emu.vi_mode_cursor.point;
        self.emu.scroll_to_point(p);
    }

    /// Move the copy cursor by whole lines (positive is down), scrolling as needed.
    pub fn vi_lines(&mut self, delta: i32) {
        let top = self.emu.topmost_line();
        let bottom = self.emu.bottommost_line();
        let mut p = self.emu.vi_mode_cursor.point;
        p.line = Line((p.line.0 + delta).clamp(top.0, bottom.0));
        self.emu.vi_goto_point(p);
    }

    /// Jump to the first (`top`) or last line of the history.
    pub fn vi_edge(&mut self, top: bool) {
        let line = if top { self.emu.topmost_line() } else { self.emu.bottommost_line() };
        self.emu.vi_goto_point(Point::new(line, Column(0)));
    }

    /// Start a selection at the copy cursor, or clear the current one.
    pub fn vi_toggle_selection(&mut self, ty: SelectionType) {
        if self.emu.selection.as_ref().is_some_and(|s| s.ty == ty) {
            self.emu.selection = None;
            return;
        }
        let mut sel = Selection::new(ty, self.emu.vi_mode_cursor.point, Side::Left);
        sel.include_all();
        self.emu.selection = Some(sel);
    }

    pub fn has_selection(&self) -> bool {
        self.emu.selection.is_some()
    }

    /// Copy cursor in viewport coordinates, if it is on screen.
    pub fn vi_cursor(&self) -> Option<(u16, u16)> {
        let p = self.emu.vi_mode_cursor.point;
        let row = p.line.0 + self.scrollback() as i32;
        (row >= 0 && row < self.size().0 as i32).then_some((row as u16, p.column.0 as u16))
    }

    /// Search history for a regex from the copy cursor. Returns false when nothing matches.
    pub fn vi_search(&mut self, pattern: &str, forward: bool) -> Result<bool, String> {
        let mut regex = RegexSearch::new(pattern).map_err(|e| e.to_string())?;
        let cursor = self.emu.vi_mode_cursor.point;
        // Start next to the cursor so repeated searches advance.
        let origin = if forward {
            cursor.add(&self.emu, alacritty_terminal::index::Boundary::None, 1)
        } else {
            cursor.sub(&self.emu, alacritty_terminal::index::Boundary::None, 1)
        };
        let dir = if forward { Direction::Right } else { Direction::Left };
        match self.emu.search_next(&mut regex, origin, dir, Side::Left, None) {
            Some(m) => {
                self.emu.vi_goto_point(*m.start());
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// URI of an OSC 8 hyperlink under a viewport cell.
    pub fn hyperlink_at(&self, row: u16, col: u16) -> Option<String> {
        let p = self.point(row, col);
        if p.line < Line(-(self.emu.grid().history_size() as i32)) {
            return None;
        }
        self.emu.grid()[p].hyperlink().map(|h| h.uri().to_string()).filter(|u| u != IMAGE_MARKER)
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref()?.process_id()
    }

    pub fn exit_code(&mut self) -> Option<u32> {
        self.child.as_mut()?.try_wait().ok().flatten().map(|s| s.exit_code())
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take()
            && matches!(child.try_wait(), Ok(None))
        {
            // The child leads its own session: hang up the whole process group (background
            // jobs included), then force it if it is still there after two seconds.
            #[cfg(windows)]
            let pgid: Option<()> = None;
            #[cfg(unix)]
            let pgid = child.process_id().map(|p| p as libc::pid_t);
            match pgid {
                // SAFETY: killpg only sends a signal.
                #[cfg(unix)]
                Some(pg) => unsafe {
                    libc::killpg(pg, libc::SIGHUP);
                },
                _ => {
                    let _ = child.kill();
                }
            }
            std::thread::spawn(move || {
                for _ in 0..20 {
                    if !matches!(child.try_wait(), Ok(None)) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                #[cfg(unix)]
                if let Some(pg) = pgid {
                    // SAFETY: as above.
                    unsafe {
                        libc::killpg(pg, libc::SIGKILL);
                    }
                }
                let _ = child.wait();
            });
        }
    }
}

/// Working directory of a process.
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut sys = System::new();
    let pid = Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always),
    );
    sys.process(pid)?.cwd().map(|p| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_term(rows: usize, cols: usize) -> (Emulator<Listener>, Processor) {
        let size = Size { rows, cols };
        (Emulator::new(TermConfig::default(), &size, Listener::default()), Processor::new())
    }

    fn cell_char(emu: &Emulator<Listener>, row: i32, col: usize) -> char {
        emu.grid()[Point::new(Line(row), Column(col))].c
    }

    #[test]
    fn handles_hvp_cursor_moves() {
        // btop positions everything with CSI row;col f.
        let (mut emu, mut p) = test_term(5, 20);
        p.advance(&mut emu, b"\x1b[2;5fXY\x1b[4;1fZ");
        assert_eq!(cell_char(&emu, 1, 4), 'X');
        assert_eq!(cell_char(&emu, 3, 0), 'Z');
    }

    #[test]
    fn scans_osc_notifications() {
        let mut s = OscScanner::default();
        let mut out = Vec::new();
        s.scan(b"a\x1b]9;hello\x07b\x1b]777;notify;T", &mut out);
        s.scan(b";body\x1b\\\x1b]9;4;1;50\x07\x1b]9;9;/home/me\x07\x1b]0;title\x07", &mut out);
        assert_eq!(out, vec![(String::new(), "hello".into()), ("T".into(), "body".into())]);
    }

    #[test]
    fn copy_mode_search_and_select() {
        let (mut emu, mut p) = test_term(4, 20);
        p.advance(&mut emu, b"alpha\r\nbravo target\r\ncharlie\r\n");
        emu.toggle_vi_mode();
        emu.vi_goto_point(Point::new(Line(0), Column(0)));
        let mut regex = RegexSearch::new("target").unwrap();
        let m = emu.search_next(&mut regex, Point::new(Line(0), Column(1)), Direction::Right, Side::Left, None);
        let start = *m.unwrap().start();
        assert_eq!(start, Point::new(Line(1), Column(6)));
        emu.vi_goto_point(start);
        let mut sel = Selection::new(SelectionType::Simple, start, Side::Left);
        sel.include_all();
        emu.selection = Some(sel);
        emu.vi_motion(ViMotion::SemanticRightEnd);
        assert_eq!(emu.selection_to_string().unwrap(), "target");
    }

    #[test]
    fn selection_copies_text() {
        let (mut emu, mut p) = test_term(3, 20);
        p.advance(&mut emu, b"hello world\r\nsecond");
        emu.selection = Some(Selection::new(SelectionType::Simple, Point::new(Line(0), Column(6)), Side::Left));
        emu.selection.as_mut().unwrap().update(Point::new(Line(1), Column(2)), Side::Right);
        assert_eq!(emu.selection_to_string().unwrap(), "world\nsec");
    }
}

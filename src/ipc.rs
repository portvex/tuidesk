//! Client/server split so a desktop survives its terminal closing (like tmux).
//!
//! The server owns windows and programs and renders with ratatui into a byte stream.
//! A client puts its terminal in raw mode, forwards parsed input events and writes the
//! server's output to the screen. Frames on the socket are `[kind u8][len u32 BE][payload]`.
//! The socket transport itself is Unix only (`socket`); on Windows tuidesk runs standalone.
// On Windows only the local display is used; the protocol and the remote side stay in one place
// for the Unix build, which still checks them for dead code.
#![cfg_attr(windows, allow(dead_code))]

use std::io::{self, Read, Write};
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::sync::mpsc::Sender;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use anyhow::{Context, Result, bail};
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
#[cfg(unix)]
use ratatui::crossterm::event::{self, Event};
use ratatui::layout::{Position, Size};
use ratatui::{Terminal, TerminalOptions, Viewport};
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use crate::app::AppEvent;
use crate::graphics::direct::ImageMode;

#[cfg(unix)]
mod socket;
#[cfg(unix)]
pub use socket::*;

/// A client connection.
#[cfg(unix)]
pub type Stream = std::os::unix::net::UnixStream;
/// There are no client connections on Windows, where tuidesk runs standalone; this type has no
/// values, so the code that handles clients compiles but can never run.
#[cfg(windows)]
pub enum Stream {}

#[cfg(unix)]
fn shutdown(s: &Stream) {
    let _ = s.shutdown(std::net::Shutdown::Both);
}
#[cfg(windows)]
fn shutdown(s: &Stream) {
    match *s {}
}

pub const PROTOCOL: u32 = 3;
/// Longest session name accepted on the command line.
pub const MAX_SESSION_NAME: usize = 32;

// Client to server.
const C_HELLO: u8 = 1;
const C_EVENT: u8 = 2;
const C_KILL: u8 = 3;
const C_MSG: u8 = 4;
const C_CELL: u8 = 5;
// Server to client.
const S_OUTPUT: u8 = 1;
const S_DETACH: u8 = 2;
const S_EXIT: u8 = 3;
const S_REPLY: u8 = 4;

const MAX_FRAME: usize = 64 * 1024 * 1024;

pub fn write_frame(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut buf = Vec::with_capacity(payload.len() + 5);
    buf.push(kind);
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf)?;
    w.flush()
}

pub fn read_frame(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    read_frame_max(r, MAX_FRAME)
}

fn read_frame_max(r: &mut impl Read, max: usize) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    if len > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((head[0], payload))
}

/// What a client tells the server when it attaches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub cols: u16,
    pub rows: u16,
    pub term: String,
    pub colorterm: String,
    /// The terminal supports the kitty keyboard protocol (needed for key release events).
    pub enhanced: bool,
    /// How the terminal can show images.
    #[serde(default)]
    pub image_mode: ImageMode,
    /// Cell size in pixels (0 when unknown).
    #[serde(default)]
    pub cell_w: u16,
    #[serde(default)]
    pub cell_h: u16,
}

impl Hello {
    /// Describe the terminal this process runs in. `images` should come from `detect_images`.
    pub fn local(cols: u16, rows: u16, enhanced: bool, image_mode: ImageMode) -> Self {
        let (cell_w, cell_h) = cell_size().unwrap_or((0, 0));
        Self {
            protocol: PROTOCOL,
            cols,
            rows,
            term: std::env::var("TERM").unwrap_or_default(),
            colorterm: std::env::var("COLORTERM").unwrap_or_default(),
            enhanced,
            image_mode,
            cell_w,
            cell_h,
        }
    }
}

/// Size of a cell in pixels, if the terminal reports its pixel size.
pub fn cell_size() -> Option<(u16, u16)> {
    let ws = ratatui::crossterm::terminal::window_size().ok()?;
    if ws.width == 0 || ws.height == 0 || ws.columns == 0 || ws.rows == 0 {
        return None;
    }
    Some((ws.width / ws.columns, ws.height / ws.rows))
}

/// Terminals known to support kitty graphics with Unicode placeholders, judged from the
/// environment: kitty and Ghostty.
fn env_supports_placeholders() -> bool {
    let var = |k: &str| std::env::var(k).unwrap_or_default().to_ascii_lowercase();
    matches!(var("TERM").as_str(), "xterm-kitty" | "xterm-ghostty")
        || matches!(var("TERM_PROGRAM").as_str(), "ghostty" | "kitty")
        || std::env::var_os("KITTY_WINDOW_ID").is_some()
}

/// Terminals known to show iTerm2 inline images: iTerm2, WezTerm and VS Code.
fn env_supports_iterm() -> bool {
    let var = |k: &str| std::env::var(k).unwrap_or_default().to_ascii_lowercase();
    matches!(var("TERM_PROGRAM").as_str(), "iterm.app" | "wezterm" | "vscode") || var("LC_TERMINAL") == "iterm2"
}

/// Best guess from the environment alone (for a server before any client attaches).
pub fn env_image_mode() -> ImageMode {
    if env_supports_placeholders() {
        ImageMode::Placeholders
    } else if env_supports_iterm() {
        ImageMode::Iterm
    } else {
        ImageMode::None
    }
}

/// How this terminal can show images: kitty placeholders (a known terminal that answers a kitty
/// graphics query), iTerm2 images (known terminals), or Sixel (advertised in the device
/// attributes). Needs raw mode; reads the answers directly from stdin (up to one second).
pub fn detect_image_mode() -> ImageMode {
    let (kitty_ok, sixel) = query_graphics();
    if kitty_ok && env_supports_placeholders() {
        ImageMode::Placeholders
    } else if env_supports_iterm() {
        ImageMode::Iterm
    } else if sixel {
        ImageMode::Sixel
    } else {
        ImageMode::None
    }
}

/// Windows consoles do not pass the terminal's answers through as raw bytes; judge from the
/// environment only.
#[cfg(windows)]
fn query_graphics() -> (bool, bool) {
    (env_supports_placeholders(), false)
}

/// Ask the terminal for kitty graphics support and its device attributes.
/// Returns (kitty graphics answered OK, Sixel advertised).
#[cfg(unix)]
fn query_graphics() -> (bool, bool) {
    let mut out = io::stdout();
    if out.write_all(b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c").and_then(|_| out.flush()).is_err() {
        return (false, false);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let mut buf = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        let mut pfd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        // SAFETY: polling and reading stdin into a local buffer.
        let ready = unsafe { libc::poll(&mut pfd, 1, left.as_millis() as libc::c_int) };
        if ready <= 0 {
            break;
        }
        let mut chunk = [0u8; 256];
        let n = unsafe { libc::read(0, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n as usize]);
        // The device attributes answer comes last; stop once it is complete.
        if let Some(p) = buf.windows(3).position(|w| w == b"\x1b[?")
            && buf[p..].contains(&b'c')
        {
            break;
        }
    }
    let kitty = buf.windows(9).any(|w| w == b"_Gi=31;OK");
    // Device attributes: ESC [ ? 62 ; 4 ; ... c, where 4 means Sixel.
    let sixel = buf
        .windows(3)
        .position(|w| w == b"\x1b[?")
        .and_then(|p| {
            let rest = &buf[p + 3..];
            let end = rest.iter().position(|b| *b == b'c')?;
            Some(rest[..end].split(|b| *b == b';').any(|param| param == b"4"))
        })
        .unwrap_or(false);
    (kitty, sixel)
}

// ------------------------------------------------------------------ output side

/// Buffers bytes and sends them as one OUTPUT frame (remote) or raw (local) on flush.
/// Messages for a client's writer thread.
enum WriterMsg {
    Frame(u8, Vec<u8>),
}

enum Out {
    /// The local terminal (standalone mode).
    Local(Box<dyn Write + Send>),
    /// A client socket, written by its own thread through a bounded queue.
    Remote(SyncSender<WriterMsg>),
}

/// Buffers bytes and sends them as one OUTPUT frame (remote) or raw (local) on flush.
#[derive(Clone)]
pub struct Sink {
    inner: Arc<Mutex<SinkInner>>,
}

struct SinkInner {
    out: Out,
    buf: Vec<u8>,
    broken: bool,
}

/// Frames waiting for a client. A client that falls this far behind is detached rather than
/// being allowed to slow the server down.
const CLIENT_QUEUE: usize = 256;

impl Sink {
    fn new(out: Out) -> Self {
        Self { inner: Arc::new(Mutex::new(SinkInner { out, buf: Vec::new(), broken: false })) }
    }

    pub fn is_broken(&self) -> bool {
        self.inner.lock().map(|i| i.broken).unwrap_or(true)
    }

    fn send_control(&self, kind: u8, payload: &[u8]) {
        if let Ok(i) = self.inner.lock()
            && let Out::Remote(q) = &i.out
        {
            let _ = q.try_send(WriterMsg::Frame(kind, payload.to_vec()));
        }
    }
}

impl Write for Sink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut i = self.inner.lock().map_err(|_| io::Error::other("poisoned"))?;
        i.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut guard = self.inner.lock().map_err(|_| io::Error::other("poisoned"))?;
        let i = &mut *guard;
        if i.buf.is_empty() || i.broken {
            i.buf.clear();
            return Ok(());
        }
        let buf = std::mem::take(&mut i.buf);
        let ok = match &mut i.out {
            Out::Local(w) => w.write_all(&buf).and_then(|_| w.flush()).is_ok(),
            // Never blocks: a full queue means the client stopped reading.
            Out::Remote(q) => q.try_send(WriterMsg::Frame(S_OUTPUT, buf)).is_ok(),
        };
        if !ok {
            i.broken = true;
        }
        // A vanished client must not crash the server; it is detached by the caller.
        Ok(())
    }
}

/// A crossterm backend whose size comes from the client rather than from a local tty.
pub struct OutBackend {
    inner: CrosstermBackend<Sink>,
    size: Size,
}

impl Backend for OutBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(Position::ORIGIN)
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }
    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }
    fn size(&self) -> io::Result<Size> {
        Ok(self.size)
    }
    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize { columns_rows: self.size, pixels: Size::new(0, 0) })
    }
    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

/// Where the desktop is currently shown: the local terminal or an attached client.
pub struct Display {
    pub terminal: Terminal<OutBackend>,
    pub sink: Sink,
    pub hello: Hello,
    /// Connection id for remote clients (None when local).
    pub conn: Option<u64>,
    /// The client socket, shut down when the display goes away so the client sees EOF.
    stream: Option<Stream>,
}

impl Display {
    fn new(out: Out, hello: Hello, conn: Option<u64>, stream: Option<Stream>) -> io::Result<Self> {
        let sink = Sink::new(out);
        let size = Size::new(hello.cols.max(1), hello.rows.max(1));
        let backend = OutBackend { inner: CrosstermBackend::new(sink.clone()), size };
        let mut terminal = Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Fullscreen })?;
        terminal.clear()?;
        Ok(Self { terminal, sink, hello, conn, stream })
    }

    pub fn local(hello: Hello) -> io::Result<Self> {
        Self::new(Out::Local(Box::new(io::stdout())), hello, None, None)
    }

    #[cfg(windows)]
    pub fn remote(stream: Stream, _hello: Hello, _conn: u64) -> io::Result<Self> {
        match stream {}
    }

    #[cfg(unix)]
    pub fn remote(stream: Stream, hello: Hello, conn: u64) -> io::Result<Self> {
        let mut writer = stream.try_clone()?;
        writer.set_write_timeout(Some(Duration::from_secs(10)))?;
        let (q, rx) = std::sync::mpsc::sync_channel::<WriterMsg>(CLIENT_QUEUE);
        std::thread::Builder::new().name(format!("client-w-{conn}")).spawn(move || {
            // Ends when the display is dropped (queue closed) or the client stops reading.
            for WriterMsg::Frame(kind, bytes) in rx {
                if write_frame(&mut writer, kind, &bytes).is_err() {
                    break;
                }
            }
            let _ = writer.shutdown(std::net::Shutdown::Both);
        })?;
        Self::new(Out::Remote(q), hello, Some(conn), Some(stream))
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.terminal.backend_mut().size = Size::new(cols.max(1), rows.max(1));
        self.hello.cols = cols;
        self.hello.rows = rows;
    }

    /// Bytes that bypass ratatui (clipboard, cursor shape, keyboard modes).
    pub fn send_raw(&mut self, bytes: &[u8]) {
        let mut sink = self.sink.clone();
        let _ = sink.write_all(bytes);
        let _ = sink.flush();
    }

    pub fn detach(&mut self, message: &str) {
        self.send_raw(b"\x1b[0 q");
        self.sink.send_control(S_DETACH, message.as_bytes());
    }

    pub fn exit(&mut self) {
        self.send_raw(b"\x1b[0 q");
        self.sink.send_control(S_EXIT, b"");
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // A healthy client gets its queued frames, then EOF from the writer thread. A stuck one
        // is cut off now, so its writer thread unblocks and the client notices.
        if self.sink.is_broken()
            && let Some(s) = &self.stream
        {
            shutdown(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, 7, b"hello").unwrap();
        write_frame(&mut buf, 2, b"").unwrap();
        let mut r = &buf[..];
        assert_eq!(read_frame(&mut r).unwrap(), (7, b"hello".to_vec()));
        assert_eq!(read_frame(&mut r).unwrap(), (2, Vec::new()));
        assert!(read_frame(&mut r).is_err());
    }

    #[test]
    fn events_serialize() {
        use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
        let ev = Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        let json = serde_json::to_vec(&ev).unwrap();
        assert_eq!(serde_json::from_slice::<Event>(&json).unwrap(), ev);
    }
}

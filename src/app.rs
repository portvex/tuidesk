use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use alacritty_terminal::vte::ansi::{CursorShape, Rgb};
use base64::Engine as _;
use ratatui::crossterm::event::Event;
use ratatui::layout::Rect;

use crate::config::{self, AppEntry, Config, Rule};
use crate::ipc::{Display, Hello};
use crate::keys::KeySpec;
use crate::lua::{LuaCmd, Plugins, WinInfo};
use crate::notify::Notifications;
use crate::session::{self, SavedWindow, Session};
use crate::settings::{Outcome, Settings};
use crate::term::{SpawnOptions, Term, process_cwd};
use crate::theme::{ColorMode, Theme};
use crate::tray::Tray;
use crate::wm::{Layout, Side, Window, Wm};

pub enum AppEvent {
    /// Input from the local terminal (standalone mode).
    Input(Event),
    PtyOutput(u64, Vec<u8>),
    PtyExit(u64),
    /// A client connected to the server.
    Attach {
        conn: u64,
        stream: crate::ipc::Stream,
        hello: Hello,
    },
    ClientInput(u64, Event),
    ClientGone(u64),
    /// The client's cell size in pixels changed (font size, display).
    CellSize(u64, (u16, u16)),
    /// `tuidesk kill`: save and shut the server down.
    Kill,
    /// `tuidesk msg ...`: a command from a script, answered through `reply`.
    Msg {
        args: Vec<String>,
        reply: Sender<Result<String, String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Kickoff,
    SpawnShell,
    Spawn(String),
    Launch(String),
    Close,
    Maximize,
    Minimize,
    FocusNext,
    FocusPrev,
    Switcher,
    SwitcherPrev,
    Tile(Side),
    ToggleLayout,
    ToggleFloating,
    SwapMaster,
    WindowMode,
    ScrollUp,
    ScrollDown,
    Desktop(usize),
    DesktopNext,
    DesktopPrev,
    MoveToDesktop(usize),
    MoveNextDesktop,
    MovePrevDesktop,
    Paste,
    CopyMode,
    Clipboard,
    Help,
    WindowMenu,
    ToggleAbove,
    Rename,
    CycleLayout,
    SetLayout(Layout),
    Runner,
    RenameDesktop,
    Overview,
    Screenshot(ShotTarget),
    Notifications,
    Settings,
    Detach,
    Reload,
    Quit,
    Lua(String),
}

impl Action {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (name, arg) = match s.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (s, ""),
        };
        let needs_arg = |f: fn(String) -> Action| (!arg.is_empty()).then(|| f(arg.to_string()));
        // Desktops are numbered from 1 in config and Lua.
        let desktop = |f: fn(usize) -> Action| arg.parse::<usize>().ok().filter(|&n| n >= 1).map(|n| f(n - 1));
        Some(match name {
            "kickoff" | "menu" => Self::Kickoff,
            "spawn_shell" | "terminal" => Self::SpawnShell,
            "spawn" => return needs_arg(Self::Spawn),
            "launch" => return needs_arg(Self::Launch),
            "lua" => return needs_arg(Self::Lua),
            "desktop" => return desktop(Self::Desktop),
            "layout" => return Layout::parse(arg).map(Self::SetLayout),
            "move_to_desktop" => return desktop(Self::MoveToDesktop),
            "close" => Self::Close,
            "maximize" => Self::Maximize,
            "minimize" => Self::Minimize,
            "focus_next" => Self::FocusNext,
            "focus_prev" => Self::FocusPrev,
            "switcher" => Self::Switcher,
            "switcher_prev" => Self::SwitcherPrev,
            "tile_left" => Self::Tile(Side::Left),
            "tile_right" => Self::Tile(Side::Right),
            "tile_top" => Self::Tile(Side::Top),
            "tile_bottom" => Self::Tile(Side::Bottom),
            "toggle_layout" => Self::ToggleLayout,
            "toggle_floating" => Self::ToggleFloating,
            "swap_master" => Self::SwapMaster,
            "window_mode" => Self::WindowMode,
            "scroll_up" => Self::ScrollUp,
            "scroll_down" => Self::ScrollDown,
            "desktop_next" => Self::DesktopNext,
            "desktop_prev" => Self::DesktopPrev,
            "move_next_desktop" => Self::MoveNextDesktop,
            "move_prev_desktop" => Self::MovePrevDesktop,
            "paste" => Self::Paste,
            "copy_mode" => Self::CopyMode,
            "clipboard" => Self::Clipboard,
            "help" => Self::Help,
            "window_menu" => Self::WindowMenu,
            "toggle_above" => Self::ToggleAbove,
            "rename" => Self::Rename,
            "rename_desktop" => Self::RenameDesktop,
            "runner" | "krunner" => Self::Runner,
            "overview" => Self::Overview,
            "screenshot" => Self::Screenshot(match arg {
                "" | "screen" => ShotTarget::Screen,
                "window" => ShotTarget::Window,
                "region" => ShotTarget::Region,
                _ => return None,
            }),
            "cycle_layout" => Self::CycleLayout,
            "notifications" => Self::Notifications,
            "settings" | "system_settings" => Self::Settings,
            "detach" => Self::Detach,
            "reload" => Self::Reload,
            "quit" => Self::Quit,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Prefix,
    Move,
    Kickoff,
    History,
    Switcher,
    /// Keyboard selection and search in a window's history.
    Copy,
    /// Clipboard history picker.
    Clipboard,
    /// Key binding cheat sheet.
    Help,
    /// Per-window operations menu.
    WindowMenu,
    /// Typing a new window or desktop name.
    Rename,
    /// KRunner-style search and launch.
    Runner,
    /// Every window on every desktop as a preview card.
    Overview,
    /// Dragging out a rectangle to capture.
    Region,
    /// The Settings dialog.
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotTarget {
    Screen,
    /// The focused window, decorations included.
    Window,
    /// A rectangle chosen with the mouse.
    Region,
}

/// Rectangle being selected for a region screenshot: (start, current) corners.
#[derive(Default)]
pub struct RegionState {
    pub start: Option<(u16, u16)>,
    pub end: (u16, u16),
}

impl RegionState {
    pub fn rect(&self) -> Option<Rect> {
        let (sx, sy) = self.start?;
        let (ex, ey) = self.end;
        Some(Rect::new(sx.min(ex), sy.min(ey), sx.abs_diff(ex) + 1, sy.abs_diff(ey) + 1))
    }
}

#[derive(Default)]
pub struct OverviewState {
    pub filter: String,
    /// Index into `App::overview_windows()`.
    pub sel: usize,
    /// Cards per row in the last drawn grid, for up/down movement.
    pub cols: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameTarget {
    Window(u64),
    Desktop(usize),
}

#[derive(Default)]
pub struct RunnerState {
    pub query: String,
    pub sel: usize,
    /// Directory that "./" and "../" queries start from (the focused window's).
    pub base: Option<PathBuf>,
    /// Folder results for the last query, so typing and redraws don't rescan the disk.
    pub dirs: std::cell::RefCell<Option<(String, Vec<PathBuf>)>>,
}

pub struct CopyState {
    pub id: u64,
    /// Pattern being typed after / or ?, and whether it searches forward.
    pub search: Option<(String, bool)>,
    pub last_search: Option<(String, bool)>,
    pub message: String,
}

#[derive(Default)]
pub struct HelpState {
    pub filter: String,
    pub scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinMenuItem {
    Minimize,
    Maximize,
    Float,
    Above,
    MoveTo(usize),
    Rename,
    Close,
}

pub struct WinMenu {
    pub id: u64,
    pub x: u16,
    pub y: u16,
    pub items: Vec<WinMenuItem>,
    pub sel: usize,
}

/// How deeply Lua-triggered actions may nest before further requests are dropped.
const MAX_LUA_DEPTH: usize = 4;

/// How long the desktop switch label stays on screen.
const OSD_TIME: Duration = Duration::from_millis(900);

/// Number of entries kept in the clipboard history.
pub const CLIP_HISTORY: usize = 20;

#[derive(Debug, Clone)]
pub enum MenuItem {
    App(usize),
    Run(String),
    Settings,
    Reload,
    Detach,
    Quit,
}

#[derive(Default)]
pub struct Kickoff {
    pub query: String,
    pub category: usize,
    pub selected: usize,
    pub scroll: usize,
    /// Item rows that fit in the menu, as last drawn.
    pub rows: usize,
    /// First category tab shown, when the tabs are wider than the menu.
    pub tab_scroll: usize,
}

impl Kickoff {
    /// Scroll the list to `scroll` and keep the selection on screen, or drawing would scroll
    /// back to it.
    pub fn scroll_to(&mut self, scroll: usize, items: usize) {
        self.scroll = scroll.min(items.saturating_sub(self.rows));
        self.selected = self.selected.clamp(self.scroll, (self.scroll + self.rows).saturating_sub(1).max(self.scroll));
    }
}

/// Alt+Tab window switcher state.
pub struct Switcher {
    pub list: Vec<u64>,
    pub idx: usize,
    pub last: Instant,
    /// Key release events are available, so releasing Alt commits.
    pub releases: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelHit {
    Kickoff,
    Task(u64),
    Layout,
    Bell,
    Desktop(usize),
}

/// Clickable regions recorded while drawing overlays.
#[derive(Default)]
pub struct Hits {
    pub panel: Vec<(Rect, PanelHit)>,
    pub menu: Option<Rect>,
    pub menu_items: Vec<(Rect, usize)>,
    pub menu_tabs: Vec<(Rect, usize)>,
    /// Kickoff's scrollbar, when the list is longer than the menu.
    pub menu_scrollbar: Option<Rect>,
    pub history: Option<Rect>,
    pub popups: Vec<(Rect, usize)>,
    pub switcher: Vec<(Rect, usize)>,
    pub list: Vec<(Rect, usize)>,
    pub overlay: Option<Rect>,
    /// Overview window cards and desktop buttons.
    pub cards: Vec<(Rect, u64)>,
    pub desk_buttons: Vec<(Rect, usize)>,
    /// Desktop widgets drawn on the wallpaper, by index into the config.
    pub widgets: Vec<(Rect, usize)>,
}

/// A window to open: new, or restored from a session.
pub struct SpawnRequest {
    pub command: Option<String>,
    pub name: String,
    pub cwd: Option<PathBuf>,
    pub restore: Option<SavedWindow>,
}

pub struct App {
    pub cfg: Config,
    pub(crate) config_dir: PathBuf,
    pub session_name: String,
    pub server: bool,
    pub theme: Theme,
    pub wm: Wm,
    pub tray: Tray,
    pub notes: Notifications,
    pub plugins: Plugins,
    pub apps: Vec<AppEntry>,
    pub mode: Mode,
    pub kickoff: Kickoff,
    pub switcher: Option<Switcher>,
    pub hits: Hits,
    pub(crate) prefix_key: KeySpec,
    pub(crate) global_keys: HashMap<KeySpec, Action>,
    pub(crate) prefix_keys: HashMap<KeySpec, Action>,
    pub(crate) drag: Option<crate::input::Drag>,
    /// Window whose program received a mouse press; it gets the drag and release too.
    pub(crate) mouse_grab: Option<u64>,
    lua_depth: usize,
    /// (time, x, y, click count) of the last left click, for double and triple clicks.
    pub(crate) last_click: Option<(Instant, u16, u16, u8)>,
    /// Text copied from a selection, pasted with middle-click or the paste action.
    pub(crate) clipboard: String,
    /// Recent copies, newest first.
    pub clips: VecDeque<String>,
    pub clip_sel: usize,
    pub copy: Option<CopyState>,
    pub help: HelpState,
    pub settings: Option<Settings>,
    pub win_menu: Option<WinMenu>,
    pub rename: Option<(RenameTarget, String)>,
    pub runner: RunnerState,
    /// Desktop names; empty entries show the number.
    pub desktop_names: Vec<String>,
    /// Short on-screen message (desktop switches), with the time it appeared.
    pub osd: Option<(String, Instant)>,
    last_desktop: usize,
    /// Pager desktop under the pointer while dragging a window or task onto it.
    pub drop_target: Option<usize>,
    pub overview: OverviewState,
    pub region: RegionState,
    /// Set while drawing a screenshot, so tuidesk's own screenshot popups are left out.
    pub capturing: bool,
    /// Mirror of the windows' images on the host terminal.
    pub host_gfx: crate::graphics::HostGraphics,
    /// How the host terminal shows images, as its client detected.
    host_images: crate::graphics::direct::ImageMode,
    /// Images drawn directly (iTerm2, Sixel): visible pieces from the last frame...
    pub regions: Vec<crate::graphics::direct::Region>,
    /// ...and what is currently on the host screen (with image generations).
    regions_shown: Vec<(crate::graphics::direct::Region, u64)>,
    /// The image protocol `regions_shown` were drawn with.
    shown_mode: crate::graphics::direct::ImageMode,
    scaled: crate::graphics::direct::ScaledCache,
    /// Title bar buttons as icons, sent to the host like a window's images.
    pub button_icons: crate::graphics::icons::ButtonIcons,
    /// Host cell size in pixels.
    cell_px: (u16, u16),
    /// Files shown on the desktop (wallpaper art, notes): path to (checked at, mtime, lines).
    files: HashMap<PathBuf, (Instant, Option<SystemTime>, Vec<String>)>,
    pub(crate) tx: Sender<AppEvent>,
    /// Bytes for the host terminal that bypass ratatui (OSC 52, cursor shape, key modes).
    pub(crate) host_out: Vec<u8>,
    pub display: Option<Display>,
    cursor_shape: Option<CursorShape>,
    mtimes: Vec<Option<SystemTime>>,
    mtimes_paths: Vec<PathBuf>,
    pub screen: Rect,
    last_focus: Option<u64>,
    last_save: Instant,
    pub(crate) quit: bool,
    pub dirty: bool,
}

fn mtime(p: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn to_rgb(c: ratatui::style::Color, fallback: (u8, u8, u8)) -> Rgb {
    let (r, g, b) = match c {
        ratatui::style::Color::Rgb(r, g, b) => (r, g, b),
        ratatui::style::Color::Indexed(i) => crate::theme::index_to_rgb(i),
        _ => fallback,
    };
    Rgb { r, g, b }
}

/// Split the screen into (desktop, panel).
pub fn screen_layout(screen: Rect, panel_top: bool) -> (Rect, Rect) {
    if screen.height < 2 {
        return (screen, Rect::new(screen.x, screen.y, screen.width, 0));
    }
    let desktop_h = screen.height - 1;
    if panel_top {
        (Rect::new(screen.x, screen.y + 1, screen.width, desktop_h), Rect::new(screen.x, screen.y, screen.width, 1))
    } else {
        (
            Rect::new(screen.x, screen.y, screen.width, desktop_h),
            Rect::new(screen.x, screen.bottom() - 1, screen.width, 1),
        )
    }
}

impl App {
    pub fn new(
        config_dir: PathBuf,
        session_name: String,
        tx: Sender<AppEvent>,
        display: Option<Display>,
        server: bool,
        size: (u16, u16),
    ) -> Self {
        let (cfg, err) = match config::load(&config_dir.join("config.toml")) {
            Ok(c) => (c, None),
            Err(e) => (config::load(std::path::Path::new("")).expect("default config"), Some(format!("{e:#}"))),
        };
        let screen = Rect::new(0, 0, size.0.max(20), size.1.max(5));
        let layout = Layout::parse(&cfg.general.layout).unwrap_or(Layout::Floating);
        let wm = Wm::new(cfg.general.desktops, layout, cfg.general.master_ratio, cfg.general.gaps);
        let plugins = Plugins::load(&config_dir);
        let (theme, _) = Theme::build(&cfg.theme.preset, &cfg.theme.overrides, ColorMode::TrueColor);
        let paths = vec![config_dir.join("config.toml"), config_dir.join("init.lua")];
        let mut app = Self {
            theme,
            wm,
            tray: Tray::new(),
            notes: Notifications::default(),
            plugins,
            apps: Vec::new(),
            mode: Mode::Normal,
            kickoff: Kickoff::default(),
            switcher: None,
            hits: Hits::default(),
            prefix_key: KeySpec::parse("ctrl+space").unwrap(),
            global_keys: HashMap::new(),
            prefix_keys: HashMap::new(),
            drag: None,
            mouse_grab: None,
            lua_depth: 0,
            last_click: None,
            clipboard: String::new(),
            clips: VecDeque::new(),
            clip_sel: 0,
            copy: None,
            help: HelpState::default(),
            settings: None,
            win_menu: None,
            rename: None,
            runner: RunnerState::default(),
            desktop_names: Vec::new(),
            osd: None,
            last_desktop: 0,
            drop_target: None,
            overview: OverviewState::default(),
            region: RegionState::default(),
            capturing: false,
            host_gfx: Default::default(),
            // Until a client says otherwise, assume the terminal that started us.
            host_images: crate::ipc::env_image_mode(),
            regions: Vec::new(),
            regions_shown: Vec::new(),
            shown_mode: crate::graphics::direct::ImageMode::None,
            button_icons: Default::default(),
            scaled: Default::default(),
            cell_px: (8, 16),
            files: HashMap::new(),
            tx,
            host_out: Vec::new(),
            display,
            cursor_shape: None,
            mtimes: paths.iter().map(mtime).collect(),
            mtimes_paths: paths,
            screen,
            last_focus: None,
            last_save: Instant::now(),
            quit: false,
            dirty: true,
            session_name,
            server,
            cfg,
            config_dir,
        };
        if let Some(h) = app.display.as_ref().map(|d| d.hello.clone()) {
            app.host_images = h.image_mode;
            if h.cell_w > 0 && h.cell_h > 0 {
                app.cell_px = (h.cell_w, h.cell_h);
            }
        }
        let warnings = app.apply_config();
        if let Some(e) = err {
            app.notify("Config error", e);
        }
        for w in warnings {
            app.notify("Config warning", w);
        }
        app.report_lua_errors();
        app
    }

    fn color_mode(&self) -> ColorMode {
        match &self.display {
            Some(d) => ColorMode::resolve(&self.cfg.general.color_mode, &d.hello.term, &d.hello.colorterm),
            None => ColorMode::TrueColor,
        }
    }

    /// Recompute everything derived from `cfg`. Returns warnings.
    fn apply_config(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        let (theme, w) = Theme::build(&self.cfg.theme.preset, &self.cfg.theme.overrides, self.color_mode());
        self.theme = theme;
        warnings.extend(w);

        match KeySpec::parse(&self.cfg.general.prefix) {
            Some(k) => self.prefix_key = k,
            None => warnings.push(format!("bad prefix key '{}'", self.cfg.general.prefix)),
        }
        let build = |map: &HashMap<String, String>, warnings: &mut Vec<String>| {
            let mut out = HashMap::new();
            for (k, v) in map {
                // "none" turns off a default binding.
                if v.trim() == "none" {
                    continue;
                }
                match (KeySpec::parse(k), Action::parse(v)) {
                    (Some(k), Some(a)) => {
                        out.insert(k, a);
                    }
                    (None, _) => warnings.push(format!("bad key '{k}'")),
                    (_, None) => warnings.push(format!("unknown action '{v}' for key '{k}'")),
                }
            }
            out
        };
        if Layout::parse(&self.cfg.general.layout).is_none() {
            warnings.push(format!(
                "unknown layout '{}' (use floating, master, monocle, grid, columns or bsp)",
                self.cfg.general.layout
            ));
        }
        self.global_keys = build(&self.cfg.keys.global, &mut warnings);
        self.prefix_keys = build(&self.cfg.keys.prefix, &mut warnings);
        self.apps = self.cfg.visible_apps();
        self.wm.gaps = self.cfg.general.gaps;
        self.wm.set_desktops(self.cfg.general.desktops);
        // Names from the config win; names given at runtime stay for the other desktops.
        self.desktop_names.resize(self.wm.desktops, String::new());
        for (i, name) in self.cfg.general.desktop_names.iter().enumerate().take(self.wm.desktops) {
            if !name.is_empty() {
                self.desktop_names[i] = name.clone();
            }
        }
        let (area, _) = screen_layout(self.screen, self.panel_top());
        self.wm.set_area(area);
        self.update_graphics();
        warnings
    }

    /// Windows shown in the overview: all desktops, current first, filtered by the typed text.
    pub fn overview_windows(&self) -> Vec<u64> {
        let f = self.overview.filter.to_lowercase();
        let mut ids: Vec<(usize, usize, u64)> = self
            .wm
            .order
            .iter()
            .enumerate()
            .filter_map(|(i, &id)| self.wm.get(id).map(|w| (i, w)))
            .filter(|(_, w)| {
                f.is_empty()
                    || w.title().to_lowercase().contains(&f)
                    || w.command.to_lowercase().contains(&f)
                    || self.desktop_label(w.desktop).to_lowercase().contains(&f)
            })
            .map(|(i, w)| {
                // Current desktop first, then the others in order.
                let rank = (w.desktop + self.wm.desktops - self.wm.desktop) % self.wm.desktops;
                (rank, i, w.id)
            })
            .collect();
        ids.sort();
        ids.into_iter().map(|(_, _, id)| id).collect()
    }

    /// Lines of a text file shown on the desktop, re-read when it changes (checked every 2s).
    pub fn file_lines(&mut self, path: &str) -> Vec<String> {
        let path = config::expand_tilde(path);
        let now = Instant::now();
        if let Some((checked, _, lines)) = self.files.get(&path)
            && now.duration_since(*checked) < Duration::from_secs(2)
        {
            return lines.clone();
        }
        let modified = mtime(&path);
        if let Some(entry) = self.files.get_mut(&path)
            && entry.1 == modified
        {
            entry.0 = now;
            return entry.2.clone();
        }
        let lines: Vec<String> = match std::fs::read_to_string(&path) {
            Ok(text) => text.lines().map(|l| l.replace('\t', "    ")).collect(),
            Err(e) => vec![format!("cannot read {}: {e}", path.display())],
        };
        self.files.insert(path, (now, modified, lines.clone()));
        lines
    }

    /// Capture `area` of the screen as it looks without overlays, save it, and copy the path or
    /// text as configured. Returns the saved path.
    pub fn screenshot(
        &mut self,
        area: Rect,
        format: Option<crate::screenshot::Format>,
        path: Option<PathBuf>,
    ) -> Result<PathBuf, String> {
        use crate::screenshot::{self as shot, Format};
        let result = (|| -> Result<(PathBuf, String), String> {
            let format = match format {
                Some(f) => f,
                None => Format::parse(&self.cfg.screenshot.format)
                    .ok_or_else(|| format!("unknown screenshot format '{}'", self.cfg.screenshot.format))?,
            };
            // Draw a clean frame offscreen, so this works with or without a client attached.
            let mode = std::mem::replace(&mut self.mode, Mode::Normal);
            self.capturing = true;
            let backend = ratatui::backend::TestBackend::new(self.screen.width, self.screen.height);
            let mut term = ratatui::Terminal::new(backend).map_err(|e| e.to_string())?;
            let drawn = term.draw(|f| crate::ui::draw(f, self)).map(|_| ());
            self.mode = mode;
            self.capturing = false;
            drawn.map_err(|e| e.to_string())?;
            let buf = term.backend().buffer().clone();
            let defaults =
                (shot::rgb(self.theme.term_fg, (252, 252, 252)), shot::rgb(self.theme.wallpaper, (27, 43, 58)));
            let stamp = chrono::Local::now();
            let title = format!("tuidesk {}", stamp.format("%Y-%m-%d %H:%M:%S"));
            let content = shot::render(&buf, area, format, defaults, &title);
            let path = match path {
                Some(p) => p,
                None => {
                    let dir = config::expand_tilde(&self.cfg.screenshot.dir);
                    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                    // Milliseconds keep names unique and sorted by time; never overwrite anyway.
                    let base = format!("tuidesk-{}", stamp.format("%Y%m%d-%H%M%S-%3f"));
                    let ext = format.extension();
                    (1..)
                        .map(|n| {
                            if n == 1 {
                                dir.join(format!("{base}.{ext}"))
                            } else {
                                dir.join(format!("{base}-{n}.{ext}"))
                            }
                        })
                        .find(|p| !p.exists())
                        .unwrap_or_else(|| dir.join(format!("{base}.{ext}")))
                }
            };
            std::fs::write(&path, content).map_err(|e| format!("{}: {e}", path.display()))?;
            let text = shot::render(&buf, area, Format::Text, defaults, &title);
            Ok((path, text))
        })();
        self.dirty = true;
        match result {
            Ok((path, text)) => {
                match self.cfg.screenshot.copy.as_str() {
                    "path" => self.copy(path.display().to_string()),
                    "text" => self.copy(text),
                    _ => {}
                }
                self.notify("Screenshot saved", path.display().to_string());
                Ok(path)
            }
            Err(e) => {
                self.notify("Screenshot failed", e.clone());
                Err(e)
            }
        }
    }

    /// "Code" for a named desktop, "3" otherwise (0-based `d`).
    pub fn desktop_label(&self, d: usize) -> String {
        match self.desktop_names.get(d) {
            Some(n) if !n.is_empty() => n.clone(),
            _ => (d + 1).to_string(),
        }
    }

    pub fn panel_top(&self) -> bool {
        self.cfg.panel.position == "top"
    }

    pub fn prefix_label(&self) -> &str {
        &self.cfg.general.prefix
    }

    /// Switch to a new configuration. Returns warnings.
    fn set_config(&mut self, cfg: Config) -> Vec<String> {
        let layout_changed = cfg.general.layout != self.cfg.general.layout;
        let ratio_changed = cfg.general.master_ratio != self.cfg.general.master_ratio;
        self.cfg = cfg;
        if layout_changed && let Some(l) = Layout::parse(&self.cfg.general.layout) {
            self.wm.set_layout(l);
        }
        if ratio_changed {
            self.wm.set_master_ratio(self.cfg.general.master_ratio);
        }
        self.apply_config()
    }

    pub(crate) fn open_settings(&mut self) {
        match Settings::open(self.config_dir.join("config.toml")) {
            Ok(s) => {
                self.settings = Some(s);
                self.mode = Mode::Settings;
            }
            Err(e) => self.notify("Settings", format!("cannot edit the configuration: {e}")),
        }
    }

    /// Act on what the Settings dialog asked for.
    pub(crate) fn settings_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::None => {}
            Outcome::Changed(cfg) => {
                let warnings = self.set_config(*cfg);
                if let Some(s) = self.settings.as_mut()
                    && !warnings.is_empty()
                {
                    s.status = warnings.join("; ");
                }
            }
            Outcome::Save | Outcome::SaveAndClose => {
                let Some(s) = self.settings.as_mut() else { return };
                match s.save() {
                    Ok(()) => {
                        let path = s.path.display().to_string();
                        // Already in effect: don't reload it as an outside change.
                        self.mtimes = self.mtimes_paths.iter().map(mtime).collect();
                        self.notify("Settings saved", path);
                        if matches!(outcome, Outcome::SaveAndClose) {
                            self.settings = None;
                            self.mode = Mode::Normal;
                        }
                    }
                    Err(e) => s.status = e,
                }
            }
            Outcome::Close => {
                if let Some(s) = self.settings.take()
                    && s.modified()
                {
                    // Back to the file as it was.
                    match config::parse(s.saved_text()) {
                        Ok(cfg) => {
                            self.set_config(cfg);
                        }
                        Err(_) => self.reload(),
                    }
                }
                self.mode = Mode::Normal;
            }
        }
        self.dirty = true;
    }

    pub(crate) fn reload(&mut self) {
        match config::load(&self.config_dir.join("config.toml")) {
            Ok(cfg) => {
                let warnings = self.set_config(cfg);
                self.plugins = Plugins::load(&self.config_dir);
                self.report_lua_errors();
                if warnings.is_empty() {
                    self.notify("tuidesk", "Configuration reloaded");
                }
                for w in warnings {
                    self.notify("Config warning", w);
                }
            }
            Err(e) => self.notify("Config error", format!("{e:#}")),
        }
    }

    pub fn notify(&mut self, title: impl Into<String>, body: impl Into<String>) {
        let t = Duration::from_secs(self.cfg.general.notification_timeout);
        self.notes.push(title, body, t);
        self.dirty = true;
    }

    // ---------------------------------------------------------------- Lua glue

    fn win_info(&self, w: &Window) -> WinInfo {
        WinInfo {
            id: w.id,
            title: w.title(),
            command: w.command.clone(),
            focused: self.wm.focused == Some(w.id),
            minimized: w.minimized,
            exit_code: w.exit_code,
            desktop: w.desktop + 1,
        }
    }

    pub(crate) fn sync_lua(&self) {
        self.plugins.set_windows(self.wm.windows.iter().map(|w| self.win_info(w)).collect());
    }

    fn report_lua_errors(&mut self) {
        for e in std::mem::take(&mut self.plugins.errors) {
            self.notify("Lua error", e);
        }
    }

    /// Execute requests queued by Lua code. Bounded so scripts cannot loop forever.
    pub(crate) fn drain_lua(&mut self) {
        // Lua handlers can trigger actions that run more Lua (an action calling itself, a
        // window_open handler that opens windows). Stop the chain instead of overflowing the stack.
        if self.lua_depth >= MAX_LUA_DEPTH {
            let dropped = self.plugins.take_cmds().len();
            if dropped > 0 {
                self.notify("Lua error", format!("nested too deep; dropped {dropped} request(s)"));
            }
            return;
        }
        self.lua_depth += 1;
        for _ in 0..16 {
            let cmds = self.plugins.take_cmds();
            if cmds.is_empty() {
                break;
            }
            for cmd in cmds {
                match cmd {
                    LuaCmd::Notify(t, b, timeout) => {
                        let timeout = Duration::from_secs(timeout.unwrap_or(self.cfg.general.notification_timeout));
                        self.notes.push(t, b, timeout);
                        self.dirty = true;
                    }
                    LuaCmd::Spawn(c) => {
                        self.spawn_command(c);
                    }
                    LuaCmd::Action(a) => match Action::parse(&a) {
                        Some(a) => self.run_action(a),
                        None => self.notify("Lua error", format!("unknown action '{a}'")),
                    },
                    LuaCmd::Focus(id) => self.wm.focus(id),
                    LuaCmd::Close(id) => self.close_window(id),
                }
            }
        }
        self.lua_depth -= 1;
        self.report_lua_errors();
        self.dirty = true;
    }

    fn emit(&mut self, event: &str, win: Option<WinInfo>) {
        self.sync_lua();
        self.plugins.emit(event, win.as_ref());
        self.drain_lua();
    }

    // ---------------------------------------------------------------- windows

    /// Directory new windows start in: the focused window's, when configured.
    pub(crate) fn new_window_cwd(&self) -> Option<PathBuf> {
        if !self.cfg.general.inherit_cwd {
            return None;
        }
        self.wm.focused_window().and_then(|w| w.term.pid()).and_then(process_cwd)
    }

    pub fn spawn_command(&mut self, command: String) -> Option<u64> {
        let name = config::command_name(&command);
        let cwd = self.new_window_cwd();
        self.spawn(SpawnRequest { command: Some(command), name, cwd, restore: None })
    }

    pub fn spawn_shell(&mut self) {
        let shell = self.cfg.shell();
        let cwd = self.new_window_cwd();
        self.spawn(SpawnRequest { command: None, name: config::command_name(&shell), cwd, restore: None });
    }

    /// Open a window. Returns its id, or None if the program could not be started.
    pub fn spawn(&mut self, req: SpawnRequest) -> Option<u64> {
        let shell = self.cfg.shell();
        let cmd_str = req.command.clone().unwrap_or_else(|| shell.clone());
        let mut rules: Vec<Rule> = self.cfg.rules.clone();
        rules.extend(self.plugins.rules());
        let rules: Vec<Rule> = rules.into_iter().filter(|r| r.matches(&cmd_str, &req.name)).collect();

        let rect = if let Some(saved) = &req.restore {
            let [x, y, w, h] = saved.rect;
            Rect::new(x, y, w, h)
        } else {
            let width = rules.iter().rev().find_map(|r| r.width).unwrap_or(self.cfg.general.window_width);
            let height = rules.iter().rev().find_map(|r| r.height).unwrap_or(self.cfg.general.window_height);
            let mut rect = self.wm.place(width, height);
            if let Some(x) = rules.iter().rev().find_map(|r| r.x) {
                rect.x = self.wm.area.x.saturating_add(x);
            }
            if let Some(y) = rules.iter().rev().find_map(|r| r.y) {
                rect.y = self.wm.area.y.saturating_add(y);
            }
            rect
        };
        let rect = crate::wm::clamp_rect(rect, self.wm.area);
        let id = self.wm.alloc_id();
        let content = crate::wm::content_rect(rect);
        let opts = SpawnOptions {
            id,
            command: req.command.as_deref(),
            shell: &shell,
            cwd: req.cwd.clone(),
            rows: content.height.max(1),
            cols: content.width.max(1),
            scrollback: self.cfg.general.scrollback,
            colors: (to_rgb(self.theme.term_fg, (252, 252, 252)), to_rgb(self.theme.term_bg, (20, 22, 24))),
            // Lets `tuidesk msg` inside a window reach this session.
            env: vec![("TUIDESK_SESSION".into(), self.session_name.clone())],
            cell_px: self.cell_px,
            graphics: self.graphics_enabled(),
        };
        let term = match Term::spawn(opts, self.tx.clone()) {
            Ok(t) => t,
            Err(e) => {
                self.notify("Failed to start", format!("{cmd_str}: {e:#}"));
                return None;
            }
        };
        let last = self.wm.desktops - 1;
        let (maximized, minimized, floating, desktop, above, title_override) = match &req.restore {
            Some(s) => (s.maximized, s.minimized, s.floating, s.desktop.min(last), s.above, s.title.clone()),
            None => (
                rules.iter().rev().find_map(|r| r.maximized).unwrap_or(false),
                rules.iter().rev().find_map(|r| r.minimized).unwrap_or(false),
                rules.iter().rev().find_map(|r| r.floating).unwrap_or(false),
                rules
                    .iter()
                    .rev()
                    .find_map(|r| r.desktop)
                    .map(|d| d.saturating_sub(1).min(last))
                    .unwrap_or(self.wm.desktop),
                rules.iter().rev().find_map(|r| r.above).unwrap_or(false),
                None,
            ),
        };
        let win = Window {
            id,
            term,
            command: cmd_str,
            launch: req.command,
            name: req.name,
            float_rect: rect,
            restore_rect: None,
            frame: rect,
            maximized,
            minimized: false,
            floating,
            urgent: false,
            exit_code: None,
            desktop,
            above,
            title_override,
        };
        let on_other_desktop = desktop != self.wm.desktop;
        self.wm.windows.push(win);
        self.wm.order.push(id);
        if minimized {
            self.wm.minimize(id);
        } else if on_other_desktop {
            self.wm.mru.push(id);
        } else {
            self.wm.focus(id);
        }
        self.wm.arrange();
        self.dirty = true;
        let info = self.wm.get(id).map(|w| self.win_info(w));
        self.emit("window_open", info);
        Some(id)
    }

    pub(crate) fn close_window(&mut self, id: u64) {
        if let Some(s) = &mut self.switcher {
            if let Some(i) = s.list.iter().position(|&w| w == id) {
                s.list.remove(i);
                if s.idx > i || s.idx >= s.list.len() {
                    s.idx = s.idx.saturating_sub(1);
                }
            }
            if s.list.len() < 2 {
                self.close_switcher(false);
            }
        }
        // Leave modes that were working on this window.
        if self.copy.as_ref().is_some_and(|c| c.id == id) {
            self.copy = None;
            self.mode = Mode::Normal;
        }
        if self.win_menu.as_ref().is_some_and(|m| m.id == id)
            || self.rename.as_ref().is_some_and(|r| r.0 == RenameTarget::Window(id))
        {
            self.win_menu = None;
            self.rename = None;
            self.mode = Mode::Normal;
        }
        if let Some(w) = self.wm.remove(id) {
            let info = self.win_info(&w);
            drop(w);
            self.emit("window_close", Some(info));
        }
        self.dirty = true;
    }

    // ---------------------------------------------------------------- sessions

    pub fn snapshot(&self) -> Session {
        let windows = self
            .wm
            .order
            .iter()
            .filter_map(|&id| self.wm.get(id))
            .map(|w| SavedWindow {
                command: w.launch.clone(),
                name: w.name.clone(),
                cwd: w.term.pid().and_then(process_cwd),
                rect: [w.float_rect.x, w.float_rect.y, w.float_rect.width, w.float_rect.height],
                maximized: w.maximized,
                minimized: w.minimized,
                floating: w.floating,
                desktop: w.desktop,
                focused: self.wm.focused == Some(w.id),
                above: w.above,
                title: w.title_override.clone(),
            })
            .collect();
        let desktops =
            (0..self.wm.desktops).map(|d| (self.wm.layout_of(d).name().to_string(), self.wm.ratio_of(d))).collect();
        Session {
            version: session::VERSION,
            desktop: self.wm.desktop,
            desktops,
            windows,
            desktop_names: self.desktop_names.clone(),
        }
    }

    pub fn save_session(&mut self) {
        self.last_save = Instant::now();
        if !self.cfg.general.restore_session {
            return;
        }
        if let Err(e) = session::save(&session::path_for(&self.session_name), &self.snapshot()) {
            self.notify("Session", format!("could not save: {e:#}"));
        }
    }

    /// Reopen windows from the last session. Returns true if any were restored.
    fn restore_session(&mut self) -> bool {
        if !self.cfg.general.restore_session {
            return false;
        }
        let path = session::path_for(&self.session_name);
        let s = match session::load(&path) {
            Ok(Some(s)) => s,
            Ok(None) => return false,
            Err(e) => {
                // Keep the old file: the next autosave would otherwise replace it.
                let kept = session::back_up(&path).map(|b| format!(" (kept as {})", b.display())).unwrap_or_default();
                self.notify("Session", format!("could not restore: {e:#}{kept}"));
                return false;
            }
        };
        for (d, name) in s.desktop_names.iter().enumerate().take(self.desktop_names.len()) {
            // The config's names take precedence over saved ones.
            if self.cfg.general.desktop_names.get(d).is_none_or(|n| n.is_empty()) {
                self.desktop_names[d] = name.clone();
            }
        }
        for (d, (layout, ratio)) in s.desktops.iter().enumerate() {
            self.wm.set_desktop_settings(d, Layout::parse(layout).unwrap_or(Layout::Floating), *ratio);
        }
        let mut focus = None;
        for saved in &s.windows {
            let before = self.wm.windows.len();
            self.spawn(SpawnRequest {
                command: saved.command.clone(),
                name: saved.name.clone(),
                cwd: saved.cwd.clone(),
                restore: Some(saved.clone()),
            });
            if saved.focused && self.wm.windows.len() > before {
                focus = self.wm.order.last().copied();
            }
        }
        self.wm.switch_desktop(s.desktop.min(self.wm.desktops - 1));
        if let Some(id) = focus {
            self.wm.focus(id);
        }
        self.wm.arrange();
        !s.windows.is_empty()
    }

    // ---------------------------------------------------------------- actions

    pub fn run_action(&mut self, action: Action) {
        // Any other action (from a key, Lua or tuidesk msg) ends an open switcher first.
        if self.switcher.is_some() && !matches!(action, Action::Switcher | Action::SwitcherPrev) {
            self.close_switcher(false);
        }
        let focused = self.wm.focused;
        match action {
            Action::Kickoff => {
                if self.mode == Mode::Kickoff {
                    self.mode = Mode::Normal;
                } else {
                    self.kickoff = Kickoff::default();
                    self.mode = Mode::Kickoff;
                }
            }
            Action::SpawnShell => self.spawn_shell(),
            Action::Spawn(c) => {
                self.spawn_command(c);
            }
            Action::Launch(name) => {
                let app = self.cfg.all_apps().into_iter().find(|a| a.name.eq_ignore_ascii_case(&name));
                match app {
                    Some(a) => {
                        let cwd = self.new_window_cwd();
                        self.spawn(SpawnRequest { command: Some(a.command), name: a.name, cwd, restore: None });
                    }
                    None => self.notify("Launcher", format!("no app named '{name}'")),
                }
            }
            Action::Close => {
                if let Some(id) = focused {
                    self.close_window(id);
                }
            }
            Action::Maximize => {
                if let Some(id) = focused {
                    self.wm.toggle_maximize(id);
                }
            }
            Action::Minimize => {
                if let Some(id) = focused {
                    self.wm.minimize(id);
                }
            }
            Action::FocusNext => self.wm.cycle(true),
            Action::FocusPrev => self.wm.cycle(false),
            Action::Switcher => self.open_switcher(true),
            Action::SwitcherPrev => self.open_switcher(false),
            Action::Tile(side) => {
                if let Some(id) = focused {
                    self.wm.quick_tile(id, side);
                }
            }
            Action::ToggleLayout => self.wm.toggle_layout(),
            Action::CycleLayout => {
                let next = self.wm.layout().next();
                self.wm.set_layout(next);
            }
            Action::SetLayout(l) => self.wm.set_layout(l),
            Action::CopyMode => {
                if let Some(id) = focused {
                    self.enter_copy_mode(id);
                }
            }
            Action::Clipboard => {
                if self.clips.is_empty() {
                    self.notify("Clipboard", "nothing copied yet");
                } else {
                    self.clip_sel = 0;
                    self.mode = Mode::Clipboard;
                }
            }
            Action::Help => {
                self.help = HelpState::default();
                self.mode = Mode::Help;
            }
            Action::WindowMenu => {
                if let Some(w) = self.wm.focused_window() {
                    let (id, x, y) = (w.id, w.frame.x, w.frame.y + 1);
                    self.open_window_menu(id, x, y);
                }
            }
            Action::ToggleAbove => {
                if let Some(w) = self.wm.focused_mut() {
                    w.above = !w.above;
                }
            }
            Action::Rename => {
                if let Some(w) = self.wm.focused_window() {
                    self.rename =
                        Some((RenameTarget::Window(w.id), w.title_override.clone().unwrap_or_else(|| w.title())));
                    self.mode = Mode::Rename;
                }
            }
            Action::RenameDesktop => {
                let d = self.wm.desktop;
                self.rename = Some((RenameTarget::Desktop(d), self.desktop_names[d].clone()));
                self.mode = Mode::Rename;
            }
            Action::Overview => {
                if self.mode == Mode::Overview {
                    self.mode = Mode::Normal;
                } else {
                    self.overview = OverviewState::default();
                    // Start on the focused window.
                    let list = self.overview_windows();
                    self.overview.sel = self.wm.focused.and_then(|f| list.iter().position(|&id| id == f)).unwrap_or(0);
                    self.mode = Mode::Overview;
                }
            }
            Action::Screenshot(ShotTarget::Region) => {
                self.region = RegionState::default();
                self.mode = Mode::Region;
            }
            Action::Screenshot(target) => {
                let area = match target {
                    ShotTarget::Window => self.wm.focused_window().map(|w| w.frame),
                    _ => Some(self.screen),
                };
                match area {
                    Some(area) => {
                        let _ = self.screenshot(area, None, None);
                    }
                    None => self.notify("Screenshot", "no focused window"),
                }
            }
            Action::Runner => {
                if self.mode == Mode::Runner {
                    self.mode = Mode::Normal;
                } else {
                    self.runner = RunnerState { base: self.new_window_cwd(), ..Default::default() };
                    self.mode = Mode::Runner;
                }
            }
            Action::ToggleFloating => {
                if let Some(id) = focused {
                    self.wm.toggle_floating(id);
                }
            }
            Action::SwapMaster => {
                if let Some(id) = focused {
                    self.wm.swap_master(id);
                }
            }
            Action::WindowMode => {
                if let Some(id) = focused {
                    self.make_floating(id);
                    self.mode = Mode::Move;
                }
            }
            Action::ScrollUp | Action::ScrollDown => {
                if let Some(w) = self.wm.focused_mut() {
                    let page = (w.content().height / 2).max(1) as isize;
                    w.term.scroll(if action == Action::ScrollUp { page } else { -page });
                }
            }
            Action::Desktop(n) => self.wm.switch_desktop(n),
            Action::DesktopNext => self.wm.switch_desktop((self.wm.desktop + 1) % self.wm.desktops),
            Action::DesktopPrev => self.wm.switch_desktop((self.wm.desktop + self.wm.desktops - 1) % self.wm.desktops),
            Action::MoveToDesktop(n) => {
                if let Some(id) = focused {
                    self.wm.move_to_desktop(id, n);
                }
            }
            Action::MoveNextDesktop | Action::MovePrevDesktop => {
                if let Some(id) = focused {
                    let n = self.wm.desktops;
                    let target = if action == Action::MoveNextDesktop {
                        (self.wm.desktop + 1) % n
                    } else {
                        (self.wm.desktop + n - 1) % n
                    };
                    // Follow the window, as KDE does with Ctrl+Alt+Shift+arrow.
                    self.wm.move_to_desktop(id, target);
                    self.wm.focus(id);
                }
            }
            Action::Paste => {
                if let Some(id) = focused {
                    let text = self.clipboard.clone();
                    self.paste_into(id, &text);
                }
            }
            Action::Notifications => {
                if self.mode == Mode::History {
                    self.mode = Mode::Normal;
                } else {
                    self.notes.unread = 0;
                    self.mode = Mode::History;
                }
            }
            Action::Detach => self.detach("detached"),
            Action::Reload => self.reload(),
            Action::Settings => {
                if self.mode != Mode::Settings {
                    self.open_settings();
                }
            }
            Action::Quit => self.quit = true,
            Action::Lua(name) => {
                self.sync_lua();
                self.plugins.call_global(&name);
                self.drain_lua();
            }
        }
        self.dirty = true;
    }

    /// Detach a window from the tiling grid / maximized state, keeping its current frame.
    pub(crate) fn make_floating(&mut self, id: u64) {
        let tiled = self.wm.get(id).is_some_and(|w| self.wm.is_tiled(w));
        if let Some(w) = self.wm.get_mut(id) {
            if w.maximized || tiled {
                w.float_rect = w.frame;
                w.maximized = false;
            }
            if tiled {
                w.floating = true;
            }
        }
        self.wm.arrange();
    }

    pub(crate) fn paste_into(&mut self, id: u64, text: &str) {
        if text.is_empty() {
            return;
        }
        let Some(w) = self.wm.get_mut(id) else { return };
        w.term.scroll_reset();
        w.term.clear_selection();
        if w.term.bracketed_paste() {
            let mut buf = b"\x1b[200~".to_vec();
            buf.extend_from_slice(text.replace('\x1b', "").as_bytes());
            buf.extend_from_slice(b"\x1b[201~");
            w.term.write(&buf);
        } else {
            w.term.write(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
        }
        self.dirty = true;
    }

    /// Copy text to tuidesk's clipboard, its history, and the host terminal's (OSC 52).
    pub(crate) fn copy(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        self.host_out.extend_from_slice(format!("\x1b]52;c;{b64}\x07").as_bytes());
        self.clips.retain(|c| *c != text);
        self.clips.push_front(text.clone());
        self.clips.truncate(CLIP_HISTORY);
        self.clipboard = text;
    }

    // ---------------------------------------------------------------- copy mode

    pub(crate) fn enter_copy_mode(&mut self, id: u64) {
        let Some(w) = self.wm.get_mut(id) else { return };
        w.term.set_copy_mode(true);
        self.copy = Some(CopyState { id, search: None, last_search: None, message: String::new() });
        self.mode = Mode::Copy;
    }

    pub(crate) fn exit_copy_mode(&mut self) {
        if let Some(c) = self.copy.take()
            && let Some(w) = self.wm.get_mut(c.id)
        {
            w.term.set_copy_mode(false);
        }
        self.mode = Mode::Normal;
    }

    // ---------------------------------------------------------------- window menu

    pub(crate) fn open_window_menu(&mut self, id: u64, x: u16, y: u16) {
        let Some(w) = self.wm.get(id) else { return };
        let mut items = vec![WinMenuItem::Minimize, WinMenuItem::Maximize];
        if self.wm.layout_of(w.desktop).is_tiled() {
            items.push(WinMenuItem::Float);
        }
        items.push(WinMenuItem::Above);
        items.extend((0..self.wm.desktops).filter(|&d| d != w.desktop).map(WinMenuItem::MoveTo));
        items.extend([WinMenuItem::Rename, WinMenuItem::Close]);
        self.win_menu = Some(WinMenu { id, x, y, items, sel: 0 });
        self.mode = Mode::WindowMenu;
    }

    pub fn win_menu_label(&self, item: WinMenuItem, id: u64) -> String {
        let w = self.wm.get(id);
        let mark = |on: bool| if on { "[*]" } else { "[ ]" };
        match item {
            WinMenuItem::Minimize => "Minimize".into(),
            WinMenuItem::Maximize => {
                if w.is_some_and(|w| w.maximized) {
                    "Restore".into()
                } else {
                    "Maximize".into()
                }
            }
            WinMenuItem::Float => format!("{} Floating", mark(w.is_some_and(|w| w.floating))),
            WinMenuItem::Above => format!("{} Keep above others", mark(w.is_some_and(|w| w.above))),
            WinMenuItem::MoveTo(d) => match self.desktop_names.get(d).filter(|n| !n.is_empty()) {
                Some(name) => format!("Move to desktop {} ({name})", d + 1),
                None => format!("Move to desktop {}", d + 1),
            },
            WinMenuItem::Rename => "Rename...".into(),
            WinMenuItem::Close => "Close".into(),
        }
    }

    pub(crate) fn activate_win_menu(&mut self) {
        self.mode = Mode::Normal;
        let Some(menu) = self.win_menu.take() else { return };
        let Some(&item) = menu.items.get(menu.sel) else { return };
        let id = menu.id;
        match item {
            WinMenuItem::Minimize => self.wm.minimize(id),
            WinMenuItem::Maximize => self.wm.toggle_maximize(id),
            WinMenuItem::Float => self.wm.toggle_floating(id),
            WinMenuItem::Above => {
                if let Some(w) = self.wm.get_mut(id) {
                    w.above = !w.above;
                }
            }
            WinMenuItem::MoveTo(d) => self.wm.move_to_desktop(id, d),
            WinMenuItem::Rename => {
                self.wm.focus(id);
                self.run_action(Action::Rename);
            }
            WinMenuItem::Close => self.close_window(id),
        }
        self.dirty = true;
    }

    // ---------------------------------------------------------------- tuidesk msg

    fn handle_msg(&mut self, args: &[String]) -> Result<String, String> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        let id_arg = |i: usize| arg(i).parse::<u64>().map_err(|_| "expected a window id".to_string());
        let rest = |i: usize| args.get(i..).map(|a| a.join(" ")).unwrap_or_default();
        let result = match arg(0).as_str() {
            "" | "help" => Ok(MSG_HELP.trim_end().to_string()),
            "notify" => {
                self.notify(arg(1), rest(2));
                Ok(String::new())
            }
            "spawn" | "run" if args.len() > 1 => match self.spawn_command(rest(1)) {
                Some(id) => Ok(id.to_string()),
                None => Err(format!("could not start '{}'", rest(1))),
            },
            "windows" | "list" => Ok(self
                .wm
                .order
                .iter()
                .filter_map(|&id| self.wm.get(id))
                .map(|w| {
                    let flag = if self.wm.focused == Some(w.id) { "*" } else { "-" };
                    format!("{}\t{}\t{}\t{}\t{}", w.id, w.desktop + 1, flag, w.title(), w.command)
                })
                .collect::<Vec<_>>()
                .join("\n")),
            "focus" => id_arg(1).and_then(|id| match self.wm.get(id) {
                Some(_) => {
                    self.wm.focus(id);
                    Ok(String::new())
                }
                None => Err(format!("no window {id}")),
            }),
            "close" => id_arg(1).and_then(|id| match self.wm.get(id) {
                Some(_) => {
                    self.close_window(id);
                    Ok(String::new())
                }
                None => Err(format!("no window {id}")),
            }),
            "desktops" => Ok(format!("{}/{}", self.wm.desktop + 1, self.wm.desktops)),
            "screenshot" => {
                // screenshot [screen|window|ID] [txt|ansi|html|svg] [PATH]
                let mut area = self.screen;
                let mut format = None;
                let mut path = None;
                for a in &args[1..] {
                    if let Some(f) = crate::screenshot::Format::parse(a) {
                        format = Some(f);
                    } else if a == "screen" {
                    } else if a == "window" {
                        area = self.wm.focused_window().map(|w| w.frame).ok_or("no focused window")?;
                    } else if let Ok(id) = a.parse::<u64>() {
                        let w = self.wm.get(id).ok_or_else(|| format!("no window {id}"))?;
                        if !self.wm.visible(w) {
                            return Err(format!("window {id} is not visible on the current desktop"));
                        }
                        area = w.frame;
                    } else {
                        path = Some(config::expand_tilde(a));
                    }
                }
                // "shot.svg" means SVG unless a format was given.
                if format.is_none() {
                    format = path
                        .as_ref()
                        .and_then(|p| p.extension())
                        .and_then(|e| crate::screenshot::Format::parse(&e.to_string_lossy()));
                }
                self.screenshot(area, format, path).map(|p| p.display().to_string())
            }
            "lua" => {
                self.sync_lua();
                let out = self.plugins.exec(&rest(1));
                self.drain_lua();
                out
            }
            _ => match Action::parse(&args.join(" ")) {
                Some(Action::Quit) => {
                    self.quit = true;
                    Ok(String::new())
                }
                Some(a) => {
                    self.run_action(a);
                    Ok(String::new())
                }
                None => Err(format!("unknown command '{}' (try: tuidesk msg help)", args.join(" "))),
            },
        };
        self.dirty = true;
        result
    }

    // ---------------------------------------------------------------- switcher

    fn open_switcher(&mut self, forward: bool) {
        if let Some(s) = &mut self.switcher {
            let n = s.list.len();
            s.idx = if forward { (s.idx + 1) % n } else { (s.idx + n - 1) % n };
            s.last = Instant::now();
            return;
        }
        let list = self.wm.switcher_list();
        match list.len() {
            0 => {}
            1 => self.wm.focus(list[0]),
            n => {
                let releases = self.display.as_ref().is_some_and(|d| d.hello.enhanced);
                if releases {
                    // Ask for key release and modifier-only events while the switcher is open.
                    self.host_out.extend_from_slice(b"\x1b[>11u");
                }
                let idx = if forward { 1 } else { n - 1 };
                self.switcher = Some(Switcher { list, idx, last: Instant::now(), releases });
                self.mode = Mode::Switcher;
            }
        }
    }

    pub(crate) fn close_switcher(&mut self, commit: bool) {
        if let Some(s) = self.switcher.take() {
            if s.releases {
                self.host_out.extend_from_slice(b"\x1b[<u");
            }
            if commit && let Some(&id) = s.list.get(s.idx) {
                self.wm.focus(id);
            }
        }
        self.mode = Mode::Normal;
        self.dirty = true;
    }

    // ---------------------------------------------------------------- display

    /// Images are shown when the config allows them and the host terminal supports them.
    pub fn graphics_enabled(&self) -> bool {
        self.cfg.general.images && self.image_mode() != crate::graphics::direct::ImageMode::None
    }

    /// The image protocol in use: forced by the config, or detected for the attached terminal.
    pub fn image_mode(&self) -> crate::graphics::direct::ImageMode {
        crate::graphics::direct::ImageMode::parse(&self.cfg.general.image_protocol).unwrap_or(self.host_images)
    }

    /// Images are drawn by tuidesk itself (iTerm2 or Sixel) and shown in this frame.
    pub fn draws_images_directly(&self) -> bool {
        self.graphics_enabled() && self.image_mode().is_direct() && !self.capturing
    }

    /// Tell every window about the host's image support and cell size.
    fn update_graphics(&mut self) {
        let (enabled, cell) = (self.graphics_enabled(), self.cell_px);
        for w in &mut self.wm.windows {
            w.term.set_graphics(enabled, cell);
        }
        self.dirty = true;
    }

    fn attach(&mut self, conn: u64, stream: crate::ipc::Stream, hello: Hello) {
        if let Some(mut old) = self.display.take() {
            old.detach("attached from another terminal");
        }
        self.host_images = hello.image_mode;
        self.regions_shown.clear();
        if hello.cell_w > 0 && hello.cell_h > 0 {
            self.cell_px = (hello.cell_w, hello.cell_h);
        }
        // The new terminal has none of our images yet.
        self.host_gfx.reset();
        self.update_graphics();
        match Display::remote(stream, hello, conn) {
            Ok(d) => {
                self.display = Some(d);
                self.cursor_shape = None;
                let warnings = self.apply_config();
                for w in warnings {
                    self.notify("Config warning", w);
                }
                self.dirty = true;
            }
            Err(e) => eprintln!("attach failed: {e}"),
        }
    }

    pub(crate) fn detach(&mut self, message: &str) {
        if !self.server {
            self.notify("tuidesk", "Detaching needs the server (run tuidesk without --standalone)");
            return;
        }
        if self.switcher.is_some() {
            self.close_switcher(false);
        }
        self.save_session();
        if let Some(mut d) = self.display.take() {
            // Leave the terminal without our images.
            d.send_raw(&self.host_gfx.free_all());
            d.detach(message);
        }
        self.mode = Mode::Normal;
        self.drag = None;
    }

    fn draw(&mut self) -> anyhow::Result<()> {
        let Some(mut display) = self.display.take() else { return Ok(()) };
        // Images go to the host before the placeholder cells that show them.
        if self.graphics_enabled() {
            let mut out = Vec::new();
            let transmit = self.image_mode() == crate::graphics::direct::ImageMode::Placeholders;
            let icons = self.cfg.general.pixel_buttons;
            if icons {
                self.button_icons.update(&self.theme, self.cell_px);
            }
            let windows = self.wm.windows.iter_mut().map(|w| (w.id, &mut w.term.gfx));
            let icons = icons.then_some((crate::graphics::icons::OWNER, &mut self.button_icons.gfx));
            self.host_gfx.sync(windows.chain(icons), &mut out, transmit);
            if !out.is_empty() {
                display.send_raw(&out);
            }
        }
        let direct = self.draws_images_directly();
        // With direct images, keep the frame: cells an image no longer covers are repainted from it.
        let mut frame: Option<ratatui::buffer::Buffer> = None;
        let mut result = display.terminal.draw(|f| crate::ui::draw(f, self)).map(|done| {
            if direct {
                frame = Some(done.buffer.clone());
            }
        });
        if direct {
            let shown = self.direct_regions();
            if self.image_mode() != self.shown_mode {
                // Switching protocol (config reload): repaint everything and redraw in the new one.
                self.shown_mode = self.image_mode();
                self.regions_shown.clear();
                result = result.and_then(|_| display.terminal.clear());
                result = result.and_then(|_| display.terminal.draw(|f| crate::ui::draw(f, self)).map(|_| ()));
                let out = self.draw_direct_images(&shown, &shown);
                display.send_raw(&out);
                self.regions_shown = shown;
            } else if shown != self.regions_shown {
                // Only what changed: repaint the cells of pieces that moved or went away, then
                // draw the new pieces. Unchanged pieces stay on screen untouched.
                let kept: Vec<_> = shown.iter().filter(|r| self.regions_shown.contains(r)).copied().collect();
                // A piece whose place and crop are unchanged (a new animation frame) is drawn
                // straight over the old one; blanking it first would flicker.
                let gone: Vec<Rect> = self
                    .regions_shown
                    .iter()
                    .filter(|r| !kept.contains(r) && !shown.iter().any(|(n, _)| n == &r.0))
                    .map(|(r, _)| r.screen)
                    .collect();
                if let Some(buf) = &frame
                    && !gone.is_empty()
                {
                    let cells: Vec<(u16, u16, ratatui::buffer::Cell)> = gone
                        .iter()
                        .flat_map(|r| r.positions())
                        .filter(|p| buf.area.contains(*p))
                        .map(|p| (p.x, p.y, buf[(p.x, p.y)].clone()))
                        .collect();
                    display.send_raw(b"\x1b7");
                    let backend = display.terminal.backend_mut();
                    result = result
                        .and_then(|_| {
                            ratatui::backend::Backend::draw(backend, cells.iter().map(|(x, y, c)| (*x, *y, c)))
                        })
                        .and_then(|_| ratatui::backend::Backend::flush(backend));
                    display.send_raw(b"\x1b8");
                }
                let changed: Vec<_> = shown.iter().filter(|r| !kept.contains(r)).copied().collect();
                let out = self.draw_direct_images(&changed, &shown);
                display.send_raw(&out);
                self.regions_shown = shown;
            }
        } else if !self.regions_shown.is_empty() {
            // Direct images were turned off: a full repaint removes them.
            result = result.and_then(|_| display.terminal.clear());
            self.regions_shown.clear();
            self.dirty = true;
        }
        // Cursor shape follows the focused program (or a bar while typing in the launcher).
        let shape = match self.mode {
            Mode::Kickoff => Some(CursorShape::Beam),
            _ => self.wm.focused_window().and_then(|w| w.term.cursor()).map(|c| c.2),
        };
        if let Some(shape) = shape
            && self.cursor_shape != Some(shape)
        {
            self.cursor_shape = Some(shape);
            let code = match shape {
                CursorShape::Underline => 4,
                CursorShape::Beam => 6,
                _ => 2,
            };
            self.host_out.extend_from_slice(format!("\x1b[{code} q").as_bytes());
        }
        if !self.host_out.is_empty() {
            display.send_raw(&std::mem::take(&mut self.host_out));
        }
        if display.sink.is_broken() {
            // The client went away mid-write, or stopped reading; keep running detached.
            eprintln!("client stopped reading or went away; detached it");
            self.save_session();
        } else {
            self.display = Some(display);
        }
        result?;
        Ok(())
    }

    /// A window's image, or a title bar icon.
    fn image(&self, win: u64, img: u32) -> Option<&crate::graphics::ImageData> {
        if win == crate::graphics::icons::OWNER {
            return self.button_icons.gfx.images.get(&img);
        }
        self.wm.get(win)?.term.gfx.images.get(&img)
    }

    /// The visible image pieces of the last frame, with the generation of each image.
    fn direct_regions(&self) -> Vec<(crate::graphics::direct::Region, u64)> {
        self.regions
            .iter()
            .filter_map(|r| {
                let (win, img) = self.host_gfx.owner(r.host_id)?;
                Some((*r, self.image(win, img)?.generation))
            })
            .collect()
    }

    /// Commands drawing the pieces in `draw` with iTerm2 or Sixel; `live` is everything visible
    /// (the cache keeps those).
    fn draw_direct_images(
        &mut self,
        draw: &[(crate::graphics::direct::Region, u64)],
        live: &[(crate::graphics::direct::Region, u64)],
    ) -> Vec<u8> {
        use crate::graphics::direct::{self, ImageMode};
        let mode = self.image_mode();
        let mut out = Vec::new();
        for (region, _) in draw {
            let mut r = *region;
            // A Sixel image touching the last row can make some terminals scroll.
            if mode == ImageMode::Sixel && r.screen.bottom() >= self.screen.bottom() {
                r.screen.height = r.screen.height.saturating_sub(1);
                if r.screen.height == 0 {
                    continue;
                }
            }
            let Some((win, img)) = self.host_gfx.owner(r.host_id) else { continue };
            let Some(data) = self.image(win, img).cloned() else { continue };
            if let Some(piece) = self.scaled.piece(mode, &data, &r, self.cell_px) {
                out.extend_from_slice(&direct::place(&r, &piece));
            }
        }
        let live: Vec<(u32, u64)> = live.iter().map(|(r, g)| (r.host_id, *g)).collect();
        self.scaled.retain(&live);
        out
    }

    // ---------------------------------------------------------------- events

    fn handle_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Input(ev) => self.handle_input(ev),
            AppEvent::ClientInput(conn, ev) => {
                if self.display.as_ref().and_then(|d| d.conn) == Some(conn) {
                    self.handle_input(ev);
                }
            }
            AppEvent::ClientGone(conn) => {
                if self.display.as_ref().and_then(|d| d.conn) == Some(conn) {
                    self.display = None;
                    self.close_switcher(false);
                    self.save_session();
                }
            }
            AppEvent::Attach { conn, stream, hello } => self.attach(conn, stream, hello),
            AppEvent::CellSize(conn, cell) => {
                if self.display.as_ref().and_then(|d| d.conn) == Some(conn) && cell.0 > 0 && cell.1 > 0 {
                    self.cell_px = cell;
                    self.update_graphics();
                }
            }
            AppEvent::Kill => self.quit = true,
            AppEvent::Msg { args, reply } => {
                let out = self.handle_msg(&args);
                let _ = reply.send(out);
            }
            AppEvent::PtyOutput(id, bytes) => self.pty_output(id, &bytes),
            AppEvent::PtyExit(id) => self.pty_exit(id),
        }
    }

    fn handle_input(&mut self, ev: Event) {
        match ev {
            Event::Key(k) => self.handle_key(k),
            Event::Mouse(m) => self.handle_mouse(m),
            Event::Paste(s) => self.handle_paste(s),
            Event::Resize(w, h) => {
                if let Some(d) = &mut self.display {
                    d.resize(w, h);
                }
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn pty_output(&mut self, id: u64, bytes: &[u8]) {
        let focused = self.wm.focused == Some(id);
        let Some(w) = self.wm.get_mut(id) else { return };
        w.term.process(bytes);
        let title = w.title();
        let bell = std::mem::take(&mut w.term.bell);
        let notes = std::mem::take(&mut w.term.notifications);
        let copies = std::mem::take(&mut w.term.clipboard);
        if bell && !focused {
            w.urgent = true;
        }
        for text in copies {
            self.copy(text);
        }
        for (t, b) in notes {
            let t = if t.is_empty() { title.clone() } else { t };
            self.notify(t, b);
        }
        self.dirty = true;
    }

    fn pty_exit(&mut self, id: u64) {
        let Some(w) = self.wm.get_mut(id) else { return };
        let mut code = None;
        for _ in 0..20 {
            code = w.term.exit_code();
            if code.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        w.exit_code = Some(code.unwrap_or(0));
        if self.cfg.general.close_on_exit {
            self.close_window(id);
        } else {
            self.dirty = true;
        }
    }

    fn tick(&mut self) {
        // A program can exit while a background job keeps its terminal open, so the reader
        // never sees end-of-file. Notice those exits here.
        let exited: Vec<u64> = self
            .wm
            .windows
            .iter_mut()
            .filter(|w| w.exit_code.is_none())
            .filter_map(|w| w.term.exit_code().map(|_| w.id))
            .collect();
        for id in exited {
            self.pty_exit(id);
        }
        if self.tray.refresh() {
            self.dirty = true;
        }
        self.sync_lua();
        if self.plugins.update_widgets() {
            self.dirty = true;
        }
        self.plugins.emit("tick", None);
        self.drain_lua();

        let now: Vec<Option<SystemTime>> = self.mtimes_paths.iter().map(mtime).collect();
        // Outside edits wait until the Settings dialog, which previews its own, is closed.
        if now != self.mtimes && self.mode != Mode::Settings {
            self.mtimes = now;
            self.reload();
        }
        if self.last_save.elapsed() >= Duration::from_secs(30) {
            self.save_session();
        }
        // The clock and popups need redrawing over time.
        self.dirty = true;
    }

    fn check_focus_change(&mut self) {
        if self.wm.focused != self.last_focus {
            self.last_focus = self.wm.focused;
            if let Some(info) = self.wm.focused_window().map(|w| self.win_info(w)) {
                self.emit("focus", Some(info));
            }
        }
    }

    /// Show the next frame of animated images in visible windows. Returns when the next frame is
    /// due. Hidden windows (other desktops, minimized) are paused.
    fn animate_images(&mut self) -> Option<Instant> {
        if !self.graphics_enabled() {
            return None;
        }
        let now = Instant::now();
        let desktop = self.wm.desktop;
        let mut next: Option<Instant> = None;
        for w in self.wm.windows.iter_mut().filter(|w| w.desktop == desktop && !w.minimized) {
            if w.term.gfx.animate(now) {
                self.dirty = true;
            }
            if let Some(t) = w.term.gfx.next_frame_at() {
                next = Some(next.map_or(t, |n| n.min(t)));
            }
        }
        next
    }

    /// Flush synchronized updates that timed out; returns the next pending deadline.
    fn check_sync(&mut self) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        for w in &mut self.wm.windows {
            if w.term.check_sync() {
                self.dirty = true;
            }
            if let Some(t) = w.term.sync_deadline() {
                next = Some(next.map_or(t, |n| n.min(t)));
            }
        }
        next
    }

    pub fn run(&mut self, rx: Receiver<AppEvent>) -> anyhow::Result<()> {
        if !self.restore_session() {
            for cmd in self.cfg.general.autostart.clone() {
                self.spawn_command(cmd);
            }
        }
        self.emit("startup", None);
        let frame_budget = Duration::from_millis(8);
        let tick_every = Duration::from_secs(1);
        let mut last_tick = Instant::now();
        let mut last_draw = Instant::now().checked_sub(frame_budget).unwrap_or_else(Instant::now);
        let mut had_popups = false;
        let mut sync_deadline: Option<Instant> = None;

        while !self.quit {
            let mut timeout = tick_every.saturating_sub(last_tick.elapsed());
            if self.dirty && self.display.is_some() {
                timeout = frame_budget.saturating_sub(last_draw.elapsed());
            }
            if had_popups || self.switcher.is_some() || self.osd.is_some() {
                timeout = timeout.min(Duration::from_millis(100));
            }
            if let Some(t) = sync_deadline {
                timeout = timeout.min(t.saturating_duration_since(Instant::now()));
            }
            match rx.recv_timeout(timeout) {
                Ok(ev) => {
                    self.handle_event(ev);
                    // Drain bursts of output without redrawing in between.
                    for _ in 0..512 {
                        match rx.try_recv() {
                            Ok(ev) => self.handle_event(ev),
                            Err(_) => break,
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            sync_deadline = self.check_sync();
            if let Some(next) = self.animate_images() {
                sync_deadline = Some(sync_deadline.map_or(next, |t| t.min(next)));
            }
            self.check_focus_change();
            if self.wm.desktop != self.last_desktop {
                self.last_desktop = self.wm.desktop;
                let d = self.wm.desktop;
                self.osd = Some((format!("Desktop {}: {}", d + 1, self.desktop_label(d)), Instant::now()));
                self.dirty = true;
            }
            if self.osd.as_ref().is_some_and(|(_, t)| t.elapsed() >= OSD_TIME) {
                self.osd = None;
                self.dirty = true;
            }
            self.switcher_timeout();
            if last_tick.elapsed() >= tick_every {
                last_tick = Instant::now();
                self.tick();
            }
            let has_popups = self.notes.has_popups();
            if has_popups != had_popups {
                had_popups = has_popups;
                self.dirty = true;
            }
            if self.dirty && self.display.is_some() && last_draw.elapsed() >= frame_budget {
                self.draw()?;
                last_draw = Instant::now();
                self.dirty = false;
            }
        }
        self.save_session();
        if let Some(mut d) = self.display.take() {
            // Leave the terminal without our images.
            d.send_raw(&self.host_gfx.free_all());
            d.exit();
        }
        Ok(())
    }
}

const MSG_HELP: &str = "\
Commands for `tuidesk msg`:
  notify TITLE [BODY...]   show a notification
  spawn COMMAND...         open a window running COMMAND; prints its id
  windows                  list windows: id, desktop, focused (*), title, command
  focus ID | close ID      focus or close a window
  desktops                 current desktop / count
  screenshot [screen|window|ID] [txt|ansi|html|svg] [PATH]
                           save a screenshot; prints its path
  lua CODE                 run Lua in the plugin environment; prints the results
  ACTION [ARG]             any key binding action, e.g. 'desktop 2', 'layout grid', 'detach'
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_actions() {
        assert_eq!(Action::parse("spawn btop -p 0"), Some(Action::Spawn("btop -p 0".into())));
        assert_eq!(Action::parse("tile_left"), Some(Action::Tile(Side::Left)));
        assert_eq!(Action::parse("desktop 2"), Some(Action::Desktop(1)));
        assert_eq!(Action::parse("move_to_desktop 1"), Some(Action::MoveToDesktop(0)));
        assert_eq!(Action::parse("desktop 0"), None);
        assert_eq!(Action::parse("layout grid"), Some(Action::SetLayout(Layout::Grid)));
        assert_eq!(Action::parse("layout nope"), None);
        assert_eq!(Action::parse("spawn"), None);
        assert_eq!(Action::parse("bogus"), None);
    }

    #[test]
    fn kickoff_scroll_keeps_the_selection_visible() {
        let mut ko = Kickoff { rows: 10, ..Default::default() };
        ko.scroll_to(5, 30);
        assert_eq!((ko.scroll, ko.selected), (5, 5));
        ko.scroll_to(100, 30);
        assert_eq!((ko.scroll, ko.selected), (20, 20));
        ko.selected = 29;
        ko.scroll_to(0, 30);
        assert_eq!((ko.scroll, ko.selected), (0, 9));
        // Fewer items than rows: nothing to scroll.
        ko.scroll_to(3, 4);
        assert_eq!(ko.scroll, 0);
    }

    #[test]
    fn panel_split() {
        let (d, p) = screen_layout(Rect::new(0, 0, 80, 24), false);
        assert_eq!(d, Rect::new(0, 0, 80, 23));
        assert_eq!(p, Rect::new(0, 23, 80, 1));
    }
}

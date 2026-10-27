use alacritty_terminal::term::cell::{Cell as TermCell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Rgb};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Hits, Mode, PanelHit, RenameTarget, screen_layout};
use crate::settings::{Edit, Kind, PAGES, Row};
use crate::theme::Theme;
use crate::tray::Charge;
use crate::wm::{Button, Window, buttons};

const LOGO: [&str; 5] = [
    " _         _     _      ",
    "| |_ _   _(_) __| | ___ ",
    "| __| | | | |/ _` |/ _ \\",
    "| |_| |_| | | (_| |  __/",
    " \\__|\\__,_|_|\\__,_|\\___|",
];

pub fn draw(f: &mut Frame, app: &mut App) {
    let screen = f.area();
    app.screen = screen;
    let (desktop, panel) = screen_layout(screen, app.panel_top());
    if desktop != app.wm.area {
        app.wm.set_area(desktop);
    }
    app.hits = Hits::default();
    let buf = f.buffer_mut();

    draw_wallpaper(buf, desktop, app);
    draw_desktop_widgets(buf, desktop, app);
    let focused = app.wm.focused;
    let dim = app.cfg.general.dim_inactive;
    for w in app.wm.stacking() {
        let is_focused = focused == Some(w.id);
        let images = (app.graphics_enabled() && !app.capturing).then_some(&app.host_gfx);
        draw_window(buf, w, &app.theme, is_focused, dim && !is_focused, images);
    }
    if app.mode == Mode::Overview {
        draw_overview(buf, desktop, app);
    }
    draw_panel(buf, panel, app);
    draw_popups(buf, desktop, app);

    let mut cursor = None;
    match app.mode {
        Mode::Kickoff => cursor = draw_kickoff(buf, desktop, app),
        Mode::History => draw_history(buf, desktop, app),
        Mode::Move => draw_hint(
            buf,
            desktop,
            &app.theme,
            " Window: arrows/hjkl move, shift+arrows/HJKL resize, m maximize, Esc done ",
        ),
        Mode::Switcher => draw_switcher(buf, desktop, app),
        Mode::Copy => cursor = draw_copy(buf, app),
        Mode::Clipboard => draw_clipboard(buf, desktop, app),
        Mode::Help => draw_help(buf, desktop, app),
        Mode::WindowMenu => draw_win_menu(buf, desktop, app),
        Mode::Rename => cursor = draw_rename(buf, desktop, app),
        Mode::Runner => cursor = draw_runner(buf, desktop, app),
        Mode::Settings => cursor = draw_settings(buf, desktop, app),
        Mode::Overview => {}
        Mode::Region => draw_region(buf, desktop, app),
        Mode::Prefix | Mode::Normal => cursor = term_cursor(app),
    }
    if let Some((text, _)) = &app.osd {
        draw_osd(buf, desktop, &app.theme, text);
    }
    // iTerm2/Sixel terminals: visible placeholder cells become image pieces drawn by tuidesk.
    app.regions =
        if app.draws_images_directly() { crate::graphics::direct::take_regions(buf, screen) } else { Vec::new() };
    if let Some(pos) = cursor {
        f.set_cursor_position(pos);
    }
}

/// Where the focused program wants the cursor, if it is visible on screen.
fn term_cursor(app: &App) -> Option<(u16, u16)> {
    let w = app.wm.focused_window()?;
    if !app.wm.visible(w) {
        return None;
    }
    let (row, col, _) = w.term.cursor()?;
    let c = w.content();
    if row >= c.height || col >= c.width {
        return None;
    }
    let (x, y) = (c.x + col, c.y + row);
    // Hidden if another window covers it.
    match app.wm.hit_test(x, y) {
        Some((id, _)) if id == w.id => Some((x, y)),
        _ => None,
    }
}

// ------------------------------------------------------------------ helpers

fn fill(buf: &mut Buffer, r: Rect, style: Style) {
    let r = r.intersection(buf.area);
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.reset();
                c.set_style(style);
            }
        }
    }
}

/// Format a time, showing "?" for an invalid format instead of panicking (chrono's
/// `to_string` panics on unknown specifiers, and the format comes from the config).
fn strftime(t: &chrono::DateTime<chrono::Local>, fmt: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    match write!(out, "{}", t.format(fmt)) {
        Ok(()) => out,
        Err(_) => "?".into(),
    }
}

/// Write text clipped to `max_x` (exclusive). Returns the x after the text.
fn put(buf: &mut Buffer, x: u16, y: u16, text: &str, max_x: u16, style: Style) -> u16 {
    if x >= max_x || y >= buf.area.bottom() {
        return x;
    }
    let max_x = max_x.min(buf.area.right());
    let (nx, _) = buf.set_stringn(x, y, text, (max_x - x) as usize, style);
    nx
}

fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in text.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    let mut out = out.trim_end().to_string();
    out.push('~');
    out
}

/// Like `truncate`, but keeps the end: "~/very/long/path" becomes "~ng/path".
fn truncate_start(text: &str, width: usize) -> String {
    if text.width() <= width || width == 0 {
        return if width == 0 { String::new() } else { text.to_string() };
    }
    let mut tail: Vec<char> = Vec::new();
    let mut w = 1;
    for c in text.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if w + cw > width {
            break;
        }
        tail.push(c);
        w += cw;
    }
    std::iter::once('~').chain(tail.into_iter().rev()).collect()
}

fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.lines() {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let needed = if line.is_empty() { word.width() } else { line.width() + 1 + word.width() };
            if needed > width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    let lines: Vec<String> = lines.into_iter().map(|l| truncate(&l, width)).collect();
    if lines.len() > max_lines {
        let mut l = lines[..max_lines].to_vec();
        if let Some(last) = l.last_mut() {
            *last = truncate(&format!("{last} ..."), width);
        }
        return l;
    }
    lines
}

fn draw_box(buf: &mut Buffer, r: Rect, theme: &Theme, border: Color, bg: Color, title: Option<&str>) {
    let r = r.intersection(buf.area);
    if r.width < 2 || r.height < 2 {
        return;
    }
    fill(buf, r, Style::new().bg(bg));
    let [h, v, tl, tr, bl, br] = theme.border.chars();
    let s = Style::new().fg(border).bg(bg);
    for x in r.x + 1..r.right() - 1 {
        buf[(x, r.y)].set_symbol(h).set_style(s);
        buf[(x, r.bottom() - 1)].set_symbol(h).set_style(s);
    }
    for y in r.y + 1..r.bottom() - 1 {
        buf[(r.x, y)].set_symbol(v).set_style(s);
        buf[(r.right() - 1, y)].set_symbol(v).set_style(s);
    }
    buf[(r.x, r.y)].set_symbol(tl).set_style(s);
    buf[(r.right() - 1, r.y)].set_symbol(tr).set_style(s);
    buf[(r.x, r.bottom() - 1)].set_symbol(bl).set_style(s);
    buf[(r.right() - 1, r.bottom() - 1)].set_symbol(br).set_style(s);
    if let Some(t) = title {
        put(buf, r.x + 2, r.y, &format!(" {t} "), r.right() - 2, s.add_modifier(Modifier::BOLD));
    }
}

// ------------------------------------------------------------------ desktop

/// Top-left corner for a `w` x `h` block anchored at `position` inside `area`, with a margin.
fn anchor(area: Rect, w: u16, h: u16, position: &str) -> (u16, u16) {
    let (mx, my) = (2u16, 1u16);
    let left = area.x + mx.min(area.width.saturating_sub(w));
    let right = area.right().saturating_sub(w + mx).max(area.x);
    let top = area.y + my.min(area.height.saturating_sub(h));
    let bottom = area.bottom().saturating_sub(h + my).max(area.y);
    let cx = area.x + area.width.saturating_sub(w) / 2;
    let cy = area.y + area.height.saturating_sub(h) / 2;
    match position {
        "top-left" => (left, top),
        "top" => (cx, top),
        "top-right" => (right, top),
        "left" => (left, cy),
        "right" => (right, cy),
        "bottom-left" => (left, bottom),
        "bottom" => (cx, bottom),
        "bottom-right" => (right, bottom),
        _ => (cx, cy),
    }
}

fn draw_lines(buf: &mut Buffer, area: Rect, lines: &[String], position: &str, style: Style) {
    let w = lines.iter().map(|l| l.width()).max().unwrap_or(0).min(area.width as usize) as u16;
    let h = (lines.len() as u16).min(area.height);
    let (x, y) = anchor(area, w, h, position);
    for (i, line) in lines.iter().take(h as usize).enumerate() {
        put(buf, x, y + i as u16, line, area.right(), style);
    }
}

fn draw_wallpaper(buf: &mut Buffer, area: Rect, app: &mut App) {
    let t = app.theme.clone();
    let wp = app.cfg.wallpaper_for(app.wm.desktop);
    let color = |c: &Option<String>, fallback: Color| {
        c.as_deref().and_then(crate::theme::parse_color).map(|c| t.mode.adapt(c)).unwrap_or(fallback)
    };
    let style = Style::new().bg(color(&wp.color, t.wallpaper)).fg(color(&wp.fg, t.wallpaper_fg));
    fill(buf, area, style);
    let position = wp.position.as_deref().unwrap_or("center");
    match wp.kind.as_deref().unwrap_or("logo") {
        "none" => {}
        "pattern" => {
            let pattern = wp.text.as_deref().filter(|p| !p.is_empty()).unwrap_or(". ");
            let len = pattern.chars().count().max(1);
            for row in 0..area.height {
                // Shift each row by one for a diagonal weave.
                let line: String = pattern.chars().cycle().skip(row as usize % len).take(area.width as usize).collect();
                put(buf, area.x, area.y + row, &line, area.right(), style);
            }
        }
        "art" => {
            let lines: Vec<String> = match (&wp.text, &wp.file) {
                (Some(text), _) => text.lines().map(str::to_string).collect(),
                (None, Some(file)) => app.file_lines(file),
                (None, None) => vec!["wallpaper: set text or file".into()],
            };
            draw_lines(buf, area, &lines, position, style);
        }
        "lua" => {
            let name = wp.widget.as_deref().unwrap_or("");
            let text = app.plugins.widget_text(name).unwrap_or_else(|| format!("no Lua widget '{name}'"));
            let lines: Vec<String> = text.lines().map(str::to_string).collect();
            draw_lines(buf, area, &lines, position, style);
        }
        _ => draw_logo(buf, area, app, style),
    }
}

fn draw_logo(buf: &mut Buffer, area: Rect, app: &App, style: Style) {
    let p = app.prefix_label();
    let hints = [
        format!("{p}, space  or  alt+f1    application launcher"),
        format!("{p}, enter  or  alt+enter terminal"),
        format!("{p}, t                    toggle floating / tiling"),
        format!("{p}, q                    quit"),
    ];
    let logo_w = LOGO[0].len() as u16;
    let block_h = LOGO.len() as u16 + 2 + hints.len() as u16;
    if area.height < block_h + 2 || area.width < 50 {
        return;
    }
    let top = area.y + (area.height - block_h) / 2;
    for (i, line) in LOGO.iter().enumerate() {
        put(buf, area.x + (area.width - logo_w) / 2, top + i as u16, line, area.right(), style);
    }
    let hint_w = hints.iter().map(|h| h.width()).max().unwrap_or(0) as u16;
    let hx = area.x + area.width.saturating_sub(hint_w) / 2;
    for (i, h) in hints.iter().enumerate() {
        put(buf, hx, top + LOGO.len() as u16 + 2 + i as u16, h, area.right(), style);
    }
}

/// 3x5 digits for the desktop clock.
fn big_glyph(c: char) -> [&'static str; 5] {
    match c {
        '0' => ["###", "# #", "# #", "# #", "###"],
        '1' => [" # ", "## ", " # ", " # ", "###"],
        '2' => ["###", "  #", "###", "#  ", "###"],
        '3' => ["###", "  #", "###", "  #", "###"],
        '4' => ["# #", "# #", "###", "  #", "  #"],
        '5' => ["###", "#  ", "###", "  #", "###"],
        '6' => ["###", "#  ", "###", "# #", "###"],
        '7' => ["###", "  #", "  #", "  #", "  #"],
        '8' => ["###", "# #", "###", "# #", "###"],
        '9' => ["###", "# #", "###", "  #", "###"],
        ':' => [" ", "#", " ", "#", " "],
        _ => ["   ", "   ", "   ", "   ", "   "],
    }
}

fn big_text(text: &str, solid: bool) -> Vec<String> {
    let mut rows = vec![String::new(); 5];
    for (i, c) in text.chars().enumerate() {
        for (row, part) in rows.iter_mut().zip(big_glyph(c)) {
            if i > 0 {
                row.push(' ');
            }
            row.push_str(&if solid { part.replace('#', "█") } else { part.to_string() });
        }
    }
    rows
}

fn bar(pct: f32, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f32).round().clamp(0.0, width as f32) as usize;
    format!("[{}{}]", "|".repeat(filled), " ".repeat(width - filled))
}

fn draw_desktop_widgets(buf: &mut Buffer, area: Rect, app: &mut App) {
    let t = app.theme.clone();
    let desktop = app.wm.desktop + 1;
    let widgets = app.cfg.desktop_widgets.clone();
    for (i, wcfg) in widgets.iter().enumerate() {
        if !wcfg.desktops.is_empty() && !wcfg.desktops.contains(&desktop) {
            continue;
        }
        let (title, lines, boxed): (Option<String>, Vec<String>, bool) = match wcfg.kind.as_str() {
            "clock" => {
                let now = chrono::Local::now();
                let time = strftime(&now, wcfg.format.as_deref().unwrap_or("%H:%M"));
                let mut lines = big_text(&time, t.border != crate::theme::BorderStyle::Ascii);
                let date = now.format("%A, %d %B").to_string();
                let w = lines[0].width();
                lines.push(String::new());
                lines.push(format!("{:^w$}", date, w = w.max(date.len())));
                (wcfg.title.clone(), lines, false)
            }
            "notes" => {
                let file = wcfg.file.clone().unwrap_or_default();
                let mut lines = if file.is_empty() { vec!["notes: set file".into()] } else { app.file_lines(&file) };
                lines.push(String::new());
                lines.push("(click to edit)".into());
                let name = std::path::Path::new(&file).file_name().map(|f| f.to_string_lossy().into_owned());
                (wcfg.title.clone().or(name), lines, true)
            }
            "sysinfo" => {
                let gb = |b: u64| b as f64 / 1024.0 / 1024.0 / 1024.0;
                let mem_pct = if app.tray.mem_total > 0 {
                    app.tray.mem_used as f32 * 100.0 / app.tray.mem_total as f32
                } else {
                    0.0
                };
                let mut lines = vec![
                    format!("CPU {} {:>3.0}%", bar(app.tray.cpu, 20), app.tray.cpu),
                    format!("RAM {} {:>3.0}%", bar(mem_pct, 20), mem_pct),
                    format!("    {:.1} of {:.1} GiB used", gb(app.tray.mem_used), gb(app.tray.mem_total)),
                ];
                if let Some((pct, _)) = app.tray.battery_pct {
                    lines.push(format!("BAT {} {:>3.0}%", bar(pct, 20), pct));
                }
                (Some(wcfg.title.clone().unwrap_or_else(|| "System".into())), lines, true)
            }
            "lua" => {
                let name = wcfg.widget.clone().unwrap_or_default();
                let text = app.plugins.widget_text(&name).unwrap_or_else(|| format!("no Lua widget '{name}'"));
                (wcfg.title.clone().or(Some(name)), text.lines().map(str::to_string).collect(), true)
            }
            other => (Some("?".into()), vec![format!("unknown widget kind '{other}'")], true),
        };
        let pad = if boxed { 4 } else { 0 };
        let content_w = lines.iter().map(|l| l.width()).max().unwrap_or(0) as u16;
        let w = wcfg.width.unwrap_or(content_w + pad).max(8).min(area.width);
        let h = wcfg.height.unwrap_or(lines.len() as u16 + if boxed { 2 } else { 0 }).min(area.height);
        let (x, y) = anchor(area, w, h, &wcfg.position);
        let r = Rect::new(x, y, w, h);
        if boxed {
            draw_box(buf, r, &t, t.border_inactive, t.menu_bg, title.as_deref());
            let style = Style::new().bg(t.menu_bg).fg(t.menu_fg);
            for (row, line) in lines.iter().take(h.saturating_sub(2) as usize).enumerate() {
                let dim = line == "(click to edit)";
                put(
                    buf,
                    x + 2,
                    y + 1 + row as u16,
                    line,
                    r.right() - 1,
                    if dim { style.fg(t.menu_dim) } else { style },
                );
            }
        } else {
            let style = Style::new().bg(t.wallpaper).fg(t.accent).add_modifier(Modifier::BOLD);
            draw_lines(buf, r, &lines, "center", style);
        }
        app.hits.widgets.push((r, i));
    }
}

/// Every window as a card with a live preview, and desktop buttons to switch or drop on.
fn draw_overview(buf: &mut Buffer, desktop: Rect, app: &mut App) {
    let t = app.theme.clone();
    fill(buf, desktop, Style::new().bg(t.wallpaper).fg(t.wallpaper_fg));
    let base = Style::new().bg(t.wallpaper).fg(t.panel_fg);
    let dimmed = Style::new().bg(t.wallpaper).fg(t.wallpaper_fg);

    // Desktop buttons.
    let mut x = desktop.x + 2;
    let y = desktop.y + 1;
    for d in 0..app.wm.desktops {
        let count = app.wm.windows.iter().filter(|w| w.desktop == d).count();
        let label = match app.desktop_names.get(d).filter(|n| !n.is_empty()) {
            Some(name) => format!(" {}: {name} ({count}) ", d + 1),
            None => format!(" {} ({count}) ", d + 1),
        };
        let style = if app.drop_target == Some(d) {
            Style::new().bg(t.accent).fg(t.panel_bg).add_modifier(Modifier::BOLD)
        } else if d == app.wm.desktop {
            Style::new().bg(t.task_active_bg).fg(t.task_active_fg).add_modifier(Modifier::BOLD)
        } else {
            Style::new().bg(t.panel_bg).fg(t.panel_fg)
        };
        let start = x;
        x = put(buf, x, y, &label, desktop.right(), style);
        app.hits.desk_buttons.push((Rect::new(start, y, x - start, 1), d));
        x += 1;
    }
    let info = if app.overview.filter.is_empty() {
        "type to filter  Enter go  Del close  drag a card onto a desktop  Esc".to_string()
    } else {
        format!("Filter: {}", app.overview.filter)
    };
    let info_x = desktop.right().saturating_sub(info.width() as u16 + 2).max(x + 1);
    put(buf, info_x, y, &info, desktop.right(), if app.overview.filter.is_empty() { dimmed } else { base });

    // Card grid.
    let grid =
        Rect::new(desktop.x + 1, desktop.y + 3, desktop.width.saturating_sub(2), desktop.height.saturating_sub(4));
    let ids = app.overview_windows();
    if ids.is_empty() {
        let msg = if app.overview.filter.is_empty() { "No windows" } else { "No matching windows" };
        draw_lines(buf, grid, &[msg.to_string()], "center", base);
        return;
    }
    let n = ids.len();
    let (gap_x, gap_y) = (2u16, 1u16);
    let mut best: Option<(f32, usize, u16, u16)> = None;
    for cols in 1..=n {
        let rows = n.div_ceil(cols) as u16;
        let cw = grid.width.saturating_sub(gap_x * (cols as u16 - 1)) / cols as u16;
        let ch = grid.height.saturating_sub(gap_y * (rows - 1)) / rows;
        if cw < 16 || ch < 5 {
            continue;
        }
        // Terminal cells are about twice as tall as wide.
        let score = (cw as f32 / 2.2).min(ch as f32);
        if best.is_none_or(|b| score > b.0) {
            best = Some((score, cols, cw.min(80), ch.min(24)));
        }
    }
    // Too many windows to fit: use the smallest cards and scroll by rows.
    let (cols, cw, ch) = match best {
        Some((_, cols, cw, ch)) => (cols, cw, ch),
        None => {
            let cols = ((grid.width + gap_x) / (16 + gap_x)).max(1) as usize;
            (cols, 16.min(grid.width), 5.min(grid.height))
        }
    };
    app.overview.cols = cols;
    app.overview.sel = app.overview.sel.min(n - 1);
    let visible_rows = ((grid.height + gap_y) / (ch + gap_y)).max(1) as usize;
    let sel_row = app.overview.sel / cols;
    let first_row = sel_row.saturating_sub(visible_rows - 1);
    let used_rows = n.div_ceil(cols).min(visible_rows) as u16;
    let used_w = cols as u16 * cw + gap_x * (cols as u16 - 1);
    let used_h = used_rows * ch + gap_y * used_rows.saturating_sub(1);
    let ox = grid.x + grid.width.saturating_sub(used_w) / 2;
    let oy = grid.y + grid.height.saturating_sub(used_h) / 2;

    for (i, &id) in ids.iter().enumerate().skip(first_row * cols).take(visible_rows * cols) {
        let (row, col) = ((i / cols - first_row) as u16, (i % cols) as u16);
        let r = Rect::new(ox + col * (cw + gap_x), oy + row * (ch + gap_y), cw, ch).intersection(grid);
        let Some(w) = app.wm.get(id) else { continue };
        let sel = i == app.overview.sel;
        let border = if sel { t.accent } else { t.border_inactive };
        draw_box(buf, r, &t, border, t.term_bg, Some(&truncate(&w.title(), r.width.saturating_sub(6) as usize)));
        let inner = Rect::new(r.x + 1, r.y + 1, r.width.saturating_sub(2), r.height.saturating_sub(2));
        draw_preview(buf, inner, w, &t);
        let tag = format!(" {} ", app.desktop_label(w.desktop));
        let tag_style = Style::new().bg(border).fg(t.panel_bg).add_modifier(Modifier::BOLD);
        put(buf, r.right().saturating_sub(tag.width() as u16 + 2), r.bottom() - 1, &tag, r.right() - 1, tag_style);
        app.hits.cards.push((r, id));
    }
}

/// A readable crop of a window: left-aligned, scrolled so the cursor line is included.
fn draw_preview(buf: &mut Buffer, inner: Rect, w: &Window, theme: &Theme) {
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    fill(buf, inner, Style::new().bg(theme.term_bg).fg(theme.term_fg));
    let (rows, _) = w.term.size();
    let cursor_row = w.term.cursor().map(|c| c.0).unwrap_or(rows.saturating_sub(1));
    let first = (cursor_row + 1).saturating_sub(inner.height).min(rows.saturating_sub(inner.height));
    w.term.for_each_cell(|row, col, cell, _, palette| {
        if row < first || row - first >= inner.height || col >= inner.width {
            return;
        }
        let wide = cell.flags.intersects(Flags::WIDE_CHAR | Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
        let blank = wide || cell.c == '\0' || cell.c == crate::graphics::PLACEHOLDER || cell.c.width() != Some(1);
        let c = if blank { ' ' } else { cell.c };
        buf[(inner.x + col, inner.y + row - first)]
            .set_char(c)
            .set_style(cell_style(cell, palette, theme, false, false));
    });
}

/// Resolve an emulator color, honoring palette changes made by the program.
/// The flag is set for "dim" named colors, which are drawn with the DIM modifier.
fn convert(c: AnsiColor, palette: &Colors, theme: &Theme) -> (Color, bool) {
    let mode = theme.mode;
    let rgb = |r: Rgb| mode.adapt(Color::Rgb(r.r, r.g, r.b));
    match c {
        AnsiColor::Spec(r) => (rgb(r), false),
        AnsiColor::Indexed(i) => match palette[i as usize] {
            Some(r) => (rgb(r), false),
            None => (mode.adapt(Color::Indexed(i)), false),
        },
        AnsiColor::Named(n) => {
            if let Some(r) = palette[n] {
                return (rgb(r), false);
            }
            let idx = n as usize;
            match n {
                NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => (theme.term_fg, false),
                NamedColor::Background => (theme.term_bg, false),
                NamedColor::DimForeground => (theme.term_fg, true),
                _ if idx < 16 => (mode.adapt(Color::Indexed(idx as u8)), false),
                // Dim variants of the 8 base colors follow DimForeground in the enum.
                _ => {
                    let base = idx.saturating_sub(NamedColor::DimBlack as usize).min(7);
                    (mode.adapt(Color::Indexed(base as u8)), true)
                }
            }
        }
    }
}

fn cell_style(cell: &TermCell, palette: &Colors, theme: &Theme, dim: bool, selected: bool) -> Style {
    let (mut fg, dim_fg) = convert(cell.fg, palette, theme);
    let (mut bg, _) = convert(cell.bg, palette, theme);
    let flags = cell.flags;
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
        if fg == Color::Reset {
            fg = Color::Black;
        }
        if bg == Color::Reset {
            bg = Color::White;
        }
    }
    if flags.contains(Flags::HIDDEN) {
        fg = bg;
    }
    if selected {
        fg = theme.menu_sel_fg;
        bg = theme.menu_sel_bg;
    }
    let mut m = Modifier::empty();
    if flags.contains(Flags::BOLD) {
        m |= Modifier::BOLD;
    }
    if flags.contains(Flags::DIM) || dim || dim_fg {
        m |= Modifier::DIM;
    }
    if flags.contains(Flags::ITALIC) {
        m |= Modifier::ITALIC;
    }
    let link = cell.hyperlink().is_some_and(|h| h.uri() != crate::term::IMAGE_MARKER);
    if flags.intersects(Flags::ALL_UNDERLINES) || link {
        m |= Modifier::UNDERLINED;
    }
    if flags.contains(Flags::STRIKEOUT) {
        m |= Modifier::CROSSED_OUT;
    }
    Style::new().fg(fg).bg(bg).add_modifier(m)
}

/// Placeholder cells: the image is identified by the foreground color (24 bits) and the
/// placement by the underline color.
fn image_style(base: Style, host_id: u32, placement: u32) -> Style {
    let rgb = |v: u32| Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
    base.fg(rgb(host_id)).underline_color(rgb(placement))
}

fn color_id(c: AnsiColor) -> u32 {
    match c {
        AnsiColor::Spec(Rgb { r, g, b }) => (r as u32) << 16 | (g as u32) << 8 | b as u32,
        AnsiColor::Indexed(i) => i as u32,
        AnsiColor::Named(_) => 0,
    }
}

/// Last placeholder cell seen on a row, to continue a run that omits its diacritics.
#[derive(Clone, Copy)]
struct PlaceholderRun {
    screen_row: u16,
    col: u16,
    image: u32,
    placement: u32,
    img_row: usize,
    img_col: usize,
}

fn draw_window(
    buf: &mut Buffer,
    w: &Window,
    theme: &Theme,
    focused: bool,
    dim: bool,
    images: Option<&crate::graphics::HostGraphics>,
) {
    let f = w.frame.intersection(buf.area);
    if f.width < 4 || f.height < 3 {
        return;
    }
    let (tbg, tfg, border) = if focused {
        (theme.title_active_bg, theme.title_active_fg, theme.border_active)
    } else {
        (theme.title_inactive_bg, theme.title_inactive_fg, theme.border_inactive)
    };

    // Title bar.
    let bar = Style::new().bg(tbg).fg(tfg);
    fill(buf, Rect::new(f.x, f.y, f.width, 1), bar);
    let btns = buttons(f);
    let title_end = btns.first().map(|(r, _)| r.x).unwrap_or(f.right()).saturating_sub(1);
    let mut title = w.title();
    let scroll = w.term.scrollback();
    if scroll > 0 {
        title.push_str(&format!("  [scroll +{scroll}]"));
    }
    if w.above {
        title.push_str("  [on top]");
    }
    if let Some(code) = w.exit_code {
        title.push_str(&format!("  [exited {code}]"));
    }
    let title_style = if w.urgent { bar.fg(theme.urgent) } else { bar };
    let title_style = if focused { title_style.add_modifier(Modifier::BOLD) } else { title_style };
    let avail = title_end.saturating_sub(f.x + 2) as usize;
    put(buf, f.x + 2, f.y, &truncate(&title, avail), title_end, title_style);
    for (r, b) in btns {
        let (label, style) = match b {
            Button::Minimize => ("[_]", bar),
            Button::Maximize => (if w.maximized { "[v]" } else { "[^]" }, bar),
            Button::Close => ("[x]", if focused { bar.fg(theme.button_close) } else { bar }),
        };
        put(buf, r.x, r.y, label, r.right(), style.add_modifier(Modifier::BOLD));
    }

    // Side and bottom borders.
    let [h, v, _, _, bl, br] = theme.border.chars();
    let bs = Style::new().fg(border).bg(theme.term_bg);
    for y in f.y + 1..f.bottom() {
        buf[(f.x, y)].set_symbol(v).set_style(bs);
        buf[(f.right() - 1, y)].set_symbol(v).set_style(bs);
    }
    for x in f.x + 1..f.right() - 1 {
        buf[(x, f.bottom() - 1)].set_symbol(h).set_style(bs);
    }
    buf[(f.x, f.bottom() - 1)].set_symbol(bl).set_style(bs);
    buf[(f.right() - 1, f.bottom() - 1)].set_symbol(br).set_style(bs);

    // Content.
    let c = w.content().intersection(buf.area);
    fill(buf, c, Style::new().bg(theme.term_bg).fg(theme.term_fg));
    let (rows, cols) = w.term.size();
    let max_cols = c.width.min(cols);
    let max_rows = c.height.min(rows);
    let mut run: Option<PlaceholderRun> = None;
    w.term.for_each_cell(|row, col, cell, selected, palette| {
        if row >= max_rows || col >= max_cols {
            return;
        }
        let flags = cell.flags;
        if flags.contains(Flags::WIDE_CHAR_SPACER) {
            return;
        }
        let (x, y) = (c.x + col, c.y + row);
        let style = cell_style(cell, palette, theme, dim, selected);
        if cell.c == crate::graphics::PLACEHOLDER {
            // The program drew this image cell itself (kitty virtual placement, as yazi does).
            let marks: Vec<usize> =
                cell.zerowidth().unwrap_or(&[]).iter().filter_map(|&m| crate::graphics::diacritic_index(m)).collect();
            let image = color_id(cell.fg) | (marks.get(2).copied().unwrap_or(0) as u32) << 24;
            let placement = cell.underline_color().map(color_id).unwrap_or(0);
            let prev =
                run.filter(|p| p.screen_row == row && p.col + 1 == col && p.image == image && p.placement == placement);
            let img_row = marks.first().copied().or(prev.map(|p| p.img_row)).unwrap_or(0);
            let img_col = marks.get(1).copied().or(prev.map(|p| p.img_col + 1)).unwrap_or(0);
            run = Some(PlaceholderRun { screen_row: row, col, image, placement, img_row, img_col });
            let size = w.term.gfx.virtuals.get(&(image, placement)).copied().or_else(|| {
                // Placement id 0 means any virtual placement of the image.
                (placement == 0).then(|| w.term.gfx.virtuals.iter().find(|((i, _), _)| *i == image).map(|(_, s)| *s))?
            });
            let shown = images.zip(size).and_then(|(host, (cols, rows))| {
                let hid = host.host_id(w.id, image)?;
                let sym = crate::graphics::placeholder(img_row, img_col)?;
                (img_row < rows as usize && img_col < cols as usize)
                    .then(|| (sym, image_style(style, hid, crate::graphics::HostGraphics::placement_id(cols, rows))))
            });
            match shown {
                Some((sym, st)) => buf[(x, y)].set_symbol(&sym).set_style(st),
                None => buf[(x, y)].set_symbol(" ").set_style(style),
            };
            return;
        }
        let wide = flags.contains(Flags::WIDE_CHAR);
        let mut sym = String::new();
        if flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) || (wide && col + 1 >= max_cols) || cell.c == '\0' {
            sym.push(' ');
        } else {
            sym.push(cell.c);
            if let Some(extra) = cell.zerowidth() {
                sym.extend(extra);
            }
        }
        buf[(x, y)].set_symbol(&sym).set_style(style);
        if wide && col + 1 < max_cols {
            let next = &mut buf[(x + 1, y)];
            next.reset();
            next.set_style(style);
        }
    });

    // Images placed at a position (kitty with a cursor position, Sixel, iTerm2), drawn over the
    // text like a graphics terminal does, lowest z first.
    let Some(host) = images else { return };
    let mut placed = w.term.placement_positions();
    placed.sort_by_key(|(i, _, _)| w.term.gfx.placements[*i].z);
    for (i, top, left) in placed {
        let p = &w.term.gfx.placements[i];
        let Some(hid) = host.host_id(w.id, p.image) else { continue };
        let pid = crate::graphics::HostGraphics::placement_id(p.cols, p.rows);
        for r in 0..p.rows as i32 {
            let row = top + r;
            if row < 0 || row >= max_rows as i32 {
                continue;
            }
            for col in 0..p.cols {
                let x = left + col;
                if x >= max_cols {
                    break;
                }
                let Some(sym) = crate::graphics::placeholder(r as usize, col as usize) else { continue };
                let cell = &mut buf[(c.x + x, c.y + row as u16)];
                let base = Style::new().bg(theme.term_bg);
                cell.set_symbol(&sym).set_style(image_style(base, hid, pid));
            }
        }
    }
}

// ------------------------------------------------------------------ panel

struct Item {
    text: String,
    style: Style,
    hit: Option<PanelHit>,
    /// Leave a space after this item.
    gap: bool,
}

fn widget(app: &App, name: &str) -> Vec<Item> {
    single(app, name).map(|i| vec![i]).unwrap_or_else(|| match name {
        "pager" => pager(app),
        _ => Vec::new(),
    })
}

/// One cell per virtual desktop (its name, or number): current highlighted, occupied in bold,
/// and the drop target highlighted while a window or task is dragged over it.
fn pager(app: &App) -> Vec<Item> {
    let t = &app.theme;
    let base = Style::new().bg(t.panel_bg).fg(t.panel_fg);
    let n = app.wm.desktops;
    if n < 2 {
        return Vec::new();
    }
    (0..n)
        .map(|d| {
            let occupied = app.wm.windows.iter().any(|w| w.desktop == d);
            let urgent = app.wm.windows.iter().any(|w| w.desktop == d && w.urgent);
            let style = if app.drop_target == Some(d) {
                Style::new().bg(t.accent).fg(t.panel_bg).add_modifier(Modifier::BOLD)
            } else if d == app.wm.desktop {
                Style::new().bg(t.task_active_bg).fg(t.task_active_fg).add_modifier(Modifier::BOLD)
            } else if urgent {
                base.fg(t.urgent).add_modifier(Modifier::BOLD)
            } else if occupied {
                base.add_modifier(Modifier::BOLD)
            } else {
                base.fg(t.menu_dim)
            };
            Item {
                text: format!(" {} ", app.desktop_label(d)),
                style,
                hit: Some(PanelHit::Desktop(d)),
                gap: d + 1 == n,
            }
        })
        .collect()
}

fn single(app: &App, name: &str) -> Option<Item> {
    let t = &app.theme;
    let base = Style::new().bg(t.panel_bg).fg(t.panel_fg);
    let item = |text: String, style: Style, hit: Option<PanelHit>| Some(Item { text, style, hit, gap: true });
    match name {
        "kickoff" => {
            let style = if app.mode == Mode::Kickoff {
                Style::new().bg(t.menu_sel_bg).fg(t.menu_sel_fg)
            } else {
                Style::new().bg(t.accent).fg(t.panel_bg)
            };
            item(app.cfg.panel.kickoff_label.clone(), style.add_modifier(Modifier::BOLD), Some(PanelHit::Kickoff))
        }
        "layout" => item(format!("[{}]", app.wm.layout().name()), base, Some(PanelHit::Layout)),
        "cpu" => item(format!("CPU {:>3.0}%", app.tray.cpu), base, None),
        "mem" => {
            let gb = |b: u64| b as f64 / 1024.0 / 1024.0 / 1024.0;
            item(format!("RAM {:.1}/{:.0}G", gb(app.tray.mem_used), gb(app.tray.mem_total)), base, None)
        }
        "battery" => {
            let (pct, state) = app.tray.battery_pct?;
            let mark = match state {
                Charge::Charging => "+",
                Charge::Full => "=",
                Charge::Discharging => "",
            };
            let style = if pct < 15.0 && state == Charge::Discharging { base.fg(t.urgent) } else { base };
            item(format!("BAT {pct:.0}%{mark}"), style, None)
        }
        "bell" => {
            let n = app.notes.unread;
            if n > 0 {
                item(format!("[{n} new]"), base.fg(t.urgent).add_modifier(Modifier::BOLD), Some(PanelHit::Bell))
            } else {
                item("[-]".into(), base.fg(t.menu_dim), Some(PanelHit::Bell))
            }
        }
        "clock" => {
            item(strftime(&chrono::Local::now(), &app.cfg.panel.clock_format), base.add_modifier(Modifier::BOLD), None)
        }
        n if n.starts_with("lua:") => {
            // Multi-line widget text is meant for the desktop; the panel shows the first line.
            let text = app.plugins.widget_text(&n[4..])?.lines().next().unwrap_or("").to_string();
            (!text.is_empty()).then_some(Item { text, style: base, hit: None, gap: true })
        }
        "tasks" | "spacer" | "pager" => None,
        other => item(format!("?{other}"), base.fg(t.urgent), None),
    }
}

fn draw_panel(buf: &mut Buffer, panel: Rect, app: &mut App) {
    if panel.height == 0 {
        return;
    }
    let t = app.theme.clone();
    let base = Style::new().bg(t.panel_bg).fg(t.panel_fg);
    fill(buf, panel, base);
    let y = panel.y;

    let widgets = app.cfg.panel.widgets.clone();
    let split = widgets.iter().position(|w| w == "tasks").unwrap_or(widgets.len());
    let mut left: Vec<Item> = widgets[..split].iter().flat_map(|w| widget(app, w)).collect();
    let mut right_groups: Vec<(&str, Vec<Item>)> =
        widgets[split..].iter().map(|w| (w.as_str(), widget(app, w))).collect();
    if app.mode == Mode::Prefix {
        left.push(Item {
            text: " PREFIX ".into(),
            style: Style::new().bg(t.urgent).fg(t.panel_bg).add_modifier(Modifier::BOLD),
            hit: None,
            gap: true,
        });
    }

    let mut x = panel.x;
    for it in &left {
        let start = x;
        x = put(buf, x, y, &it.text, panel.right(), it.style);
        if let Some(h) = it.hit {
            app.hits.panel.push((Rect::new(start, y, x - start, 1), h));
        }
        x += it.gap as u16;
    }
    let left_end = x;

    // Like Plasma's task manager, show only the current desktop's windows.
    let ids: Vec<u64> =
        app.wm.order.iter().copied().filter(|&id| app.wm.get(id).is_some_and(|w| app.wm.on_current(w))).collect();
    let has_tasks = widgets.iter().any(|w| w == "tasks") && !ids.is_empty();
    let width_of = |groups: &[(&str, Vec<Item>)]| -> u16 {
        groups.iter().flat_map(|(_, items)| items).map(|i| i.text.width() as u16 + 2 * i.gap as u16).sum()
    };
    // On a narrow screen, give the task manager room by dropping tray widgets.
    if has_tasks {
        let wanted = (ids.len() as u16 * 10).min(30);
        for drop in ["cpu", "mem", "battery", "layout", "lua:"] {
            if panel.right().saturating_sub(width_of(&right_groups)).saturating_sub(left_end) >= wanted {
                break;
            }
            right_groups.retain(|(name, _)| !name.starts_with(drop));
        }
    }
    let right: Vec<Item> = right_groups.into_iter().flat_map(|(_, items)| items).collect();
    let right_w: u16 = right.iter().map(|i| i.text.width() as u16 + 2 * i.gap as u16).sum();
    let mut rx = panel.right().saturating_sub(right_w).max(left_end);
    let right_start = rx;
    for it in &right {
        rx += it.gap as u16;
        let start = rx;
        rx = put(buf, rx, y, &it.text, panel.right(), it.style);
        if let Some(h) = it.hit {
            app.hits.panel.push((Rect::new(start, y, rx.saturating_sub(start), 1), h));
        }
        rx += it.gap as u16;
    }

    // Task manager.
    if !widgets.iter().any(|w| w == "tasks") {
        return;
    }
    let avail = right_start.saturating_sub(left_end + 1);
    if ids.is_empty() || avail < 4 {
        return;
    }
    let each = (avail / ids.len() as u16).clamp(4, 28);
    let mut tx = left_end;
    for id in ids {
        if tx + each > right_start {
            break;
        }
        let Some(w) = app.wm.get(id) else { continue };
        let style = if app.wm.focused == Some(id) && !w.minimized {
            Style::new().bg(t.task_active_bg).fg(t.task_active_fg).add_modifier(Modifier::BOLD)
        } else if w.urgent {
            base.fg(t.urgent).add_modifier(Modifier::BOLD)
        } else if w.minimized {
            base.fg(t.menu_dim)
        } else {
            base
        };
        let label = if w.minimized { format!("({})", w.title()) } else { w.title() };
        let label = format!(" {} ", truncate(&label, each.saturating_sub(3) as usize));
        fill(buf, Rect::new(tx, y, each - 1, 1), style);
        put(buf, tx, y, &label, tx + each - 1, style);
        app.hits.panel.push((Rect::new(tx, y, each - 1, 1), PanelHit::Task(id)));
        tx += each;
    }
}

// ------------------------------------------------------------------ overlays

fn draw_popups(buf: &mut Buffer, desktop: Rect, app: &mut App) {
    let t = app.theme.clone();
    let width = 46.min(desktop.width);
    if width < 12 {
        return;
    }
    let x = desktop.right() - width;
    let top_panel = app.panel_top();
    let mut edge = if top_panel { desktop.y } else { desktop.bottom() };
    let popups: Vec<(usize, String, Vec<String>)> = app
        .notes
        .popups()
        .filter(|(_, n)| !(app.capturing && n.title.starts_with("Screenshot")))
        .map(|(i, n)| (i, n.title.clone(), wrap(&n.body, width as usize - 4, 4)))
        .collect();
    for (idx, title, body) in popups {
        let h = (body.len() as u16 + 2).max(3);
        let r = if top_panel {
            let r = Rect::new(x, edge, width, h);
            edge += h;
            r
        } else {
            if edge < desktop.y + h {
                break;
            }
            edge -= h;
            Rect::new(x, edge, width, h)
        };
        if r.bottom() > desktop.bottom() {
            break;
        }
        let title = if title.is_empty() { "Notification".to_string() } else { title };
        draw_box(buf, r, &t, t.accent, t.notif_bg, Some(&truncate(&title, width as usize - 6)));
        let s = Style::new().bg(t.notif_bg).fg(t.notif_fg);
        for (i, line) in body.iter().enumerate() {
            put(buf, r.x + 2, r.y + 1 + i as u16, line, r.right() - 1, s);
        }
        app.hits.popups.push((r, idx));
    }
}

fn draw_kickoff(buf: &mut Buffer, desktop: Rect, app: &mut App) -> Option<(u16, u16)> {
    let t = app.theme.clone();
    let w = 64.min(desktop.width);
    // Tall enough for every entry (apps and the session items) up to 30 rows; fixed while
    // filtering, so the menu does not jump as you type.
    let h = (app.apps.len() as u16 + 4 + 6).clamp(22, 30).min(desktop.height);
    if w < 24 || h < 8 {
        return None;
    }
    let y = if app.panel_top() { desktop.y } else { desktop.bottom() - h };
    let r = Rect::new(desktop.x, y, w, h);
    draw_box(buf, r, &t, t.accent, t.menu_bg, Some("Applications"));
    app.hits.menu = Some(r);
    let base = Style::new().bg(t.menu_bg).fg(t.menu_fg);
    let dimmed = base.fg(t.menu_dim);
    let inner_right = r.right() - 1;

    // Search field.
    let sx = put(buf, r.x + 2, r.y + 1, "Search: ", inner_right, dimmed);
    let query = truncate(&app.kickoff.query, (inner_right - sx - 1) as usize);
    let cx = put(buf, sx, r.y + 1, &query, inner_right, base.add_modifier(Modifier::BOLD));

    // Category tabs. When they are wider than the menu they scroll to keep the current one in
    // view, with clickable arrows for the hidden ones.
    let cats = app.categories();
    let ko = &mut app.kickoff;
    ko.category = ko.category.min(cats.len() - 1);
    let tab_w = |c: &String| c.width() as u16 + 3;
    // Tabs end before a gap and the right arrow.
    let room = (inner_right - 2).saturating_sub(r.x + 2) + 1;
    if cats.iter().map(tab_w).sum::<u16>() <= room {
        ko.tab_scroll = 0;
    } else {
        ko.tab_scroll = ko.tab_scroll.min(ko.category);
        while ko.tab_scroll < ko.category && cats[ko.tab_scroll..=ko.category].iter().map(tab_w).sum::<u16>() > room {
            ko.tab_scroll += 1;
        }
    }
    let (category, first) = (ko.category, ko.tab_scroll);
    let mut tx = r.x + 2;
    let mut last = first;
    for (i, c) in cats.iter().enumerate().skip(first) {
        if tx + tab_w(c) > r.x + 2 + room && i > first {
            break;
        }
        let style = if i == category {
            Style::new().bg(t.menu_sel_bg).fg(t.menu_sel_fg).add_modifier(Modifier::BOLD)
        } else {
            dimmed
        };
        let start = tx;
        tx = put(buf, tx, r.y + 2, &format!(" {c} "), inner_right, style);
        app.hits.menu_tabs.push((Rect::new(start, r.y + 2, tx - start, 1), i));
        tx += 1;
        last = i;
    }
    if first > 0 {
        put(buf, r.x + 1, r.y + 2, "‹", inner_right, base.fg(t.accent));
        app.hits.menu_tabs.push((Rect::new(r.x + 1, r.y + 2, 1, 1), first - 1));
    }
    if last + 1 < cats.len() {
        put(buf, inner_right - 1, r.y + 2, "›", inner_right, base.fg(t.accent));
        app.hits.menu_tabs.push((Rect::new(inner_right - 1, r.y + 2, 1, 1), last + 1));
    }
    let [hz, ..] = t.border.chars();
    for x in r.x + 1..inner_right {
        buf[(x, r.y + 3)].set_symbol(hz).set_style(dimmed);
    }

    // Items.
    let items = app.menu_items();
    let list_top = r.y + 4;
    let visible = h.saturating_sub(6) as usize;
    let ko = &mut app.kickoff;
    ko.rows = visible;
    ko.selected = ko.selected.min(items.len().saturating_sub(1));
    if ko.selected < ko.scroll {
        ko.scroll = ko.selected;
    } else if visible > 0 && ko.selected >= ko.scroll + visible {
        ko.scroll = ko.selected + 1 - visible;
    }
    let (selected, scroll) = (ko.selected, ko.scroll);
    if items.is_empty() {
        put(buf, r.x + 3, list_top, "No matches", inner_right, dimmed);
    }
    let name_w = 22usize.min(w as usize / 2);
    for (row, (i, item)) in items.iter().enumerate().skip(scroll).take(visible).enumerate() {
        let y = list_top + row as u16;
        let (name, desc) = app.menu_label(item);
        let sel = i == selected;
        let (ns, ds) = if sel {
            let s = Style::new().bg(t.menu_sel_bg).fg(t.menu_sel_fg);
            (s.add_modifier(Modifier::BOLD), s)
        } else {
            (base.add_modifier(Modifier::BOLD), dimmed)
        };
        let row_rect = Rect::new(r.x + 1, y, w - 2, 1);
        if sel {
            fill(buf, row_rect, ns);
        }
        let nx = put(buf, r.x + 3, y, &truncate(&name, name_w), inner_right, ns);
        put(buf, nx.max(r.x + 3 + name_w as u16 + 2), y, &desc, inner_right - 1, ds);
        app.hits.menu_items.push((row_rect, i));
    }
    if items.len() > visible && visible > 0 {
        // Scrollbar on the right border: the thumb's size and place show the visible part.
        let bar = Rect::new(inner_right, list_top, 1, visible as u16);
        let thumb = (visible * visible / items.len()).max(1);
        let at = scroll * (visible - thumb) / (items.len() - visible);
        for row in 0..visible {
            let (sym, fg) = if (at..at + thumb).contains(&row) { ("┃", t.accent) } else { ("│", t.menu_dim) };
            buf[(bar.x, bar.y + row as u16)].set_symbol(sym).set_style(Style::new().bg(t.menu_bg).fg(fg));
        }
        app.hits.menu_scrollbar = Some(bar);
        let info = format!(" {}/{} ", selected + 1, items.len());
        put(buf, inner_right - info.len() as u16 - 1, r.bottom() - 1, &info, inner_right, dimmed);
    }
    put(buf, r.x + 2, r.bottom() - 2, "Enter launch  Esc close  Left/Right category", inner_right, dimmed);
    Some((cx, r.y + 1))
}

fn draw_history(buf: &mut Buffer, desktop: Rect, app: &mut App) {
    let t = app.theme.clone();
    let w = 60.min(desktop.width);
    let n = app.notes.items.len();
    let h = ((n.max(1) as u16) + 4).min(desktop.height).min(20);
    if w < 20 || h < 5 {
        return;
    }
    let y = if app.panel_top() { desktop.y } else { desktop.bottom() - h };
    let r = Rect::new(desktop.right() - w, y, w, h);
    draw_box(buf, r, &t, t.accent, t.notif_bg, Some("Notifications"));
    app.hits.history = Some(r);
    let base = Style::new().bg(t.notif_bg).fg(t.notif_fg);
    let dimmed = base.fg(t.menu_dim);
    if n == 0 {
        put(buf, r.x + 2, r.y + 1, "No notifications", r.right() - 1, dimmed);
    }
    for (row, note) in app.notes.items.iter().rev().take(h as usize - 4).enumerate() {
        let y = r.y + 1 + row as u16;
        let x = put(buf, r.x + 2, y, &note.created_at.format("%H:%M ").to_string(), r.right() - 1, dimmed);
        let x = put(buf, x, y, &note.title, r.right() - 1, base.add_modifier(Modifier::BOLD));
        let body = note.body.replace('\n', " ");
        let sep = if note.title.is_empty() { "" } else { ": " };
        put(buf, x, y, &format!("{sep}{body}"), r.right() - 2, base);
    }
    put(buf, r.x + 2, r.bottom() - 2, "c clear  any other key closes", r.right() - 1, dimmed);
}

fn draw_switcher(buf: &mut Buffer, desktop: Rect, app: &mut App) {
    let Some(sw) = &app.switcher else { return };
    let t = app.theme.clone();
    let rows: Vec<(String, String, bool)> =
        sw.list.iter().filter_map(|&id| app.wm.get(id)).map(|w| (w.title(), w.command.clone(), w.minimized)).collect();
    let w = 64.min(desktop.width);
    let h = (rows.len() as u16 + 4).min(desktop.height);
    if w < 20 || h < 5 {
        return;
    }
    let r = Rect::new(desktop.x + (desktop.width - w) / 2, desktop.y + (desktop.height - h) / 2, w, h);
    draw_box(buf, r, &t, t.accent, t.menu_bg, Some("Windows"));
    let base = Style::new().bg(t.menu_bg).fg(t.menu_fg);
    let dimmed = base.fg(t.menu_dim);
    let visible = (h - 4) as usize;
    let first = sw.idx.saturating_sub(visible.saturating_sub(1));
    for (row, (i, (title, cmd, minimized))) in rows.iter().enumerate().skip(first).take(visible).enumerate() {
        let y = r.y + 1 + row as u16;
        let sel = i == sw.idx;
        let row_rect = Rect::new(r.x + 1, y, w - 2, 1);
        let (ts, cs) = if sel {
            let s = Style::new().bg(t.menu_sel_bg).fg(t.menu_sel_fg);
            fill(buf, row_rect, s);
            (s.add_modifier(Modifier::BOLD), s)
        } else {
            (base.add_modifier(Modifier::BOLD), dimmed)
        };
        let title = if *minimized { format!("({title})") } else { title.clone() };
        let name_w = (w as usize * 3 / 5).saturating_sub(4);
        put(buf, r.x + 3, y, &truncate(&title, name_w), r.right() - 1, ts);
        put(
            buf,
            r.x + 4 + name_w as u16,
            y,
            &truncate(cmd, (w as usize).saturating_sub(name_w + 7)),
            r.right() - 2,
            cs,
        );
        app.hits.switcher.push((row_rect, i));
    }
    let hint = if sw.releases { "release Alt to switch  Esc cancel" } else { "Tab next  Enter switch  Esc cancel" };
    put(buf, r.x + 2, r.bottom() - 2, hint, r.right() - 1, dimmed);
}

/// Copy mode: status line over the window's last row, and the copy cursor.
fn draw_copy(buf: &mut Buffer, app: &App) -> Option<(u16, u16)> {
    let c = app.copy.as_ref()?;
    let w = app.wm.get(c.id).filter(|w| app.wm.visible(w))?;
    let area = w.content().intersection(buf.area);
    if area.height == 0 {
        return None;
    }
    let t = &app.theme;
    let y = area.bottom() - 1;
    let style = Style::new().bg(t.accent).fg(t.panel_bg).add_modifier(Modifier::BOLD);
    let (text, typing) = match &c.search {
        Some((p, fwd)) => (format!(" {}{p}", if *fwd { '/' } else { '?' }), true),
        None if !c.message.is_empty() => (format!(" COPY  {} ", c.message), false),
        None => (" COPY  v select  y copy  / search  n next  q quit ".to_string(), false),
    };
    fill(buf, Rect::new(area.x, y, area.width, 1), style);
    let end = put(buf, area.x, y, &truncate(&text, area.width as usize), area.right(), style);
    if typing {
        return Some((end.min(area.right() - 1), y));
    }
    let (row, col) = w.term.vi_cursor()?;
    (row < area.height - 1 && col < area.width).then_some((area.x + col, area.y + row))
}

/// A centered list box. Returns the inner rows' rect.
fn list_box(buf: &mut Buffer, desktop: Rect, app: &mut App, title: &str, w: u16, rows: u16) -> Option<Rect> {
    let t = app.theme.clone();
    let w = w.min(desktop.width);
    let h = (rows + 4).min(desktop.height);
    if w < 20 || h < 5 {
        return None;
    }
    let r = Rect::new(desktop.x + (desktop.width - w) / 2, desktop.y + (desktop.height - h) / 2, w, h);
    draw_box(buf, r, &t, t.accent, t.menu_bg, Some(title));
    app.hits.overlay = Some(r);
    Some(Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 4))
}

fn draw_clipboard(buf: &mut Buffer, desktop: Rect, app: &mut App) {
    let clips: Vec<String> = app.clips.iter().cloned().collect();
    let Some(inner) = list_box(buf, desktop, app, "Clipboard", 70, clips.len() as u16) else { return };
    let t = app.theme.clone();
    let base = Style::new().bg(t.menu_bg).fg(t.menu_fg);
    let dimmed = base.fg(t.menu_dim);
    for (i, clip) in clips.iter().enumerate().take(inner.height as usize) {
        let y = inner.y + i as u16;
        let row = Rect::new(inner.x, y, inner.width, 1);
        let sel = i == app.clip_sel;
        let style = if sel { Style::new().bg(t.menu_sel_bg).fg(t.menu_sel_fg) } else { base };
        if sel {
            fill(buf, row, style);
        }
        let n = clip.lines().count();
        let first = clip.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        let more = if n > 1 { format!("  (+{} lines)", n - 1) } else { String::new() };
        let num = if i < 9 { format!("{} ", i + 1) } else { "  ".into() };
        let x = put(buf, inner.x + 1, y, &num, inner.right(), if sel { style } else { dimmed });
        let avail = (inner.right() - x) as usize;
        let x = put(buf, x, y, &truncate(first, avail.saturating_sub(more.len() + 1)), inner.right(), style);
        put(buf, x, y, &more, inner.right(), if sel { style } else { dimmed });
        app.hits.list.push((row, i));
    }
    put(buf, inner.x + 1, inner.bottom() + 1, "Enter/1-9 paste  d delete  c clear  Esc close", inner.right(), dimmed);
}

fn draw_help(buf: &mut Buffer, desktop: Rect, app: &mut App) {
    let rows = app.help_rows();
    let height = desktop.height.saturating_sub(4);
    let Some(inner) = list_box(buf, desktop, app, "Keys", 96, height) else { return };
    let t = app.theme.clone();
    let base = Style::new().bg(t.menu_bg).fg(t.menu_fg);
    let dimmed = base.fg(t.menu_dim);
    let head = base.fg(t.accent).add_modifier(Modifier::BOLD);
    let filter = format!("Filter: {}", app.help.filter);
    put(buf, inner.x + 1, inner.y, &filter, inner.right(), dimmed);

    // Lines with a heading whenever the section changes.
    let mut lines: Vec<(Option<&str>, String, String)> = Vec::new();
    let mut section = "";
    for (sec, key, action) in &rows {
        if *sec != section {
            section = sec;
            lines.push((Some(sec), String::new(), String::new()));
        }
        lines.push((None, key.clone(), action.clone()));
    }
    let visible = inner.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    app.help.scroll = app.help.scroll.min(max_scroll);
    let key_w = 34.min(inner.width as usize / 2);
    for (i, (sec, key, action)) in lines.iter().skip(app.help.scroll).take(visible).enumerate() {
        let y = inner.y + 2 + i as u16;
        match sec {
            Some(s) => {
                put(buf, inner.x + 1, y, s, inner.right(), head);
            }
            None => {
                put(buf, inner.x + 3, y, &truncate(key, key_w), inner.right(), base.add_modifier(Modifier::BOLD));
                put(buf, inner.x + 4 + key_w as u16, y, action, inner.right() - 1, dimmed);
            }
        }
    }
    if rows.is_empty() {
        put(buf, inner.x + 3, inner.y + 2, "No matches", inner.right(), dimmed);
    }
    let plural = if rows.len() == 1 { "" } else { "s" };
    let footer = format!("type to filter  arrows/PgUp/PgDn scroll  Esc close  ({} key{plural})", rows.len());
    put(buf, inner.x + 1, inner.bottom() + 1, &footer, inner.right(), dimmed);
}

/// System Settings: pages on the left, the selected page's rows on the right.
fn draw_settings(buf: &mut Buffer, desktop: Rect, app: &mut App) -> Option<(u16, u16)> {
    let title = if app.settings.as_ref()?.modified() { "System Settings (unsaved)" } else { "System Settings" };
    let inner = list_box(buf, desktop, app, title, 100, desktop.height.saturating_sub(6))?;
    let t = app.theme.clone();
    let s = app.settings.as_mut()?;
    let base = Style::new().bg(t.menu_bg).fg(t.menu_fg);
    let dimmed = base.fg(t.menu_dim);
    let sel_style = Style::new().bg(t.menu_sel_bg).fg(t.menu_sel_fg);

    let pages_w = 14.min(inner.width / 3);
    for (i, page) in PAGES.iter().enumerate().take(inner.height as usize) {
        let row = Rect::new(inner.x, inner.y + i as u16, pages_w, 1);
        let style = if i == s.page { sel_style } else { base };
        fill(buf, row, style);
        put(buf, row.x + 1, row.y, page.name, row.right(), style);
        app.hits.menu_tabs.push((row, i));
    }
    let bar = inner.x + pages_w;
    for y in inner.y..inner.bottom() {
        put(buf, bar, y, t.border.chars()[1], bar + 1, dimmed);
    }

    let x0 = bar + 2;
    let right = inner.right().saturating_sub(1);
    if x0 + 10 >= right {
        return None;
    }
    let label_w = 26.min((right - x0) / 2);
    let vx = x0 + label_w + 1;
    let rows = s.rows();
    let visible = inner.height.saturating_sub(2) as usize;
    if s.sel < s.scroll {
        // Keep a heading right above the selection in view.
        s.scroll = s.sel.saturating_sub(usize::from(s.sel > 0 && !rows[s.sel - 1].selectable()));
    } else if s.sel >= s.scroll + visible {
        s.scroll = s.sel + 1 - visible;
    }
    let widgets = s.widgets();
    let mut cursor = None;
    for (i, row) in rows.iter().enumerate().skip(s.scroll).take(visible) {
        let y = inner.y + (i - s.scroll) as u16;
        let selected = i == s.sel;
        let line = Rect::new(x0 - 1, y, right - x0 + 1, 1);
        let style = if selected && s.edit.is_none() { sel_style } else { base };
        if selected {
            fill(buf, line, style);
        }
        let dim = if selected && s.edit.is_none() { style } else { dimmed };
        let (label, value, value_style): (String, String, Style) = match *row {
            Row::Heading(h) => {
                put(buf, x0, y, h, right, base.fg(t.accent).add_modifier(Modifier::BOLD));
                continue;
            }
            Row::Field(f) => {
                let v = s.value(f).unwrap_or_default();
                let arrows = |v: &str| if selected { format!("< {v} >") } else { v.to_string() };
                let (text, st) = match f.kind {
                    Kind::Bool => ((if v == "true" { "[x]" } else { "[ ]" }).to_string(), style),
                    Kind::Choice(_) | Kind::Int { .. } | Kind::Ratio { .. } => (arrows(&v), style),
                    Kind::Text if v.is_empty() => ("(empty)".into(), dim),
                    Kind::Color if !s.is_set(f) => (format!("   {v} (preset)"), dim),
                    Kind::Color => (format!("   {v}"), style),
                    Kind::Text | Kind::Key => (v, style),
                };
                if matches!(f.kind, Kind::Color)
                    && let Some(c) = crate::theme::parse_color(&s.value(f).unwrap_or_default())
                {
                    put(buf, vx, y, "  ", right, Style::new().bg(t.mode.adapt(c)));
                }
                (f.label.to_string(), text, st)
            }
            Row::Widget(w) => {
                let (name, on) = widgets.get(w).cloned().unwrap_or_default();
                let mark = if on { "[x]" } else { "[ ]" };
                (format!("  {mark} {name}"), String::new(), style)
            }
            Row::Binding(table, b) => {
                let (key, action) = s.bindings(table).get(b).cloned().unwrap_or_default();
                if action == "none" {
                    (format!("  {key}"), "(off)".into(), dim)
                } else {
                    (format!("  {key}"), action, style)
                }
            }
            Row::AddBinding(_) => ("  Add a key...".into(), String::new(), dim),
        };
        put(buf, x0, y, &truncate(&label, label_w as usize), vx, style.add_modifier(Modifier::BOLD));
        if !selected || s.edit.is_none() {
            put(buf, vx, y, &truncate(&value, (right - vx) as usize), right, value_style);
            app.hits.list.push((line, i));
            continue;
        }
        let edit_style = base.add_modifier(Modifier::BOLD);
        match s.edit.as_ref()? {
            Edit::Text { text, .. } | Edit::Action { text, .. } => {
                if let Some(Edit::Action { key, .. }) = &s.edit
                    && matches!(row, Row::AddBinding(_))
                {
                    fill(buf, Rect::new(x0, y, label_w, 1), base);
                    put(buf, x0, y, &truncate(&format!("  {key}"), label_w as usize), vx, edit_style);
                }
                let shown = truncate_start(text, (right - vx).saturating_sub(1) as usize);
                let end = put(buf, vx, y, &shown, right, edit_style.add_modifier(Modifier::UNDERLINED));
                cursor = Some((end, y));
            }
            Edit::CaptureField(_) | Edit::CaptureBinding(_) => {
                put(buf, vx, y, "press the new key (Esc cancels)", right, base.fg(t.accent));
            }
        }
        app.hits.list.push((line, i));
    }

    let (msg, msg_style) = if s.status.is_empty() { (s.help(), dimmed) } else { (s.status.clone(), base.fg(t.urgent)) };
    put(buf, x0, inner.bottom().saturating_sub(1), &truncate(&msg, (right - x0) as usize), right, msg_style);
    let footer = if s.confirm {
        "Unsaved changes:  s save  d discard  Esc keep editing"
    } else if s.edit.is_some() {
        "Enter apply  Esc cancel  ctrl+u clear"
    } else {
        "Tab page  arrows select/change  Enter edit  Del default  ctrl+s save  Esc close"
    };
    let footer_style = if s.confirm { base.fg(t.accent).add_modifier(Modifier::BOLD) } else { dimmed };
    put(buf, inner.x + 1, inner.bottom() + 1, footer, inner.right(), footer_style);
    cursor
}

fn draw_win_menu(buf: &mut Buffer, desktop: Rect, app: &mut App) {
    let Some(menu) = &app.win_menu else { return };
    let (id, x, y, sel) = (menu.id, menu.x, menu.y, menu.sel);
    let labels: Vec<String> = menu.items.clone().into_iter().map(|i| app.win_menu_label(i, id)).collect();
    let t = app.theme.clone();
    let w = (labels.iter().map(|l| l.width()).max().unwrap_or(10) as u16 + 4).min(desktop.width);
    let h = (labels.len() as u16 + 2).min(desktop.height);
    let x = x.min(desktop.right().saturating_sub(w));
    let y = y.max(desktop.y).min(desktop.bottom().saturating_sub(h));
    if w < 3 || h < 3 {
        return;
    }
    let r = Rect::new(x, y, w, h);
    draw_box(buf, r, &t, t.accent, t.menu_bg, None);
    app.hits.overlay = Some(r);
    let base = Style::new().bg(t.menu_bg).fg(t.menu_fg);
    for (i, label) in labels.iter().enumerate().take(h as usize - 2) {
        let row = Rect::new(r.x + 1, r.y + 1 + i as u16, w - 2, 1);
        let style = if i == sel { Style::new().bg(t.menu_sel_bg).fg(t.menu_sel_fg) } else { base };
        fill(buf, row, style);
        put(buf, row.x + 1, row.y, label, row.right(), style);
        app.hits.list.push((row, i));
    }
}

fn draw_rename(buf: &mut Buffer, desktop: Rect, app: &mut App) -> Option<(u16, u16)> {
    let (target, text) = app.rename.clone()?;
    let title = match target {
        RenameTarget::Window(_) => "Rename window".to_string(),
        RenameTarget::Desktop(d) => format!("Rename desktop {}", d + 1),
    };
    let inner = list_box(buf, desktop, app, &title, 60, 1)?;
    let t = &app.theme;
    let base = Style::new().bg(t.menu_bg).fg(t.menu_fg);
    let label = if matches!(target, RenameTarget::Window(_)) { "Title: " } else { "Name: " };
    let x = put(buf, inner.x + 1, inner.y, label, inner.right(), base.fg(t.menu_dim));
    let shown = truncate(&text, (inner.right() - x - 1) as usize);
    let end = put(buf, x, inner.y, &shown, inner.right(), base.add_modifier(Modifier::BOLD));
    put(
        buf,
        inner.x + 1,
        inner.bottom() + 1,
        "Enter save (empty resets)  Esc cancel",
        inner.right(),
        base.fg(t.menu_dim),
    );
    Some((end, inner.y))
}

/// KRunner: a search line near the top with mixed results below.
fn draw_runner(buf: &mut Buffer, desktop: Rect, app: &mut App) -> Option<(u16, u16)> {
    let items = app.runner_items();
    let t = app.theme.clone();
    let w = 72.min(desktop.width);
    let shown = items.len().min(10) as u16;
    let top = desktop.y + desktop.height / 6;
    let room = desktop.bottom().saturating_sub(top);
    let h = (shown + 3 + u16::from(!items.is_empty())).min(room);
    if w < 24 || h < 3 {
        return None;
    }
    // Results that do not fit below the search line are scrolled, not drawn past the box.
    let shown = shown.min(h.saturating_sub(4));
    let r = Rect::new(desktop.x + (desktop.width - w) / 2, top, w, h);
    draw_box(buf, r, &t, t.accent, t.menu_bg, Some("Run"));
    app.hits.overlay = Some(r);
    let base = Style::new().bg(t.menu_bg).fg(t.menu_fg);
    let dimmed = base.fg(t.menu_dim);
    let x = put(buf, r.x + 2, r.y + 1, "> ", r.right() - 1, base.fg(t.accent).add_modifier(Modifier::BOLD));
    let query = truncate(&app.runner.query, (r.right() - x - 2) as usize);
    let cursor_x = put(buf, x, r.y + 1, &query, r.right() - 1, base.add_modifier(Modifier::BOLD));
    if items.is_empty() {
        if app.runner.query.is_empty() {
            let hint = "apps, windows, 2*21, ~/dir, man ls, actions";
            put(buf, x, r.y + 1, hint, r.right() - 1, dimmed);
        }
        return Some((cursor_x, r.y + 1));
    }
    app.runner.sel = app.runner.sel.min(items.len() - 1);
    if shown == 0 {
        return Some((cursor_x, r.y + 1));
    }
    let first = app.runner.sel.saturating_sub(shown as usize - 1);
    for (row, (i, item)) in items.iter().enumerate().skip(first).take(shown as usize).enumerate() {
        let y = r.y + 3 + row as u16;
        let row_rect = Rect::new(r.x + 1, y, w - 2, 1);
        let sel = i == app.runner.sel;
        let style = if sel { Style::new().bg(t.menu_sel_bg).fg(t.menu_sel_fg) } else { base };
        if sel {
            fill(buf, row_rect, style);
        }
        let (kind, text) = app.runner_label(item);
        let room = (w as usize).saturating_sub(13);
        // Paths are most useful at their end.
        let text = if kind == "Folder" { truncate_start(&text, room) } else { truncate(&text, room) };
        put(buf, r.x + 2, y, kind, r.x + 10, if sel { style } else { dimmed });
        put(buf, r.x + 10, y, &text, r.right() - 1, style);
        app.hits.list.push((row_rect, i));
    }
    let [hz, ..] = t.border.chars();
    for x in r.x + 1..r.right() - 1 {
        buf[(x, r.y + 2)].set_symbol(hz).set_style(dimmed);
    }
    Some((cursor_x, r.y + 1))
}

/// Brief centered label, e.g. after switching desktops.
fn draw_osd(buf: &mut Buffer, desktop: Rect, theme: &Theme, text: &str) {
    let w = (text.width() as u16 + 6).min(desktop.width);
    if desktop.height < 5 || w < 8 {
        return;
    }
    let r = Rect::new(desktop.x + (desktop.width - w) / 2, desktop.y + desktop.height / 2 - 1, w, 3);
    draw_box(buf, r, theme, theme.accent, theme.menu_bg, None);
    let style = Style::new().bg(theme.menu_bg).fg(theme.menu_fg).add_modifier(Modifier::BOLD);
    put(buf, r.x + 3, r.y + 1, text, r.right() - 1, style);
}

/// Region screenshot: the selection is shown inverted, with instructions on top.
fn draw_region(buf: &mut Buffer, desktop: Rect, app: &App) {
    if let Some(r) = app.region.rect() {
        let r = r.intersection(buf.area);
        for y in r.top()..r.bottom() {
            for x in r.left()..r.right() {
                let cell = &mut buf[(x, y)];
                cell.modifier.toggle(Modifier::REVERSED);
            }
        }
        let size = format!(" {}x{} ", r.width, r.height);
        let style = Style::new().bg(app.theme.accent).fg(app.theme.panel_bg).add_modifier(Modifier::BOLD);
        put(buf, r.x, r.bottom().min(buf.area.bottom() - 1), &size, buf.area.right(), style);
    } else {
        draw_hint(buf, desktop, &app.theme, " Screenshot: drag to select an area, Esc cancels ");
    }
}

fn draw_hint(buf: &mut Buffer, desktop: Rect, theme: &Theme, text: &str) {
    let w = (text.width() as u16 + 2).min(desktop.width);
    let r = Rect::new(desktop.x + (desktop.width - w) / 2, desktop.y, w, 3);
    draw_box(buf, r, theme, theme.accent, theme.menu_bg, None);
    put(buf, r.x + 1, r.y + 1, text, r.right() - 1, Style::new().bg(theme.menu_bg).fg(theme.menu_fg));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_truncates() {
        assert_eq!(truncate("hello world", 6), "hello~");
        assert_eq!(truncate("hi", 6), "hi");
        // Placeholder cells must take exactly one column, or ratatui would misplace cells.
        assert_eq!(crate::graphics::placeholder(3, 7).unwrap().width(), 1);
        let t = chrono::Local::now();
        assert_eq!(strftime(&t, "%Q"), "?");
        assert_eq!(strftime(&t, "%Y").len(), 4);
        assert_eq!(truncate_start("/a/b/target-dir", 11), "~target-dir");
        assert_eq!(truncate_start("short", 11), "short");
        assert_eq!(wrap("one two three four", 9, 5), vec!["one two", "three", "four"]);
        assert_eq!(wrap("a b c d e f", 3, 2), vec!["a b", "c~"]);
    }
}

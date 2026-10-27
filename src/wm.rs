use std::collections::HashMap;

use ratatui::layout::Rect;

use crate::term::Term;

pub const MIN_W: u16 = 16;
pub const MIN_H: u16 = 4;
pub const MAX_DESKTOPS: usize = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Floating,
    /// One large master window, the rest stacked beside it.
    Master,
    /// Every window fills the desktop; the focused one is on top.
    Monocle,
    Grid,
    Columns,
    /// Binary space partitioning: each window takes half of the space left (dwindle).
    Bsp,
}

impl Layout {
    /// Order used by `cycle_layout`.
    pub const ALL: [Layout; 6] =
        [Layout::Floating, Layout::Master, Layout::Monocle, Layout::Grid, Layout::Columns, Layout::Bsp];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "floating" | "float" => Self::Floating,
            "tiling" | "master" | "tile" => Self::Master,
            "monocle" => Self::Monocle,
            "grid" => Self::Grid,
            "columns" => Self::Columns,
            "bsp" | "dwindle" => Self::Bsp,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Floating => "floating",
            Self::Master => "master",
            Self::Monocle => "monocle",
            Self::Grid => "grid",
            Self::Columns => "columns",
            Self::Bsp => "bsp",
        }
    }

    pub fn is_tiled(self) -> bool {
        self != Self::Floating
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&l| l == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Maximize,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub bottom: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Title,
    Button(Button),
    Border(Edges),
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

pub struct Window {
    pub id: u64,
    pub term: Term,
    /// Command line shown to the user and matched by rules.
    pub command: String,
    /// What was launched: None for the login shell, Some(command) otherwise. Saved in sessions.
    pub launch: Option<String>,
    pub name: String,
    /// Geometry while floating and not maximized.
    pub float_rect: Rect,
    /// Floating geometry to go back to after a quick tile.
    pub restore_rect: Option<Rect>,
    /// Current on-screen frame (border included), recomputed by `Wm::arrange`.
    pub frame: Rect,
    pub maximized: bool,
    pub minimized: bool,
    /// In tiling layout, a floating window keeps its own geometry.
    pub floating: bool,
    pub urgent: bool,
    pub exit_code: Option<u32>,
    pub desktop: usize,
    /// Kept above normal windows ("always on top").
    pub above: bool,
    /// Title set by the user, replacing the program's.
    pub title_override: Option<String>,
}

impl Window {
    pub fn title(&self) -> String {
        if let Some(t) = &self.title_override {
            return t.clone();
        }
        match self.term.title.as_deref() {
            Some(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => self.name.clone(),
        }
    }

    pub fn content(&self) -> Rect {
        content_rect(self.frame)
    }
}

pub fn content_rect(frame: Rect) -> Rect {
    Rect {
        x: frame.x + 1,
        y: frame.y + 1,
        width: frame.width.saturating_sub(2),
        height: frame.height.saturating_sub(2),
    }
}

/// Title bar buttons, right-aligned: [_][^][x]
pub fn buttons(frame: Rect) -> Vec<(Rect, Button)> {
    if frame.width < 14 {
        return vec![(Rect::new(frame.right().saturating_sub(4), frame.y, 3, 1), Button::Close)];
    }
    let r = frame.right();
    vec![
        (Rect::new(r - 10, frame.y, 3, 1), Button::Minimize),
        (Rect::new(r - 7, frame.y, 3, 1), Button::Maximize),
        (Rect::new(r - 4, frame.y, 3, 1), Button::Close),
    ]
}

pub struct Wm {
    /// Z-order, bottom first.
    pub windows: Vec<Window>,
    /// Creation order, used for tiling and focus cycling.
    pub order: Vec<u64>,
    /// Most recently focused first, used by the window switcher.
    pub mru: Vec<u64>,
    pub focused: Option<u64>,
    /// Current virtual desktop (0-based).
    pub desktop: usize,
    pub desktops: usize,
    layouts: Vec<Layout>,
    /// Tiled layout to return to when toggling back from floating.
    last_tiled: Vec<Layout>,
    ratios: Vec<f32>,
    pub area: Rect,
    pub gaps: u16,
    next_id: u64,
    cascade: u16,
}

impl Wm {
    pub fn new(desktops: usize, layout: Layout, master_ratio: f32, gaps: u16) -> Self {
        let desktops = desktops.clamp(1, MAX_DESKTOPS);
        Self {
            windows: Vec::new(),
            order: Vec::new(),
            mru: Vec::new(),
            focused: None,
            desktop: 0,
            desktops,
            layouts: vec![layout; MAX_DESKTOPS],
            last_tiled: vec![if layout.is_tiled() { layout } else { Layout::Master }; MAX_DESKTOPS],
            ratios: vec![master_ratio.clamp(0.1, 0.9); MAX_DESKTOPS],
            area: Rect::default(),
            gaps,
            next_id: 1,
            cascade: 0,
        }
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn layout(&self) -> Layout {
        self.layouts[self.desktop]
    }

    pub fn layout_of(&self, desktop: usize) -> Layout {
        self.layouts[desktop.min(MAX_DESKTOPS - 1)]
    }

    pub fn master_ratio(&self) -> f32 {
        self.ratios[self.desktop]
    }

    pub fn ratio_of(&self, desktop: usize) -> f32 {
        self.ratios[desktop.min(MAX_DESKTOPS - 1)]
    }

    pub fn set_master_ratio(&mut self, ratio: f32) {
        self.ratios[self.desktop] = ratio.clamp(0.1, 0.9);
        self.arrange();
    }

    /// Restore per-desktop settings (from a saved session).
    pub fn set_desktop_settings(&mut self, desktop: usize, layout: Layout, ratio: f32) {
        if desktop < MAX_DESKTOPS {
            self.layouts[desktop] = layout;
            if layout.is_tiled() {
                self.last_tiled[desktop] = layout;
            }
            self.ratios[desktop] = ratio.clamp(0.1, 0.9);
        }
    }

    pub fn set_desktops(&mut self, n: usize) {
        self.desktops = n.clamp(1, MAX_DESKTOPS);
        let last = self.desktops - 1;
        for w in &mut self.windows {
            w.desktop = w.desktop.min(last);
        }
        if self.desktop > last {
            self.switch_desktop(last);
        }
    }

    pub fn on_current(&self, w: &Window) -> bool {
        w.desktop == self.desktop
    }

    pub fn visible(&self, w: &Window) -> bool {
        !w.minimized && self.on_current(w)
    }

    /// Default geometry for a new floating window, cascading from the center.
    pub fn place(&mut self, width_pct: u16, height_pct: u16) -> Rect {
        let a = self.area;
        let w = (a.width as u32 * width_pct.clamp(10, 100) as u32 / 100) as u16;
        let h = (a.height as u32 * height_pct.clamp(10, 100) as u32 / 100) as u16;
        let w = w.max(MIN_W).min(a.width);
        let h = h.max(MIN_H).min(a.height);
        let base_x = a.x + (a.width - w) / 2;
        let base_y = a.y + (a.height - h) / 2;
        let step = self.cascade % 6;
        self.cascade += 1;
        let x = (base_x + step * 3).min(a.right().saturating_sub(w));
        let y = (base_y + step).min(a.bottom().saturating_sub(h));
        Rect::new(x, y, w, h)
    }

    pub fn get(&self, id: u64) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    pub fn focused_window(&self) -> Option<&Window> {
        self.get(self.focused?)
    }

    pub fn focused_mut(&mut self) -> Option<&mut Window> {
        let id = self.focused?;
        self.get_mut(id)
    }

    pub fn remove(&mut self, id: u64) -> Option<Window> {
        let idx = self.windows.iter().position(|w| w.id == id)?;
        let win = self.windows.remove(idx);
        self.order.retain(|&o| o != id);
        self.mru.retain(|&o| o != id);
        if self.focused == Some(id) {
            self.focused = None;
            self.focus_top();
        }
        self.arrange();
        Some(win)
    }

    /// Focus the most recently used visible window on the current desktop.
    pub fn focus_top(&mut self) {
        let next = self.mru.iter().copied().find(|&id| self.get(id).is_some_and(|w| self.visible(w)));
        match next {
            Some(id) => self.focus(id),
            None => self.focused = None,
        }
    }

    /// Focus and raise a window, restoring it and switching to its desktop if needed.
    pub fn focus(&mut self, id: u64) {
        let Some(idx) = self.windows.iter().position(|w| w.id == id) else {
            return;
        };
        let mut win = self.windows.remove(idx);
        let mut rearrange = win.minimized;
        win.minimized = false;
        win.urgent = false;
        if win.desktop != self.desktop {
            self.desktop = win.desktop;
            rearrange = true;
        }
        self.windows.push(win);
        self.focused = Some(id);
        self.mru.retain(|&o| o != id);
        self.mru.insert(0, id);
        if rearrange {
            self.arrange();
        }
    }

    pub fn minimize(&mut self, id: u64) {
        if let Some(w) = self.get_mut(id) {
            w.minimized = true;
        }
        if self.focused == Some(id) {
            self.focused = None;
            self.focus_top();
        }
        self.arrange();
    }

    pub fn toggle_maximize(&mut self, id: u64) {
        if let Some(w) = self.get_mut(id) {
            w.maximized = !w.maximized;
        }
        self.arrange();
    }

    pub fn switch_desktop(&mut self, desktop: usize) {
        if desktop >= self.desktops || desktop == self.desktop {
            return;
        }
        self.desktop = desktop;
        self.focused = None;
        self.focus_top();
        self.arrange();
    }

    pub fn move_to_desktop(&mut self, id: u64, desktop: usize) {
        if desktop >= self.desktops {
            return;
        }
        if let Some(w) = self.get_mut(id) {
            w.desktop = desktop;
        }
        if self.focused == Some(id) {
            self.focused = None;
            self.focus_top();
        }
        self.arrange();
    }

    /// Windows on the current desktop, most recently used first (for the switcher).
    pub fn switcher_list(&self) -> Vec<u64> {
        let mut list: Vec<u64> =
            self.mru.iter().copied().filter(|&id| self.get(id).is_some_and(|w| self.on_current(w))).collect();
        for &id in &self.order {
            if !list.contains(&id) && self.get(id).is_some_and(|w| self.on_current(w)) {
                list.push(id);
            }
        }
        list
    }

    /// Cycle focus through the current desktop's windows in creation order.
    pub fn cycle(&mut self, forward: bool) {
        let ids: Vec<u64> =
            self.order.iter().copied().filter(|&id| self.get(id).is_some_and(|w| self.on_current(w))).collect();
        if ids.is_empty() {
            return;
        }
        let n = ids.len();
        let cur = self.focused.and_then(|f| ids.iter().position(|&o| o == f));
        let next = match cur {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None => 0,
        };
        self.focus(ids[next]);
    }

    pub fn is_tiled(&self, w: &Window) -> bool {
        self.layout_of(w.desktop).is_tiled() && !w.floating && !w.maximized && !w.minimized
    }

    /// Switch between floating and the last tiled layout used on this desktop.
    pub fn toggle_layout(&mut self) {
        let next = if self.layout().is_tiled() { Layout::Floating } else { self.last_tiled[self.desktop] };
        self.set_layout(next);
    }

    pub fn set_layout(&mut self, layout: Layout) {
        self.layouts[self.desktop] = layout;
        if layout.is_tiled() {
            self.last_tiled[self.desktop] = layout;
            let d = self.desktop;
            for w in self.windows.iter_mut().filter(|w| w.desktop == d) {
                w.floating = false;
            }
        }
        self.arrange();
    }

    pub fn toggle_floating(&mut self, id: u64) {
        if let Some(w) = self.get_mut(id) {
            w.floating = !w.floating;
            w.maximized = false;
        }
        self.focus(id);
        self.arrange();
    }

    /// Move a window to the master slot in tiling layout.
    pub fn swap_master(&mut self, id: u64) {
        let tiled = self.tiled_ids();
        if !tiled.contains(&id) {
            return;
        }
        let Some(&master) = tiled.first() else { return };
        let target = if master == id { tiled.get(1).copied() } else { Some(id) };
        let Some(target) = target else { return };
        let (Some(a), Some(b)) =
            (self.order.iter().position(|&o| o == master), self.order.iter().position(|&o| o == target))
        else {
            return;
        };
        self.order.swap(a, b);
        self.arrange();
    }

    /// Quick tile (floating) or reorder (tiling) a window.
    pub fn quick_tile(&mut self, id: u64, side: Side) {
        let tiled = self.get(id).is_some_and(|w| self.is_tiled(w));
        if tiled {
            let Some(pos) = self.order.iter().position(|&o| o == id) else { return };
            match side {
                Side::Left => {
                    let item = self.order.remove(pos);
                    self.order.insert(0, item);
                }
                Side::Right => {
                    let item = self.order.remove(pos);
                    self.order.push(item);
                }
                // Swap with the neighbouring tile, skipping windows on other desktops,
                // floating or minimized ones.
                Side::Top | Side::Bottom => {
                    let tiled = self.tiled_ids();
                    let i = tiled.iter().position(|&t| t == id).unwrap_or(0);
                    let other = if side == Side::Top { i.checked_sub(1) } else { Some(i + 1) };
                    if let Some(&other) = other.and_then(|j| tiled.get(j))
                        && let Some(opos) = self.order.iter().position(|&o| o == other)
                    {
                        self.order.swap(pos, opos);
                    }
                }
            }
            self.arrange();
            return;
        }
        let a = self.area;
        let half_w = a.width / 2;
        let half_h = a.height / 2;
        let rect = match side {
            Side::Left => Rect::new(a.x, a.y, half_w, a.height),
            Side::Right => Rect::new(a.x + half_w, a.y, a.width - half_w, a.height),
            Side::Top => Rect::new(a.x, a.y, a.width, half_h),
            Side::Bottom => Rect::new(a.x, a.y + half_h, a.width, a.height - half_h),
        };
        if let Some(w) = self.get_mut(id) {
            if w.restore_rect.is_none() {
                w.restore_rect = Some(w.float_rect);
            }
            w.maximized = false;
            w.float_rect = rect;
        }
        self.arrange();
    }

    /// Give a quick-tiled window its pre-tile size back (used when dragging it away).
    pub fn untile(&mut self, id: u64) -> Option<Rect> {
        let w = self.get_mut(id)?;
        let r = w.restore_rect.take()?;
        w.float_rect.width = r.width;
        w.float_rect.height = r.height;
        Some(w.float_rect)
    }

    fn tiled_ids(&self) -> Vec<u64> {
        self.order
            .iter()
            .copied()
            .filter(|&id| self.get(id).is_some_and(|w| self.on_current(w) && self.is_tiled(w)))
            .collect()
    }

    fn tile_rects(&self) -> HashMap<u64, Rect> {
        let ids = self.tiled_ids();
        let g = self.gaps;
        let a = Rect {
            x: self.area.x + g,
            y: self.area.y + g.min(1),
            width: self.area.width.saturating_sub(2 * g),
            height: self.area.height.saturating_sub(2 * g.min(1)),
        };
        let rects = tile(self.layout(), ids.len(), a, self.master_ratio());
        // Leave the gap on the right and bottom of every tile that has a neighbour there.
        let gapped = rects.into_iter().map(|r| {
            let mut r = r;
            if r.right() < a.right() {
                r.width = r.width.saturating_sub(g);
            }
            if r.bottom() < a.bottom() {
                r.height = r.height.saturating_sub(g.min(1));
            }
            r
        });
        ids.into_iter().zip(gapped).collect()
    }

    /// Recompute frames on the current desktop and resize the programs inside.
    pub fn arrange(&mut self) {
        let tiles = self.tile_rects();
        let area = self.area;
        let desktop = self.desktop;
        for w in &mut self.windows {
            if w.minimized || w.desktop != desktop {
                continue;
            }
            w.frame = if w.maximized {
                area
            } else if let Some(r) = tiles.get(&w.id) {
                *r
            } else {
                w.float_rect = clamp_rect(w.float_rect, area);
                w.float_rect
            };
            let c = content_rect(w.frame);
            w.term.resize(c.height, c.width);
        }
    }

    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
        self.arrange();
    }

    /// Visible windows bottom to top: normal windows, then "always on top" ones.
    pub fn stacking(&self) -> impl Iterator<Item = &Window> {
        let visible = |above: bool| self.windows.iter().filter(move |w| self.visible(w) && w.above == above);
        visible(false).chain(visible(true))
    }

    pub fn hit_test(&self, x: u16, y: u16) -> Option<(u64, Hit)> {
        let stack: Vec<&Window> = self.stacking().collect();
        for w in stack.into_iter().rev() {
            let f = w.frame;
            if x < f.x || x >= f.right() || y < f.y || y >= f.bottom() {
                continue;
            }
            if y == f.y {
                if let Some((_, b)) = buttons(f).into_iter().find(|(r, _)| x >= r.x && x < r.right()) {
                    return Some((w.id, Hit::Button(b)));
                }
                return Some((w.id, Hit::Title));
            }
            let edges = Edges { left: x == f.x, right: x + 1 == f.right(), bottom: y + 1 == f.bottom() };
            if edges.left || edges.right || edges.bottom {
                return Some((w.id, Hit::Border(edges)));
            }
            return Some((w.id, Hit::Content));
        }
        None
    }

    /// The first tiled window, whose right edge controls the master ratio.
    pub fn master(&self) -> Option<u64> {
        if self.layout() != Layout::Master {
            return None;
        }
        self.tiled_ids().first().copied()
    }
}

/// Split `a` into `n` tiles for a tiled layout, in window order.
pub fn tile(layout: Layout, n: usize, a: Rect, master_ratio: f32) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    // Splits never take more than the space there is (areas can be 0 wide on tiny screens).
    let split_w = |r: Rect, w: u16| {
        let w = w.min(r.width);
        (Rect { width: w, ..r }, Rect { x: r.x + w, width: r.width - w, ..r })
    };
    let split_h = |r: Rect, h: u16| {
        let h = h.min(r.height);
        (Rect { height: h, ..r }, Rect { y: r.y + h, height: r.height - h, ..r })
    };
    // Equal parts of `total` that always add up to it.
    let share = |total: u16, parts: usize, i: usize| -> u16 {
        let base = total as usize / parts;
        (base + usize::from(i < total as usize % parts)) as u16
    };
    match layout {
        Layout::Floating | Layout::Monocle => vec![a; n],
        _ if n == 1 => vec![a],
        Layout::Master => {
            let master_w = if a.width >= 2 * MIN_W {
                ((a.width as f32 * master_ratio) as u16).max(MIN_W).min(a.width - MIN_W)
            } else {
                // Too narrow for two minimum-width tiles: follow the ratio, keep both visible.
                ((a.width as f32 * master_ratio) as u16).clamp(1, a.width.saturating_sub(1).max(1))
            };
            let (master, stack) = split_w(a, master_w);
            let mut out = vec![master];
            let mut y = stack.y;
            for i in 0..n - 1 {
                let h = share(stack.height, n - 1, i);
                out.push(Rect { y, height: h, ..stack });
                y += h;
            }
            out
        }
        Layout::Columns => {
            let mut x = a.x;
            (0..n)
                .map(|i| {
                    let w = share(a.width, n, i);
                    let r = Rect { x, width: w, ..a };
                    x += w;
                    r
                })
                .collect()
        }
        Layout::Grid => {
            let cols = (n as f64).sqrt().ceil() as usize;
            let rows = n.div_ceil(cols);
            let mut out = Vec::with_capacity(n);
            let mut y = a.y;
            for row in 0..rows {
                let h = share(a.height, rows, row);
                let in_row = (n - row * cols).min(cols);
                let mut x = a.x;
                for c in 0..in_row {
                    let w = share(a.width, in_row, c);
                    out.push(Rect::new(x, y, w, h));
                    x += w;
                }
                y += h;
            }
            out
        }
        Layout::Bsp => {
            let mut out = Vec::with_capacity(n);
            let mut rest = a;
            for i in 0..n - 1 {
                let (first, second) =
                    if i % 2 == 0 { split_w(rest, rest.width / 2) } else { split_h(rest, rest.height / 2) };
                out.push(first);
                rest = second;
            }
            out.push(rest);
            out
        }
    }
}

/// Keep at least part of a window on screen and within minimum size.
pub fn clamp_rect(r: Rect, area: Rect) -> Rect {
    let width = r.width.clamp(MIN_W.min(area.width), area.width.max(1));
    let height = r.height.clamp(MIN_H.min(area.height), area.height.max(1));
    let x = r.x.clamp(area.x, area.right().saturating_sub(width));
    let y = r.y.clamp(area.y, area.bottom().saturating_sub(height));
    Rect { x, y, width, height }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_window_inside() {
        let area = Rect::new(0, 0, 80, 23);
        let r = clamp_rect(Rect::new(70, 20, 30, 10), area);
        assert_eq!(r, Rect::new(50, 13, 30, 10));
        let r = clamp_rect(Rect::new(0, 0, 4, 2), area);
        assert_eq!((r.width, r.height), (MIN_W, MIN_H));
    }

    #[test]
    fn tiled_layouts_cover_the_area() {
        let a = Rect::new(0, 0, 100, 30);
        for layout in [Layout::Master, Layout::Grid, Layout::Columns, Layout::Bsp] {
            for n in 1..=7 {
                let rects = tile(layout, n, a, 0.55);
                assert_eq!(rects.len(), n);
                let area: u32 = rects.iter().map(|r| r.width as u32 * r.height as u32).sum();
                assert_eq!(area, 3000, "{layout:?} with {n} windows: {rects:?}");
            }
        }
        assert_eq!(tile(Layout::Monocle, 3, a, 0.5), vec![a; 3]);
        // Found by fuzzing: a zero-width area must not underflow.
        let empty = Rect::new(25, 0, 0, 0);
        for layout in Layout::ALL {
            assert!(tile(layout, 46, empty, 0.0).iter().all(|r| r.width == 0 && r.x >= 25));
        }
        // Narrow screens still show both master and stack.
        let narrow = tile(Layout::Master, 2, Rect::new(0, 0, 12, 5), 0.5);
        assert_eq!((narrow[0].width, narrow[1].width), (6, 6));
    }

    #[test]
    fn grid_and_bsp_shapes() {
        let a = Rect::new(0, 0, 100, 30);
        let g = tile(Layout::Grid, 4, a, 0.5);
        assert_eq!(g[0], Rect::new(0, 0, 50, 15));
        assert_eq!(g[3], Rect::new(50, 15, 50, 15));
        let b = tile(Layout::Bsp, 3, a, 0.5);
        assert_eq!(b, vec![Rect::new(0, 0, 50, 30), Rect::new(50, 0, 50, 15), Rect::new(50, 15, 50, 15)]);
        assert_eq!(Layout::parse("tiling"), Some(Layout::Master));
        assert_eq!(Layout::Bsp.next(), Layout::Floating);
    }

    #[test]
    fn buttons_are_right_aligned() {
        let b = buttons(Rect::new(10, 5, 40, 10));
        assert_eq!(b[2], (Rect::new(46, 5, 3, 1), Button::Close));
        assert_eq!(b[0].1, Button::Minimize);
    }
}

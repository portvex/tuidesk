//! Tiled layout geometry. Properties: one tile per window, inside the area, no overlaps, and
//! (except monocle) the tiles cover the whole area.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use ratatui::layout::Rect;
use tuidesk::wm::{Layout, tile};

#[derive(Debug, Arbitrary)]
struct Input {
    layout: u8,
    windows: u8,
    x: u8,
    y: u8,
    width: u16,
    height: u16,
    ratio: u8,
}

fuzz_target!(|i: Input| {
    let layout = Layout::ALL[i.layout as usize % Layout::ALL.len()];
    let n = i.windows as usize % 40;
    let area = Rect::new(i.x as u16, i.y as u16, i.width % 400, i.height % 200);
    let rects = tile(layout, n, area, i.ratio as f32 / 255.0);
    assert_eq!(rects.len(), n);
    for r in &rects {
        assert!(
            r.x >= area.x && r.y >= area.y && r.right() <= area.right() && r.bottom() <= area.bottom(),
            "{r:?} outside {area:?}"
        );
    }
    if n == 0 || matches!(layout, Layout::Monocle | Layout::Floating) || area.width < 2 * 16 {
        return;
    }
    let total: u64 = rects.iter().map(|r| r.width as u64 * r.height as u64).sum();
    assert_eq!(total, area.width as u64 * area.height as u64, "{layout:?} {n} in {area:?}: {rects:?}");
    for (a, ra) in rects.iter().enumerate() {
        for rb in &rects[a + 1..] {
            assert!(ra.intersection(*rb).is_empty() || ra.area() == 0 || rb.area() == 0, "{ra:?} overlaps {rb:?}");
        }
    }
});

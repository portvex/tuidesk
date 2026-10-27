//! Sequences of kitty graphics commands (chunked transfers, placements, deletes, queries),
//! one per NUL-separated part of the input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tuidesk::graphics::{Delete, Graphics};

fuzz_target!(|data: &[u8]| {
    let mut gfx = Graphics::default();
    for body in data.split(|b| *b == 0) {
        let effect = gfx.kitty(body, (7, 15), true);
        match effect.delete {
            Some(Delete::Image { id, placement, free }) => gfx.delete_image(id, placement, free),
            Some(Delete::All { free }) => gfx.delete_where(free, |_| true),
            _ => {}
        }
        if let Some((_, _, cols, rows, _, _)) = effect.place {
            assert!((1..=297).contains(&cols) && (1..=297).contains(&rows));
        }
        for &(cols, rows) in gfx.virtuals.values() {
            assert!((1..=297).contains(&cols) && (1..=297).contains(&rows));
        }
    }
});

//! Program output splitting and the image commands it extracts.
//! Property: splitting the same bytes across two reads gives the same pieces as one read.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tuidesk::graphics::{self, Graphics, Piece, Splitter};

/// Join adjacent text pieces, since where text is cut is not significant.
fn normalize(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for p in pieces {
        match (out.last_mut(), p) {
            (Some(Piece::Text(prev)), Piece::Text(t)) => prev.extend_from_slice(&t),
            (_, p) => out.push(p),
        }
    }
    out
}

fuzz_target!(|data: &[u8]| {
    let Some((&cut, data)) = data.split_first() else { return };
    let cut = cut as usize % (data.len() + 1);

    let whole = normalize(Splitter::default().split(data));
    let mut s = Splitter::default();
    let mut parts = s.split(&data[..cut]);
    parts.extend(s.split(&data[cut..]));
    assert_eq!(whole, normalize(parts));

    let mut gfx = Graphics::default();
    for piece in whole {
        match piece {
            Piece::Kitty(body) => {
                let effect = gfx.kitty(&body, (8, 16), true);
                if let Some(d) = effect.delete {
                    gfx.delete_where(matches!(d, graphics::Delete::All { free: true }), |_| true);
                }
            }
            Piece::Sixel(d) => {
                let _ = graphics::sixel_image(&d);
            }
            Piece::Iterm(b) => {
                let _ = graphics::iterm_image(&b, (8, 16), 80);
            }
            _ => {}
        }
    }
});

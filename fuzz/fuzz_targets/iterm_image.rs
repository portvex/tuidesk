//! iTerm2 inline image arguments and payloads (image decoding included).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some((_, cols, rows)) = tuidesk::graphics::iterm_image(data, (9, 18), 120) {
        assert!((1..=297).contains(&cols) && (1..=297).contains(&rows));
    }
});

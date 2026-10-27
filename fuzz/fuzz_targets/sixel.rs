//! Sixel decoding. Property: the pixel buffer matches the reported size.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(img) = tuidesk::graphics::sixel_image(data) {
        assert!(img.width > 0 && img.height > 0);
        assert_eq!(img.data.len(), img.width as usize * img.height as usize * 4);
    }
});

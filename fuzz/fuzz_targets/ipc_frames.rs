//! Client/server frames read from an untrusted stream.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut r = data;
    while let Ok((_, payload)) = tuidesk::ipc::read_frame(&mut r) {
        assert!(payload.len() <= data.len());
    }
});

//! Text parsed from users and scripts: key specs, actions, layouts, calculator expressions.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let _ = tuidesk::keys::KeySpec::parse(text);
    let _ = tuidesk::app::Action::parse(text);
    let _ = tuidesk::wm::Layout::parse(text);
    if let Some(result) = tuidesk::runner::calculate(text) {
        assert!(!result.is_empty() && result != "NaN");
    }
});

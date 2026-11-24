//! Text parsed from users and scripts: key specs, actions, layouts, calculator expressions,
//! clock formats.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let _ = tuidesk::keys::KeySpec::parse(text);
    let _ = tuidesk::app::Action::parse(text);
    let _ = tuidesk::wm::Layout::parse(text);
    // Clock formats: a whole second, and a time with a sub-second part, which
    // exercises the %.f and %+ rewrites.
    let t = jiff::Timestamp::UNIX_EPOCH.to_zoned_datetime(jiff::tz::TimeZone::UTC);
    let _ = tuidesk::ui::strftime(&t, text);
    if let Ok(t) = jiff::Timestamp::new(1_795_000_000, 120_000_000) {
        let _ = tuidesk::ui::strftime(&t.to_zoned_datetime(jiff::tz::TimeZone::UTC), text);
    }
    if let Some(result) = tuidesk::runner::calculate(text) {
        assert!(!result.is_empty() && result != "NaN");
    }
});

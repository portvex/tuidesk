//! User configuration merged over the defaults, and the Settings dialog working on it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fuzz_target!(|text: &str| {
    if let Ok(cfg) = tuidesk::config::parse(text) {
        // The user's [[apps]] merged over the built-in catalogue, with hidden_apps applied.
        let _ = cfg.all_apps();
    }
    let Ok(mut s) = tuidesk::settings::Settings::from_text("/nonexistent/config.toml".into(), text.to_string()) else {
        return;
    };
    // Change and reset a value on every page; whatever the file holds, nothing may panic.
    for page in 0..tuidesk::settings::PAGES.len() {
        s.set_page(page);
        for code in [KeyCode::Right, KeyCode::Down, KeyCode::Enter, KeyCode::Esc, KeyCode::Delete, KeyCode::End] {
            let _ = s.key(&KeyEvent::new(code, KeyModifiers::NONE));
        }
        let _ = s.rows();
        let _ = s.text();
    }
});

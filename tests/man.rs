//! The manual pages in `man/` against the program: every option, setting and action has to be
//! documented, and the pages must pass `mandoc -Tlint` where mandoc is installed.

use std::path::Path;
use std::process::Command;

/// A page's text with roff's escaped hyphens as plain ones, so names can be searched for.
fn page(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("man").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).replace("\\-", "-")
}

#[test]
fn pages_show_the_crate_version() {
    for name in ["tuidesk.1", "tuidesk-config.5", "tuidesk-lua.7"] {
        let th = page(name).lines().find(|l| l.starts_with(".TH ")).map(str::to_string);
        let want = format!("\"tuidesk {}\"", env!("CARGO_PKG_VERSION"));
        assert!(th.as_deref().is_some_and(|l| l.contains(&want)), "{name}: {th:?} lacks {want}");
    }
}

#[test]
fn command_page_lists_every_option() {
    let out = Command::new(env!("CARGO_BIN_EXE_tuidesk")).arg("--help").output().unwrap();
    let help = String::from_utf8(out.stdout).unwrap();
    let man = page("tuidesk.1");
    let options: Vec<&str> = help
        .split_whitespace()
        .map(|w| w.trim_end_matches(','))
        .filter(|w| w.starts_with('-') && w.len() > 1)
        .collect();
    assert!(options.len() >= 10, "{options:?}");
    for opt in options {
        assert!(man.contains(opt), "tuidesk.1 does not document {opt}");
    }
    for cmd in ["ls", "kill", "msg", "notify", "spawn", "windows", "desktops", "screenshot"] {
        assert!(
            man.contains(&format!(".B {cmd}"))
                || man.contains(&format!(".BR {cmd}"))
                || man.contains(&format!(".BI {cmd}")),
            "{cmd}"
        );
    }
}

#[test]
fn config_page_lists_every_setting_and_action() {
    let man = page("tuidesk-config.5");
    let defaults: toml::Table = toml::from_str(tuidesk::config::DEFAULT_CONFIG).unwrap();
    for table in ["general", "panel", "screenshot"] {
        for key in defaults[table].as_table().unwrap().keys() {
            assert!(man.contains(key.as_str()), "tuidesk-config.5 does not document {table}.{key}");
        }
    }
    let keys = defaults["keys"].as_table().unwrap();
    for bindings in keys.values() {
        for action in bindings.as_table().unwrap().values() {
            let name = action.as_str().unwrap().split_whitespace().next().unwrap();
            assert!(man.contains(name), "tuidesk-config.5 does not document the action {name}");
        }
    }
}

#[test]
fn pages_pass_mandoc_lint() {
    if Command::new("mandoc").arg("-h").output().is_err() {
        eprintln!("skipping: mandoc not installed");
        return;
    }
    for name in ["tuidesk.1", "tuidesk-config.5", "tuidesk-lua.7"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("man").join(name);
        let out = Command::new("mandoc").args(["-Tlint", "-W", "warning"]).arg(&path).output().unwrap();
        let report = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success() && report.trim().is_empty(), "{name}:\n{report}");
    }
}
